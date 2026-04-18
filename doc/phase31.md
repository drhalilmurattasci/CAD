# Phase 31 — World Partition & Open-World Streaming

Phase 30 closed the 1.x arc with a lookback, a gap list, and a soft promise that a 2.0 series would exist if the gaps justified it. They did. Phase 31 opens that series.

RustForge 1.x shipped a scene model that was good enough for the games its users wanted to make: `.ron` files, structurally mergeable (Phase 12), stable `SceneId(u64)` references (Phase 4), loaded whole. The pattern breaks when the map stops fitting in memory. A 16 km × 16 km open world at modern density is not a scene with 40,000 entities; it is 1,600 scenes of 200 entities each, none of which are relevant more than 500 m from the camera, and most of which have to stream off disk while the game is running at 60 Hz. Unreal answered this with World Partition in UE5. Phase 31 answers it with a RustForge-shaped equivalent: a grid, a streamer, a per-entity file format, HLOD-baked proxies, Level Instances, and a debugger that tells you why the cell you are standing in is still loading.

This is the AAA-scale opener of the 2.0+ arc. The phase is wide — ten subsystems — but every one of them is load-bearing for phases that follow it. None of them is optional. A partial World Partition (e.g. grid without HLOD, or streaming without per-entity files) would be worse than the 1.x monolithic scene because it would lock in migration debt without the payoff.

Out of scope, named early so the phase does not drift: infinite procedural worlds (a separate future phase in the 2.0 arc), planetary-scale curvature, MMO shard servers (Phase 34's problem when we get there), and runtime chunk editing from scripts (the editor bakes cells; the runtime reads them). A cell can be *modified* at runtime — entity transforms, spawns, despawns — but the cell *set* is fixed at bake.

## Goals

By end of Phase 31:

1. **World Partition grid** — world divided into cells (default 64 × 64 m, configurable per-project), hash-addressed on disk, loaded independently.
2. **Distance streaming** — async loader pool driven by camera + spectator positions with configurable radius per tier.
3. **Data layers** — named, orthogonal-to-grid layers (`Gameplay`, `Cinematic_Intro`, `DLC1`) toggled by script, independent of spatial streaming.
4. **HLOD** — baked merged-mesh proxies per cell cluster, LOD0..LODN, feeding Phase 21 virtualized geometry.
5. **Incremental cell baking** — editor command rebakes only dirty cells; writes a streaming manifest.
6. **Entity streaming by transform** — entities migrate cells as they move; `SceneId` stable across cell boundaries.
7. **One-file-per-actor (OFPA)** — each entity persisted as its own `.ron` file; Phase 12 structural merge now works per-entity.
8. **Level Instances** — reusable sub-scene references (by reference, not copy), scope-limited to cutscenes, districts, rooms.
9. **Streaming debugger** — viewport overlay of cell load state, extending Phase 10 diagnostics.
10. **Open-World project template** — world outliner by cell, "Load Region" focused-work tool, per-tier memory caps (Phase 22).
11. **Seamless boundaries** — light probes and nav mesh (Phase 26) stitch across cell borders; no visible seams.

## 1. The grid

Every other section assumes this one. Get the grid model right first; everything else composes against it.

```rust
// crates/rustforge-core/src/world/partition.rs
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CellCoord { pub x: i32, pub y: i32, pub z: i32 }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GridConfig {
    pub cell_size_m:     f32,   // default 64.0
    pub vertical_cells:  bool,  // false = 2D grid, true = 3D (vertical worlds)
    pub origin_offset:   Vec3,  // grid origin in world space
}

impl GridConfig {
    pub fn cell_of(&self, p: Vec3) -> CellCoord {
        let q = (p - self.origin_offset) / self.cell_size_m;
        CellCoord {
            x: q.x.floor() as i32,
            y: if self.vertical_cells { q.y.floor() as i32 } else { 0 },
            z: q.z.floor() as i32,
        }
    }
}
```

A cell is addressed on disk by `blake3(cell_coord || project_id)` truncated to 16 hex chars. Hash-addressing, not `cell_-3_0_12.ron`, for the same reason Phase 5's asset pipeline used GUIDs: path-based addressing breaks the moment someone renames the project folder or ships a DLC that wants to extend the grid at different origin.

Default cell size is 64 m. The choice is not arbitrary:

- Small enough that a median entity density (200/cell) fits in a single OFPA directory listing without pagination.
- Large enough that a typical character (run speed 6 m/s) crosses a cell no more than once every ~10 s, keeping reparenting churn low.
- Power-of-two multiple of the Phase 26 nav-mesh tile size (16 m), so nav tiles align on cell borders.

Projects can override (`project.toml::world_partition.cell_size_m = 32.0` for dense cities, `128.0` for sparse terrain) but the default is the one that works for 80% of cases.

```
ASCII view of a 2D grid around the camera (cell_size = 64m, radius = 192m):

     z
     ^
     |  . . . . . . .
     |  . . L L L . .      L = loaded
     |  . L L L L L .      S = streaming in
     |  . L L C L L .      U = unload pending
     |  . L L L L L .      C = camera cell
     |  . . L L L . .      . = unloaded
     |  . . . . . . .
     +-------------------> x
```

## 2. The async loader

The streamer runs on a dedicated tokio runtime with a pool sized from platform tier (Phase 22): 2 threads on Low, 4 on Medium, 8 on High. It consumes a stream of `LoadRequest { cell: CellCoord, priority: f32 }` and produces `CellLoaded { cell, entities: Vec<EntityHandle> }` events onto the main ECS queue.

```rust
// crates/rustforge-core/src/world/streamer.rs
pub struct CellStreamer {
    inflight:  HashMap<CellCoord, JoinHandle<Result<LoadedCell>>>,
    loaded:    HashMap<CellCoord, LoadedCell>,
    budget:    MemoryBudget,       // from Phase 22 tier
    io_pool:   AsyncIoPool,
}

impl CellStreamer {
    pub fn tick(&mut self, camera: Vec3, spectators: &[Vec3], world: &mut World) {
        let wanted = self.cells_in_radius(camera, spectators);
        self.start_loads_for(&wanted - &self.loaded.keys().copied().collect());
        self.unload_farthest_until_under_budget(camera);
        self.drain_completed(world);
    }
}
```

Priority is `-distance_to_nearest_observer`; cells already visible but not loaded have priority boosted by a constant so the streamer preferentially fills holes in view before speculative loads behind the camera. Priority is recomputed every frame; an in-flight load is not cancelled if its priority drops, because cancelling partial I/O on Windows is more expensive than letting it finish and immediately unloading.

Cells are unloaded when both: (a) no observer is within radius, and (b) the loaded cell count exceeds the platform-tier budget. The two-condition rule prevents the thrash where a spectator at the radius edge oscillates cells in and out on sub-frame camera jitter.

Spectators (other players in multiplayer, sequencer cameras, AI head-tracking sources) contribute to the wanted set. The server-authoritative path in Phase 17's replication stack already enumerates spectators; reuse that list.

## 3. One-file-per-actor (OFPA)

The single biggest migration in the phase. A 1.x scene is one `.ron` file with N entities. A 2.0 scene is a *directory* with a manifest and N entity files:

```
MyWorld/
  world.ron                 # world header: grid config, layers, defaults
  _manifest.ron             # cell → entity-list index (rebuilt by bake)
  entities/
    ab/cd/abcd1234....ron   # one entity per file, two-byte shard dirs
    ab/cd/abcde567....ron
    ...
  cells/
    00000000/               # cell hash
      hlod_lod0.mesh
      hlod_lod1.mesh
      light_probes.bin
      nav_tile.bin
  level_instances/
    tavern_interior.ron     # reusable sub-scene
```

Each entity file is addressed by its `SceneId` (Phase 4), stored two bytes deep to avoid per-directory file count limits on Windows. The manifest is purely a cache: losing it triggers a full scan rebuild, not data loss.

OFPA lights up Phase 12's structural merge at the *entity* level. Two authors editing different entities in the same cell now produce zero-conflict merges because they edit different files. Two authors editing the *same* entity still go through Phase 12's RON-aware merge, which was always the intended design for the collision case.

Migration path for 1.x scenes: a one-shot editor command `rustforge-editor migrate-scene --to-world` explodes a monolithic `.ron` into an OFPA directory, preserving every `SceneId`. The reverse command exists for projects that want to back out; it is kept in the editor forever.

## 4. Entity streaming and cell membership

An entity's cell is derived from its transform. The streamer maintains `CellMembership { cell: CellCoord, dirty: bool }` as an ECS component; the transform-change system marks `dirty` on any translation write; the end-of-frame streaming tick reparents dirty entities whose computed cell differs from their current cell.

```rust
fn reparent_system(
    mut q: Query<(Entity, &GlobalTransform, &mut CellMembership)>,
    grid: Res<GridConfig>,
    mut events: EventWriter<CellMembershipChanged>,
) {
    for (e, xf, mut m) in q.iter_mut() {
        if !m.dirty { continue; }
        let new = grid.cell_of(xf.translation);
        if new != m.cell {
            events.send(CellMembershipChanged { entity: e, from: m.cell, to: new });
            m.cell = new;
        }
        m.dirty = false;
    }
}
```

Critically, `SceneId` does not change on reparent. References from other entities (Phase 4's `EntityRef` resolving through the `HashMap<SceneId, Entity>` table) stay valid even across cell boundaries, because the resolution table is world-scoped, not cell-scoped.

Edge case: an entity whose transform puts it in an *unloaded* cell. The streamer responds by either (a) loading the destination cell if within radius, (b) despawning the entity but retaining its file on disk if not. The entity is revived from disk when the cell next loads. Games that want persistence across despawn must not rely on component state in despawned entities; they must serialize to the entity file itself.

## 5. HLOD baking

Hierarchical LOD is the feature that makes a radius-192m streamer look like a 2km view distance. Far cells are replaced by a baked merged-mesh proxy — a single draw call for dozens of entities, at LOD0..LODN.

The bake is a pipeline stage (Phase 5 cook integration):

1. For each cell, collect all static-mesh entities.
2. Group by material; merge geometry per group (vertex dedupe, tangent rebuild).
3. Simplify each group at progressively aggressive thresholds → LOD0..LOD3.
4. Cluster adjacent cells into 4×4 super-cells at LOD4+ (the HLOD *hierarchy*).
5. Write `cells/<hash>/hlod_lodN.mesh` + a proxy manifest.

The simplifier is the meshlet-cluster LOD chain from Phase 21, reused. A cell's HLOD feeds the Phase 21 virtualized geometry pipeline as a single cluster hierarchy; the runtime picks LOD based on screen-space projected error, not cell distance — which means a tall tower visible from far away correctly pops to a higher LOD than a low bush at the same distance.

Incremental: the bake hashes each entity's contribution inputs (mesh GUID, material GUID, transform). If every input hash in a cell matches the cached manifest, the cell is skipped. In practice a ten-minute full bake becomes a five-second incremental after the first run.

Dynamic entities (skeletal meshes, moving platforms) do not contribute to HLOD. They stream with their cell and are invisible past the streaming radius. Games that need far-distance visible dynamics (a flying airship on the horizon) author them as "always loaded" via the data layer system (§7).

## 6. Level Instances

A Level Instance is a sub-scene referenced by path, instanced at a transform. It is not a copy — it is a pointer. Editing the sub-scene updates every instance.

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct LevelInstance {
    pub source: AssetId,          // points at level_instances/*.ron
    pub xform:  Transform,
    pub overrides: Vec<Override>, // per-instance property overrides
}
```

Scope is deliberate: Level Instances are for things you want to re-use *by reference* — the same tavern interior appearing in five towns, a cutscene staging set shared between two missions, a modular room kit. They are not for general prefab composition (that is Phase 8's existing prefab system, which copies on instantiation). Two systems, two use cases; do not conflate them.

An instance is streamed as a single unit: loading the instance loads its entire sub-scene atomically, regardless of cell boundaries within the instance. The instance's bounding volume contributes to cell membership; a large instance can cause multiple cells to stay loaded together.

Overrides are *named property paths* (reflected through Phase 2's reflection system). A common pattern: same tavern, different NPC in the corner per instance. The override list is the place where per-instance variation lives; everything else is shared.

## 7. Data layers

Layers are orthogonal to the grid. A cell can be loaded while some of its entities are hidden because their layer is inactive, and vice-versa.

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct DataLayer {
    pub name:           String,
    pub default_active: bool,
    pub initially_loaded: bool,   // streaming eligibility at startup
}

#[derive(Reflect, Serialize, Deserialize)]
pub struct LayerMembership(pub SmallVec<[LayerId; 4]>);
```

Scripts (Phase 8 scripting, Phase 14 gameplay) toggle layers:

```rust
world.layers_mut().set_active("Cinematic_Intro", true);  // show intro-only props
world.layers_mut().set_active("DLC1", feature_flag);     // gate DLC content
```

Typical uses: gameplay-state gating (`BossDefeated_Layer` flips scenery after the boss dies), cinematic staging (props that exist only during a cutscene), DLC gating, difficulty variants (`Hard_Mode_Enemies`), season variants (`Winter_Decorations`).

Layer changes are cheap — they toggle entity visibility and physics/AI participation but do not unload cells. A cell whose only active entities were on a layer that just deactivated stays loaded; the streamer does not know about layers, and does not need to.

## 8. The streaming debugger

Extends Phase 10's diagnostics. A viewport overlay, toggled by `F8` in editor and a console command in shipped builds:

```
Cell state overlay:
   green  = loaded, steady
   yellow = loading (progress bar underneath)
   blue   = unload pending (grace window)
   red    = error (hover for cause)
   grey   = unloaded, in radius (budget-evicted)
   empty  = unloaded, out of radius
```

The overlay is rendered as a 2D minimap anchored lower-right, plus — when a cell is selected — a 3D wireframe box in the viewport at the cell's bounds. Clicking a cell in the minimap selects it and focuses the outliner on its entities.

Per-cell inspector panel: load time (avg/p50/p99), entity count, HLOD LOD currently picked, memory footprint, last-unload cause. If a cell is stuck in `loading` for more than 2s the debugger raises a diagnostic and captures a trace of the I/O calls that cell's loader is waiting on.

Shipped builds: the overlay is present but behind a `--debug-streaming` flag. The per-cell inspector is editor-only.

## 9. Editor UX

The Open World project template seeds:

- `world.ron` with a default 64 m grid.
- An empty `entities/` directory.
- A `SunLight` entity in cell (0,0,0), "always loaded" via a `__Bootstrap` data layer.
- A spawn-point entity, also on `__Bootstrap`.

World outliner groups by cell:

```
World Outliner
├── [__Bootstrap]  (always loaded)
│   ├── SunLight
│   └── PlayerSpawn
├── Cell (-2, 0)   [loaded]
│   ├── ...
├── Cell (-1, 0)   [loaded]
│   ├── ...
└── Cell (0, 0)    [loading 47%]
```

"Load Region" tool: select a rectangle in the top-down world view; the editor loads every cell in that rectangle and pins them (overriding the distance streamer). Designers working on a specific district pin its cells so they do not unload every time the editor camera moves. A pinned-cell budget cap warns when the pin set exceeds the platform tier budget.

## 10. Seams

Two specific seams must not be visible.

**Light probes.** Phase 23's probe volumes are baked per-cell but sampled with trilinear interpolation across cell boundaries. The bake writes probe data into the adjacent cell's boundary band too, duplicated on both sides, so a probe sampled at a cell border interpolates from data present in both of its loaded neighbours. If a neighbour is not loaded, the sampler falls back to the band copy.

**Nav mesh.** Phase 26 already tiled the nav mesh at 16 m. Cells align on 64 m boundaries (= 4 nav tiles), so cell borders are nav-tile borders. The nav tile builder writes per-tile portal edges to the cell on both sides; at runtime, when adjacent cells are loaded, the portal edges are stitched into a cross-cell path.

Not handled by this phase: audio occlusion seams (Phase 18's responsibility), shadow cascade seams for VSMs at cell borders (already handled by Phase 21's virtual shadow atlas since it does not care about scene partitioning), and physics scene seams (Phase 19's broadphase is grid-based and already tolerant).

## 11. Memory budget

Platform tiers (Phase 22) declare a max loaded-cell count and a max per-frame load budget:

```rust
match tier {
    PlatformTier::Mobile   => Budget { cells: 25,  load_mb_per_sec: 20 },
    PlatformTier::Console  => Budget { cells: 81,  load_mb_per_sec: 150 },
    PlatformTier::Desktop  => Budget { cells: 121, load_mb_per_sec: 400 },
    PlatformTier::High_PC  => Budget { cells: 225, load_mb_per_sec: 1200 },
}
```

When loaded-cell count exceeds the cap, the streamer unloads farthest-first until under cap. If the radius setting produces a wanted set bigger than the cap, the radius is silently clamped and a diagnostic is emitted — better to see a shorter view distance than to OOM on a mobile target.

Load bandwidth cap is per-second, not per-frame: a frame with 30 ms of budget left can still consume the whole quota if the previous frame used none. This is a token-bucket, not a hard per-frame gate, because asset I/O is bursty and a hard gate produces worse p99 load latencies for the same average throughput.

## 12. Build order

In order, because each step unblocks the next:

1. **Grid model.** `GridConfig`, `CellCoord`, hash addressing. No streaming yet. Entities still live in one file. The grid exists purely as a spatial index.
2. **Async loader prototype.** Single-cell load/unload against a hand-built manifest, no OFPA yet. Prove the tokio pool, the priority queue, the budget.
3. **OFPA migration.** Explode 1.x scenes into per-entity files. Ship the migration command in editor. Phase 12 merge works per-entity after this lands.
4. **HLOD baker.** Meshlet-cluster simplifier from Phase 21 wired to cell groups. Baked proxies only; runtime swap happens via Phase 21's existing LOD selection.
5. **Entity streaming.** `CellMembership`, reparent-on-transform-change, cross-cell reference stability. The "entity moves between cells" story only works after OFPA is in.
6. **Level Instances.** Reusable sub-scene references. Overrides. Depends on OFPA for sub-scene representation.
7. **Streaming debugger.** Overlay, per-cell inspector. Extends Phase 10. Last in build order because it is the system that makes the preceding ones *diagnosable*, and cannot be built until there is something to diagnose.
8. **Data layers.** Named layer toggling, script API. Independent of the spatial streamer; safe to land last.

## Scope ❌

Explicitly excluded:

- **Infinite procedural worlds.** Separate future phase in the 2.0 arc. The grid here is finite and authored.
- **Planetary-scale curvature.** The world is flat. Curved-planet coordinate systems are Phase 34+ speculative.
- **MMO shard servers.** Cross-shard persistence and player handoff are Phase 34 territory, not 31.
- **Runtime chunk editing from scripts.** Games can move, spawn, and despawn entities at runtime. They cannot add, remove, or resize cells. Cells are an authoring-time concept baked at cook.
- **Streaming of non-spatial data.** Save games, dialogue trees, quest state — not the streamer's job; they load once at startup.
- **Per-entity LOD override of HLOD.** HLOD is a cell-group bake; individual entities do not opt out except by being on a never-contributes-to-HLOD layer (e.g. dynamic props).
- **Multi-origin floating-point rebasing.** Single world origin at f32. If you need a 50 km world with no precision loss, rebase at the camera transform (a separate, smaller feature in a later 2.0 phase).

## Risks

- **OFPA file count explosion.** A 1,600-cell world at 200 entities/cell is 320,000 files. Two-byte-shard dirs keep per-directory counts sane, but the whole tree slows `git status` on large projects. Mitigation: a `.gitattributes` rule treating `entities/**/*.ron` as a lock-free type, plus a `rustforge-vcs compact` command that packs cold cells into a single archive readable by the streamer. The compact format is an alternate path, not a replacement.
- **Rebake churn.** Moving one static mesh retriggers HLOD bake for its cell. If the project is on a shared build server, rebake traffic dominates CI time. Mitigation: remote bake cache keyed on the entity-contribution hash; in practice 95% of "moved meshes" were authored moves of already-baked content, so the cache hits.
- **Cross-cell reference failure modes.** Entity A in cell (0,0) references Entity B in cell (5,5). Cell (5,5) never loads. `EntityRef` resolution returns `None` and the code path must handle it. Mitigation: the reflection-driven validator (Phase 2) statically detects cross-cell references and surfaces them in the outliner with a "may not resolve" warning. The reference is still legal; the author is warned.
- **Streaming hitches at tier boundaries.** A mobile player on the edge of the 25-cell budget hits constant evict-and-reload. Mitigation: the budget is a soft cap with a 20% hysteresis band; evictions wait for the excess to exceed 20% of budget before firing.
- **Save-game bloat.** Per-entity files make save-game of "world state that diverged from authored state" expensive: a naive save writes the full entity set. Mitigation: save deltas, not entity files. The save layer (Phase 14) records *changes since bake*, not full entity state.
- **Data-layer explosion.** A project that uses layers for every gameplay flag ends up with 400 layers and a query that walks all of them per frame. Mitigation: soft cap of 64 layers per project with a loud warning, hard cap of 256. `LayerId` is a u8.
- **Level Instance override drift.** An instance overrides property X on one child entity; the source scene is edited to delete that child. The override is now orphaned. Mitigation: the loader detects orphaned overrides, logs them, and preserves the override payload in the instance file so a later re-add of the child restores it.

## Exit criteria

Phase 31 ships when:

1. An open-world project (cell count ≥ 1,000) loads at 60 Hz on desktop tier with no hitches greater than 16 ms attributable to cell streaming.
2. HLOD-baked proxies render at draw-call counts within 2× of the cell's own LOD4 single-mesh output.
3. Incremental cell bake on a project with 1% dirty cells completes in under 10 s on a warm cache.
4. Two authors editing different entities in the same cell produce zero merge conflicts via `git merge`.
5. Entity-migration across cells preserves `SceneId` with zero reference loss in the reference-tracker test suite.
6. The streaming debugger overlay is reachable in both editor and shipped builds, reports correct cell state, and captures a usable trace on any cell stuck loading for > 2 s.
7. Data layer toggles from script cost < 100 µs for a 64-layer project, independent of cell count.
8. A 1.x monolithic scene migrates to OFPA with `migrate-scene --to-world` in a single command, round-trips through `--to-scene`, and passes bit-identical comparison on everything except cell-membership components (which are derived, not authored).
9. Mobile tier runs at 25 loaded cells with bandwidth-capped loads, never OOMs on the long-haul soak test, and holds 30 Hz.
10. Phase 26's nav mesh stitches cleanly at cell borders: pathfinding across four cells (a 256 m walk) returns a path with no seams at cell boundaries in 100% of 10,000 randomized queries.
11. A Level Instance referenced three times in the world, authored once, updates all three instances on author edit with no manual re-link.
12. No regression in Phase 12 structural merge: every merge test that passed against monolithic scenes passes against OFPA with at least equivalent conflict counts.

Phase 32 follows. The grid is the foundation the rest of 2.0 builds on.
