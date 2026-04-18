# Phase 32 — Advanced Terrain, Foliage & Splines

Phase 8 §3 shipped the terrain editor's minimum viable surface: raise, lower, smooth, flatten, paint a material-layer weight, one undo entry per stroke. It got the pattern right — brush-over-clipmap, per-tile snapshot, command-stack integration — but it left every feature that makes a terrain actually *look* like a place on the cutting-room floor. Phase 20 later gave us a material graph that can blend layers with real authoring control; Phase 21 gave us bindless textures and a tier system that covers the GPU side; Phase 31 put world partition cells underneath everything and made terrain stream with the rest of the scene. The depth gap from Unreal Landscape is no longer shader fidelity or streaming — it's the *authoring furniture*: roads that carve the terrain, grass that covers the hills, scatter rules that place the trees, RVT so a road blends into the ground it sits on, an erosion bake so the hills don't look pulled-out-of-a-heightmap.

Phase 32 closes that gap and stops. No procedural mountain generator; no volumetric cave simulator; no real-time erosion. The goal is a shippable open-world authoring surface that reuses every pattern already in the engine — brushes from Phase 8, graph nodes from Phase 20, cell streaming from Phase 31, bindless material IDs from Phase 21 — and adds only the domain-specific pieces those patterns cannot express directly.

## Goals

By end of Phase 32:

1. **Landscape splines** — author spline curves over terrain with per-point width / material / height-push, used for roads, rivers, trails.
2. **Foliage painter** — brush-paint instanced meshes onto the terrain surface with density / scale / slope / altitude masks.
3. **Grass system** — GPU-instanced grass driven by material-layer weights, wind, and LOD-to-billboard at distance.
4. **Procedural scattering** — rule-based (non-painted) spawn of meshes from grid / Poisson / clustered patterns gated by slope / altitude / layer weight.
5. **Runtime Virtual Textures (RVT)** — cache terrain material shading into a virtual texture that nearby meshes sample, so roads, puddles, and decals blend seamlessly into the ground.
6. **Terrain erosion bake** — compute-shader hydraulic + thermal erosion as an authored bake, producing height + sediment outputs.
7. **Cliff / overhang mesh stitching** — when the heightmap cannot express the shape (arches, caves, undercuts), splice mesh geometry in with a material-masked transition.
8. **Terrain & foliage colliders** — Rapier heightfield for the ground, instanced cheap colliders optional per foliage layer.
9. **Streaming integration** — every new asset type (splines, foliage instances, grass regions, RVT tiles, mesh stitches) streams with Phase 31 world-partition cells.
10. **Authoring panel extension** — Phase 8's Terrain Tools panel grows Splines / Foliage / Grass / Scatter tabs sharing its command-stack integration.

## 1. Landscape splines

The headline authoring feature. A landscape spline is a Catmull-Rom (or cubic Bézier, internally converted) curve over the terrain with per-control-point attributes that drive three things: a height deformation of the terrain underneath, a material-layer paint along the path, and optionally a mesh-instancing sweep (barriers, fences, rails) along the curve.

```
  P0 ───── P1 ─────── P2 ──── P3
   ●        ●          ●       ●      ← control points (draggable)
   │        │          │       │
   w=4      w=6        w=6     w=4    ← per-point width
   h=0      h=-1       h=-2    h=0    ← per-point height delta
   mat=road mat=road   mat=road mat=trail

  Terrain rendered under curve:

    ···░░░░▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░···
    ······░░▓▓██████████▓▓░░····   ← material blend painted
    ········░▓████████▓░········
```

### 1.1 Data model

```rust
// crates/rustforge-core/src/terrain/spline.rs
pub struct LandscapeSpline {
    pub id:         SplineId,
    pub points:     Vec<SplinePoint>,   // ordered along curve
    pub closed:     bool,
    pub material:   AssetGuid,          // material graph for the paint blend
    pub mesh:       Option<AssetGuid>,  // optional swept mesh (guardrail, curb)
    pub cell_keys:  SmallVec<[CellKey; 8]>, // Phase 31 cells this spline touches
}

pub struct SplinePoint {
    pub pos:        Vec3,      // world space; y is authored, not snapped
    pub tangent_in: Vec3,
    pub tangent_out:Vec3,
    pub width:      f32,       // world units, the half-width of the paint/deform band
    pub falloff:    f32,       // extra world units for the soft edge
    pub height_push:f32,       // +up / -down relative to sampled terrain height
    pub layer_idx:  u8,        // which material layer to paint
}
```

`cell_keys` is a precomputed list of every Phase 31 partition cell the spline's AABB intersects, so streaming a cell in knows which splines to re-bake.

### 1.2 Authoring: the spline editor panel

```
┌─ Terrain Tools ─────────────────────────────────────┐
│ [Sculpt] [Paint] [Splines] [Foliage] [Grass] [Scat] │  ← tabs
├─────────────────────────────────────────────────────┤
│ Spline: "south_road"     [New] [Duplicate] [Delete] │
│                                                      │
│ Points:  4     Closed: [ ]                           │
│  P0   (12, h, 47)   w=4.0  h=+0.0  mat=road         │
│  P1   (18, h, 49)   w=6.0  h=-1.0  mat=road  [edit] │
│  P2*  (24, h, 51)   w=6.0  h=-2.0  mat=road         │ ← selected
│  P3   (31, h, 53)   w=4.0  h=+0.0  mat=trail        │
│                                                      │
│ Sweep mesh:  [ guardrail.glb   ▼ ]   spacing 2.0 m  │
│ Bake:  [Rebake height + paint]  ⟳ auto on drag-end   │
└─────────────────────────────────────────────────────┘
```

Interactions:

- **Click-on-terrain in Add mode** appends a point, snapping Y to current terrain height; Shift-click inserts between the two nearest points (sorted by arc length).
- **Drag a control point** in the viewport with tangent handles visible. Alt-drag breaks the symmetric tangent; plain drag keeps it smooth.
- **Per-point attribute edits** go through Phase 6 commands. Width / height-push drags coalesce with the drag-stop rule from Phase 8 §2.3.
- **Rebake** recomputes the terrain deformation and paint, writing to the clipmap via `TerrainEdit` (Phase 8 §3.4). Auto-on-drag-end is on by default; big splines over expensive bakes can disable it and drive manually.

### 1.3 Baking splines into terrain

A spline bake is a stamp: sample the curve at fixed arc-length intervals, walk a rasterized band `width + falloff` on either side, modify each touched terrain texel's height and layer-weight accordingly, push the undo snapshot (same path as Phase 8 §3.2). The stamp is deterministic, so splines can be re-baked at any time without drift.

```rust
pub struct SplineBake;
impl SplineBake {
    pub fn apply(
        spline: &LandscapeSpline,
        terrain: &mut TerrainEdit,
        cells: &[CellKey],
    ) -> CompositeCommand { /* ... */ }
}
```

Baking one spline rewrites every cell in `cell_keys`. The command's snapshot region is the spline's AABB — which can be large for a 2 km road, and is the primary memory risk (§ Risks).

### 1.4 The three spline deformations

- **Height push** — per-point Y delta lerp'd along arc length, applied radially with a cosine falloff over the `width + falloff` band. Zero push = purely cosmetic paint.
- **Material paint** — writes layer-weight into the terrain material stack (Phase 20 consumes the weight as a graph input). Soft edge over `falloff`.
- **Ramp flatten** — optional: project each terrain sample onto the line between adjacent control points and pull it to that height, instead of adding a delta. Rivers/roads want this; trails don't.

### 1.5 Mesh sweep

Optional per-spline: place instanced meshes along the curve at `spacing`, oriented to the tangent, anchored at `width`. A guardrail, a fence, a river bank. Instances are exactly the same instanced meshes the foliage system (§2) uses, stored per-cell in the foliage instance buffer — no second instancing path.

## 2. Foliage painter

Paint-brush placement of instanced meshes. Different from splines: foliage is unordered, per-instance (not per-curve), and has masks.

```
  ┌─ Foliage Layers ───────────────────────────┐
  │ [+]                                         │
  │  oak_tree.glb        density ███░░ 0.6     │
  │    scale jitter 0.8-1.2    slope <30°      │
  │    altitude 0-80 m         layer mask:grass │
  │                                              │
  │  pine_tree.glb       density ██░░░ 0.4     │
  │    scale jitter 0.9-1.1    slope <40°      │
  │    altitude 40-200 m       layer mask:rock  │
  │                                              │
  │  boulder.glb         density █░░░░ 0.1     │
  │    scale jitter 0.5-2.0    slope <20°      │
  │    altitude 0-300 m        layer mask:any   │
  │                                              │
  │ Brush: ●radius 8m  ●strength 1.0  [erase]  │
  └─────────────────────────────────────────────┘
```

### 2.1 Layer model

```rust
pub struct FoliageLayer {
    pub id:          FoliageLayerId,
    pub mesh:        AssetGuid,
    pub material:    Option<AssetGuid>,   // override; default = mesh material
    pub density:     f32,                 // instances per m² at full strength
    pub scale_range: (f32, f32),
    pub yaw_jitter:  f32,                 // radians, 0..=PI
    pub align_to_normal: bool,
    pub slope_max:   f32,                 // degrees; reject above
    pub altitude:    (f32, f32),          // world Y range
    pub layer_mask:  Vec<LayerMaskEntry>, // terrain material layers that allow this
    pub cast_shadow: bool,
    pub collider:    Option<FoliageColliderKind>,  // None, Capsule, Convex
}
```

### 2.2 Instance storage, streaming

Foliage instances live per Phase 31 cell, serialized with the cell's other data. An instance is 32 bytes (position, yaw, uniform scale, layer-id); a dense forest cell with 50 k instances is 1.5 MB on disk, streamed in with the cell.

```rust
pub struct FoliageInstance {
    pub pos:      Vec3,         // 12
    pub yaw:      f32,          // 4
    pub scale:    f32,          // 4
    pub layer_id: u16,          // 2
    pub _pad:     [u8; 10],     // 10  → 32 bytes, aligned for GPU upload
}
```

Per-cell instance buffers upload to a GPU-side bindless instance array indexed by cell. Culling is frustum-per-cell (cells are already frustum-culled by Phase 31) plus per-instance distance cull in a compute pass.

### 2.3 Brush semantics

- Dab pass rasterizes a density kernel under the cursor; new instances are added until the target density for the layer is met in each grid bin, placement points sampled via blue-noise (precomputed Poisson set tiled over the world).
- Masks are evaluated at placement: slope from terrain normal, altitude from Y, layer-weight from the material layer sampler. A placement that fails any mask is dropped, not queued for retry — this keeps the brush deterministic at a given spot.
- Erase brush removes instances whose centers fall under the cursor up to the brush strength fraction per frame.
- Every mouse-down / mouse-up is one undo entry (same pattern as Phase 8 §3.2). Command stores before/after per-cell instance deltas, not whole instance buffers, so memory is proportional to *changed* instances not total.

## 3. Grass system

Grass is **not** painted instance-by-instance. It's a GPU-instanced carpet driven by terrain material-layer weights. If the `grass_green` layer has weight > 0.3 at a point, grass grows there; no painting required.

### 3.1 Data

```rust
pub struct GrassType {
    pub id:            GrassTypeId,
    pub mesh:          AssetGuid,     // small mesh (blade cluster)
    pub billboard:     AssetGuid,     // far-LOD billboard
    pub layer_source:  LayerIndex,    // which material layer drives density
    pub density:       f32,           // blades per m² at weight=1.0
    pub scale_range:   (f32, f32),
    pub wind_params:   WindCurve,     // amplitude + frequency, sampled in VS
    pub fade_distance: f32,           // billboard transition
    pub cull_distance: f32,           // hard cull
}
```

### 3.2 Placement and LOD

Placement is fully GPU-driven. Each frame, per visible cell, a compute pass:

1. Samples a regular grid over the cell at `density`-derived spacing.
2. Reads the terrain layer weight at each grid point.
3. Jitters position within the cell (blue-noise LUT), samples height from the clipmap.
4. Emits a mesh instance to the near-buffer if within fade distance, a billboard to the far-buffer otherwise.

```
   cell bounds                 terrain layer weight
   ┌────────────┐              ░░░░▒▒▒▒▓▓▓▓▓▓▒▒░░
   │ · · · · ·  │      →       ░░▒▒▓▓▓▓▓▓▓▓▒▒░░░░
   │ · · · · ·  │              ░░░░▒▒▓▓▓▓▒▒░░░░░░
   │ · · · · ·  │
   └────────────┘      →       instance buffer:
     grid samples                near: 1432 meshes
                                 far:  4821 billboards
```

Wind is a vertex-shader perturbation; no CPU simulation. The `WindCurve` feeds two sines and a noise sample — classic GDC-slide grass.

### 3.3 Streaming and memory

Grass is *derived*, not stored. A cell's grass is recomputed on stream-in and freed on stream-out; no grass instances are serialized. This is the biggest difference from foliage. Memory is the transient GPU buffer per visible cell.

## 4. Procedural scattering

The rule-based counterpart to foliage painting. No brush; spawn is a *rule* the user authors, the engine runs it at bake time (or stream-in), instances are persisted per cell like painted foliage.

```
┌─ Scatter Rule: "forest_canopy" ────────────────────┐
│ Pattern:  [ Poisson disk  ▼ ]    min dist 3.0 m    │
│ Area:     [ world mask "forest_zone"  ▼ ]          │
│ Spawn:                                              │
│   oak_tree.glb        weight 0.6                    │
│   pine_tree.glb       weight 0.3                    │
│   fallen_log.glb      weight 0.1                    │
│ Masks:                                              │
│   slope < 30°     altitude 20-150 m                 │
│   terrain layer "grass" > 0.5                       │
│ Density:  0.05 / m²                                 │
│ Seed:     42                                        │
│ [Bake]  [Clear]                                     │
└────────────────────────────────────────────────────┘
```

### 4.1 Patterns

- **Grid** — uniform spacing with per-cell jitter. Crops.
- **Poisson disk** — minimum-distance blue-noise. Forests.
- **Clustered** — Poisson cluster centers, dense spawn around each. Flowers, bushes.

### 4.2 Execution

A scatter rule is a deterministic function of `(seed, cell_key)`. Baking iterates every cell the rule's area mask touches and writes the resulting instances into that cell's foliage instance buffer (reusing the §2 pipeline — procedural instances are foliage instances with a `source: Procedural(rule_id)` tag).

Re-baking a rule replaces only its tagged instances; hand-painted instances in the same cell are untouched. This is the promise that lets users combine both — paint an exception into a procedural forest without losing the exception on the next rebake.

## 5. Runtime Virtual Textures (RVT)

The seam problem: a road mesh (from a spline sweep) sits on terrain. The terrain's splatmap blends rock / grass / dirt; the road mesh is a flat gray. Where they meet is a hard edge. RVT fixes this by baking the terrain's shaded result into a screen-space-adjacent virtual texture cache that nearby meshes can sample and blend with — the road's edges read the terrain underneath and bleed into it.

### 5.1 Physical layout

```
  Logical RVT: infinite 2D coverage over the world at 4 texel/m
  Physical pool: 2048 x 2048 of 128 x 128 pages
  Indirection:   sparse table (cell_key → page_id) kept per tile

  ┌────────────┐          ┌──────────────┐
  │ world XY   │ indirect │ 128x128 page │
  │ sample     │─────────▶│ RGBA (color) │
  │ uv         │          │ + normal     │
  └────────────┘          └──────────────┘
```

- Min tier: **Medium**. Low tier disables RVT and falls back to per-mesh material.
- Pool size: 128 MB default, configurable.
- Pages bake on-demand: first visibility of a world region triggers a compute pass that renders the terrain material (Phase 20 graph, top-down ortho) into the allocated page.
- Invalidation: spline bake / foliage paint / terrain sculpt marks the touching pages dirty, they re-bake next frame.

### 5.2 Consumers

Meshes declare `reads_rvt: bool` in their material. Roads, puddles, decals, paint-blend meshes sample the RVT with world-XY in their shader and blend against it. Static props that don't need the blend save the sample cost.

### 5.3 Interaction with the post graph and bindless

RVT pages live in the bindless texture array (Phase 21 §2). The indirection table is a storage buffer. Fits naturally; no new binding-layout work on High tier. Medium tier with fallback descriptors carries an extra bind-group slot cost.

## 6. Erosion bake

Authored, not runtime. A one-shot compute pass over the heightmap that simulates hydraulic + thermal erosion for N iterations and writes new height + a sediment mask (usable as an extra terrain material layer: "where sediment collected, paint mud").

### 6.1 Inputs

```rust
pub struct ErosionParams {
    pub iterations:       u32,       // 50_000 to 500_000 typical
    pub rain_rate:        f32,
    pub evaporation:      f32,
    pub capacity:         f32,
    pub deposition:       f32,
    pub erosion_rate:     f32,
    pub talus_angle:      f32,       // thermal — angle of repose
    pub min_slope:        f32,
    pub seed:             u64,
}
```

### 6.2 Algorithm

Compute-shader droplet simulation: N droplets per iteration, each traces a gradient-descent path, picking up sediment on steep declines, depositing on flats. Thermal pass redistributes material along over-steep slopes each M iterations. Runs fully on GPU; a 4k × 4k heightmap with 200 k iterations finishes in tens of seconds on a High-tier GPU.

### 6.3 Output and undoability

Bake writes into the terrain clipmap through `TerrainEdit` (Phase 8 §3.4). The whole bake is one composite command. Undo restores the pre-bake heightmap and sediment layer. Preview mode bakes at quarter resolution into a scratch texture overlaid in the viewport; commit writes at full resolution.

## 7. Cliffs, overhangs, and mesh stitching

A heightmap is a function `Y = f(X, Z)`. Arches, caves, undercuts, overhangs — all are `Y = f(X, Z)` with multiple Y values. The heightmap cannot express them. The long-standing answer is **mesh stitching**: author a mesh that occupies the space where the heightmap fails, mask the terrain material to blend with the mesh's material at the seam.

### 7.1 The stitch asset

```rust
pub struct TerrainMeshStitch {
    pub id:          StitchId,
    pub mesh:        AssetGuid,
    pub transform:   Mat4,
    pub blend_mask:  AssetGuid,  // 2D texture; 1 = mesh material, 0 = terrain
    pub terrain_aabb: Aabb,      // where the heightmap should NOT draw
}
```

- The terrain renderer reads a per-cell list of `TerrainMeshStitch::terrain_aabb` and skips tiles inside those boxes.
- The stitched mesh renders normally.
- The seam material on the mesh samples the RVT (§5) at world XY and the stitch's `blend_mask` to fade into the terrain shading.

### 7.2 Authoring

A stitch is authored as a regular mesh prop placed in the world (Phase 2 / Phase 8 inspector). The "terrain stitch" component marks it as a hole-puncher. No dedicated panel; the existing prop-placement workflow is enough. This is deliberately minimal — full cave systems need dedicated tooling that this phase is not providing (§ Scope out).

## 8. Collision

### 8.1 Terrain collider

Rapier supports heightfield colliders natively. Phase 32 builds one per visible terrain cell, refreshed on heightmap edit (spline bake, erosion bake, sculpt). Collider lives on the cell's collider set; streaming in/out attaches/detaches it.

```rust
impl TerrainCollider {
    pub fn build_cell(cell: &TerrainCell) -> rapier3d::geometry::Collider {
        let heights = cell.heightmap.to_row_major();
        ColliderBuilder::heightfield(heights, cell.scale).build()
    }
}
```

### 8.2 Foliage colliders

Per-layer opt-in:

- `None` — no collider, instances are pure visuals (grass, small rocks).
- `Capsule` — centerline capsule, cheap, ideal for trees.
- `Convex` — from the mesh's convex hull; more accurate but per-instance memory.

Colliders are created per-instance when the cell streams in, destroyed on stream-out. Instance count × collider handle is the memory story; cap via layer config (`collider_max: 500` per cell per layer).

### 8.3 Grass colliders

Never. Grass is visual-only. If a game needs grass to knock the player back, it's a gameplay effect layered on terrain proximity, not a collider.

## 9. Streaming integration with Phase 31

Every data type in this phase streams with Phase 31 cells:

| Asset | Storage | Streamed with cell |
|-------|---------|---------------------|
| Terrain clipmap tile | per-cell | yes (already, Phase 8 + 31) |
| Foliage instances (painted) | per-cell | yes |
| Foliage instances (scattered) | per-cell, tagged | yes |
| Grass | derived | regenerated on stream-in |
| Splines | spline-owned, cell-referenced | spline loads when ANY touched cell loads |
| Spline meshes (swept) | per-cell | yes |
| RVT pages | bounded pool | LRU; evict on cell unload |
| Terrain colliders | per-cell | yes |
| Stitch meshes | per-cell (by placement) | yes |
| Erosion bake outputs | baked into clipmap | yes |

The only cross-cell citizen is the spline itself — a 2 km road crosses dozens of cells. The spline's master record lives in a project-wide spline asset file; each cell keeps a spline-id list of references. Loading a cell loads the spline if not already loaded; unloading the last referencing cell drops the spline.

## 10. Authoring flow — extending Phase 8 Terrain Tools

Phase 8 shipped a single Terrain Tools panel with Sculpt / Paint tabs. Phase 32 adds four tabs, all sharing the panel's dockable host and command-stack integration:

```
┌─ Terrain Tools ─────────────────────────────────────┐
│ [Sculpt] [Paint] [Splines] [Foliage] [Grass] [Scat] │
└─────────────────────────────────────────────────────┘
```

Each tab owns a sub-editor. Each sub-editor pushes commands (spline edits, foliage layer changes, scatter rule edits) through the Phase 6 stack. Undo works uniformly — Ctrl+Z over a foliage paint, a spline drag, a scatter bake, all go through one stack.

An **Erosion** action lives under Sculpt as a modal dialog, not a tab, because it's a bake operation, not an ongoing editor state.

## 11. Build order within Phase 32

Each step is independently shippable. Steps 1–3 have no runtime dependency on later ones; later steps reuse the earlier infrastructure.

1. **Foliage painter + instance streaming (§2)** — smallest standalone thing that validates per-cell instance serialization against Phase 31. No graph, no RVT, no deformation — just paint-and-stream. The pattern every later step reuses.
2. **Procedural scattering (§4)** — reuses the §2 instance pipeline with a deterministic spawner in front. Adds the concept of a rule, introduces re-bake semantics.
3. **Grass (§3)** — GPU-only, no persistence; builds on material-layer sampling.
4. **Landscape splines (§1)** — largest authoring surface; needs the `TerrainEdit` from Phase 8 and the material-layer write paths from §2/§3 already exercised.
5. **Runtime Virtual Textures (§5)** — the seam-blending payoff, lands after splines so spline meshes can be the first consumers.
6. **Cliff / overhang stitching (§7)** — tiny code surface; depends on RVT for the seam blend to actually look good.
7. **Erosion bake (§6)** — compute-shader heavy; lands late because it's the most GPU-invasive and the least path-critical.
8. **Collision (§8)** — per-type rollout: heightfield first (needed by Phase 31 gameplay), foliage capsules second, convex third.
9. **Authoring panel integration (§10)** — glues the four tabs into Terrain Tools; the sub-editors exist by this point, this step only owns the dock / tab wiring.

## 12. Scope — what's NOT in Phase 32

- ❌ **Gaia / World-Machine-style procedural terrain authoring suite.** No node-graph macro-terrain generator with layered noise, plate tectonics, climate simulation. An erosion bake is the whole procedural concession.
- ❌ **Volumetric terrain / caves at scale.** Mesh stitching handles a few arches and overhangs. A sparse voxel octree terrain with real cave networks is its own multi-phase project.
- ❌ **Tectonic / geological simulation.**
- ❌ **Real-time erosion during gameplay.** Bake only. A river that carves a canyon while the player watches is out.
- ❌ **Weather-driven runtime terrain change** (snow accumulating in tracks, mud from rain). Material-layer authoring effects at runtime are fine; persisting them into the heightmap is not.
- ❌ **Splines for non-terrain purposes** (cable splines, rail splines, AI patrol paths). Landscape splines only. A general-purpose spline system is a separate phase.
- ❌ **Foliage with skeletal animation** (trees with per-branch simulation). Vertex-shader wind only.
- ❌ **Grass collision / grass interaction.** Grass is visual.
- ❌ **Water simulation.** A spline can paint a river into the terrain; the water surface itself (waves, flow, foam) is a separate phase.
- ❌ **Terrain LOD authoring UI.** LOD is automatic from the clipmap; no user knobs in this phase beyond what Phase 8 exposed.
- ❌ **Spline-driven mesh deformation at runtime** (a rope that bends with the wind). Static sweeps only.
- ❌ **Distance-field terrain.** Clipmap only; SDF terrain is research-tier.

## 13. Risks

- **Spline bake undo memory.** A 2 km spline over 4 m-wide cells touches hundreds of cells; one bake's snapshot can exceed 100 MB. Phase 6 §6's 500 MB cap is reachable. Mitigate: spline bakes serialize snapshots to disk for the command (referenced by path, not in-memory) when their size exceeds 50 MB, and reload on undo. Document the disk-snapshot path clearly.
- **Foliage instance counts in dense forests.** 200 k instances per cell × 32 bytes = 6 MB per cell, × 9 visible cells = 54 MB resident. Streaming is fine; the GPU upload cost when a cell streams in is the real risk. Budget 1 ms/cell on the reference High-tier GPU and amortize stream-in across frames.
- **Grass density blowing out GPU memory on Medium tier.** An aggressive `density = 20 /m²` across 9 visible cells is millions of blade instances. The compute-generated near/far buffers are transient, but transient can still OOM. Cap total grass budget per frame in GPU memory settings (Phase 21 §11 budget panel); clamp density when the cap is near.
- **Scatter determinism under rule edits.** Changing a scatter rule's seed should regenerate; changing a cosmetic field (layer color tint) should not. Hash the set of *generation-affecting* fields explicitly (seed, pattern, area, density, masks, spawn list) and exclude the cosmetic ones from the hash — otherwise every trivial edit invalidates every cell's scatter cache.
- **RVT page thrash near detail-rich meshes.** A player standing where a road, a river, and a lake edge converge needs RVT pages for all three. 128 MB pool can fill. Profile, expose the pool cap, evict LRU; log when eviction of a just-used page happens (same bug signature as the VSM pool in Phase 21 §4).
- **Erosion bake runtime on huge maps.** 8k × 8k heightmap with 500 k iterations is minutes. UI needs a cancel button and a progress bar; back out to pre-bake on cancel. Never block the editor's event loop on the bake.
- **Stitch AABB occlusion holes.** A stitch's `terrain_aabb` skips terrain tiles; if the mesh inside doesn't actually cover the AABB, the player falls through. Validate at save time — cast rays down through the AABB from above, fail the save if any ray exits the AABB without hitting the stitch mesh.
- **Foliage collider count on dense forests.** 50 k capsule colliders per cell × 9 cells = 450 k active colliders. Rapier handles it but build time is non-trivial. Build asynchronously on a worker as the cell streams; don't block stream-in on collider construction.
- **Spline + foliage interaction.** A spline cuts a road through a forest — the procedurally scattered trees on top of the road. Scatter rule's "terrain layer > 0.5" mask saves this *if the spline paints the road layer before scatter bakes*. Order: always rebake scatter after any spline bake that affects its area. Track dependencies.
- **RVT and material-graph coupling.** A material graph change (Phase 20) that affects terrain shading invalidates every RVT page. Can't be avoided; make the cost visible ("material edit → RVT pool flush, next cells will re-bake on visibility"). Don't hide it behind a silent lag.
- **Grass wind determinism across clients.** A multiplayer game with client-side grass wind will see mismatches. Feed wind time from a shared clock; document that grass simulation is client-local and never observed gameplay-side.
- **Panel tab count overflow.** Six tabs on Terrain Tools is getting crowded. Two-row tabs or a dropdown "More ▼" is the standard fix; don't add a seventh tab without that.
- **Editor-only spline bake vs. shipped game.** Splines ship as their baked terrain + their instance-buffer contributions; the spline data itself (control points, attributes) can be stripped on ship. Gate retention behind a `include_editor_data` project flag, default off for ship builds. Saves disk.

## 14. Exit criteria

Phase 32 is done when all of these are true:

- [ ] A user can author a landscape spline end-to-end: add points by click-on-terrain, drag with tangent handles, edit per-point width / height-push / material / falloff, rebake, see the terrain deform and layer-paint along the curve, and undo the bake cleanly.
- [ ] Splines with optional swept meshes produce correctly oriented mesh instances at authored spacing, stored per-cell.
- [ ] Foliage painter with at least three layers (tree, rock, bush) places instanced meshes driven by slope / altitude / layer-weight masks, with a functioning erase brush.
- [ ] Every foliage paint stroke produces exactly one undo entry; undo restores the exact pre-stroke per-cell instance set.
- [ ] Grass renders on a target-layer test scene at authored density with fade-to-billboard at distance, vertex-shader wind, and zero persistent storage (regenerated on stream-in).
- [ ] Procedural scatter rule generates deterministic instances for a given `(seed, cell_key)`; re-running a rule produces byte-identical instances; editing a cosmetic field does not invalidate generated instances.
- [ ] RVT pages allocate on-demand from a bounded physical pool; road / puddle / decal meshes sampling RVT blend into the terrain shading with no visible seam on a reference scene.
- [ ] Spline bake / foliage paint / sculpt invalidate the correct RVT pages; next frame re-bakes them.
- [ ] Erosion bake runs on a 4k × 4k heightmap and produces visible hydraulic gullies + thermal talus under canonical parameters; results route through `TerrainEdit` as one undoable command.
- [ ] Erosion bake has a working cancel + progress UI; cancel restores the pre-bake heightmap exactly.
- [ ] Terrain heightfield colliders build per cell and refresh on height edit (spline bake, erosion bake, sculpt).
- [ ] Foliage layers opt into capsule or convex colliders; instance collider build is off the main thread and does not block cell stream-in.
- [ ] A stitched mesh with a `terrain_aabb` correctly punches a hole in the clipmap; RVT-sampled seam blends into the ground; save-time validation rejects stitches whose meshes don't cover their AABB.
- [ ] All four new panel tabs (Splines, Foliage, Grass, Scatter) live under Phase 8's Terrain Tools panel, route their edits through the Phase 6 command stack, and share the panel's dock placement.
- [ ] Every per-cell asset introduced by this phase (painted foliage, scattered foliage, sweep meshes, stitch meshes, heightfield collider, RVT invalidation record) streams correctly through Phase 31: stream-in loads it, stream-out frees it, and a full world scan shows no leaks.
- [ ] Ship builds without the `editor` feature render all Phase 32 content identically (splines as baked terrain + instance contributions, scatter as pre-generated instances, grass generated at runtime, no spline editor code present).
- [ ] `rustforge-core` still builds and runs without the `editor` feature.
