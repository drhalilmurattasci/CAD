# Phase 40 — Procedural Content Generation

Unreal 5.2 shipped the PCG Framework and changed the expectation of what open-world authoring looks like. A level designer drops a handful of splines and volumes, attaches a graph that scatters, masks, filters, and spawns, and the world fills itself — forest floors, ruin fields, dirt paths, enemy encampments — with no per-instance placement and no side trip through Houdini. Unreal's earlier answer was the Foliage tool plus rule processors plus Blueprints; the PCG Framework unified all of it into a DAG with typed edges and bake-or-runtime evaluation. Houdini Engine is the other reference: the same model, older and more expressive, behind a per-seat license and a separate app.

Phase 32 gave RustForge rule-based scattering for foliage, but it is not a graph, not composable, and not reusable outside the terrain tab. Phase 40 lifts that pattern into a first-party PCG system spanning terrain, spawning, nav seeds, and any future plugin consumer. It reuses the shared node-graph widget from Phase 20 (PCG is the fifth domain after material / audio / VFX / dialogue). It is deterministic — every node takes a seed — because Phase 14 replication and Phase 33 replay compat both require it. Bake integrates with Phase 31 world-partition cells; runtime evaluation respects a per-frame budget. The bet is *incremental bake*, not real-time AAA-scale PCG — Unreal recommends bake for anything shippable, and we make bake a first-class path.

## Goals

By end of Phase 40:

1. **PCG graph asset** — `.rpcg` RON file, DAG of typed nodes, edited in an AssetEditor-style tab (Phase 8).
2. **Shared graph widget reuse** — fifth consumer of `rustforge-node-graph` (Phase 20); no new graph UI code.
3. **Typed edges** — point cloud, mesh, spline, density field, entity set, int / float scalar; no erased "any" port.
4. **Generator node palette v1** — grid scatter, Poisson disk, cluster scatter, from-spline, from-mesh-surface, from-volume.
5. **Filter / transform palette v1** — mask by slope / altitude / material-layer / tag, set-ops (union / intersect / difference), randomize, noise-offset, attribute filter, cluster.
6. **Consumer palette v1** — spawn entities (component list as attribute), place foliage (extends Phase 32), carve terrain heightmap, scatter meshes, seed nav-mesh (Phase 26).
7. **Per-node bake toggle** — bake for static world chunks, runtime for gameplay-driven spawns; asset cache stores baked outputs.
8. **Deterministic seeding** — every node accepts a seed; graph output reproducible across runs, machines, replays.
9. **Debug visualizer** — viewport overlay showing intermediate stage points, masks, and final instances (extends Phase 10).
10. **Houdini Engine adapter** — optional plugin: load `.hda` as a PCG node with exposed parameters.
11. **World partition integration** — graphs assigned to cells; bake incremental per-cell, reruns on cell dirty.
12. **Runtime cost budget** — per-frame time cap; graph evaluation spreads across frames when exceeded.
13. **Plugin-authored nodes** — Phase 11 plugin API extended to register PCG nodes.

## 1. The graph model and how it reuses Phase 20

Phase 20 factored a domain-agnostic `rustforge-node-graph` crate with `NodeDomain` as the plug point. PCG is the fifth domain (after material, audio, VFX, dialogue). The contract is the same: the widget holds positions, draws bezier curves between compatible ports, and forwards edit events; the domain provides node types, port types, a palette, and a compile step.

```rust
// crates/rustforge-pcg/src/domain.rs
pub struct PcgDomain;

impl NodeDomain for PcgDomain {
    type Node          = PcgNode;
    type PortType      = PcgPortType;
    type CompileOutput = PcgProgram;          // flattened eval plan
    type CompileError  = PcgCompileError;

    fn palette() -> &'static dyn Palette<Self> { &PCG_PALETTE }
    fn compile(graph: &Graph<Self>) -> Result<PcgProgram, Vec<PcgCompileError>> {
        compile::plan(graph)
    }
}
```

Port types are narrow on purpose. Houdini gets away with untyped data-flow on thirty years of attribute conventions; we do not, and we don't want silent coercions.

```rust
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PcgPortType {
    Points,       // PointCloud: positions + attributes
    Mesh,         // MeshSet: instances with transforms
    Spline,       // SplineSet: Catmull-Rom / Bezier
    Density,      // DensityField: scalar grid, Phase 32 compatible
    Entities,     // EntitySet: spawn descriptors
    Scalar(ScalarKind), // Int | Float
}
```

```
           ┌──────────────── Compatibility ────────────────┐
           │                                               │
  Points ──┼── Points | Density (via rasterize) | Entities │
  Mesh   ──┼── Mesh                                        │
  Spline ──┼── Spline | Points (via sample)                │
  Density──┼── Density | Points (via threshold-sample)     │
  Entities─┼── Entities                                    │
  Scalar ──┼── Scalar (param inputs only)                  │
           └───────────────────────────────────────────────┘
```

No implicit coercions — explicit `SampleSpline` or `RasterizePoints` nodes cross domains. Always a named stage to inspect in debugging.

## 2. Node layout on disk — the `.rpcg` format

```
assets/pcg/forest_floor.rpcg
────────────────────────────
(
    schema: 1,
    seed: 0xBAD5EED,            // graph-level default; nodes may override
    nodes: [
        ( id: 1, kind: FromVolume,    params: { volume: @vol_forest } ),
        ( id: 2, kind: GridScatter,   params: { spacing: 2.0, jitter: 0.4, seed: 17 } ),
        ( id: 3, kind: MaskBySlope,   params: { max_deg: 32.0 } ),
        ( id: 4, kind: NoiseOffset,   params: { amp: 0.8, freq: 0.1, seed: 42 } ),
        ( id: 5, kind: SpawnFoliage,  params: { asset: @tree_oak, density: 1.0 } ),
    ],
    edges: [
        ( from: (1, "out"), to: (2, "bounds") ),
        ( from: (2, "out"), to: (3, "in") ),
        ( from: (3, "out"), to: (4, "in") ),
        ( from: (4, "out"), to: (5, "in") ),
    ],
    bake: (
        cells: All,               // All | Listed([CellCoord]) | None
        runtime_override: false,
    ),
)
```

Structural merge from Phase 12 works: nodes are records, edges are value-equal and hashable. Two authors adding nodes to the same graph merge without conflict.

## 3. Generator nodes

Generators produce the initial data stream. Six ship in v1; more by plugin (see §11).

### 3.1 GridScatter

```rust
pub struct GridScatter {
    pub bounds:  InputPort<Density>,   // mask: where to scatter (0.0..1.0 * density)
    pub spacing: f32,
    pub jitter:  f32,                  // 0.0 = exact grid, 1.0 = fully jittered in cell
    pub seed:    u64,
    pub out:     OutputPort<Points>,
}

impl PcgNode for GridScatter {
    fn eval(&self, ctx: &EvalCtx) -> PointCloud {
        let bounds = ctx.input(self.bounds);
        let mut rng = rng_for_node(ctx.graph_seed ^ self.seed, ctx.node_id);
        let mut out = PointCloud::with_capacity(bounds.estimated_samples());
        for cell in bounds.aabb_iter(self.spacing) {
            let base = cell.center();
            let j = self.jitter * self.spacing * 0.5;
            let p = base + vec3(rng.gen_range(-j..=j), 0.0, rng.gen_range(-j..=j));
            if bounds.sample(p) > rng.gen::<f32>() {
                out.push(Point::at(p));
            }
        }
        out
    }
}
```

### 3.2 Poisson disk

Bridson sampling with a cell lookup grid. AABB or density-field input; min-distance-guaranteed point output. Trees, rocks, anything that should *not* cluster.

### 3.3 ClusterScatter

Seed-and-spread: N Poisson seed points, N_i gaussian-falloff children per seed. Mushroom rings, rubble piles, enemy patrols.

### 3.4 FromSpline / FromMeshSurface / FromVolume

Converters pulling points from non-point geometry. `FromMeshSurface` uses area-weighted barycentric sampling for uniform density regardless of tessellation. `FromVolume` accepts Phase 32's volume brush.

## 4. Filter and transform nodes

These reshape the stream without changing its fundamental kind.

```
    Points ──► MaskBySlope ──► Points'     // drop points where terrain slope > threshold
    Points ──► NoiseOffset ──► Points'     // jitter by noise field
    Points ──► Randomize ───► Points'      // assign random rotation/scale attributes
    Points ──► Cluster ─────► Points'      // group by proximity, expose group_id attr
    Points ──► AttrFilter ──► Points'      // keep/drop by attribute predicate
    (A, B) ──► Union ──────► Points'       // set union by position hash
    (A, B) ──► Intersect ──► Points'
    (A, B) ──► Difference ─► Points'
```

Slope / altitude / layer masks read the Phase 32 terrain directly — no duplicated sampling path. `MaskByTag` reads the Phase 2 reflection tag set on spawned entities (useful for runtime graphs that re-scatter around player-built objects).

Transforms preserve point attributes (id, seed, group). Dropped points are dropped, not flagged — no "disabled" state; graphs that need one insert a `Split` node.

## 5. Consumer nodes

Consumers sink the stream into the engine. They have no output port — a consumer is a leaf.

### 5.1 SpawnEntities

```rust
pub struct SpawnEntities {
    pub points:     InputPort<Points>,
    pub components: Vec<ComponentSpec>,    // reflected type + field map; see Phase 2
    pub cell_owner: Option<CellCoord>,     // None = pick by point position
}
```

`ComponentSpec` reuses the reflection registry — script-authored components spawn like built-ins. Emits batched `Commands::spawn_batch` into the Phase 31 per-cell world.

### 5.2 PlaceFoliage

Phase 32 foliage was authored-by-brush; here it is authored-by-graph. Writes instanced transforms directly into the cell's foliage instance buffer. Replaces Phase 32's rule scatter — the `Scatter` tab rewires to open the bound `.rpcg`.

### 5.3 CarveTerrain

Density field or point cloud pushes / pulls the terrain heightmap. Bake-only — runtime carving invalidates GPU-resident RVT tiles.

### 5.4 ScatterMeshes

Points in, mesh instances out, written to the Phase 21 bindless instance table. Bake writes immutable static regions per cell; runtime writes to the dynamic region.

### 5.5 SeedNavMesh

Points in, nav-mesh "influence" markers out. Phase 26's nav bake expands walkable regions around them. No manual nav-volume painting for the enemy-camp clearing case.

## 6. Bake vs runtime

A per-node toggle, but it composes upward: if any downstream consumer is set to bake, everything feeding it is baked too (runtime input can't feed a baked output, the data doesn't exist yet). The compiler (§7) enforces this.

```
     ┌─────── node.bake = true  (default for static world) ────────┐
     │  - evaluated at editor-side cell bake time                  │
     │  - output written to asset cache keyed by                   │
     │      blake3(graph_guid || cell || seed || node_hash)        │
     │  - runtime load is a mmap of cached bytes                   │
     └─────────────────────────────────────────────────────────────┘

     ┌─────── node.bake = false  (runtime) ────────────────────────┐
     │  - evaluated in the PCG runtime system tick                 │
     │  - output lives for cell residency, regenerates on reload   │
     │  - subject to §9 per-frame budget                           │
     └─────────────────────────────────────────────────────────────┘
```

Default to bake. Runtime PCG is a hammer authors reach for too early. A forest is not dynamic; an enemy patrol around the player is. If the author flips a runtime consumer on and hits the frame budget ceiling (§9), the editor hints: "did you mean bake?"

## 7. The compile step

Graph → plan is a topological sort with constant folding and bake / runtime partitioning.

```rust
pub struct PcgProgram {
    pub bake_stages:    Vec<Stage>,   // run at editor bake time, per cell
    pub runtime_stages: Vec<Stage>,   // run on the main-thread PCG system
    pub seed_plan:      SeedPlan,     // precomputed per-node seeds (§8)
}

pub struct Stage {
    pub node_id:   NodeId,
    pub inputs:    SmallVec<[Slot; 4]>,
    pub output:    Slot,
    pub kind:      StageKind,         // Generator | Filter | Consumer
    pub cell_scope: CellScope,        // Global | PerCell
}
```

Compile errors route back to originating nodes with red badges, same pattern Phase 20 established for WGSL errors. Missing inputs, type mismatches, and bake-after-runtime violations all produce node-local errors, never silent partial output. Compile never crashes the editor.

## 8. Deterministic seeding

Phase 14 and Phase 33 replay both require it: a PCG graph must produce byte-identical output given the same (graph, cell, world seed, input assets). The seeding rule:

```
node_seed(node_id, graph_seed, world_seed, cell_coord) =
    blake3_to_u64( graph_seed  |
                   world_seed  |
                   cell_hash   |
                   node_id     |
                   node.seed_offset )
```

No node uses `thread_rng` or wall-clock time; every RNG is seeded through this chain. The runtime evaluator asserts it at debug-build time by running each stage twice and byte-comparing output. A mismatch is a bug ticket, not a warning — replay desync is the downstream consequence. Floating-point determinism follows Phase 14's physics path: FTZ / DAZ pinned, `libm` for transcendentals, no SIMD fast-path. Slower, accepted — replays and networked co-op matter more than the 10 % throughput left on the table.

## 9. Runtime cost budget

Runtime graphs execute in the PCG system after Phase 2's gameplay tick, wall-clock-budgeted:

```rust
pub struct PcgRuntimeBudget {
    pub ms_per_frame:   f32,    // default 2.0 ms at 60 Hz
    pub carry_over_cap: f32,    // max 4.0 ms borrowed from idle frames
}
```

When a runtime graph exceeds its slice, evaluation pauses at a stage boundary and resumes next frame. Stage boundaries are cheap — intermediate values live in the program's slot table (already heap-allocated), no "partial point cloud" state.

```
Frame N     : [Stage 1] [Stage 2] [Stage 3]·························  (budget hit)
Frame N+1   : ···························[Stage 4] [Stage 5]·······
Frame N+2   : ···································[Stage 6 Consumer]
```

The debugger (§10) lights deferred stages red so authors see the bottleneck; the Phase 8 profiler tab adds a PCG row broken down by graph and stage.

## 10. Debug visualizer

Extends Phase 10 overlays. A graph open in the editor has a toggle per node: "preview this stage". The viewport overlays the stage output.

```
┌─ Viewport ────────────────────────────────────────────────────┐
│                                                                │
│   Stage 2 (GridScatter)    ····  1024 pts                     │
│   Stage 3 (MaskBySlope)    ····   687 pts  (dropped 337)      │
│   Stage 5 (SpawnFoliage)   ▲▲▲   687 mesh instances           │
│                                                                │
│         · · · · · · ·     ← stage 2 raw points (green)        │
│         · ·   ·   · ·     ← stage 3 after slope mask (cyan)   │
│         ▲ ▲   ▲   ▲ ▲     ← stage 5 spawned trees (magenta)   │
│                                                                │
│   [show stage 2] [show stage 3] [hide stage 5]                │
└────────────────────────────────────────────────────────────────┘
```

Non-intrusive: no re-bake required — the editor runs the graph in preview mode on a sampled cell, caching intermediate outputs in memory. Toggles are instant. Large cells downsample above a configurable visible cap (default 50 000). Attribute inspection: click a point, get its attribute dict — ID, seed, mask values, group index, source generator. The feature Houdini users miss most in Unreal PCG; we ship it in v1.

## 11. Houdini Engine adapter

Opt-in plugin behind a Cargo feature and a Houdini Engine license check. When enabled, `.hda` files become importable assets that register as PCG nodes.

```rust
// plugins/rustforge-houdini-adapter/src/lib.rs
#[rustforge::plugin]
fn register(reg: &mut PluginRegistry) {
    reg.pcg_node_from_factory::<HdaNode>("Houdini HDA");
}

pub struct HdaNode {
    hda_path: PathBuf,
    session:  HoudiniSession,
    exposed:  Vec<HdaParam>,   // mirrored as PCG node params
}
```

The adapter's job is narrow: present an HDA as a PCG node with typed I/O matching its exposed ports. Evaluation cooks through the Houdini Engine session process and returns a point cloud or mesh. Bake-only — no Houdini process per frame. Most likely feature to slip; listed because the design must not preclude it. If it misses 3.0, §12's plugin API still covers user-written Houdini-like nodes in pure Rust.

## 12. Plugin-authored nodes

Phase 11 gave plugins a registration API. PCG adds one entry point:

```rust
impl PluginRegistry {
    pub fn register_pcg_node<N: PcgNode + 'static>(&mut self, kind: &'static str) { ... }
}
```

Plugin nodes are pure Rust trait objects, loaded through Phase 11's dynamic-load path — no `unsafe`, no C FFI. They participate in palette, compile, determinism, and debug visualizer on equal footing with built-ins.

## 13. World partition integration

Every bake-mode graph is assigned to a cell or set of cells. The bake pipeline:

```
┌─ Editor: "Bake Cells" command ──────────────────────────────┐
│                                                              │
│  For each dirty cell C:                                     │
│    For each graph G assigned to C:                          │
│      program = compile(G)                                   │
│      for stage in program.bake_stages.where(cell_scope==C): │
│        out = eval(stage, seed_for(G, C))                    │
│        cache.put(key(G, C, stage, seed), out)               │
│    cell_manifest[C].pcg_entries = {G: cache_keys}           │
└──────────────────────────────────────────────────────────────┘
```

"Dirty" means: cell heightmap changed (Phase 32), spline crossing changed, graph edited, or input asset GUID changed. Clean cells are not rebaked — the central economic claim of the phase. A 256 km² world has ~6400 cells at 64 m; rebaking all on every edit is minutes, rebaking only the three cells the edit touched is seconds. Cache is content-hashed — identical outputs across cells share a blob. Storage win more than speed; foliage-dense worlds produce hundreds of MB of baked PCG per km².

## 14. The AssetEditor tab

Phase 8's `AssetEditor` trait already did all the scaffolding work. `.rpcg` registers a factory:

```rust
impl AssetEditor for PcgGraphEditor {
    fn asset_type(&self) -> TypeId { TypeId::of::<PcgGraph>() }
    fn asset_guid(&self) -> AssetGuid { self.guid }
    fn title(&self) -> String { format!("{} [pcg]", self.name) }
    fn dirty(&self) -> bool { self.cmd_stack.is_dirty() }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut EditorContext) {
        egui::SidePanel::left("palette").show_inside(ui, |ui| self.palette.ui(ui));
        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.graph_widget.ui(ui, &mut self.graph, &mut self.cmd_stack);
        });
        egui::SidePanel::right("inspector").show_inside(ui, |ui| self.inspector.ui(ui, &self.graph));
    }
    ...
}
```

No new panel code: graph widget (Phase 20), palette widget (Phase 20), reflection inspector (Phase 2). PCG is *composition* — the new code is nodes, compile, bake pipeline, runtime scheduler, visualizer.

## 15. Build order

1. **Graph model** (`PcgNode`, `PcgPortType`, `.rpcg` RON), registration into `rustforge-node-graph`.
2. **Scatter generators** (GridScatter, Poisson, Cluster, FromSpline, FromMeshSurface, FromVolume).
3. **Filters and transforms** (masks, set-ops, randomize, noise-offset, attribute filter, cluster).
4. **Entity consumer** (`SpawnEntities`), proving the end-to-end flow on a trivial graph.
5. **Foliage consumer** (`PlaceFoliage`), retiring Phase 32's rule scatter.
6. **Bake pipeline** (per-cell, incremental, cache, manifest).
7. **Debug visualizer** (overlay, per-stage preview, attribute inspection, budget annotations).
8. **Houdini adapter** (plugin, gated, optional — can slip to 3.1).

Each step is a shippable sub-phase: a graph editable, a graph bakeable, a graph renderable at runtime, a graph replayable deterministically.

## ❌ Scope

Not in this phase:

- **Full Houdini replacement** — the adapter is a bridge, not a rewrite. Houdini-depth users keep their license.
- **GPU-only PCG pipeline** — CPU default in 3.0; a few node types (Poisson on dense bounds, noise fields) opt into a compute-shader fast path where speedup is > 10x. Full GPU PCG is a later phase and depends on Phase 36 compute-shader authoring.
- **ML-driven PCG** — style-transfer scatter, neural placement, reference-photo matching — Phase 39's territory.
- **Real-time PCG at AAA scale** — no 30 ms full-graph-per-frame at 128 m cells. Incremental bake is the shipping path; runtime PCG is a bounded gameplay-spawn budget, not a world-dressing engine.

## Risks

- **Determinism drift**. Any library call with non-deterministic parallel reduction (e.g. float-sort on a parallel unstable sort) breaks Phase 14. Mitigation: CI asserts byte-equal output across two runs on every built-in node; plugin-registered nodes must be `#[deterministic]`-marked; a panic fires in debug builds if an unmarked node runs in a networked session.
- **Cache invalidation sprawl**. "Graph edited" is coarse — a display-color tweak shouldn't invalidate bakes. Mitigation: `node_hash` excludes cosmetic fields.
- **Houdini license friction**. Non-Houdini users see HDA-authored assets as missing. Mitigation: Houdini-seated authors bake to raw `.rpcg` artifacts checked into the project — no runtime Houdini dependency.
- **Runtime budget starvation**. Three 2 ms runtime graphs thrash the scheduler. Mitigation: shared budget across all runtime graphs, priority-queued by distance-to-camera; far graphs starve before near ones.
- **Phase 32 regression**. Retiring rule scatter migrates existing projects. Mitigation: first-open converter writes equivalent `.rpcg` for legacy scatter regions; old format remains readable for one minor version.

## Exit criteria

A PCG phase is done when all of the following are true on a reference 4 km² open-world project:

1. Open `forest_floor.rpcg` in the editor. Edit a node parameter. The viewport updates the previewed cell within 250 ms.
2. "Bake Cells" completes a full world bake in under 90 s on the reference hardware (16-core Zen 4). Re-bake after a one-node edit completes in under 3 s.
3. Delete a baked cell's cache file. Reopen the project. The missing cell is rebaked on demand without author intervention.
4. Start a 4-player networked session. All four clients render visually identical PCG scatter. Replay recorded from client 1 is playable on client 4's machine with zero desync.
5. Runtime graph spawning enemies around the player holds its 2 ms / frame budget under 60 Hz with 200 active spawn points; exceeding the budget defers evaluation to the next frame and logs the deferral.
6. The debug visualizer renders stage-N output for any selected node in the open graph, including attribute inspection on click.
7. Retired Phase 32 scatter regions load as auto-converted `.rpcg` assets; they render visually identical to their pre-upgrade state.
8. A plugin-registered PCG node participates in palette, bake, runtime, determinism checks, and visualizer with no modification to the host.
9. (Optional, slippable) A Houdini HDA loads as a PCG node, exposes its parameters, and bakes to cache.

When all nine pass on the reference project, Phase 40 ships. Phase 41 gets to assume that world dressing is a solved problem and move on to whatever systemic gameplay owes its depth to that foundation.
