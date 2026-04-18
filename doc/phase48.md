# Phase 48 — Virtualized Geometry Streaming

Phase 21 shipped meshlet cluster rendering with compute-shader culling, a DAG LOD, and indirect draws. In the honest accounting at the end of that phase, the scope list read: **no on-disk cluster pages, no streaming, meshlets stay in RAM**. That is the half of Nanite which turns a "cluster renderer" into *Nanite*. Without it, the scene's entire meshlet corpus has to fit in GPU memory today — great for 12M-triangle test scenes, a wall the moment an art team opens a Quixel Megascans library. Phase 48 closes the gap. It adds the disk-paged virtualized cluster store, the GPU-resident cluster cache, the streaming scheduler, the pointer table, the fallback cluster, and the World Partition and HLOD integrations that make a 5-billion-triangle source world stream at 60 fps on a 2021 mid-range desktop.

Phase 48 is not a rewrite of Phase 21. Phase 21's cluster builder, GPU culler, and indirect draw path stay. This phase attaches a streaming layer underneath them: clusters become addressable by page, the culler learns to ask "is this cluster resident?" before emitting a draw, and a background pipeline fetches the pages that weren't. The interface between the renderer and the cache is a single `ClusterPointerTable` — if it returns a slot, the cluster is resident; if it returns a sentinel, the culler substitutes the mesh's fallback cluster and marks the fault. That is the whole API surface. Everything else in this phase supports those two operations.

What this phase explicitly does not do: skeletal virtualized geometry (UE 5.5 only just barely ships it, and the rig-to-cluster coherence problem is genuinely hard — future phase), GPU mesh generation (that belongs to Phase 40's PCG runtime, not here), runtime LOD editing in the editor (authoring-only concern), lossless reassembly of the source mesh from its cluster pages (clusters are a render format, not a round-trip format), and console backends (still blocked by wgpu — documented, not engineered around).

## Goals

By end of Phase 48:

1. **On-disk cluster hierarchy** — each imported mesh ships a compressed BVH of clusters with per-node screen-error metric, laid out as header + page index + payload pages for streaming.
2. **Fixed-size cluster pages** — 128 KB pages holding ~512 clusters each, zstd-compressed at cook, aligned for fast GPU upload.
3. **GPU-resident cluster cache** — fixed-size ring buffer (1 GB on desktop High, 512 MB baseline), LRU eviction, lock-free slot allocator.
4. **Pointer table** — per-mesh `ClusterId → PageSlot` table, GPU-readable, updated on fetch and evict, queried by the Phase 21 culler every frame.
5. **Streaming scheduler** — consumes per-frame GPU fault readback, prioritizes by screen coverage and camera trajectory, dispatches to an I/O thread pool.
6. **Fallback cluster** — every mesh carries a permanently-resident lowest-LOD cluster, used whenever a higher-LOD cluster is in flight or evicted. No frame ever emits nothing.
7. **World Partition integration** — cell load / unload events pre-fetch / evict associated cluster pages; Phase 31's streaming direction hints feed the scheduler.
8. **HLOD integration** — Phase 31 HLOD proxies are themselves virtualized meshes, using the same pipeline.
9. **Phase 42 profiler extension** — new "virtualized geometry" panel: resident count, fetch queue depth, eviction rate, fault rate, per-mesh breakdown.
10. **Author-time importer stats** — mesh importer (Phase 5) surfaces post-cluster-build numbers: cluster count, hierarchy depth, page count, estimated streaming footprint.
11. **Per-mesh full-residency override** — critical assets (player model, UI meshes, flagship props) can opt out of streaming and pin the whole cluster corpus in cache.
12. **Platform tier tuning** — cache size, prefetch radius, page compression level defaults per tier; mobile and WebGL2 forced to full-residency.
13. **Phase 47 ray-tracing integration** — RT BVH is built over resident clusters only; non-resident clusters contribute their mesh's fallback geometry to ray hits.
14. **Editor residency debug view** — viewport overlay color-coding clusters by state (resident / fetching / fallback / evicted).

## 1. The cluster hierarchy on disk

Phase 21 built meshlet clusters at import but kept them in a flat per-mesh buffer. Phase 48 reorganizes the same clusters into a BVH and serializes the BVH to the cooked mesh.

```rust
// crates/rustforge-core/src/geometry/virtualized/hierarchy.rs
#[derive(Serialize, Deserialize)]
pub struct ClusterHierarchy {
    pub root: ClusterNodeId,
    pub nodes: Vec<ClusterNode>,      // BVH nodes
    pub clusters: Vec<ClusterHeader>, // leaves (per-cluster meta, no payload)
    pub fallback: ClusterId,          // always-resident lowest-LOD cluster
    pub page_index: PageIndex,        // cluster_id -> (page_id, offset_in_page)
}

#[derive(Serialize, Deserialize)]
pub struct ClusterNode {
    pub bounds: BoundingSphere,
    pub screen_error_at_1m: f32,   // used by culler to compute LOD cut
    pub children: SmallVec<[ClusterNodeId; 8]>,
    pub leaf_clusters: Range<u32>, // cluster_id range if node is a leaf
}

#[derive(Serialize, Deserialize)]
pub struct ClusterHeader {
    pub bounds: BoundingSphere,
    pub cone: NormalCone,
    pub page_id: PageId,       // which page holds this cluster
    pub page_offset: u32,      // byte offset inside the page
    pub payload_size: u32,     // compressed size; uncompressed is implied
    pub material_id: u32,
}
```

The BVH is built at import with a surface-area-heuristic split, capped at 8-way branching. Eight-way matches the GPU culler's SIMD width better than binary for the traversal the Phase 21 culler already runs — we re-use its existing node-test kernel with a widened lane count.

Opinion: this layout looks heavier than the Phase 21 flat buffer and is. A 400k-tri Quixel asset goes from ~20 MB flat to ~24 MB packaged (BVH overhead ~15%). The tradeoff buys streaming; there is no version of "streaming but without a spatial index" that is not strictly worse.

## 2. The page format

Clusters are grouped into 128 KB pages at cook time. The grouping is **spatial** — clusters inside the same BVH subtree go into the same page when they fit — not sequential in cluster-id order. A traversal that descends into a subtree is overwhelmingly likely to need all its children; keeping them in one page means one I/O fetches the lot.

```
Page layout (128 KB, aligned to 4 KB on disk):

 ┌─────────────────────────────────────────────┐
 │  PageHeader   (32 B)                        │
 │    magic, version, cluster_count, zstd_size │
 ├─────────────────────────────────────────────┤
 │  ClusterOffsets[cluster_count]  (8 B each)  │
 ├─────────────────────────────────────────────┤
 │  zstd stream                                │
 │    - cluster 0 vertex + index data          │
 │    - cluster 1 ...                          │
 │    ...                                      │
 └─────────────────────────────────────────────┘
```

- **Page size** is 128 KB compressed. Why this size: large enough that zstd's window amortizes (~10% better ratio than 32 KB pages), small enough that one fetch is ~2 ms on NVMe and fits in a single upload staging buffer without fragmentation.
- **Compression** is zstd level 9 at cook. Decompression is ~1.5 GB/s on one thread; Phase 9's shipping profile already runs zstd-19 for asset data, we drop to 9 here because decode latency matters more than on-disk size.
- **Meshoptimizer ebit encoding** is applied to index data *before* zstd. Empirically gives another 20% on top of zstd alone for index streams.
- **Alignment**: pages start at 4 KB file offsets. DirectStorage (Phase-future) and our current async-read pool both want this.

Rejected alternative: per-cluster files. A 5-billion-triangle world has ~40 million clusters. 40 million files on Windows is 40 million MFT entries. No.

## 3. Importer integration (Phase 5 extension)

Phase 5's mesh importer currently produces a `CookedMesh` with LOD levels and the Phase 21 meshlet buffer. Phase 48 extends the same importer:

```rust
// crates/rustforge-core/src/assets/importers/mesh.rs
pub struct MeshImportResult {
    // existing Phase 5 output:
    pub lods: Vec<LodLevel>,
    pub meshlets: MeshletBuffer,
    // new in Phase 48:
    pub hierarchy: ClusterHierarchy,
    pub pages: Vec<PageBlob>,       // each 128 KB, zstd-compressed
    pub import_stats: ClusterImportStats,
}

pub struct ClusterImportStats {
    pub cluster_count: u32,
    pub hierarchy_depth: u8,
    pub page_count: u32,
    pub total_pages_compressed: u64,
    pub total_pages_uncompressed: u64,
    pub fallback_cluster_tris: u32,
    pub estimated_working_set_mb: f32, // at typical view distance
}
```

The importer panel (Phase 5's asset inspector) shows these stats after import, with a `Force full residency` checkbox per mesh and a project-level default. Flagship assets — the player mesh, the weapon — get it on; environment props leave it off.

Determinism (Phase 9 exit criterion): the cluster BVH build is seeded from `blake3(source_bytes)`, not from wall time or memory addresses. Two cooks produce byte-identical pages.

## 4. The GPU-resident cluster cache

One big buffer on the GPU, partitioned into fixed-size slots:

```rust
// crates/rustforge-core/src/render/virtualized/cache.rs
pub struct ClusterCache {
    pub gpu_buffer: wgpu::Buffer,           // 1 GB typical
    pub slot_size: u32,                     // 8 KB — one decompressed cluster
    pub slot_count: u32,                    // cache_size / slot_size
    pub free_list: Mutex<VecDeque<SlotId>>,
    pub lru: Mutex<LruIndex<ClusterHandle>>,
    pub resident: DashMap<ClusterHandle, SlotId>,
}

#[derive(Copy, Clone, Hash, Eq, PartialEq)]
pub struct ClusterHandle {
    pub mesh: AssetGuid,
    pub cluster: ClusterId,
}
```

- **Slot size** is 8 KB decompressed — chosen so 124-tri / 64-vert clusters fit with vertex attributes (position, normal, tangent, UV) packed. Larger would waste; smaller would fragment.
- **Free-list + LRU** are both touched from the main thread only. Worker threads push completed uploads onto a queue; the main thread integrates them under the caches's locks. No multi-writer lock contention.
- **Eviction** runs when the scheduler would have to wait for a slot. Evict the LRU cluster whose last-touched frame is oldest. Never evict a cluster whose mesh has `force_full_residency`; those are pinned.
- **Fallback cluster pinning** — every loaded mesh's fallback cluster is pinned at mesh-load time and only released on mesh unload. The scheduler treats them as always-resident.

Opinion: 1 GB is a lot. It is also deliberately conservative — Nanite on UE5 uses ~768 MB as its stated default, but we are cutting ebit corners and packing less tightly, so we budget higher. The High baseline should feel tight enough that users notice eviction, not so tight it triggers thrashing on sane scenes.

## 5. The pointer table

Every frame, the Phase 21 culler produces a list of `(mesh, cluster_id)` pairs for visible clusters. Before Phase 48, the cluster data was wherever it was in a flat buffer. Now the culler has to ask where it lives *right now*:

```wgsl
// crates/rustforge-core/src/shaders/virtualized/pointer_table.wgsl
struct PointerTableEntry {
    slot_id: u32,         // SLOT_INVALID = 0xFFFFFFFF if not resident
    fallback_slot: u32,   // mesh's fallback cluster slot (always valid)
    last_touch_frame: u32,
    flags: u32,
}

@group(0) @binding(0) var<storage, read> pointer_table: array<PointerTableEntry>;

fn resolve_cluster(mesh: u32, cluster: u32) -> u32 {
    let key = pointer_table_index(mesh, cluster);
    let e = pointer_table[key];
    if (e.slot_id == 0xFFFFFFFFu) {
        // fault: record for readback, use fallback
        report_fault(mesh, cluster);
        return e.fallback_slot;
    }
    return e.slot_id;
}
```

Table layout: one entry per `(mesh, cluster_id)`, hashed. Size is bounded by `sum(mesh.cluster_count for mesh in loaded_meshes)` — at ~16 B/entry and 40M clusters in memory-loaded meshes, ~640 MB for the table on paper. Mitigation: the table is itself cluster-indirected — one small GPU buffer per mesh rather than one global table — and only *loaded* meshes contribute. A scene with 5000 Quixel instances of 400 distinct meshes has 400 pointer tables, ~6 MB each in the worst case, ~2.4 GB. That is too much. See §12 Risks.

Correction for §5: the per-mesh table is in RAM, uploaded lazily to the GPU only for meshes with visible clusters this frame. Resident-set size on GPU is proportional to *drawn* meshes, not *loaded* meshes. Typical cost ~32 MB.

## 6. The streaming scheduler and the I/O pool

```
 ┌────────────┐    per-frame     ┌─────────────┐
 │  Culler    │─── fault list ──▶│  Scheduler  │
 │  (Phase 21)│                  │             │
 └────────────┘                  │  - priority │
                                 │  - coalesce │
                                 │  - throttle │
                                 └──────┬──────┘
                                        │ enqueue
                                        ▼
                                  ┌───────────┐
                                  │  IO pool  │───▶ read page from disk
                                  │  (4 thrd) │     zstd decode
                                  └─────┬─────┘     ebit decode
                                        │ page blob
                                        ▼
                                  ┌───────────┐
                                  │  Upload   │───▶ wgpu queue write
                                  │  (main)   │     update pointer table
                                  └───────────┘
```

Per-frame loop (main thread):

1. Read back GPU fault buffer from N-2 frames ago (double-buffered to avoid stall).
2. Group faults by page. One page fetch satisfies all clusters in it.
3. Compute priority per pending page: `coverage * proximity * (1 + flight_path_hint)`.
4. Push top K requests onto the I/O pool's queue, where K = max_inflight - already_inflight (cap at 16 inflight fetches).
5. Drain completed uploads: allocate cache slots, evict if needed, write the pointer table.

**Flight-path hint** comes from Phase 31's camera trajectory predictor (already used for cell streaming). Cluster pages on cells the camera is moving toward get a `1.5x` priority boost. Pages on cells the camera is leaving get `0.5x`, encouraging the LRU to free them.

I/O pool sizes per tier (see §10): desktop High 4 threads, Medium 2, mobile / web 0 (no streaming).

## 7. The fallback cluster — pop-in avoidance

The single biggest cosmetic complaint about any streaming system is pop-in. The fallback cluster is how Phase 48 avoids it.

- Every mesh carries exactly one `fallback_cluster` at the root of its LOD DAG: the coarsest single cluster that represents the whole mesh. For a 400k-tri asset this is typically a ~128-triangle shell.
- On mesh load, the fallback cluster is fetched immediately (single page, synchronous during async load) and pinned.
- Whenever the culler asks for a cluster that is not resident, the pointer table returns the fallback's slot. The mesh always draws *something*.
- As higher-LOD clusters stream in, the pointer table is updated and the mesh progressively sharpens.
- The transition is not blended in this phase — the cluster simply switches. Phase 21's dither-across-one-frame LOD transition composes on top if enabled.

This is the single most important user-visible feature of Phase 48. A scene with 50% of its clusters evicted should look blurry, not broken.

## 8. World Partition integration (Phase 31)

Phase 31 streams cells. Phase 48 streams clusters. They have to cooperate, not race:

```rust
// crates/rustforge-core/src/world/partition/streamer.rs  (existing, extended)
impl CellStreamer {
    fn on_cell_load_begin(&mut self, cell: CellCoord, cluster_sched: &mut ClusterScheduler) {
        let manifest = self.cell_manifest(cell);
        for mesh_instance in &manifest.meshes {
            // Request the fallback immediately — synchronous with cell load.
            cluster_sched.ensure_fallback(mesh_instance.mesh_guid);
            // Seed the scheduler with expected-visible pages.
            cluster_sched.prefetch_for_instance(mesh_instance, PREFETCH_PRIORITY);
        }
    }

    fn on_cell_unload(&mut self, cell: CellCoord, cluster_sched: &mut ClusterScheduler) {
        let manifest = self.cell_manifest(cell);
        for mesh_instance in &manifest.meshes {
            cluster_sched.release_instance_hint(mesh_instance);
            // Pages that are only referenced by this cell drop priority;
            // LRU will free them naturally.
        }
    }
}
```

Two invariants:

- A cell is never reported "loaded" to gameplay until every mesh instance in it has its fallback cluster resident. Gameplay can run; higher-LOD clusters stream in behind.
- Cluster cache eviction never crosses into a cell's fallback set. If pressure forces it, that is a budget overrun and §9's profiler fires a warning.

## 9. HLOD integration (Phase 31)

Phase 31's HLOD bake produces one merged-mesh proxy per cell cluster at each distance level. Those proxies go through the same Phase 48 pipeline: cluster-built, paged, streamed, fallback'd.

The only wrinkle: HLOD proxies are themselves extremely geometry-dense by design (they represent thousands of source meshes at distance). The cluster build for an HLOD proxy routinely produces 20k+ clusters. Page count per proxy is high. The prefetch radius for HLOD pages is therefore computed separately from regular-mesh pages — HLOD pages belong to *far* cells and should be preloaded aggressively, regular-mesh pages to *near* cells and can afford to lag.

A sanity check at bake: if an HLOD proxy's cluster count exceeds the sum of its source meshes', the bake rejected it (it's gotten more complex, not less — a bug in the mesh-merge path).

## 10. Platform tier tuning

Phase 21 already established `RealismTier::{Low, Medium, High}`. Phase 48 tier defaults:

| Tier / Target              | Cache size | IO threads | Prefetch radius | Streaming |
|----------------------------|-----------:|-----------:|----------------:|-----------|
| Desktop High (3060+, M2+)  |    1024 MB |          4 |           256 m | yes       |
| Desktop Medium baseline    |     512 MB |          2 |           128 m | yes       |
| Desktop Low                |     256 MB |          2 |            64 m | yes       |
| Console (wgpu gap)         |        n/a |        n/a |             n/a | blocked   |
| Mobile (Phase 22)          |     full-res |        0 |               — | no        |
| Web — WebGPU (Phase 22)    |     full-res |        0 |               — | no        |
| Web — WebGL2 (Phase 22)    |     full-res |        0 |               — | no        |

**Mobile and web** force full residency. There is no async readback story on WebGL2, and on WebGPU the Chrome/Safari throttling of worker I/O is too unpredictable to build a scheduler against. The importer flags geometry-heavy scenes when their total cluster corpus exceeds the mobile cache estimate — an authoring-time warning, not a runtime error.

**Console** is honest-unsupported. When wgpu grows console backends, the same tier slots in. Documenting this now keeps the API shape honest.

## 11. Ray tracing integration (Phase 47)

Phase 47 ships hardware ray-traced reflections and shadows on top of the resident cluster set. The RT BVH is built over **only resident clusters**, rebuilt incrementally when the pointer table changes. Non-resident clusters contribute their mesh's **fallback geometry** to the RT BVH — not nothing. A reflection of a distant tree shows the tree's fallback shell, not empty space.

Implication to document in the Phase 47 manual: reflections may show lower geometric detail than the rasterized pixels for the same surface, because the rasterizer has access to fully-resident clusters in the near view frustum while the reflection ray may be querying distant meshes that are fallback-only. This is a known and accepted artifact. Spending RT budget on keeping the BVH fully up-to-date with every streamed cluster would dominate the ray tracing cost.

## 12. Build order within Phase 48

Each step is independently shippable against the Phase 21 renderer:

1. **Cluster hierarchy builder** — extend Phase 21's meshlet build with BVH construction. No runtime change yet; cooked assets grow.
2. **Page format + compression** — serialize the hierarchy's clusters into 128 KB zstd pages. Runtime still loads everything at startup; change is cook-only.
3. **Importer integration (Phase 5)** — stats UI, `force full residency` checkbox, deterministic build seed.
4. **GPU cache + pointer table** — the runtime structures. Loader still populates everything synchronously at mesh load; behaviorally equivalent to Phase 21.
5. **Streaming scheduler + I/O pool** — now meshes load only the fallback synchronously, the rest streams. The first user-visible behavior change.
6. **GPU fault readback** — closes the loop; the culler reports what it needed and didn't get.
7. **Fallback cluster + pop-in avoidance** — the fallback path has been there since step 4, now it's *used*.
8. **World Partition integration (Phase 31)** — cell load/unload hooks feed the scheduler.
9. **HLOD integration (Phase 31)** — proxies go through the same pipeline.
10. **Phase 42 profiler integration** — the "virtualized geometry" panel.
11. **Phase 47 RT integration** — BVH is built over resident set + fallbacks.
12. **Platform tier tuning** — the table in §10 becomes the shipped defaults; mobile / web force full-residency paths.

## 13. Scope — what's NOT in Phase 48

- ❌ **Skeletal virtualized geometry.** Rig-driven cluster coherence is genuinely hard; UE 5.5's version still has limits. Future phase.
- ❌ **Runtime LOD editing.** Editor authoring concern; clusters are built at import, not tweaked at play time.
- ❌ **GPU mesh generation.** Phase 40's PCG runtime owns procedurally-born geometry.
- ❌ **Lossless source round-trip.** Clusters are a render format. Exporting a source mesh back out reconstructs only what the BVH preserved, which is not the source mesh.
- ❌ **Console platform backends.** Blocked on wgpu; documented, not engineered around.
- ❌ **Animation clip streaming.** Phase 31's asset streaming covers that broadly; clusters stream, animations stream, they don't need to share a scheduler.
- ❌ **Virtual texturing coupling.** Phase 48 is geometry only. Cluster UVs still index bindless textures from Phase 21. A future virtual-texturing phase co-schedules pages with clusters; not this one.

## 14. Risks

- **Pointer table bloat.** The corrected §5 analysis — per-mesh tables, GPU-resident only for drawn meshes — works only if mesh draw count is bounded. A scene with 10k distinct meshes visible at once breaks it. Mitigation: profile draw-call counts in real scenes, document the ceiling, error at load time if exceeded rather than silently stuttering.
- **Fault-to-fetch latency.** Three-frame round-trip (detect → readback → fetch → upload) at 60 fps is ~50 ms. Fast camera motion outruns the scheduler and the scene is fallback-only briefly. Mitigation: the Phase 31 flight-path hint is load-bearing, not optional; disable it and the system visibly stutters.
- **LRU thrashing on pan scenes.** A camera that sweeps across a 5-billion-triangle environment can evict and re-fetch the same pages repeatedly. Mitigation: don't evict pages younger than 60 frames unless the cache is genuinely full; prefer expanding the scheduler backpressure instead.
- **I/O pool tail latency.** One slow read blocks its thread; with 4 threads and 16 inflight, one 200 ms I/O stalls 25% of throughput. Mitigation: cap per-fetch time at 100 ms; timeouts re-enqueue at lower priority and log a disk-health warning.
- **HLOD proxy cluster explosion.** A proxy that merges 8000 source meshes produces its own vast cluster tree; streaming its pages is its own problem. Mitigation: proxies get a separate cluster budget, tracked in §9's profiler, flagged at bake if over.
- **Editor replay of streaming.** The editor runs the streamer differently from shipped games (no fixed camera, frequent teleports). The scheduler must handle "camera jumped 500 m" without thrashing the cache — mitigation: on camera discontinuity events from Phase 31, the scheduler flushes pending fetches and reseeds from the new position.
- **Phase 21 coupling.** If the Phase 21 culler changes its cluster-indirection API, every Phase 48 integration breaks. Mitigation: `ClusterPointerTable` is the sole interface, versioned; Phase 21 commits to preserving it.
- **RT BVH rebuild cost (Phase 47).** Rebuilding the BVH every time the pointer table changes is too expensive. Mitigation: batch pointer-table diffs across 8 frames, rebuild the RT BVH once per batch; accept that reflections lag geometry by ~130 ms during heavy streaming.
- **Determinism under concurrent I/O.** Upload order depends on disk latency, which is not deterministic. Mitigation: the *final* rendered frame depends only on which clusters are resident, not upload order; the pointer table is sorted before GPU upload each frame so shader output is deterministic given a resident set.
- **Mobile full-residency denial.** A mobile scene author assumes streaming works and ships a 10 GB cluster corpus. Mitigation: importer-time warning at 256 MB total; build-time error at 1 GB for mobile targets.
- **Scheduler starvation of critical assets.** A cinematic cutscene needs the hero mesh at full LOD *now*. Mitigation: the `force_full_residency` override pins those meshes; scheduler never de-prioritizes pinned-mesh pages.

## 15. Exit criteria

Phase 48 is done when all of these are true:

- [ ] Cluster hierarchy is built at import for every mesh; hierarchy is deterministic under the `blake3(source_bytes)` seed (two cooks produce byte-identical pages).
- [ ] Cooked meshes serialize as header + page index + 128 KB zstd-compressed pages; Meshoptimizer ebit applied to index streams; on-disk size is within 20% of Phase 21's flat meshlet buffer at equivalent quality.
- [ ] Phase 5 importer panel surfaces cluster count, hierarchy depth, page count, and estimated working-set MB per mesh. `Force full residency` checkbox round-trips through `.meta`.
- [ ] Runtime loads a mesh in under 10 ms median (fallback cluster only); higher-LOD clusters stream without blocking the main thread.
- [ ] GPU cluster cache implements LRU eviction; pinned clusters (fallback + `force_full_residency`) are never evicted; eviction under steady-state pressure cycles ≤ 10% of slots per second on the canonical Quixel fly-through.
- [ ] Pointer table lookups from the Phase 21 culler cost ≤ 1 µs per cluster on the reference High-tier GPU; GPU fault readback adds ≤ 0.3 ms / frame.
- [ ] Streaming scheduler prioritizes by coverage × proximity × flight-path-hint; top-K selection cost on the main thread stays under 0.1 ms / frame even with 4k pending pages.
- [ ] Every loaded mesh has its fallback cluster resident before its first draw; no frame ever draws a mesh with no geometry, regardless of cache pressure.
- [ ] Phase 31 cell-load events fire the scheduler's `ensure_fallback` + `prefetch_for_instance`; cell-unload drops priorities; a cross-cell canonical teleport test produces zero black (zero-geometry) frames.
- [ ] HLOD proxies go through the Phase 48 pipeline with no proxy-specific code paths in the renderer; HLOD cluster count ≤ source cluster count is asserted at bake.
- [ ] Phase 42 profiler "virtualized geometry" panel shows resident count, fetch queue depth, eviction rate, fault rate, per-mesh cluster residency. All numbers match the engine's internal counters within a per-second sample window.
- [ ] Phase 47 RT BVH is rebuilt incrementally from resident clusters + fallback geometry; rebuild cost ≤ 2 ms / frame amortized across the 8-frame batch window.
- [ ] **Quixel Megascans perf target**: a scene with 5000 instances of 400 distinct Megascans assets (avg 400k tris each, 2 billion source tris in scene) renders at steady 60 fps on an RTX 3070 at 1440p, fallback usage ≤ 5% of drawn clusters during normal traversal, no visible pop-in on the canonical 60-second fly-through.
- [ ] **Scale target**: a 5-billion-triangle source-geometry world loads its camera cell's fallback set in under 500 ms on NVMe; reaches steady-state residency (fault rate ≤ 1% of drawn clusters) within 3 seconds of standing still.
- [ ] Mobile target (Phase 22) runs a geometry-reasonable scene (≤ 200 MB total cluster corpus) in full-residency mode at tier-appropriate frame rate; the importer flags any scene over 256 MB.
- [ ] WebGPU target runs full-residency; WebGL2 target runs full-residency with the Phase 22 fallback mesh path (no cluster culler at all — clusters are pre-merged at build for that target).
- [ ] Editor viewport residency debug view color-codes clusters (resident / fetching / fallback / evicted) and matches the profiler panel's counts.
- [ ] Every test in this phase is exercised as a reproducible scene fixture checked into the repo; frame-rate and residency numbers are regression-tracked per commit.
- [ ] Phase 21 `RenderFeature` trait is respected: `VirtualizedGeometryStreaming::min_tier()` returns `Medium`; on `Low` the fallback is full residency identical to Phase 21 behavior.
- [ ] Phase 9 shipping build includes cluster pages in `assets.pak`; incremental cook skips unchanged meshes' page regeneration; cook is deterministic per Phase 9's determinism criterion.
