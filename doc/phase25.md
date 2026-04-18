# Phase 25 — Advanced Physics Authoring

RustForge 1.0 shipped with Rapier under the hood: rigid bodies, colliders, a fixed-timestep step loop, and the minimum authoring UI to place a box and watch it fall. That covered 80% of what indie projects need and 20% of what "Chaos-equivalent" means. Phase 25 is the rest — the authoring tools, specialized simulators, and asset pipeline that turn a rigid-body library into a physics *content system*.

The framing is deliberate. We are not replacing Rapier. Rapier already gives us a fast, broadly-correct constraint solver with a deterministic SIMD mode. What Unreal built around PhysX/Chaos — PhAT, fracture tool, vehicle editor, cloth paint, ragdoll blending, constraint authoring — is almost entirely *editor work with thin runtime components*. Phase 25 adds those layers on top of Rapier, keeps the solver a single swappable dependency, and is careful about what it does *not* ship (§ Scope ❌).

Upstream dependencies: Phase 2 (RTT, reflection), Phase 6 (undo stack — every authoring tool here must coalesce into commands), Phase 7 (PIE — live preview of physics assets), Phase 10 (rewind buffer, debug visualizer base), Phase 14 (deterministic net sim — Phase 25's determinism policy is dictated here).

## Goals

By end of Phase 25:

1. **Destruction authoring** — pre-fracture Voronoi decomposition at import, runtime activation on impact threshold, debris pooling, dockable Fracture panel with preview.
2. **Cloth** — PBD on skinned meshes with a pin painter and stiffness paint tool.
3. **Ragdoll authoring** — per-skeleton joint/limit/motor setup, live preview, animated↔ragdoll blend.
4. **Vehicles** — suspension/tire/engine component stack on Rapier joints, raycast wheels, chassis-visualization editor.
5. **PhAT-equivalent** — per-skeleton collision proxy + constraint asset; ragdoll and rigid-body hit volumes source from one file.
6. **Constraints** — fixed/hinge/slider/ball/generic-6DOF with per-constraint authoring UI.
7. **Force fields** — wind/gravity/radial scene entities.
8. **Soft body** (optional, gated) — reduced-coordinate FEM for select platforms.
9. **Determinism policy** — simd-deterministic + fixed timestep, verified against Phase 10 rewind and Phase 14 net-sim harness.
10. **Debug visualizer** — wireframe, contacts, constraint violation, cloth stress overlays wired into the Phase 10 viewport toggle registry.

## 1. Determinism policy (read this first)

Everything else in this phase depends on it. Rapier has a `simd-deterministic` feature that guarantees bit-identical stepping across x86_64/aarch64 *if* the caller respects three rules:

```rust
pub struct PhysicsConfig {
    pub fixed_dt: f32,             // 1.0 / 60.0, never vary
    pub max_substeps: u32,         // 4 — clamp catch-up after hitches
    pub integration: IntegrationParameters { /* locked */ },
    pub solver_iters: (u32, u32),  // (velocity, position) — both fixed
}
```

1. **Fixed timestep only.** The render loop accumulates and steps zero-or-more whole `fixed_dt`s per frame. No variable-dt path exists in 1.0+.
2. **Deterministic broad-phase.** Rapier's SAP is deterministic; we do not swap it out. Island ordering is stable.
3. **Stable insertion order.** Bodies are inserted in entity-spawn order, which is stable across a replay because Phase 10 records entity creation commands.

If any tool in this phase introduces non-determinism (floating-point intrinsics, parallel reductions with unspecified order, `HashMap` iteration order) it is a **regression blocker**. There is a `tests/determinism/` harness that steps a 500-body scene for 30 seconds on two threads and byte-compares snapshots. Every PR that touches `rustforge-physics/` runs it.

Caveat inherited by Phase 14: cross-platform deterministic networking is still "best effort" — Rapier's simd path is deterministic *same-binary-same-arch*. Different compiler versions can diverge. Phase 14 handles this by distributing a shared binary, not by trusting cross-compiler determinism.

## 2. Crate layout

```
crates/
├── rustforge-physics/           # the Rapier wrapper; existed pre-1.0
│   ├── src/
│   │   ├── world.rs             # PhysicsWorld: step, snapshot, restore
│   │   ├── body.rs              # RigidBody, Collider component wrappers
│   │   ├── constraints.rs       # Fixed | Hinge | Slider | Ball | Generic6DOF
│   │   ├── field.rs             # ForceField: Wind | Gravity | Radial
│   │   ├── vehicle.rs           # Vehicle runtime (raycast-wheel model)
│   │   ├── cloth.rs             # PBD solver
│   │   ├── ragdoll.rs           # PhysicsAsset runtime: build joints from asset
│   │   ├── destruction.rs       # Fracture activation, debris pool
│   │   └── softbody.rs          # #[cfg(feature = "softbody")]
│   └── Cargo.toml
└── rustforge-editor/
    └── src/panels/physics/
        ├── fracture.rs          # Fracture panel (§3)
        ├── cloth_paint.rs       # Pin/stiffness paint (§4)
        ├── phat.rs              # Physics Asset Tool (§5)
        ├── vehicle.rs           # Vehicle editor (§6)
        ├── constraints.rs       # Per-constraint inspectors (§7)
        └── viz.rs               # Physics debug viz registration (§10)
```

`rustforge-physics` compiles without the `editor` feature; it is the runtime. All authoring UI lives in `rustforge-editor` and produces *assets* (§5, §3) that the runtime consumes.

## 3. Destruction — pre-fractured Voronoi

Runtime fracture (cutting meshes on impact) is tempting and wrong. It is non-deterministic by default, chews CPU, and produces ugly debris unless tuned per-asset. Every shipped destructible game that looks good (Chaos, Red Faction's Geo-Mod) pre-fractures assets offline.

We do the same. On import, a mesh tagged `fracture = true` in its sidecar gets a Voronoi decomposition baked into a companion asset:

```rust
pub struct FractureAsset {
    pub cells: Vec<FractureCell>,           // each cell: convex hull + render mesh
    pub connectivity: Vec<(u32, u32, f32)>, // (a, b, bond_strength) — adjacency graph
    pub impact_threshold: f32,              // impulse magnitude to break a bond
    pub debris_lifetime: f32,               // seconds before pool reclaim
}

pub struct FractureCell {
    pub hull: ConvexHull,                   // Rapier collider shape
    pub render_mesh: MeshHandle,            // the shard's visual mesh
    pub mass: f32,
    pub center_of_mass: Vec3,
}
```

Runtime has three states: **intact** (single rigid body, Voronoi hidden), **shattering** (first frame after threshold exceeded — spawn cells, destroy intact body, apply impulse split proportional to bond graph), **debris** (cells are independent rigid bodies, pooled for recycling after `debris_lifetime`).

Debris pooling is mandatory — a grenade that spawns 200 rigid bodies each frame will starve the solver. Pool size is scene-authored; overflow culls oldest-first by Phase 10's frame-captured timestamp.

### 3.1 Fracture panel

```
┌─ Fracture ────────────────────────────────────────────────┐
│ Source:   meshes/pillar.mesh                              │
│ Method:   ● Voronoi   ○ Clustered Voronoi   ○ Planar Cuts │
│ Cells:    [ 64 ]   Seed: [ 1337 ]   [Regenerate]          │
│ Preview:  [▣ Intact] [▣ Shards] [□ Bonds] [□ Debris]      │
│                                                           │
│ Impact threshold:  [------●------] 12.4 N·s               │
│ Debris lifetime:   [---●---------]  4.0 s                 │
│ Pool budget:       [ 200 ] shards global                  │
│                                                           │
│ [Bake] [Revert] [Test-Drop →]                             │
└───────────────────────────────────────────────────────────┘
```

`Test-Drop` is a one-click PIE shortcut — spawn the asset in a sandbox sub-scene, drop a 10 kg sphere on it, visualize the break. This is the tuning loop; make it one click.

Undo: every slider coalesces per Phase 6 §4. `Regenerate` with a new seed is a single undoable command.

## 4. Cloth — PBD on skinned meshes

Mass-spring is easier to write; PBD (Position-Based Dynamics) is easier to tune. We pick PBD. Verlet integration, Jacobi constraint projection, 8 iterations per step, distance + bending constraints.

```rust
pub struct ClothComponent {
    pub mesh: MeshHandle,                   // the skinned mesh the cloth drives
    pub particles: Vec<ClothParticle>,      // one per vertex
    pub distance_c: Vec<DistanceConstraint>,// edge constraints
    pub bending_c: Vec<BendConstraint>,     // dihedral-angle constraints
    pub pins: BitSet,                       // painted anchor vertices
    pub stiffness_map: Vec<f32>,            // per-vertex 0..1 (painted)
    pub wind_response: f32,                 // coupling coefficient to ForceField::Wind
}
```

Pins are hard-constraints: their position each step is driven by the underlying skeletal animation. Stiffness painting modulates local constraint strength — a character cape wants stiff shoulders, floppy hem.

### 4.1 Cloth paint tool

A viewport overlay, not a dockable panel. Active when a cloth component is selected:

```
┌─ Cloth Paint ─────────────────────────┐
│ Mode:  ● Pin   ○ Stiffness   ○ Mass   │
│ Brush: radius [ 12 ]  falloff [ 0.7 ] │
│ Value: [--●-------] 0.25              │
│ [Invert] [Smooth] [Fill] [Clear]      │
└───────────────────────────────────────┘
```

Painting writes to a per-vertex buffer on the cloth asset. The panel is a thin wrapper over the same vertex-paint widget Phase 11 (terrain) should have already shipped — reuse it.

Cloth does *not* participate in deterministic rewind. It is marked `rewind = visual-only` and reseeded on rewind-to-frame events. Trying to rewind PBD cloth exactly is a rabbit hole; acknowledge and move on.

## 5. PhAT — the Physics Asset

A `PhysicsAsset` is per-skeleton and is the single source of truth for "what collides and what constrains" for anything animated. Ragdolls and rigid-body hit detection both consume it.

```rust
pub struct PhysicsAsset {
    pub skeleton: SkeletonHandle,
    pub bodies:   Vec<PhatBody>,            // one entry per skeleton bone that has a proxy
    pub joints:   Vec<PhatJoint>,           // constraints between proxies
}

pub struct PhatBody {
    pub bone: BoneId,
    pub shape: ColliderShape,               // Capsule | Box | Sphere | ConvexHull
    pub local_transform: Transform,         // shape pose relative to bone
    pub mass: f32,
    pub collision_group: u16,
}

pub struct PhatJoint {
    pub parent: BoneId,
    pub child:  BoneId,
    pub kind:   ConstraintKind,
    pub limits: JointLimits,                // swing1/swing2/twist cones, linear limits
    pub motor:  Option<Motor>,              // target velocity + max force
}
```

### 5.1 PhAT panel

```
┌─ Physics Asset: HumanMale ──────────────────────────────────┐
│ Skeleton: skeletons/human_male.skel     [Auto-generate…]    │
│                                                             │
│ ▼ Bodies (14)                                               │
│   • pelvis     capsule  r=0.18 h=0.22  mass 9.0 kg   [Edit] │
│   • spine_01   capsule  r=0.15 h=0.30  mass 7.5 kg   [Edit] │
│   • upper_arm  capsule  r=0.06 h=0.30  mass 2.1 kg   [Edit] │
│   ...                                                       │
│                                                             │
│ ▼ Joints (13)                                               │
│   • pelvis→spine_01    Ball   ±30° swing / ±15° twist       │
│   • spine_01→spine_02  Ball   ±20° swing / ±10° twist       │
│   ...                                                       │
│                                                             │
│ Preview: [▶ Drop] [▶ Kick] [▶ Hang-by-hand] [Blend: ●—— 0.0]│
└─────────────────────────────────────────────────────────────┘
```

`Auto-generate…` produces a reasonable initial proxy layout by fitting capsules to bone-weighted mesh AABBs — this is what PhAT does, and it is 90% correct 10% of the time. It is a starting point, not an end state. The panel's whole job is to make the refinement loop fast.

Preview buttons are canned test cases: drop (gravity), kick (impulse at pelvis), hang-by-hand (pin both hands, gravity). `Blend` drives a mix factor between animation-driven and physics-driven pose per bone. The blend curve per bone is authored in the same panel as a separate tab.

### 5.2 Ragdoll runtime

At play time, spawning an entity with `(Skeleton, PhysicsAsset)` builds a Rapier compound: one rigid body per `PhatBody`, Rapier joints from `PhatJoint`. The bodies are kinematic (pose-driven by animation) until a trigger flips them to dynamic — trigger is typically a damage event or explicit `actor.ragdoll()` call from script.

Blend: per-bone `blend ∈ [0, 1]`. `0` = fully animated (kinematic), `1` = fully physical (dynamic), intermediate = critically-damped drive toward animated pose. The blend field is a `Vec<f32>` the size of the skeleton, authored on the asset and mutable at runtime.

## 6. Vehicles

Vehicles are the hardest non-optional item in this phase because "vehicle that feels right" is its own sub-field. We do the cheap right thing: raycast wheels, arcade-capable but simulation-extensible, matched roughly to Rapier's joint-and-raycast patterns.

```rust
pub struct Vehicle {
    pub chassis: BodyHandle,
    pub wheels: Vec<Wheel>,
    pub engine: Engine,
    pub gearbox: Gearbox,
    pub steering_input: f32,                // -1..1
    pub throttle_input: f32,                // -1..1
    pub brake_input:    f32,                // 0..1
}

pub struct Wheel {
    pub local_anchor: Vec3,                 // attach point on chassis
    pub radius: f32,
    pub suspension: Suspension,             // rest_len, stiffness, damping, max_travel
    pub tire: Tire,                         // friction curves: long + lateral
    pub steer_mask: f32,                    // 0 = fixed, 1 = full steering
    pub drive_mask: f32,                    // 0 = free-spin, 1 = driven
}
```

Each step per wheel: downward raycast from anchor, compute suspension force (spring + damper) along the ray, compute tire forces in contact-plane basis, apply all forces to chassis body as a compound impulse at the contact point. The chassis is a normal Rapier dynamic body; the wheels are *not* bodies — only visuals + the raycast model.

This is the Rocket League / BeamNG-mobile / Halo-Warthog model. It does not capture deformation, tire slip at the carcass level, or aero. Gate "real" simulation behind `feature = "advanced-vehicle"` that a future phase can own.

### 6.1 Vehicle editor

Chassis visualization + per-wheel layout + drive-curve graphs:

```
┌─ Vehicle: CarBase ──────────────────────────────────────────┐
│ [3D Chassis View — wheel anchors as gizmos]                 │
│                                                             │
│ ▼ Wheels (4)           steer drive                          │
│   • FL  (-0.8, 0.0, 1.3)  1.0  0.5                          │
│   • FR  ( 0.8, 0.0, 1.3)  1.0  0.5                          │
│   • RL  (-0.8, 0.0,-1.3)  0.0  1.0                          │
│   • RR  ( 0.8, 0.0,-1.3)  0.0  1.0                          │
│                                                             │
│ ▼ Engine                                                    │
│   Torque curve: [graph editor — rpm × N·m]                  │
│   Redline:      [ 7200 ] rpm                                │
│                                                             │
│ ▼ Gearbox   [ -1 | 1 : 3.5 | 2 : 2.1 | 3 : 1.4 | 4 : 1.0 ]  │
│                                                             │
│ [▶ Test Track] [Telemetry →]                                │
└─────────────────────────────────────────────────────────────┘
```

`Test Track` opens a sandbox scene preloaded with a closed loop, drops the vehicle in, and opens a telemetry panel (speed, RPM, gear, wheel grip) alongside. Tuning loop = drive it, tweak a slider, drive again. Like the fracture panel: make the loop one click.

## 7. Constraints — first-class authoring

Five concrete kinds plus a generic. Each has a per-entity inspector panel generated through reflection but with bespoke visualizations:

| Kind           | Visualization                                    |
| -------------- | ------------------------------------------------ |
| `Fixed`        | Lock icon between anchors                        |
| `Hinge`        | Axis line + swing arc                            |
| `Slider`       | Dual arrow along travel axis                     |
| `Ball`         | Swing cone (θ₁, θ₂) + twist arc                  |
| `Generic6DOF`  | Six-axis gizmo with per-axis lock/limit/free     |

Authoring invariant: **a constraint's visualization must be visible in the viewport whenever either endpoint entity is selected.** This is non-negotiable — invisible constraints are the main reason ragdoll setups drive people insane. The viz registers with the Phase 10 debug-viz system (§10).

Motors are per-constraint optional. Motor UI is a collapsed subsection (target velocity, max force) that users open only when needed.

## 8. Force fields

Scene entities that apply body-space or world-space forces during the physics step:

```rust
pub enum ForceField {
    Wind   { direction: Vec3, base_speed: f32, gust: Gust },
    Gravity{ direction: Vec3, magnitude: f32, zone: Zone },
    Radial { center: Vec3, falloff: Falloff, strength: f32 },
}

pub struct Zone { pub shape: ColliderShape, pub falloff: Falloff }
```

Each field is a scene entity with a transform and a visualization (arrows for wind, radial gradient for gravity, concentric rings for radial). Bodies whose AABB overlaps the zone receive the force. Wind also couples to cloth (§4) via `wind_response`.

Gotcha: force fields are evaluated *before* the solver step, inside the same fixed-dt tick. They are deterministic. They are *not* evaluated on kinematic bodies, only dynamic.

## 9. Soft body (optional)

Gated behind `feature = "softbody"`. Reduced-coordinate FEM: each soft body has a low-res tetrahedral "cage" mesh driving a high-res render mesh via linear blend skinning. We ship the Projective Dynamics variant — compact, stable, tunable stiffness.

Scope ruthlessly: no topology changes (no cutting), no coupling to cloth, no fracture interaction. A single soft body is a blob that deforms and returns. That is the 1.0 deliverable behind this feature flag.

Gate recommendation: only enable on desktop platforms at ≥ "High" physics quality. Console mid-tier: off. Mobile: off. Do not hide the feature's cost.

## 10. Debug visualizer — extending Phase 10

Phase 10 shipped a viewport-overlay registry. Phase 25 registers six new overlays:

```rust
viz.register("physics.colliders",   collider_wireframe_pass);
viz.register("physics.contacts",    contact_point_pass);
viz.register("physics.constraints", constraint_visual_pass);
viz.register("physics.forces",      force_field_visual_pass);
viz.register("physics.cloth_stress",cloth_stress_heatmap_pass);
viz.register("physics.com",         center_of_mass_pass);
```

Each pass draws into the editor-only overlay render target and is zero-cost in shipped builds (Phase 2 §2 rule). Constraint-violation drawing uses red for "limit hit" and yellow for "approaching limit" — the visual cue that pays off when debugging ragdolls.

Contact-point rendering must respect Phase 10's frame-capture: when paused and scrubbing the rewind ring, contact points shown are the ones from the captured frame, not the live solver.

## 11. Build order

1. Determinism harness (`tests/determinism/`) — even before any feature. Every subsequent PR runs it.
2. Constraint authoring (§7) + generic-6DOF runtime. Everything below depends on stable constraints.
3. PhAT panel (§5) — unlocks ragdoll.
4. Ragdoll runtime + blend.
5. Force fields (§8) — small, unblocks cloth wind and vehicle aero later.
6. Cloth PBD + paint tool (§4).
7. Destruction: fracture baker → runtime activation → debris pool → panel (§3).
8. Vehicle runtime → vehicle editor (§6).
9. Debug viz passes (§10) — landed incrementally alongside each feature, but *audited as a set* at the end.
10. Soft body, behind feature flag (§9). Skip if schedule slips; nothing else blocks on it.

## Scope ❌

- ❌ GPU fluid sim (SPH, FLIP, PIC) — separate phase, requires different compute infrastructure
- ❌ Hair and fur simulation — strand dynamics is its own authoring problem
- ❌ FEM destruction — Voronoi pre-fracture is the shipped answer; no mesh cutting
- ❌ ML-driven physics (learned constraint solvers, neural cloth) — research, not 1.0
- ❌ Differentiable physics — gradient-through-solver is an R&D feature
- ❌ Runtime Voronoi fracture — assets must be pre-baked
- ❌ Cross-platform cross-compiler determinism — inherit Phase 14's binary-distribution stance
- ❌ Swapping Rapier — a valid future phase, not this one; no abstraction tax paid now

## Risks

- **Authoring-tool debt.** Seven panels is a lot. If the vehicle editor or PhAT is rushed, users will reject the whole phase as "Rapier with a bad UI." Budget two full weeks per major panel, minimum.
- **Determinism regressions.** Every parallel iteration or unordered map in `rustforge-physics/` is a landmine. CI runs the determinism harness; do not allow it to be skipped.
- **Ragdoll tuning loop is slow.** If PhAT preview does not iterate in under 2 seconds from click-to-result, users abandon it. Keep the preview sandbox hot-loaded.
- **Destruction content cost.** Pre-fracture assets are heavy on disk and memory. Ship a warning in the Fracture panel when a single asset's shard data exceeds 8 MB.
- **Cloth quality ceiling.** PBD is tunable but not photoreal. Set user expectations in docs: "cloth is expressive, not film-grade."
- **Rapier upstream drift.** We pin a Rapier version per engine release. Version bumps are their own PR with a full determinism re-run.

## Exit criteria

- A sample scene containing (a) a fractured pillar, (b) a cloth banner in a wind zone, (c) a ragdoll triggered by an explosion, (d) a drivable vehicle, (e) a hinge-constrained door with motor, runs for 10 minutes at locked 60 Hz on mid-tier desktop hardware with zero frame-time regressions above baseline Rapier.
- The determinism harness passes on x86_64 Linux, x86_64 Windows, and aarch64 macOS with byte-identical 30-second snapshots.
- Phase 10 rewind scrubs through a 300-frame window of the sample scene without desync (cloth excepted — visual-only reseed is expected).
- Phase 14 net-sim harness round-trips a 4-player match of the sample scene with deterministic lockstep and no divergence under 120 ms simulated latency.
- Every authoring panel routes all mutations through Phase 6 commands; Ctrl+Z is never broken.
- Six debug-viz overlays are present and toggleable from the Phase 10 overlay menu.
