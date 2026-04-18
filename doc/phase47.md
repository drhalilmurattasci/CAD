# Phase 47 — Unified Global Illumination

Phase 21 shipped voxel cone tracing as the primary GI, with screen-space probes as a Medium-tier fallback. That was the right bet in the 1.0 timeline — wgpu had no ray-query, VCT's leak/blockiness was the price of shipping real-time GI at all, and lightmaps stayed mandatory for shipping quality. Phase 36 then layered hardware-ray-traced GI on top as an Ultra-tier upgrade. That was also the right bet in the 2.0 timeline — ray-query stabilized, RTX-class GPUs finally deserved a first-class path, and the offline path tracer needed the same BVH plumbing anyway.

What shipped from those two phases is a renderer with **two parallel GI systems**. They don't share authoring. They don't share probes. They don't share denoisers. They don't share a quality slider — the project opts into one or the other at tier-detection time, and a creator who authored a scene on a High-tier box gets a different look on an Ultra-tier box because the GI solver changed underneath them. That's not a tier system; that's two products sharing a binary. VCT's light leaks were a known defect we accepted in Phase 21 because the alternative was no GI. They are no longer acceptable when the Ultra path exists and looks correct on the same content.

Phase 47 replaces both. **One GI pipeline. SDF-based software tracing as the primary path, BVH-based hardware tracing as a tier upgrade inside the same pipeline, one set of probes, one radiance cache, one final gather, one denoiser.** The authoring surface is a single quality slider; the engine picks SW vs HW at boot based on device capability and never asks the creator to care. This is the Lumen architectural bet — Lumen's strength was never the SDF tracer on its own, nor the HW RT path on its own; it was that both drop into the *same* pipeline stages with the *same* probes feeding the *same* final gather, so a scene that lights correctly on Xbox Series S also lights correctly on an RTX 4090 with strictly higher fidelity, not a different look.

Phases 21 and 36 are not rolled back. VCT is deprecated and removed from the GI pipeline; it stays referenced in the codebase only until migration notices expire. Phase 36's HW RT GI path is folded into Phase 47's unified pipeline as the Ultra tier's trace primitive. Phase 36's path tracer, lightmap baker, and RT reflections/shadows are untouched — those remain separate subsystems with their own scopes.

## Goals

By end of Phase 47:

1. **Unified GI pipeline** — one set of pipeline stages (scene SDF assembly → ray trace → screen-space probes → radiance cache update → final gather → denoise) shared across all tiers. SW SDF and HW BVH are alternate implementations of the *trace primitive* inside this pipeline, not separate systems.
2. **Per-mesh SDF bake** — the mesh importer (Phase 5) produces a compressed sparse 3D SDF texture per mesh, cached alongside the mesh asset. Bake time and disk cost are documented and budgeted.
3. **Scene SDF assembly** — per-frame compute pass assembles a sparse scene-level SDF structure over per-mesh SDFs and current transforms. Incremental: static world SDFs cached, dynamic objects re-stamped only when moved.
4. **Software SDF ray-march** — the default GI trace primitive. Runs on High and Medium tiers. Replaces VCT as the primary GI technique; the light-leak and voxel-blockiness artefacts VCT suffered do not reappear because SDF tracing converges at surface granularity.
5. **Screen-space probes** — per-frame probe placement in screen space near visible pixels. Probes accumulate radiance via the trace primitive. The screen-space probe scheme Phase 21 used as a fallback graduates to first-class citizen.
6. **Persistent world-space radiance cache** — cell-based world probe cache reused across frames and visible cells (aligns with Phase 31 spatial partitioning). Updated amortized round-robin: N probes per frame, not all probes every frame.
7. **Final gather** — per-pixel integration reads both screen-space probes and radiance cache, producing diffuse + rough-specular indirect in a single pass.
8. **Reflections via the same trace primitive** — glossy and mirror reflections reuse the same SW SDF or HW BVH trace. SSR remains as the near-field contact-reflection fallback; probe-based reflections remain for cheap infrequent updates. Phase 36's standalone RT-reflection path is folded in.
9. **Shadows via the same trace primitive** — unified ray-traced soft shadows share the same SW/HW trace. Virtual Shadow Maps (Phase 21) stay available as an opt-in for directional-cascade coverage in scenes that exceed the RT shadow budget.
10. **Sky lighting through the radiance cache** — sky radiance is sampled into the cache during its amortized update. No separate sky-lighting pass.
11. **HW RT as an Ultra-tier swap-in** — on ray-query-capable hardware the trace primitive swaps from SDF march to BVH ray-query with identical inputs and outputs. Same denoiser, same probes, same final gather.
12. **Single authoring model** — one quality slider, Low / Medium / High / Ultra, picks SDF resolution, probe density, ray counts, bounce count, and reflection quality. There is no "software vs hardware" switch exposed to authoring.
13. **Migration from P21 VCT and P36 HW RT GI** — both are deprecated this phase. `.rmat` materials, light probes, and scenes continue to load without asset migration; the GI solver is rebound at load.
14. **PIE compatibility** — Phase 7 Play-In-Editor Stop resets the radiance cache and probe state so replay determinism holds for recorded sessions.
15. **Editor preview and debug views** — viewport renders GI live during edit; dedicated debug view modes visualize probe positions, SDF iso-slices, radiance-cache occupancy, and trace-primitive selection per pixel.
16. **Measurable performance targets** — High tier reaches 60 fps at 1440p on RTX 3070 and RX 6700 XT; Medium tier reaches 60 fps at 1440p on RX 6600 and RTX 3050; Ultra tier reaches 60 fps at 1440p on RTX 3070 with HW BVH trace and spatiotemporal denoiser, on the Phase 21 reference scene.

## 1. The unified pipeline, in one picture

Every tier walks the same stages. Only the trace-primitive implementation and the knob values change.

```
                    ┌─────────────────────────────────────────────┐
                    │ 1. scene SDF assembly (SW) OR TLAS refit (HW)│
                    └────────────────────┬────────────────────────┘
                                         │
                    ┌────────────────────▼────────────────────────┐
                    │ 2. trace primitive                           │
                    │    SW: ray-march mesh SDFs → scene SDF       │
                    │    HW: ray-query BVH (wgpu ray-query)        │
                    └────────────────────┬────────────────────────┘
                                         │
           ┌─────────────────────────────┼─────────────────────────────┐
           │                             │                             │
  ┌────────▼────────┐          ┌─────────▼────────┐         ┌──────────▼─────────┐
  │ 3a. screen-space│          │ 3b. world radiance│         │ 3c. reflections &  │
  │     probes      │          │     cache (amort.)│         │     shadows        │
  └────────┬────────┘          └─────────┬────────┘         └──────────┬─────────┘
           │                             │                             │
           └─────────────────┬───────────┘                             │
                             │                                         │
                  ┌──────────▼──────────┐                              │
                  │ 4. final gather      │                              │
                  │  diffuse + spec-low  │                              │
                  └──────────┬──────────┘                              │
                             │                                         │
                  ┌──────────▼──────────┐                              │
                  │ 5. shared denoiser   │◀─────────────────────────────┘
                  │  (spatiotemporal)    │
                  └──────────┬──────────┘
                             │
                             ▼
                     composited indirect
```

Tier-to-stage mapping:

```
stage              Low (WebGL2)      Medium (desktop)        High (Vulkan/D3D12)      Ultra (RT-capable)
-----------------  ---------------   ---------------------   ----------------------   --------------------------
scene primitive    sky-only          static SDF only         static + dynamic SDF     BVH (TLAS refit)
trace              sky cube sample   SDF march, half-res     SDF march, full-res      BVH ray-query
screen probes      off               64/frame, 2 rays/probe  256/frame, 4 rays/probe  256/frame, 8 rays/probe
radiance cache     flat ambient      2k cells, 32/frame      16k cells, 128/frame     16k cells, 256/frame
bounces            0                 1                       2                        2 (plus infinite via cache)
reflections        probes only       SSR + probes            SSR + SDF trace + probes BVH trace + SSR fallback
shadows            cascades (VSM)    cascades (VSM)          RT-SW + VSM cascades     RT-HW + VSM optional
```

The rows are identical stages; only the per-cell values and the trace implementation change. That is the entire point of the phase.

## 2. Per-mesh SDF bake

An SDF is a sparse 3D texture storing the signed distance from each voxel to the nearest surface of the mesh. Negative inside, positive outside, zero at the surface.

```rust
// crates/rustforge-core/src/asset/sdf.rs
pub struct MeshSdf {
    /// Voxel grid resolution on the mesh's longest axis. Other axes scale
    /// to preserve cubic voxels.
    pub resolution: u32,
    /// World-space bounds the grid covers, in the mesh's local space.
    pub bounds: Aabb,
    /// Sparse block storage: 8^3 bricks, only bricks that straddle the
    /// surface (abs(distance) < brick_diagonal) are stored.
    pub bricks: Vec<SdfBrick>,
    /// BC4-compressed signed-distance payload (single-channel, 8-bit
    /// signed, scale in MeshSdfHeader).
    pub payload: Vec<u8>,
}
```

Resolution ladder (chosen by the mesh importer, not the author):

| Mesh class        | Resolution | Typical bake time | Typical disk size |
|-------------------|-----------:|------------------:|------------------:|
| Prop (< 2 m)      |        32³ |            < 50ms |             ~4 KB |
| Set piece (< 8 m) |        64³ |           ~250 ms |            ~28 KB |
| Building          |       128³ |             ~2 s  |           ~220 KB |
| Landscape chunk   |       256³ |             ~15 s |           ~1.6 MB |

Bake runs as an extra artefact kind in the Phase 5 asset pipeline, cached on disk next to the mesh. It is deterministic — identical input mesh + resolution produces identical SDF bytes — so CI caches hit. Reskinned meshes bake against the bind pose; skinning is handled at scene-assembly time (§3.1).

Opinion: we do not expose SDF resolution to the author. Importers pick it from mesh bounding-box diagonal. A "high-quality SDF" checkbox in the import dialog would be authoring debt with no upside — every scene would either leave it on (paying bake time and disk) or leave it off (shipping blocky GI). Automatic sizing lands a single sensible value.

## 3. Scene SDF assembly

Every frame we produce a structure the ray-march shader can step through efficiently: a top-level BVH-over-SDF-bricks, keyed by instance transform, where each brick points into a big SDF atlas texture holding the per-mesh SDF bricks for all resident meshes.

```rust
// crates/rustforge-core/src/render/gi/scene_sdf.rs
pub struct SceneSdf {
    /// Atlas of all resident mesh SDFs. Bricks packed; one lookup table
    /// from (mesh_id, brick_index) → atlas coord.
    pub atlas: wgpu::Texture,            // R8_SNORM 3D, 4096³ virtual
    pub atlas_lut: wgpu::Buffer,         // array<AtlasEntry>
    /// Instance table: per instance, transform + mesh_id + aabb.
    pub instances: wgpu::Buffer,         // array<SceneSdfInstance>
    /// Top-level BVH over instance AABBs; rebuilt on topology change,
    /// refit every frame.
    pub tlas_nodes: wgpu::Buffer,        // array<BvhNode>
    /// Cached: hash of the static instance set; mismatch triggers rebuild.
    static_set_hash: u64,
}
```

Assembly rules:

- **Static instances** hash once at scene load; their TLAS region is built once and skipped on the per-frame refit.
- **Dynamic instances** get a per-frame refit of their sub-TLAS region. Refit is a parallel compute dispatch; no CPU BVH construction in the hot path.
- **Skinned meshes** are handled the same way Phase 36 handles skinned BLAS: a compute pass writes deformed positions into a scratch vertex buffer; the *bind-pose SDF is unchanged*, and skinning artefacts at the SDF scale are beneath the GI error budget. We do not re-bake SDFs for skinned meshes per frame — that's the performance cliff we avoided.
- **Eviction**: meshes not present in any visible instance for 4 seconds drop from the atlas; their bricks get reclaimed. Eviction is a separate amortized pass, not in the render hot path.

The atlas is 4096³ virtual and ~1 GB residency in High tier at full density. Medium tier caps at 2048³ virtual and 256 MB.

## 4. Software SDF ray-march

The trace primitive in SW form is a sphere-traced march through the scene SDF. Each step reads the distance value at the current point and advances by that distance; termination happens on a distance-below-epsilon hit, a step-count cap, or a t-max exit.

```wgsl
// crates/rustforge-core/src/shaders/gi/sdf_trace.wgsl
@group(0) @binding(0) var scene_sdf_atlas : texture_3d<f32>;
@group(0) @binding(1) var scene_sdf_samp  : sampler;
@group(0) @binding(2) var<storage, read> tlas_nodes : array<BvhNode>;
@group(0) @binding(3) var<storage, read> instances  : array<SceneSdfInstance>;

fn sdf_trace(origin: vec3<f32>, dir: vec3<f32>, tmax: f32) -> TraceHit {
    var t : f32 = 0.01;
    var steps : u32 = 0u;
    loop {
        if (steps >= 96u || t >= tmax) { break; }
        let p = origin + dir * t;
        // Top-level BVH descent selects the nearest candidate instance;
        // on a miss, we fall through to sky sampling.
        let cand = bvh_descend(p, dir);
        if (cand.instance == INSTANCE_NONE) { break; }
        let local_p = transform_to_local(cand.instance, p);
        let d = sample_sdf(cand.mesh, local_p);
        if (d < 0.002) {
            return TraceHit(true, p, cand.instance, material_from_hit(cand, local_p));
        }
        t = t + max(d, 0.002);
        steps = steps + 1u;
    }
    return TraceHit(false, vec3<f32>(0.0), INSTANCE_NONE, MaterialSample());
}
```

Opinions baked into the shader:

- Step cap of 96 is a budget, not a correctness bound. Rays that exhaust it are treated as misses and fall through to the radiance cache or sky. The alternative — leaving them as black — reintroduces Phase 21's leak complaints in a new costume.
- `d < 0.002` is a world-space epsilon tuned to our unit convention (1 unit = 1 m). Mesh-local epsilons per-instance would be more accurate and are not worth the register pressure.
- We sphere-trace in world space, not per-instance space, to amortize the BVH descent cost across all distance reads along a ray. Transforming only at sample time is cheaper than transforming per step.

## 5. Screen-space probes

At the start of each frame, a compute pass spawns screen-space probes on a jittered 16×16 grid across visible pixels. Each probe fires `rays_per_probe` rays via the trace primitive (SW or HW) and accumulates incoming radiance into a small octahedral map.

```rust
// crates/rustforge-core/src/render/gi/ss_probes.rs
pub struct ScreenSpaceProbes {
    pub grid: UVec2,                // screen / 16, jittered each frame
    pub octahedral_side: u32,       // 8 on High, 4 on Medium
    pub rays_per_probe: u32,        // 4 / 8 depending on tier
    pub radiance: wgpu::Texture,    // RGBA16F, packed probes
    pub history: wgpu::Texture,     // previous frame for reprojection
}
```

Probes reproject from the previous frame using camera motion + depth; a probe with reprojection confidence above threshold blends new rays with its history, which is what makes 4–8 rays per probe sufficient rather than the hundreds a naive Monte Carlo final gather would demand.

## 6. World-space radiance cache

Screen-space probes alone cannot carry GI across view changes without flicker. A persistent world-space probe cache backs them up. Cells are anchored to the Phase 31 world grid; each cell holds an octahedral irradiance map plus a shorter-window specular cache.

```rust
// crates/rustforge-core/src/render/gi/radiance_cache.rs
pub struct RadianceCache {
    pub grid: WorldGrid,            // shares Phase 31 cell coords
    pub irradiance: wgpu::Texture,  // per-cell octahedral, RGBA16F
    pub specular:   wgpu::Texture,  // per-cell, shorter TTL
    pub age:        wgpu::Buffer,   // per-cell, frame-count since update
    pub budget_per_frame: u32,      // N cells updated this frame
}
```

Update policy:

- Cells near the camera update more often (LRU + distance-weighted round-robin).
- Each updated cell fires `rays_per_cell_update` rays through the trace primitive, writes the new radiance, decays its `age`.
- A cell that has never been updated starts from the sky cube, not black. This eliminates the first-frame black-bounce artefact that Phase 21's probe fallback suffered.

The cache is deterministic given a fixed scheduler seed, which is what keeps Phase 7 PIE replay reproducible (§13).

## 7. Final gather

The final gather is a per-pixel full-screen compute pass. For each shaded pixel:

1. Look up the nearest screen-space probes (4 bilinearly-weighted taps).
2. Look up the radiance-cache cell the pixel sits in (trilinear across 8 cells).
3. Weight screen-space probes heavily when their reprojection confidence is high; fall back to the radiance cache when not.
4. Produce `indirect_diffuse` and `indirect_specular_low_roughness`; higher-roughness specular comes from the reflection pass.

One pass. One output. One denoiser input. This is the pipeline boundary that the old two-GI-systems architecture failed to enforce; in the unified pipeline it is the invariant that holds everything else honest.

## 8. HW RT as an Ultra-tier swap-in

On Ultra-tier devices, `sdf_trace()` is replaced with a ray-query path against the Phase 36 TLAS. The call sites are identical — same inputs, same `TraceHit` return shape — so the downstream pipeline stages compile against the trace primitive without knowing which implementation it is.

```wgsl
// crates/rustforge-core/src/shaders/gi/bvh_trace.wgsl  (Ultra only)
@group(0) @binding(0) var scene_tlas : acceleration_structure;

fn sdf_trace(origin: vec3<f32>, dir: vec3<f32>, tmax: f32) -> TraceHit {
    var rq : ray_query;
    rayQueryInitialize(&rq, scene_tlas,
        RayDesc(0u, 0xFFu, 0.001, tmax, origin, dir));
    rayQueryProceed(&rq);
    let it = rayQueryGetCommittedIntersection(&rq);
    if (it.kind == RAY_QUERY_COMMITTED_TRIANGLE_HIT) {
        return TraceHit(true, hit_pos(it), it.instance_id, material_from_it(it));
    }
    return TraceHit(false, vec3<f32>(0.0), INSTANCE_NONE, MaterialSample());
}
```

The function name is intentional: the name reflects the *role*, not the technique. Every caller is oblivious to which file provided the body — the render graph binds either `sdf_trace.wgsl` or `bvh_trace.wgsl` at pipeline construction. Phase 36's standalone HW RT GI shader — the one that re-implemented screen-space probing with its own denoiser — is deleted.

## 9. Shadows unified

Phase 36 shipped RT shadows as a separate feature with its own TLAS trace and its own denoise pass. Phase 47 collapses that trace into the unified trace primitive and routes shadow occlusion queries through the same entry point. VSM remains available for directional-cascade coverage on scenes that exceed the RT shadow budget; it is now opt-in per light, not implicit per tier.

```rust
// crates/rustforge-core/src/render/gi/shadow.rs
pub enum ShadowMode {
    /// Default on all tiers. Uses the unified trace primitive for
    /// per-light occlusion queries.
    RayTraced { soft_radius: f32 },
    /// Per-light override. Useful on very-long directional cascades
    /// where RT cost is unjustified.
    VirtualShadowMap { cascades: u32 },
    /// Fallback for Low tier.
    CascadedShadowMap { cascades: u32 },
}
```

## 10. Authoring surface

One slider. Four values. Everything else derives from it.

```toml
# project.toml
[rendering]
gi_quality = "High"   # Low | Medium | High | Ultra
# Optional per-platform overrides:
[rendering.platform.windows]
gi_quality = "Ultra"
```

Rules:

- `Ultra` is requestable but materializes only on an Ultra-tier device; otherwise clamps to High with a one-line log. This is the only place tier detection surfaces to authoring.
- The slider does not say "software" or "hardware". Creators who ask why their Ultra-tier box looks better than their teammate's High-tier box learn that it uses HW RT under the hood; they do not learn that they had a switch for it. Switches create support tickets.
- Per-light overrides (shadow mode, probe density scalar) are an advanced-panel feature, documented but off the main authoring path.

## 11. Migration from P21 VCT and P36 HW RT GI

Neither deprecation breaks content.

- `.rmat` materials, light probes, and scenes load unchanged. The GI solver is rebound at load; material inputs that previously read from VCT cones now read from the unified trace primitive. No asset rewriter ships.
- Scenes that hand-placed VCT volumes (a Phase 21 authoring feature for voxel-grid-anchored GI hints) have those volumes downgraded to radiance-cache bias regions; the authoring UI keeps working and maps to the new system on save.
- Phase 36's HW RT GI passes are removed from the render graph. The TLAS it built is now owned by the unified pipeline; the RT reflection and RT shadow features it shipped continue working against the shared TLAS.
- Phase 21's VCT voxelization pass, voxel texture, and cone-trace shader are marked `#[deprecated]` for the remainder of the 3.x line, then deleted in 4.0. Projects pinned to 2.x continue to have VCT; the 3.0 migration guide documents the look change creators should expect (less leak, slightly different bounce falloff).
- Tier detection at boot picks the best available trace primitive once per run. Runtime switching is not supported (same rule as Phases 21 and 36; acceleration-structure residency is too invasive to reshape mid-frame).

## 12. Performance tiers and targets

Targets are measured on the Phase 21 reference scene ("Cornell-ish room plus one outdoor courtyard plus one foliage cluster") at 1440p, native render, TAA-U off except where noted.

| Tier   | Reference GPU       | Target | Notes                                    |
|--------|---------------------|-------:|------------------------------------------|
| Low    | Intel UHD / WebGL2  |   n/a  | GI disabled; sky + flat ambient only     |
| Medium | RX 6600, RTX 3050   | 60 fps | SDF half-res, 1 bounce, 64 probes/frame  |
| High   | RTX 3070, RX 6700XT | 60 fps | SDF full-res, 2 bounces, 256 probes/frame|
| Ultra  | RTX 3070 + RT       | 60 fps | HW BVH, spatiotemporal denoise, 2 bounces|

Regression CI runs the reference scene on a pinned RTX 3070 box (Phase 10 profiling rig) every nightly; a 5% frame-time regression on any tier fails the build. The 1440p-on-RX 6600 target is what forces Medium-tier decisions to stay honest — if it slips, we cut probe density before we slip the target.

## 13. PIE integration

Phase 7 Play-In-Editor Stop must reset GI state so recorded sessions replay deterministically:

- Radiance cache: clear to "never updated" so sky-seeded values regenerate from the replay's camera path.
- Screen-space probes: history cleared; first replay frame has no reprojection.
- Scene SDF atlas: preserved (it's content, not state); TLAS refit runs from the replayed transform stream.

Stop also cancels any in-flight amortized cache updates so the frame counter aligns with the replay's tick zero. Without this, a replayed scrub past frame 30 would observe a radiance cache warmed by the editor's idle time, not the replay's simulated time, and "lighting looks different on replay" bug reports would flood in.

## 14. Editor preview and debug views

The viewport renders GI live during edit. Three debug view modes land with this phase:

- **Probe positions** — screen-space probes as dots, colored by reprojection confidence; world-space cache cells as wireframe boxes, colored by age.
- **SDF iso-slice** — a movable plane shows the scene SDF as a grayscale distance field. Great for diagnosing a mesh whose SDF bake missed its bounds.
- **Trace primitive per pixel** — each pixel shaded by which primitive produced its last GI ray (green = screen-space probe hit, blue = radiance cache hit, red = final-gather fallback, white = sky). Makes tier downgrade paths visible at a glance.

Opinion: all three are debug view modes, not always-on overlays. We do not ship a "show GI stats" widget pinned to the viewport. The signal-to-noise of live GI stats is terrible and the creators who need the info are the same creators who will happily press a key.

## 15. Build order

1. SDF mesh importer — per-mesh bake, cache, disk format.
2. Scene SDF assembly — per-frame compute pass, atlas residency, dynamic refit.
3. Software SDF ray-march — the trace primitive, Medium and High tiers.
4. Screen-space probes — spawn, trace, accumulate, reproject.
5. World-space radiance cache — cells, amortized update, sky seeding.
6. Final gather — the per-pixel integration pass.
7. Shared denoiser — spatiotemporal reuse across probes and final-gather output.
8. HW RT swap-in — Ultra-tier trace primitive replacement, same interface.
9. Reflections unification — glossy/mirror reflections through the shared trace.
10. Shadows unification — RT shadows through the shared trace, VSM opt-in.
11. Deprecate P21 VCT pass — remove from render graph, leave source behind deprecation gate.
12. Deprecate P36 HW RT GI path — remove standalone passes, point callers at unified pipeline.
13. Migration docs — look-delta notes, per-light override guide, project.toml slider.
14. Editor debug views — probe positions, SDF iso-slice, trace-primitive view.
15. Performance CI gate — reference scene targets wired into nightly.

## Scope ❌

Out of scope for Phase 47:

- **Path-traced reference GI for final frames** — Phase 36's offline path tracer remains the authority for marketing shots, cinematics, and lightmap bakes. It is not folded into Phase 47's pipeline.
- **Precomputed lightmaps** — Phase 36 retains lightmap baking for static-only scenes that want the cost model of "zero GI work per frame." The runtime still consumes the light atlas for those projects.
- **ML-predicted probe placement** — a future phase may train a model to spawn world-space probes in likely-sampled cells ahead of time. Not this phase.
- **Surfel-based GI** — a different algorithmic family; evaluated and not pursued. The SDF + probe architecture is chosen deliberately.
- **Signed-distance-field-based collision** — SDFs produced here are used only for rendering. Physics collision (Phase 14) does not read them. Re-using the asset for collision queries is a nontrivial semantics change (SDFs bake against render meshes, not collision meshes) and is not pursued here.
- **Runtime tier switching** — consistent with Phases 21 and 36; the acceleration-structure residency churn is not affordable.
- **Per-object GI quality overrides** — the per-light shadow override is the only per-object knob. A per-object GI slider would invalidate the "one authoring story" premise.

## Risks

- **SDF atlas residency on Medium tier.** 256 MB is tight for scenes with many unique meshes. Mitigation: eviction is aggressive (4-second idle timeout), and the Phase 5 importer is instructed to prefer instancing by default so unique-mesh count stays moderate. If the budget is still exceeded, Medium tier silently disables dynamic-object SDF contribution (static-only GI) and logs a warning; this is a degradation, not a crash.
- **Leak-like artefacts on thin geometry.** SDFs store distance-to-surface; objects thinner than a voxel vanish from the SDF entirely. Mitigation: importer warns on thin-geometry detection and offers either resolution bump or exclusion from SDF (the mesh still renders; it simply does not cast GI). Phase 21's VCT had worse, not better, behavior on thin geometry.
- **HW RT and SW SDF divergence over time.** The two trace primitives must agree on material evaluation, else a creator's Ultra-tier preview diverges from the High-tier look. Mitigation: `material_from_hit()` is a *single* shared function consumed by both trace files; divergence is physically prevented at the shader level. A CI test renders a canary scene on both paths and compares perceptual difference; >2% delta fails the build.
- **Radiance cache cold-start on level load.** First frame after level load has an empty cache; relying entirely on sky seeding produces a visible warm-up. Mitigation: level loader fires a one-time radiance-cache prime pass that updates N_cells cells at load time (budget ~200 ms on Medium tier, accounted for in the Phase 12 loading-screen budget).
- **Deprecating VCT mid-3.x line.** Projects on 2.x may have hand-placed VCT volumes central to their look. Mitigation: 3.x keeps VCT behind a deprecation gate for the whole minor line; projects can opt back in with a per-project flag while they migrate. The 4.0 release is the hard cut.
- **Phase 36 HW RT GI users have a regression risk.** Their existing Ultra-tier projects now go through the unified pipeline instead of the Phase 36 standalone path. Mitigation: side-by-side screenshot CI on the Phase 36 reference scenes; any perceptual regression >3% triggers an investigation before Phase 47 ships.

## Exit criteria

- All Phase 21 reference-scene screenshots re-render under Phase 47 with no leaks present (VCT's documented leak cases pass on the new pipeline).
- Medium tier hits 60 fps at 1440p on RX 6600 and RTX 3050 on the Phase 21 reference scene.
- High tier hits 60 fps at 1440p on RTX 3070 and RX 6700 XT.
- Ultra tier hits 60 fps at 1440p on RTX 3070 with HW BVH trace and shared spatiotemporal denoiser enabled.
- The render graph contains exactly one GI-producing subgraph; the old VCT subgraph and the old Phase 36 RT GI subgraph are absent (deprecated VCT source file stays behind a compile gate, not in any graph).
- `project.toml` exposes exactly one GI authoring knob (`gi_quality`) plus documented per-light overrides; no "software vs hardware" switch exists anywhere in the editor UI.
- PIE replay of a 60-second recorded session reproduces its final frame to within the Phase 7 determinism tolerance on all tiers.
- Migration documentation lands: per-light override guide, expected look-delta notes from VCT→unified, deprecation timeline through 4.0.
- CI perf-regression gate on the reference scene is live and wired to block merge on >5% frame-time regression at any tier.
