# Phase 41 — Advanced Physics II: Hair, Fur & Fluids

Phase 25 shipped destruction, cloth, ragdoll, vehicles, and a PhAT-equivalent on top of Rapier — and explicitly punted three simulation domains that didn't fit the rigid-body-and-PBD mould: hair/fur strands, particle-based fluids (SPH, FLIP), and gaseous voxel fluids (smoke, fire). Phase 25 §5 called those "different solvers, different authoring, defer to 3.0+." Phase 41 is where they land.

The framing stays the same as Phase 25: we are not replacing Rapier, we are adding *specialized* simulators next to it. Each of the three domains here has its own solver loop, its own asset type, and its own authoring panel — but they all live inside `rustforge-physics-fx`, feed Phase 18's particle renderer where appropriate, hand splash/flow events to Phase 17 audio, and honour the Phase 21/22 realism tier so Low-tier targets still produce *something*.

Upstream dependencies: Phase 8 (AssetEditor pattern for Groom Editor), Phase 17 (audio cues for splash/flow), Phase 18 (particle render path we piggy-back on for foam/spray), Phase 21 (tier system), Phase 22 (Low tier fallbacks), Phase 24/35 (skeleton attachment for hair), Phase 25 (Rapier colliders we read for hair/fluid scene collision, determinism policy), Phase 7 (PIE Stop resets).

## Goals

By end of Phase 41:

1. **Hair/fur strand rendering** — GPU strip-per-strand with anisotropic Marschner-lite shading, per-tier strand density.
2. **PBD hair physics** — Position-Based Dynamics per strand, strand-to-strand self-collision approximation, wind forces, Rapier collider-list scene collision.
3. **Groom Editor panel** — AssetEditor-style tool for authoring hair shape: guide curves, styling (comb, cut, length jitter), preview; produces `.rgroom` RON assets.
4. **Fur LOD ladder** — full strands on High/Ultra, reduced count on Medium, billboard card-based fur strip technique on Low.
5. **Skeleton attachment** — hair roots bind to skeleton root-bones (Phase 24/35), sim runs in-phase with the animation tick.
6. **SPH fluid** — compute-based smoothed-particle hydrodynamics for splashes, puddles, beverages; hands foam/spray particles to Phase 18.
7. **FLIP fluid** — compute-based hybrid particle/grid solver for small pools and contained bodies; High tier only.
8. **Fluid surface rendering** — screen-space thick-particle path (view-space normals, smoothed depth) plus optional marching-cubes mesh extraction at a lower frequency for stills/hero shots.
9. **Smoke/fire voxel grid** — 3D Navier-Stokes on a sparse voxel grid, cheap enough for Medium tier at limited resolution; Phase 18 emitters can spawn into or read from it.
10. **`.rfluid` volume asset** with preset library: water splash, smoke plume, small fire, waterfall, blood splatter.
11. **Audio hand-off** — splash/impact/flow events emit to the Phase 17 event bus as typed cues.
12. **Per-effect tier budgets** — each active hair/fluid instance declares a budget; the runtime scales it down if the frame blows past.
13. **PIE integration** — Phase 7 Stop resets every active fluid sim and hair sim to its groom pose / empty-domain state, deterministically.

## 1. Crate layout

```
crates/
├── rustforge-physics-fx/               # new, no editor deps
│   ├── src/
│   │   ├── lib.rs
│   │   ├── tier.rs                     # re-exports RealismTier budget constants
│   │   ├── hair/
│   │   │   ├── mod.rs                  # Groom runtime, HairInstance
│   │   │   ├── pbd.rs                  # PBD solver
│   │   │   ├── collide.rs              # strand/collider + pseudo self-collision
│   │   │   ├── wind.rs                 # wind field sampling
│   │   │   └── render.rs               # strand strip builder, shader bind
│   │   ├── fluid/
│   │   │   ├── mod.rs                  # FluidInstance enum: Sph | Flip | Gas
│   │   │   ├── sph.rs                  # SPH compute passes
│   │   │   ├── flip.rs                 # FLIP compute passes (High only)
│   │   │   ├── gas.rs                  # voxel smoke/fire
│   │   │   ├── surface_ss.rs           # screen-space fluid render
│   │   │   └── surface_mc.rs           # marching cubes, deferred / hero
│   │   ├── event.rs                    # SplashEvent, FlowEvent -> Phase 17
│   │   ├── budget.rs                   # per-effect scaler
│   │   └── pie.rs                      # reset hooks (Phase 7)
│   └── Cargo.toml
└── rustforge-editor/
    └── src/asset_editors/
        ├── groom/                      # Groom Editor (§5)
        └── fluid/                      # Fluid preset browser (§11)
```

The runtime crate compiles without `editor`. Authoring UI lives in `rustforge-editor` and writes `.rgroom` / `.rfluid` assets that the runtime consumes — same shape as Phase 25.

## 2. Hair strand rendering — why strips, why not cards by default

Three viable representations: polygon cards (cheap, dated on realistic), strand strips (one strip of quads per strand, modern, memory-hungry), full cylindrical geometry with HW tess (film-quality, not runtime-viable on wgpu's portable baseline). Phase 41 picks **strand strips** as the High/Ultra representation; Low tier explicitly falls back to cards (§4) so the feature degrades rather than vanishing.

Per-strand data:

```rust
pub struct Strand {
    pub root:     Vec3,            // bind-pose root in groom local space
    pub points:   Vec<Vec3>,       // 4–16 control points
    pub widths:   Vec<f32>,        // taper along strand
    pub root_uv:  Vec2,            // for scalp-texture sampling
    pub bone_idx: u16,             // skeleton attachment (Phase 24/35)
}

pub struct Groom {
    pub strands:       Vec<Strand>,
    pub strands_per_guide: u32,    // interpolation multiplier at runtime
    pub bbox:          Aabb,
    pub scalp_texture: Option<AssetGuid>,
}
```

Guide-vs-render split: the `.rgroom` stores **guide strands** (500–5000). At runtime we **interpolate** render strands (up to 100k on High) by barycentric blend on the scalp mesh's triangle, seeded so the same scalp point always produces the same strand.

### 2.1 Strip builder (compute)

```wgsl
// hair/expand.wgsl — one workgroup per strand, emits triangle strip
@compute @workgroup_size(64)
fn expand(@builtin(global_invocation_id) gid : vec3<u32>) {
    let s = strands[gid.x];
    // build N segments; each emits two verts offset along
    // cross(view.forward, tangent) to face camera, widths[i] wide.
}
```

One indirect draw per groom. Draw order is front-to-back coarse; we rely on the Phase 21 hair-pass alpha-to-coverage + depth prepass to avoid full sort.

### 2.2 Shading — Marschner-lite

Full Marschner is overkill. We ship a Kajiya-Kay tangent-aware specular plus one shifted secondary highlight and a diffuse — cheap, hair-shaped, runs on Medium. High tier layers a Beckmann TRT lobe on top. Root darkening uses the shifted-hue trick; no per-strand path tracing.

## 3. Hair physics — PBD on strands

PBD for the same reasons Phase 25 cloth picked it: stable at large timesteps, easy to constrain, deterministic under fixed dt, idiom already in-house.

Per-strand state:

```rust
pub struct StrandSim {
    pub pos:       Vec<Vec3>,    // current positions
    pub prev_pos:  Vec<Vec3>,    // for Verlet
    pub inv_mass:  Vec<f32>,     // 0.0 at root = pinned
    pub rest_len:  Vec<f32>,     // N-1 segment lengths
}
```

Constraints per iteration, in order:

1. **Distance** — segment rest-length (stiffness 1.0).
2. **Bending** — three-point cosine, stiffness from groom asset.
3. **Collision** — strand-point vs Rapier collider list (spheres + capsules, hair-appropriate radii; authors tag which colliders hair sees — no full broad-phase re-query).
4. **Self-collision (approximate)** — guides hashed into a coarse 3D grid by root position; same-cell overlap repels. Not correct (O(n²) is murder) but stops clumps collapsing.
5. **Pin** — root positions overwritten from the bound bone each tick (§6).

```rust
pub struct PbdConfig {
    pub sub_steps:      u32,   // 2 on Medium, 4 on High
    pub iterations:     u32,   // 4 / 8
    pub bend_stiffness: f32,
    pub wind_scale:     f32,
    pub damping:        f32,
}
```

Wind sampled from global wind field (Phase 25 force fields) plus a cheap per-groom curl-noise term, applied as acceleration before the constraint loop. Bone-space direction so head turns don't fling hair. Self-collision uses guides only, render strands inherit the response by barycentric weight — looks fine in motion, stills may show interpenetration (documented).

## 4. Fur LOD ladder

```
Ultra   : full strands, 80–100k render strands per character, 8 pbd iters
High    : full strands,  40–60k,                               6 pbd iters
Medium  : reduced,       10–20k,                               4 pbd iters
Low     : CARDS fallback (no strands, no sim)
```

Low-tier cards use the **grass-strip billboard** technique: alpha-cut quads arranged in clumps over the scalp, animated by a two-bone skinned sway. No sim cost, WebGL2-safe. Distance fade inside each tier interpolates strand count linearly between `min_count` and `max_count` — same curve Phase 18 uses for spawn LOD. Low-tier card switch is hard, not cross-fade (mixing representations at runtime costs more than it saves).

## 5. Groom Editor — authoring panel

AssetEditor (Phase 8 §1) with a preview viewport (Phase 8 §2.1 pattern). Opens on double-click of a `.rgroom` in the content browser.

```
 ┌───────────────────────────────────────────────────────────────────┐
 │ Groom: short_tousled.rgroom*                          [Save] [x]  │
 ├─────────────┬─────────────────────────────────────┬───────────────┤
 │ Tools       │                                     │ Properties    │
 │ ───────     │        PREVIEW VIEWPORT             │ ──────────    │
 │ [New curve] │   (mannequin head, groom rendered,  │ Guide count:  │
 │ [Comb]      │    lit, orbit camera)               │   1,248       │
 │ [Cut]       │                                     │ Render×:  64  │
 │ [Smooth]    │                                     │ Bend stiff: 0.4│
 │ [Jitter]    │                                     │ Wind:     0.6 │
 │ [Pin]       │                                     │ Root colour:  │
 │             │                                     │   #3a2a1a     │
 │ Brush size  │                                     │ Tip colour:   │
 │   [===|--]  │                                     │   #5a4a2a     │
 │ Strength    │                                     │ [Preview sim] │
 │   [==|----] │                                     │ [Bake LODs]   │
 ├─────────────┴─────────────────────────────────────┴───────────────┤
 │ Timeline: 0.00 ▶ [========|--------]  1.00   Wind preset: Breeze  │
 └───────────────────────────────────────────────────────────────────┘
```

Each tool is a Phase 6 command — one undo per stroke, same discipline Phase 8 §3 terrain brushes use. Comb rotates tangents toward stroke direction; Cut shortens guides to hit-point distance; Smooth is Laplacian relax; Jitter randomises lengths by ±N% (seeded per-asset); Pin toggles skeleton-bound roots. `[Preview sim]` runs live PBD in the editor; `[Bake LODs]` writes the density ladder so runtime doesn't re-interpolate from scratch.

## 6. Skeleton attachment

Hair roots attach to a named bone chain — conventionally `head_root` — at import. Groom Editor lets authors override which bone any guide binds to, for braids/tails.

```rust
pub struct HairInstance {
    pub groom:    AssetHandle<Groom>,
    pub skeleton: ComponentId<SkeletonPose>,  // Phase 24/35
    pub bind:     Vec<HairBind>,              // bone + local offset
    pub sim:      Vec<StrandSim>,
}
```

Tick order: `animation_tick → hair::pre_pin → hair::pbd → hair::expand (compute) → render`. Running hair after animation means IK-driven heads carry correct hair; running before expand means the strip builder sees the solved positions.

## 7. SPH fluid — small-scale splashes

SPH is right for small, unconfined, fast fluid: splashes, puddles, beverages, blood. Also the easiest compute-only fluid to ship — every step reduces to "for each particle, neighbour loop."

```rust
pub struct SphParams {
    pub particle_mass:   f32,
    pub rest_density:    f32,   // water ≈ 1000
    pub kernel_radius:   f32,   // h
    pub stiffness:       f32,
    pub viscosity:       f32,
    pub surface_tension: f32,
    pub gravity:         Vec3,
}
```

Per-tick compute passes: `build_grid` (hash to h-sized cells) → `compute_density` (gather 27 neighbours) → `compute_forces` (pressure + viscosity + tension + external) → `integrate` (symplectic Euler, clamp max speed) → `resolve_colliders` (project vs Rapier static colliders) → `emit_events` (impact-speed threshold → SplashEvent). Neighbour search is the cost driver; authors bound per-domain (no world-fill).

Limits: Medium 16k × 1 domain, High 64k × 4, Ultra 256k tier-deep. Low: no SPH, falls back to Phase 18 sprite splash. Foam/spray spawn as Phase 18 particles via a dedicated emitter variant that reads the SPH domain's high-curvature flags each tick — don't author foam twice.

## 8. FLIP fluid — small pools, contained bodies

FLIP is hybrid: particles carry velocity (low numerical diffusion), pressure-projection happens on a grid. Right for contained fluid — pools, tanks, buckets, small rivers — where you want an actual surface, not just splash particles. **High tier only**: the Poisson pressure solve is the cost Medium can't eat.

Passes: P2G (scatter velocities to MAC grid) → gravity → velocity extrapolation into air → Jacobi pSolve (20/40/80 iters configurable) → project (subtract gradient) → G2P (α-blend FLIP↔PIC) → advect → static-collider push-out. Grid fixed per asset (64³ or 128³); domain is axis-aligned internally.

Budgets: High = 1 domain, 64³, 128k particles. Ultra = 2 domains, 128³, 512k. Medium or lower loads of FLIP scenes are transparently substituted with SPH of the same bbox plus a cook-time warning — don't silently drop, don't refuse to run, do substitute.

## 9. Fluid surface rendering

Two paths, per-asset.

**Screen-space (default).** Render particles as thick spheres to depth-only → bilateral blur → reconstruct view-space normals → shade as thin dielectric with SSR (Phase 21), refract scene behind by the normal → composite before forward transparents. Medium+. Looks like water on contact; less convincing on calm large surfaces (use a plane mesh — ocean scale is a future phase, not this one).

**Marching cubes (opt-in).** For hero shots or low-motion effects (puddle, held drink): extract a mesh from density field, update at 15–30 Hz (not every tick), render as a translucent mesh through Phase 20 material graph. Higher quality, higher cost.

```rust
pub enum FluidSurface { ScreenSpace, Mesh { update_hz: f32 } }
```

## 10. Smoke and fire — voxel Navier-Stokes

A third solver: Navier-Stokes on a 3D sparse voxel grid (density + velocity + temperature), Jos-Stam-style with VDB-shaped sparsity for empty cells. Used for smoke plumes, explosion aftermath, campfires, fog swirls.

```rust
pub struct GasGrid {
    pub resolution:  [u32; 3],     // 32³/64³/128³
    pub cell_size:   f32,
    pub density:     Texture3D,    // f16
    pub velocity:    Texture3D,    // rgba16f, MAC-staggered logically
    pub temperature: Texture3D,    // f16
    pub buoyancy:    f32,
    pub dissipation: f32,
    pub vorticity:   f32,          // confinement strength
}
```

Per-tick passes: `advect_density → advect_velocity → apply_buoyancy → vorticity_confinement → divergence → jacobi_pressure (N iters) → project`.

Phase 18 emitters gain `Render::Volume { grid }` — front-to-back ray-march in the same transparent pass as volumetric fog (Phase 21). Fire uses blackbody-temperature lookup; smoke uses density × colour. Source injection goes the other way: a Phase 18 emitter tagged `SourceInto { grid }` seeds density/temperature from its rasterised footprint each tick. Medium runs one 32³ grid; High one 64³; Ultra one 128³ or two 64³.

## 11. `.rfluid` asset and preset library

```rust
pub enum FluidKind {
    Sph { params: SphParams,    emitter: SpawnRule, surface: FluidSurface },
    Flip { params: FlipParams,  emitter: SpawnRule, surface: FluidSurface },
    Gas { grid: GasGridDef,     sources: Vec<GasSource>, render: GasRender },
}

pub struct FluidAsset {
    pub name:   String,
    pub domain: Aabb,
    pub kind:   FluidKind,
    pub budget: Budget,
}
```

Preset library shipped with the engine:

| Preset          | Kind | Notes                                       |
|-----------------|------|---------------------------------------------|
| `water_splash`  | SPH  | impact-event driven, 8k particle burst      |
| `puddle_drip`   | SPH  | continuous low-rate, surface: ScreenSpace   |
| `beverage_full` | FLIP | 32³ grid, mesh surface, High only           |
| `small_fire`    | Gas  | 32³, strong buoyancy, blackbody shade       |
| `smoke_plume`   | Gas  | 64³, low buoyancy, mid dissipation          |
| `waterfall`     | SPH  | continuous emitter, foam spawn enabled      |
| `blood_splatter`| SPH  | surface tension high, short lifetime, ScreenSpace |

Preset browser is a tile grid; each tile shows an auto-baked 1-second loop — same trick Phase 18's VFX browser uses for emitter thumbnails.

## 12. Audio hand-off

Splash, flow, impact events are typed structs on the Phase 17 event bus, not hard-coded `audio::play` calls. The sim doesn't know about audio assets.

```rust
pub enum FluidAudioEvent {
    Splash     { at: Vec3, intensity: f32, domain: FluidId },
    Impact     { at: Vec3, speed: f32,     domain: FluidId },
    Flow       { at: Vec3, rate: f32,      domain: FluidId },
    Ignite     { at: Vec3, intensity: f32 },
    Extinguish { at: Vec3 },
}
```

Projects wire these in the project-level event map (same file Phase 25's impact-sounds live in). Empty mapping = silent. Emission is rate-limited per domain (min 20 ms between splashes, 80 ms between flows) to stop a single burst firing 300 voices.

## 13. Budgets and auto-scaling

Each instance declares a `Budget`:

```rust
pub struct Budget {
    pub max_cpu_ms: f32,
    pub max_gpu_ms: f32,
    pub priority:   u8,    // 0..255, low scaled first
}
```

Runtime keeps an EMA of each instance's cost. When the frame total exceeds the tier budget: scale the cheapest, lowest-priority instance first (drop iterations, reduce sub-steps, halve particle count); escalate next frame if still over; never disable outright unless it would otherwise stall — surface `BudgetSaturated` telemetry to the Phase 8 §6 profiler. Mirrors Phase 18's particle auto-throttle for the same reason: content *will* exceed budget on day one, and refusing to run it is worse than running it degraded.

## 14. PIE integration (Phase 7 Stop)

On PIE Stop: every `HairInstance` resets `sim.pos[i] = groom.bind_pose[i]`, `prev_pos = pos`; every SPH/FLIP domain clears particle buffers and respawns from `emitter` rest-state; every GasGrid zeroes density/velocity/temperature; rate-limited audio timers reset so a restart doesn't swallow the first splash.

Determinism rule inherited from Phase 25: SPH/FLIP/gas solvers all seed RNG from the Phase 10 replay-stable RNG, run fixed-dt only, use compute atomics only where order-independent (density accumulation). FLIP pressure solve is Jacobi (deterministic) — no Gauss-Seidel, no red-black, no CG.

## Build order

1. Hair strand rendering — strip expansion compute + Kajiya-Kay shader. No sim yet, static groom only, one hard-coded test asset. Validates the render path in isolation.
2. PBD hair physics — wire the solver, pin to a test skeleton, verify stability at 1/60 fixed dt.
3. Groom Editor — AssetEditor scaffold, comb/cut/smooth/jitter, `.rgroom` read/write, preview viewport.
4. Fur LOD — density ladder, card-based Low fallback, distance interpolation.
5. SPH fluid — compute passes, screen-space surface, foam handoff to Phase 18, splash event to Phase 17.
6. FLIP fluid — MAC-grid, pressure solve, mesh surface option, High-tier gate plus SPH substitution for lower tiers.
7. Smoke/fire voxel grid — Navier-Stokes passes, volumetric rendering handshake with Phase 18.
8. Audio hand-off polish — rate-limit tuning, preset-library sound bindings, profile event spam.

Each step lands with its own PR, its own tests, and extends the `tests/determinism/` harness established in Phase 25.

## Scope ❌

Explicitly out of scope for Phase 41 — call-outs, not omissions:

- ❌ Film-quality FEM destruction. Phase 25 shipped pre-fractured Voronoi; Phase 41 does not revisit.
- ❌ Ocean-scale water surface (Gerstner-plus-FFT, shore foam, refraction at horizon scale). That's a separate future rendering phase if we ever want it.
- ❌ Fully two-way rigid-fluid coupling at AAA complexity. Fluid reads Rapier colliders; rigid bodies do not receive fluid forces in 41 beyond a coarse drag applied at impact events.
- ❌ Differentiable simulation / gradient-based authoring. Research-tier, not a user-facing tool.
- ❌ Character biological fluids — blood flow in vessels, sweat film, tears. Out of scope both technically and in taste.
- ❌ Cloth dynamics beyond what Phase 25 shipped. No new cloth solver here; hair does not reuse cloth PBD and cloth does not reuse hair PBD.
- ❌ Hardware ray-traced water caustics. We don't do HW-RT anywhere in the engine yet (Phase 21 §7 declined reflections); we don't start here.
- ❌ GPU-driven strand self-collision at correctness parity with offline. The hash-grid approximation is the whole story.

## Risks

- **Strand memory on High/Ultra.** 100k render strands × 16 points × 32 bytes = 50 MB per character. With four visible characters that's 200 MB before any physics state. Mitigation: guide-count-capped authoring, render strand interpolation entirely on GPU (no CPU-side render strand array), strict LOD distance.
- **FLIP pressure-solve cost blowing Ultra budget.** Jacobi at 40 iterations on 128³ is expensive; CG is faster but we picked Jacobi for determinism. Mitigation: per-asset iteration cap, auto-downshift via budget system (§13), no FLIP below High.
- **Screen-space fluid sorting artifacts** — particles behind opaques render wrong without correct depth test; tiny fluid bodies vanish into the sub-pixel. Mitigation: bilateral blur radius scales with resolution, Phase 21 prepass depth is authoritative, known failure case documented.
- **Gas-grid popping at camera translation.** World-aligned grid boundary cuts the plume mid-view. Mitigation: asset declares domain bbox, emitter-parent-follow option offsets grid origin to nearest cell, no fractional re-sample.
- **Determinism regression from any third-party compute library.** We write our own compute kernels; no wgpu-distance dependencies for SPH/FLIP/gas. Every PR runs the Phase 25 determinism harness with a fluid scene added.
- **Hair attachment drift on scaled skeletons.** Non-uniform scale on head bones produces shear. Mitigation: groom editor refuses to bake a non-uniformly-scaled bind, surfaces a warning; runtime re-orthonormalises bone basis for hair root.
- **Per-effect budget starvation.** A dozen SPH domains in one scene sum to more than one FLIP. Mitigation: budget reporting surfaces in Phase 8 §6 profiler as a dedicated fluid panel, cook-time warning if static scene total exceeds tier budget.

## Exit criteria

1. A character with a 4096-guide groom on Ultra renders at 60 fps on reference hardware (RTX 4070-class), hair sim included, in a Phase 21 lit scene.
2. Medium-tier target runs the same scene at 60 fps with reduced strand count and no visible popping at LOD boundaries.
3. Low-tier WebGL2 target runs the same scene with card-based fur at 60 fps; no strand rendering attempted.
4. An SPH water splash with 16k particles runs at 60 fps on Medium; screen-space surface composites correctly against Phase 21 opaque + SSR.
5. A FLIP puddle with 128k particles, 64³ grid, mesh surface renders at 60 fps on High.
6. A 64³ smoke plume with one Phase 18 source emitter runs at 60 fps on Medium; a 128³ equivalent on High.
7. Groom Editor round-trips: open preset, edit, save, reopen, preview matches. Undo collapses one stroke into one entry. Dirty flag behaves as every other AssetEditor in Phase 8.
8. PIE Stop resets every active hair and fluid sim to its initial state within one frame; a replay from Phase 10 rewind produces bit-identical fluid positions across two runs on the same binary.
9. Splash, flow, impact, ignite events appear on the Phase 17 event bus with correct throttling; a scene with 10 simultaneous splash sources does not exceed 20 audio voices through fluid events.
10. Budget auto-scaler demonstrably drops a deliberately over-budget scene from 40 ms/frame to under the tier ceiling within 10 frames, surfaces in profiler, never outright disables an instance.
11. Cook-time warnings fire when a scene loads FLIP / 128³ gas / Ultra grooms into a Medium-or-lower target; substitutions apply, game still runs.
12. Determinism harness extended with one hair, one SPH, one FLIP, one gas scene; byte-identical snapshots over 30 s on two threads.
