# Phase 26 — AI, Behavior Trees & Navigation

Unreal's AIModule is one of the three or four features most commonly cited when teams evaluate switching engines and bounce off. It bundles a navigation mesh (Recast/Detour), pathfinding, a behavior-tree runtime and editor, a blackboard, a perception system, and the Environment Query System — all wired together and debuggable out of the box. RustForge, post-1.0, has **none of this**. You can author a scene, script gameplay, and ship a game, but the moment a designer wants an enemy that patrols, reacts to sight, and flanks the player, they are on their own with hand-rolled state machines in Rust.

Phase 26 closes that gap. It builds a nav-mesh baker on top of the collision world from Phase 22 (physics), a pathfinding layer, a behavior-tree runtime with a dedicated RON asset format and a node-graph editor that reuses the Phase 20 shared widget, a typed blackboard, a perception system, and an MVP Environment Query System. It extends Phase 10's debugger with AI-specific inspectors. It is the largest "fill the Unreal gap" phase after physics, and like physics it earns its keep on the first playtest where the enemies stop walking into walls.

## Goals

By end of Phase 26:

1. Nav meshes bake from scene colliders into `.rnav` assets, incrementally rebuild on scene edits, and are visualizable in the viewport.
2. Pathfinding returns string-pulled paths on the nav mesh in under 1 ms for typical queries; optional ORCA crowd avoidance is available for up to ~256 agents.
3. Behavior trees author as `.rbt` RON assets, run on a fixed tick inside the ECS, and support sequence / selector / decorator / task / service node kinds.
4. A behavior-tree editor reuses the Phase 20 node-graph widget, with a dedicated node palette, live validation, and round-trip RON.
5. A typed blackboard provides per-agent key-value storage that BT nodes read and write through a checked API.
6. The perception system (sight, hearing, damage) produces stimuli consumed by BT services; cones and radii are configurable per agent and visualized in the editor.
7. AI tasks are user-extensible via both compile-time Rust (`impl BtTask`) and the Phase 11 WASM plugin surface.
8. An MVP EQS runs generator / filter / scorer pipelines for "find cover" and "find flank" queries; results integrate with BT tasks and render as a scoring heat-map overlay.
9. The Phase 10 debugger gains a live BT view (active path highlight), blackboard inspector, nav-mesh overlay, and EQS heat-map toggle.

## 1. Navigation mesh

Ship `recast-rs` (or the equivalent Rust-native Recast port — if none is production-ready, bind the C Recast/Detour from `recastnavigation` via a thin `rustforge-nav-sys` crate and keep the surface narrow). Recast has been the de-facto nav-mesh pipeline for fifteen years; writing a new one is a phase of its own.

### 1.1 Nav volume entity

```rust
#[derive(Component, Reflect)]
pub struct NavVolume {
    pub bounds: Aabb,                 // world-space region to bake
    pub agent: AgentProfile,          // radius, height, max slope, step height
    pub tile_size: f32,               // meters per Recast tile; default 32.0
    pub cell_size: f32,               // voxel XZ size; default 0.25
    pub cell_height: f32,             // voxel Y size; default 0.20
}

#[derive(Reflect, Clone)]
pub struct AgentProfile {
    pub radius: f32,
    pub height: f32,
    pub max_slope_deg: f32,
    pub step_height: f32,
}
```

Multiple `NavVolume`s can coexist; each produces its own `NavMeshAsset` keyed by GUID. Overlapping volumes with differing `AgentProfile` cover the "small spider and large ogre walk different meshes" case without a separate nav-layer system.

### 1.2 Baking

Input geometry comes from Phase 22's `ColliderShape` components plus any mesh tagged `#[nav_static]`. The baker runs in a `rayon` task pool, tile by tile, and writes a `NavMeshAsset`:

```
crates/rustforge-nav/
├── bake/
│   ├── collect.rs       # scene -> Recast input geometry
│   ├── tile.rs          # one tile bake on a worker
│   └── stitch.rs        # detour tile mesh assembly
├── asset.rs             # NavMeshAsset (serde + guid)
├── query.rs             # pathfinding API
└── debug.rs             # triangle soup for viewport overlay
```

Bakes are **per-tile and incremental**. When the editor observes a collider transform change or a static mesh edit, it flags overlapping tiles dirty and requeues them. A full rebake is a project command; typical edits touch one to four tiles.

### 1.3 Editor visualization

A View menu toggle overlays the nav-mesh triangles (semi-transparent blue, thicker edges on tile boundaries). Off-mesh connections render as yellow arcs. The overlay is a single wgpu pass sourced from `NavMeshAsset::debug_triangles()` and cached until the asset GUID changes.

## 2. Pathfinding

Detour-style A\* with a string-pulling funnel smoother. The public API is async-looking but synchronous for short paths:

```rust
pub struct NavQuery<'a> {
    mesh: &'a NavMeshAsset,
    filter: &'a QueryFilter,
}

impl<'a> NavQuery<'a> {
    pub fn find_path(&self, from: Vec3, to: Vec3) -> Result<Path, NavError>;
    pub fn raycast(&self, from: Vec3, to: Vec3) -> Option<Vec3>;        // hit or None
    pub fn random_point_in_radius(&self, center: Vec3, r: f32) -> Option<Vec3>;
}
```

`QueryFilter` maps area types (walk, swim, crouch, jump-link) to costs so designers can make AI prefer roads over grass without a separate graph.

### 2.1 Crowd avoidance (optional)

ORCA via `rvo2-rs` or equivalent, wired as a `CrowdAgent` component. Agents register with a `Crowd` resource; the solver runs once per fixed tick before `TransformPropagate`. Keep it behind a `crowd` feature flag — small games with a handful of enemies don't need it and the solver is not free.

### 2.2 Path following

A `PathFollow` component owns the current `Path`, a target index, and a look-ahead distance. A system in the `AiStage` moves the agent along the path and clears the component when the goal is reached. Steering is deliberately left to the game — BT tasks can own richer locomotion.

## 3. Behavior tree runtime

BT over state machines for the default. State trees (Unreal's newer alternative) are discussed in §9 and deferred.

### 3.1 The asset

`.rbt` is RON, versioned, and round-trips through the editor without reordering keys:

```ron
BehaviorTree(
    version: 1,
    blackboard: "assets/ai/grunt.bb",
    root: Selector(
        children: [
            Sequence(
                children: [
                    Decorator(kind: BlackboardSet("target"), child:
                        Task(kind: "MoveTo", params: { "key": "target", "accept_radius": 1.5 })),
                    Task(kind: "Attack", params: { "key": "target" }),
                ],
            ),
            Task(kind: "Patrol", params: { "route": "patrol_a" }),
        ],
    ),
    services: [
        Service(kind: "Perception", interval: 0.25, params: {}),
        Service(kind: "AmmoWatch",  interval: 1.0,  params: {}),
    ],
)
```

### 3.2 Node kinds

```
Root
 └── Selector  (try children left-to-right, succeed on first success)
      ├── Sequence  (run children left-to-right, fail on first failure)
      │    ├── Decorator  (wraps one child with a precondition or modifier)
      │    │    └── Task  (leaf: action that ticks and returns Running/Success/Failure)
      │    └── Task
      └── Task
Services (not in the execution tree; tick at fixed intervals while their owning branch is active)
```

Decorators ship with: `BlackboardSet(key)`, `BlackboardCompare(key, op, value)`, `Cooldown(seconds)`, `Loop(count)`, `Invert`, `ForceSuccess`, `ForceFailure`. That covers 90% of what Unreal designers reach for. More land as user extensions.

### 3.3 Tick model

BTs tick in a dedicated `AiStage` that runs after `Input` and before `Physics` at a fixed 20 Hz by default (configurable per-tree). Per-tick cost is bounded: each tree walks at most one active path plus service pumps. A tree never recurses across frames — `Running` leaves freeze the walk until the next tick.

```rust
pub trait BtTask: Send + Sync + 'static {
    fn start(&mut self, ctx: &mut BtCtx) { let _ = ctx; }
    fn tick(&mut self, ctx: &mut BtCtx, dt: f32) -> BtStatus;
    fn end(&mut self, ctx: &mut BtCtx, status: BtStatus) { let _ = (ctx, status); }
}

pub enum BtStatus { Running, Success, Failure }
```

`BtCtx` exposes the owning entity, the blackboard, the scene query API, the nav query API, and a structured log channel that the debugger (§8) subscribes to.

## 4. Behavior tree editor

The BT editor is an `AssetEditor` (Phase 8 §1) that reuses the node-graph widget from Phase 20. That widget already handles pan, zoom, selection, box-select, reroute nodes, connection validation, and marquee copy/paste — none of that is rebuilt here.

### 4.1 What this editor adds on top of Phase 20

- **Palette panel** on the left, grouped by category: Composites, Decorators, Tasks (core + plugin), Services.
- **Execution-order annotations**: child-connection ports render their index (`1.`, `2.`, `3.`) so designers see the left-to-right execution order without reading the layout.
- **Validation pass** on every graph edit: cycles, dangling tasks, wrong port types. Errors surface as red badges on nodes and in a bottom-strip "Problems" list (mirrors the style from the Content Browser search in Phase 5).
- **Blackboard link**: a sidebar selector picks the associated `.bb` asset; decorators and tasks with key-typed params get a real dropdown, not a free-text field.

### 4.2 RON round-trip

Editor saves walk the graph in stable order (DFS, child-index sorted) and emit RON with no positional jitter. Graph positions are stored in a sibling `.rbt.layout` file so diffs on the real asset stay semantic.

## 5. Blackboard

```rust
#[derive(Asset, Reflect)]
pub struct BlackboardSchema {
    pub keys: Vec<BbKey>,
}

#[derive(Reflect, Clone)]
pub struct BbKey {
    pub name: SmolStr,
    pub ty:   BbType,        // Bool, Int, Float, Vec3, Entity, String, AssetRef
    pub default: BbValue,
}
```

A runtime `Blackboard` component is allocated per agent from its schema. Reads and writes go through typed accessors — `bb.get_vec3("target_pos")?` fails fast if the key name or type is wrong. String interning on `SmolStr` keeps lookups cheap.

Schemas are assets so multiple trees can share the same key layout, and the editor can populate dropdowns everywhere a blackboard key is a parameter.

## 6. AI tasks (extension point)

Two paths to custom tasks. Both register through Phase 11's plugin surface.

### 6.1 Compile-time Rust

```rust
#[derive(BtTask)]
#[bt_task(name = "MoveTo", category = "Locomotion")]
pub struct MoveTo {
    #[bb_key] pub target: BbKey<Vec3>,
    pub accept_radius: f32,
}

impl BtTask for MoveTo { /* start / tick / end */ }
```

The `BtTask` derive wires registration, RON parameter decoding, and editor palette metadata in one pass. No runtime cost over a hand-written `impl`.

### 6.2 WASM

Same signature, shipped as a Phase 11 plugin. Host exposes a narrow ABI: blackboard getters/setters, nav queries, scene queries, and structured logging. No raw ECS access — WASM tasks cannot mutate components outside their agent.

## 7. Perception

```rust
#[derive(Component, Reflect)]
pub struct Perception {
    pub sight: Option<SightSense>,     // cone: half_angle_deg, range, forgetting_time
    pub hearing: Option<HearingSense>, // radius, sensitivity
    pub damage: bool,                  // always-on; any incoming damage registers
}

pub struct Stimulus {
    pub source: Entity,
    pub pos:    Vec3,
    pub kind:   StimulusKind,          // Sighted | Heard | Damaged
    pub time:   f32,
    pub strength: f32,                 // 0..1, decays until forgetting_time
}
```

The perception system runs in `AiStage` at 10 Hz by default. It emits stimuli into a per-agent ring buffer; the canonical `Perception` BT service drains that buffer into blackboard keys (`perceived_target`, `last_seen_pos`, `time_since_lost`).

Sight checks go through the physics layer (Phase 22) as visibility raycasts against a query filter; hearing is a world-space radius on `NoiseEvent` emitters pushed by gameplay. Damage plugs into the standard damage event bus.

Editor visualization draws sight cones and hearing radii in the viewport when an agent is selected, with colors for the three senses matching the stimulus types.

## 8. Environment Query System (MVP)

Unreal's EQS is a three-stage pipeline: **generator** makes candidate points, **filters** remove bad ones, **scorers** rank the rest.

```rust
pub struct EqsQuery {
    pub generator: Box<dyn EqsGenerator>,     // Grid, DonutAroundTarget, VisibleFromCover
    pub filters: Vec<Box<dyn EqsFilter>>,     // InsideNavMesh, NotVisibleTo(entity), DistanceFrom(entity, min, max)
    pub scorers: Vec<(Box<dyn EqsScorer>, f32)>, // (scorer, weight)
}
```

Ship two baked queries end-to-end in this phase — **FindCover** (grid around target, filtered to nav mesh, filtered to "not visible to threat", scored by distance to self) and **FindFlank** (donut around target, filtered to visible from target's back arc, scored by angle off threat forward). Everything else is on the post-26 backlog.

An `EqsRun` BT task fires a query, awaits one frame, writes the top result into a blackboard key. Queries run off-thread on the `rayon` pool; 128-point queries finish inside one 20 Hz AI tick in typical scenes.

### 8.1 Heat-map overlay

When an EQS task is selected in the debugger (§9), the viewport overlays scored points as a colored point cloud (blue = low score, red = high, filtered-out shown dim). This is the single most useful bit of EQS tooling Unreal has and is cheap to replicate.

## 9. State trees — deferred

Unreal introduced StateTree as a flatter, easier-to-reason-about alternative to BTs for long-running NPC logic. It composes well with BTs (state owns a subtree) and is worth a later phase. For Phase 26 the recommendation is explicit: **BT first, StateTree later**. Shipping both at once doubles editor surface area for a speculative benefit.

When StateTree lands it should reuse the Phase 20 widget, the blackboard (§5), and the perception system (§7) unchanged. Only the runtime and asset format are new.

## 10. AI controller pattern

```rust
#[derive(Component)]
pub struct AiController {
    pub tree: AssetRef<BehaviorTreeAsset>,
    pub blackboard: Blackboard,
    pub perception: Perception,
    pub bt_state: BtRuntimeState,            // active path, service timers
}
```

One component, one entity, one tree. Designers who want a pawn plus a separate controller entity (the classic Unreal split) can model that by putting `AiController` on a child entity that references the pawn via a blackboard `Entity` key. Keep the core model simple; idioms layer on top.

## 11. Debugging — extends Phase 10

The Phase 10 debugger gains an AI tab with four inspectors, each attachable to any selected entity carrying `AiController`:

- **BT view** — live render of the tree graph with the currently active path highlighted in orange, last-tick results annotated on each visited node, and a small scrollback of recent ticks. Clicking a node pins a breakpoint that pauses the agent on entry.
- **Blackboard inspector** — table of keys with live values; reflection-driven so custom `BbType`s render with their editor widgets.
- **Nav-mesh overlay** — the viewport toggle from §1.3, plus a query tracer that shows the A\* open list and final string-pulled corridor for the last path request from the selected agent.
- **EQS heat-map** — §8.1, driven by the last EQS run on the selected agent.

All four are read-only observers; nothing here mutates game state, so they are safe to leave on during playtest captures.

## Build order

1. **Nav mesh** — baker, asset, editor overlay. Proves the geometry pipeline end to end.
2. **Pathfinding** — A\*, string-pulling, `NavQuery`, `PathFollow`. Agents can walk a preset path.
3. **BT runtime** — core node kinds, `BtTask` trait, fixed-tick execution, derive macro, RON asset.
4. **BT editor** — Phase 20 widget reuse, palette, validation, blackboard link, round-trip save.
5. **Blackboard + perception** — typed schemas, sight/hearing/damage stimuli, viewport visualization.
6. **EQS (MVP)** — generator/filter/scorer pipeline, FindCover + FindFlank, heat-map overlay, BT task integration.

Crowd avoidance, WASM tasks, and the debugger AI tab slot in parallel to 3–6 as those dependencies land.

## Scope ❌

Explicitly out of Phase 26:

- ❌ LLM-driven or reinforcement-learning agents — a future phase once the deterministic stack is solid and shippable.
- ❌ Facial animation, lip-sync, or emotion systems for NPCs — belongs with the animation phase.
- ❌ Dialogue systems, branching conversation authoring, localization of NPC lines.
- ❌ Procedural quest or mission generation.
- ❌ Level-of-detail AI simulation (far-field approximation, crowd decimation). Worth doing; not here.
- ❌ Squad-level coordination primitives beyond what a shared blackboard buys.
- ❌ A networked-AI authority model — Phase 25 (networking) owns replication of `AiController` state.
- ❌ StateTree runtime and editor — called out in §9, deferred by design.
- ❌ Off-mesh link authoring tool — bakes respect manual off-mesh connections in the scene but a dedicated editor comes later.

## Risks

- **Recast binding maturity**: if `recast-rs` is not production-ready at implementation time, the C FFI fallback adds a build dependency and a `build.rs` that many users have not hit before. Budget one engineer-week to smooth Windows/macOS/Linux builds and document the toolchain requirement.
- **Editor node-graph reuse breakage**: Phase 20's widget was designed for shaders and materials first. BT-specific needs (child ordering, service decoration, decorator-wraps-child) may stress its assumptions; reserve time to extend it without forking.
- **Incremental bake correctness**: tile stitching is the classic Recast footgun. Ship a full-rebake fallback command from day one so users can always get a clean mesh.
- **BT performance at scale**: 500 agents each ticking 50-node trees at 20 Hz is 500 k node visits per second. Benchmark early, and if this bites, the fix is archetype-ordered batching of `AiStage`, not a runtime rewrite.
- **EQS scope creep**: it is tempting to implement the full Unreal generator/filter/scorer zoo. Resist. Two end-to-end canned queries teach more than twenty half-finished primitives.
- **Perception false positives**: sight raycasts through complex scenes catch foliage and thin props. Expose the raycast filter in `Perception` so designers can tune it per game.
- **Plugin ABI surface growth**: WASM tasks demand a stable blackboard and nav-query ABI. Cut the surface to the minimum that the two canned EQS queries need, and version it from day one.

## Exit criteria

Phase 26 ships when all of the following are true:

1. A demo scene with 32 patrolling agents runs at 60 FPS on the reference box, with nav-mesh baked from scene colliders and visualized in the viewport.
2. `cargo test -p rustforge-nav` passes unit tests for path-find correctness on six hand-built meshes (flat, stairs, bridge, gap, donut, pillar room).
3. The BT editor round-trips every node kind through RON with zero key reordering, verified by a snapshot test over 12 authored trees.
4. An example project `examples/grunts/` ships a `Grunt` agent with patrol + perception + chase + attack behaviors, authored entirely in the editor with no gameplay Rust beyond the custom `Attack` task.
5. The debugger AI tab shows live BT state, blackboard values, nav overlay, and EQS heat-map for the selected agent, and keeps up at 60 FPS with the example scene running.
6. FindCover and FindFlank return a top-scored point within 2 ms on a 128-candidate grid in the reference scene.
7. Docs cover: BT authoring walkthrough, custom-task cookbook (Rust + WASM), blackboard schema reference, perception tuning guide, nav-mesh baking knobs, EQS MVP recipes. One migration note for users who prototyped AI with hand-rolled state machines pre-26.
