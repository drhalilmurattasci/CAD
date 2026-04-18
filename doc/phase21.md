# Phase 21 — Advanced Rendering

The 1.0 renderer shipped a solid deferred PBR pipeline with clustered lights, cascaded shadows, SSR, SSAO, volumetric fog, and a hand-wired post chain. It was enough to ship with. It is not enough to compete. Phase 21 closes the Unreal rendering gap where wgpu makes it feasible — and explicitly refuses to chase the parts where it doesn't. There is no Nanite parity here, no hardware-RT GI, no path tracer. There is a meshlet-cluster pipeline that does 80% of what Nanite does on the geometry it was actually going to push to the GPU anyway; a voxel-cone-trace GI good enough to delete most lightmap baking; Virtual Shadow Maps so an open world stops streaking; an FSR-equivalent upscaler so the High tier is reachable on mid-range hardware; and a post stack rebuilt on the Phase 20 node graph so tonemap and bloom stop being hardcoded.

Every feature in this phase declares a minimum hardware **realism tier**. Low tier is WebGL2-safe so Phase 22's web target keeps working. Medium tier is what a 2018 desktop can run. High tier assumes Vulkan / D3D12 / Metal with compute and bindless. The tier is detected at engine init from wgpu's reported limits; features that miss their tier are silently replaced with the fallback path, not error-logged.

## Goals

By end of Phase 21:

1. **Tier system** — `RealismTier::{Low, Medium, High}` detected at startup from wgpu limits; every render feature declares `min_tier()` and falls back gracefully.
2. **Bindless texture arrays** on High tier (Vulkan / D3D12 / Metal argument buffers); descriptor-per-draw on Medium; atlas packing on Low.
3. **Temporal upscaler** (TAA-U variant — our own, FSR2-shaped) as an optional render-scale path; off by default.
4. **Virtual Shadow Maps** (tiled sparse 16k virtual atlas) — High tier only; Medium / Low keep cascaded shadow maps.
5. **Real-time GI** — voxel cone tracing (High) with screen-space probe fallback (Medium). No hardware RT. No lightmaps mandatory.
6. **Virtualized geometry** — meshlet cluster rendering with compute-shader LOD selection and software rasterization for tiny triangles (High only).
7. **Reflections** — probe-based specular + SSR composited, tuned per-tier. HW-RT reflections deferred.
8. **Volumetric light shafts** — optional add-on to the existing volumetric fog pass.
9. **Post-processing stack rebuilt on Phase 20 node graph** — tonemap, bloom, DoF, motion blur, color grading, vignette as swappable graph nodes instead of a hardcoded chain.
10. **Material-graph integration** — High-tier-only master inputs (subsurface, coat, anisotropy) light up when the project targets High; compile-error if shipped Medium / Low.
11. **GPU memory budgeting** — per-feature allocator with estimates, headroom warnings, and a single editor knob to step the whole project down a tier.

## 1. The tier system

Every other section depends on this one. Get it right first.

```rust
// crates/rustforge-core/src/render/tier.rs
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum RealismTier {
    Low = 0,    // WebGL2 / downlevel: no compute, no storage textures,
                // bind-group caps, u32 index only, no push constants
    Medium = 1, // wgpu "core" on desktop: compute, storage buffers,
                // decent bind-group count, multi-draw-indirect with emu
    High = 2,   // Vulkan / D3D12 / Metal: bindless, timestamp queries,
                // large workgroup storage, 64-bit atomics (optional)
}

impl RealismTier {
    pub fn detect(adapter: &wgpu::Adapter, surface: &wgpu::Surface) -> Self {
        let lim = adapter.limits();
        let feat = adapter.features();
        if feat.contains(wgpu::Features::TEXTURE_BINDING_ARRAY)
            && feat.contains(wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING)
            && lim.max_bind_groups >= 8
            && lim.max_storage_buffers_per_shader_stage >= 16
        {
            RealismTier::High
        } else if lim.max_compute_workgroup_storage_size > 0
            && lim.max_storage_buffers_per_shader_stage >= 8
        {
            RealismTier::Medium
        } else {
            RealismTier::Low
        }
    }
}

pub trait RenderFeature {
    fn min_tier(&self) -> RealismTier;
    fn enable(&mut self, ctx: &mut RenderCtx) -> Result<()>;
    fn fallback(&self) -> Option<Box<dyn RenderFeature>>;
}
```

A project's `project.toml` gets a `rendering.target_tier` key (default `Auto`). In `Auto`, `detect()` wins. With an explicit tier, the engine asserts the device meets it and refuses to run lower — useful for "we only ship to PS5-class hardware" scenarios.

Tier detection happens **once**, before any render feature initializes. Swapping tiers at runtime is not supported; the required resource layout changes would need a full pipeline rebuild.

## 2. Bindless textures (High tier)

The 1.0 renderer binds a small descriptor set per material. Bindless lifts that to a global array indexed by material ID, which is the prerequisite for virtualized geometry (§6) and compute-driven draw lists in general.

```wgsl
// crates/rustforge-core/src/shaders/bindless.wgsl
@group(0) @binding(0) var textures: binding_array<texture_2d<f32>, 4096>;
@group(0) @binding(1) var samplers: binding_array<sampler, 64>;
@group(0) @binding(2) var<storage, read> materials: array<MaterialParams>;

fn sample_material(mat_id: u32, uv: vec2<f32>) -> vec4<f32> {
    let m = materials[mat_id];
    return textureSampleLevel(textures[m.albedo_idx], samplers[m.sampler_idx], uv, 0.0);
}
```

### 2.1 Tier fallback

- **Medium** — per-draw material bind group; the call site looks identical from the frontend, the backend dispatches through a `MaterialBinding` trait with two impls.
- **Low** — atlas packing at import (Phase 5 asset pipeline extended). The material ID becomes a UV transform into one of a handful of atlases. Small projects only; a 4k atlas holds ~64 4×4 PBR materials at 512² per slot.

### 2.2 Residency

Bindless does not solve virtual texturing. A 4096-slot array is a bindless *ceiling*, not a magic streaming system. Textures still need residency management — that lands later (future phase), not here. Phase 21's bindless layer assumes the working set fits; budget warnings in §11 fire when it doesn't.

## 3. Temporal upscaler (TAA-U)

An FSR2/3-shaped upscaler of our own. Not "we wrapped FSR" — FSR's license and vendor coupling make that a bad fit. Instead: a temporal reconstruction pass that takes a 66% or 50% scaled render target plus history + motion vectors + depth and produces native-res output. Well-trodden territory; the shader is a few hundred lines.

```
        input 1280x720              history 1920x1080
              │                           │
              ▼                           ▼
  ┌───────────────────────────────────────────────┐
  │  reproject: motion_vec + depth → prev-pixel   │
  │  reject:   variance clip + depth test         │
  │  resolve:  Lanczos upscale + clamp blend      │
  └───────────────────────────────────────────────┘
              │
              ▼
        output 1920x1080  → becomes next frame's history
```

- Min tier: **Medium** (needs compute).
- Opt-in per project; default off. A rendered-at-native game should not pay the history cost.
- Motion vectors are already emitted for motion blur; reuse that pass.
- Disocclusion: when reprojection fails (large `|motion|` + depth mismatch), fall back to spatial upscale at that pixel — Lanczos-3 on the input frame.
- Integrates with post stack (§9): runs *before* tonemap, *after* the lighting resolve. The stack graph exposes the upscaler as a node so projects can move it.

WGSL snippet for the reproject step:

```wgsl
let prev_uv = uv - motion_vec;
let prev_depth = textureSampleLevel(history_depth, s, prev_uv, 0.0).r;
let reject = abs(prev_depth - current_depth) > DEPTH_REJECT_THRESHOLD
          || any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0));
let prev_color = textureSampleLevel(history_color, s, prev_uv, 0.0);
let clipped = clip_to_aabb(prev_color.rgb, neighborhood_min, neighborhood_max);
let w = select(HISTORY_WEIGHT, 0.0, reject);
out = mix(current_color, clipped, w);
```

## 4. Virtual Shadow Maps

Cascaded shadow maps work until the scene gets big, then they streak, shimmer, and waste resolution on distant junk. VSM is a **tiled sparse virtual atlas**: a logical 16k × 16k shadow buffer, physically backed by a pool of 128² pages. Each frame a compute pass marks which pages the visible scene needs, allocates them out of the pool, and renders only those.

```
  logical atlas (16384 x 16384)       physical pool (2048 x 2048)
  ┌────────────────────┐              ┌──────────────┐
  │ ░░▓▓░░░░░░░░░░░░░░ │   mapping    │ ▓▓▓▓░░░░░░░ │
  │ ▓▓▓▓▓▓░░░░░░░░░░░░ │  ────────▶   │ ▓▓▓▓░░░░░░░ │
  │ ▓▓▓▓▓▓▓▓░░░░░░░░░░ │              │ ░░░░░░░░░░░ │
  └────────────────────┘              └──────────────┘
     virtual tiles                      resident pages
```

- Min tier: **High**. Medium and Low keep the Phase 1.0 4-cascade CSM path.
- Page-marking pass runs in compute after the depth prepass; visible-cluster bounds produce a list of required pages.
- Page allocator is a simple LRU over the physical pool. Eviction of a still-visible page in the same frame is a bug — budget the pool for worst-case visible page count + 10%.
- Rendering: one `multi_draw_indirect` per page set, tile-scissored. Where multi-draw-indirect is unavailable, a CPU-side submit loop.
- Denoise: none. Hard shadows only in this phase. PCF / PCSS are sampler-side tweaks that compose on top.

Fallback logic when a required page can't be allocated (pool exhausted): use the coarsest resident page and log a single budget warning per frame. No panic.

## 5. Real-time global illumination

The choice is voxel cone tracing versus a screen-space / world-space probe system. Both are viable. Hardware RT GI is not — wgpu's RT story is still evolving and shipping our highest-impact GI on an unstable backend is a mistake.

- **High tier** — voxel cone tracing. Voxelize the scene into a 256³ clipmap (four cascades, 64³ each). One cone per diffuse sample, five cones for specular. Re-voxelize dynamic objects per frame; static geometry once at load.
- **Medium tier** — screen-space probe placement. World-space probes populated from screen-space reflections + ambient; gaps filled with a low-res irradiance cache. Cheaper, blurrier, no occlusion leaks through thin walls.
- **Low tier** — baked lightmaps only, as Phase 1.0 already ships. The editor keeps the bake button.

```
   voxelize (geom shader emu)  ─▶  clipmap<0..3>  ─▶  cone-trace (compute)
          │                                                │
          └── dynamic re-voxel each frame (compute)        └── resolve into GBuffer
```

Opinion: VCT leaks a little, misses fine contact, and isn't a lightmap killer — but it's dramatically better than ambient-only and dramatically cheaper than path tracing. For a 1.x renderer it's the right call. The compute-based voxelizer supersedes the geometry-shader trick the original VCT papers used; wgpu has no geometry shaders anyway.

## 6. Virtualized geometry (Nanite-lite)

Not Nanite. Nanite is years of work on cluster compression, software rasterization, hierarchical culling, and streaming. Phase 21 ships the subset wgpu supports and stops before the pit.

**What's in:**

- Meshes are built into **meshlets** at import — 64-vertex / 124-triangle clusters, with per-cluster bounding sphere + cone.
- Clusters are organized in a simple **DAG LOD** (Nanite's idea, our much simpler execution): each mesh has 4 LOD levels, clusters at each level overlap their neighbors. No streaming, no cluster tile pages.
- **Compute culling pass** — frustum + occlusion (HiZ from previous frame) + cone backface + LOD selection. Output: a compacted index buffer of visible clusters.
- **Indirect draw** — one `draw_indexed_indirect` per material bucket (materials from bindless §2).
- **Software raster for tiny triangles** is out of scope. Sub-pixel triangles stay as hardware draws; this means very high poly scenes still hit the hardware fast path, which in wgpu is fine up to ~10M visible tris on mid hardware.

```
meshlet buffer ─▶ cull (compute) ─▶ visible_indices  ─▶ indirect draw (per material)
       ▲              │                                        │
       │              └─▶ HiZ from prev frame                  │
   streaming (OUT OF SCOPE)                                    ▼
                                                         GBuffer write
```

- Min tier: **High**. Medium / Low use the 1.0 mesh-level LOD path (vertex buffer per LOD, CPU-selected).
- No Nanite-style on-disk cluster pages. Build-time cluster DAG is kept in RAM.
- Interop with Phase 20 material graph: the bindless material array (§2) is the same one clusters index into; graph changes reroute through material ID, not through pipeline rebuilds.

## 7. Reflections

- **Baseline (all tiers)** — box / sphere reflection probes authored in the editor, blended in the deferred lighting pass. Same as 1.0.
- **SSR (Medium, High)** — screen-space reflections, hierarchical depth marching in compute on High, raster-fallback on Medium.
- **Composite** — SSR preferred where it has a hit; probes fill the misses. Glossy surfaces fall back to probes entirely (SSR on rough surfaces is temporal noise with extra steps).
- **HW-RT reflections** — **out of scope**, reconsider when wgpu RT stabilizes.

The reflection probe authoring UI already exists in Phase 8 terms as a specialized editor inheriting the shared preview pattern; this phase only extends it with per-tier quality.

## 8. Volumetric light shafts

An optional add-on to Phase 1.0's volumetric fog. Same froxel grid, same transmittance integration; new input is a 1/4-res shadow sample of the primary directional light scattered through the froxel. God-rays through windows and tree canopies without a separate pass.

- Min tier: **Medium**.
- Off by default. Projects enable in `rendering.volumetrics.light_shafts = true`.
- No extra memory cost; reuses the fog froxel volume.

## 9. Post-processing stack on the Phase 20 node graph

Phase 20 ships a material node graph. That same graph infrastructure — node registry, typed sockets, WGSL code generation, topological evaluation — is exactly the right shape for the post stack.

The 1.0 post stack is a hardcoded chain: resolve → bloom → DoF → motion-blur → tonemap → vignette → grade. You cannot reorder it. You cannot branch it. You cannot insert a custom node. Phase 21 rebuilds it as a graph.

```
  ┌──────────┐   ┌──────────┐   ┌──────────┐   ┌───────────┐
  │ Lit HDR  │─▶│  Bloom   │─▶│  DoF     │─▶│ Motion Blur│─┐
  └──────────┘   └──────────┘   └──────────┘   └───────────┘ │
                                                             ▼
                                         ┌───────────┐   ┌────────┐
                                         │ Tonemap   │─▶│ Grade  │─▶ Output
                                         └───────────┘   └────────┘
                                                             ▲
                                          (optional) ────────┘
                                          Vignette, LUT, film grain
```

- Nodes are the same `Node` trait Phase 20 defined for materials, plus a post-specific `PostNode` marker that restricts IO to full-screen RGBA16F or R16F targets.
- The graph compiles to a sequence of compute or fragment passes; adjacent nodes with compatible formats fuse into one dispatch.
- Per-project stack: the project's `rendering.post_graph` is a serialized graph. Sensible default graph ships matching the 1.0 chain exactly, so existing projects look identical on upgrade.
- Tonemap is a node, not a global option. ACES, Reinhard, AgX, neutral, custom LUT all ship as stock nodes.
- Upscaler from §3 is a node with a `reconstruct` output. Place it before tonemap by default; let projects move it.
- Nodes declare a `min_tier`; if the device can't run the graph as authored, nodes are skipped (with a warning) and their input forwards to their output. A graph that is only "tonemap → grade" is always safe.

This is the single biggest user-facing upgrade in Phase 21. Everything else is invisible quality; the post graph lets users *see* what changed.

## 10. Material graph integration

Phase 20 defines master material inputs. Phase 21 adds the inputs that only make sense at High tier:

- **Subsurface scattering** (screen-space, separable Gaussian) — High only.
- **Clear coat** (second specular lobe) — High only.
- **Anisotropy** (tangent-aligned specular) — High, Medium degrades to isotropic.
- **Sheen** (fabric) — Medium, High.

The graph editor shows these inputs always, but the project's target tier gates them:

- Project target `Auto` or `High` — inputs compile.
- Project target `Medium` / `Low` with a High-only input connected — the graph compiler errors at save time with a clear message ("Subsurface requires High tier; target tier is Medium"). No silent downgrade — silently dropping a feature the user wired up is worse than a build error.

## 11. GPU memory budgeting

Phase 10 introduced CPU memory budgets. Phase 21 extends that to GPU:

```rust
pub struct GpuBudget {
    pub textures:       u64,  // bindless array, probes, atlases
    pub meshlets:       u64,  // cluster + index buffers
    pub shadow_pool:    u64,  // VSM physical pool
    pub gi_voxels:      u64,  // VCT clipmap
    pub history:        u64,  // TAA-U history
    pub post:           u64,  // post graph intermediates
    pub headroom:       u64,  // remaining vs. adapter limit
}
```

Every feature reports its live allocation. The editor's renderer settings panel shows the breakdown and a single bar-per-feature. Warnings fire at 80% of the adapter's reported `max_buffer_size` × working-set factor.

A "step project down one tier" button in the same panel rewrites `rendering.target_tier` and shows the new budget side-by-side with current. Decisions become visible.

## 12. Build order within Phase 21

Each step is independently shippable; each depends on its predecessors but not on the ones that follow.

1. **Tier detection + `RenderFeature` trait (§1)** — no visual change, but every later step slots into this scaffolding. Lands first, gets reviewed carefully, then everything else hangs off it.
2. **Bindless on High tier (§2)** — unlocks §6 and the post graph's bindless sampler lookups. Medium fallback lands at the same time so nothing regresses on desktop without bindless.
3. **Temporal upscaler (§3)** — small, self-contained, user-visible. A good validator that the graph-node integration (§9) will work, since TAA-U is the first pass that reads its own history.
4. **Virtual Shadow Maps (§4)** — High tier only; CSM stays as the Medium / Low path. Lands before GI so VCT's shadow sampling benefits.
5. **Real-time GI (§5)** — the biggest quality jump of the phase. Voxelizer first, cone tracer second, editor knobs third.
6. **Virtualized geometry (§6)** — depends on bindless (§2) and benefits from VSM (§4) and GI (§5) already being in place so the bring-up scene looks correct.
7. **Post stack on Phase 20 graph (§9)** — requires Phase 20 to be merged. Migrates the existing hardcoded chain to a default graph; default graph is byte-for-byte identical output to 1.0's chain.
8. **Material-graph high-tier inputs (§10)** — subsurface, coat, anisotropy, sheen. Depends on Phase 20 and on §5 (GI) because the lighting response changes.
9. **Volumetric light shafts (§8)** — tiny, late, nice-to-have.
10. **GPU budget panel (§11)** — lands last because it needs every feature's allocation reporting to be real.

## 13. Scope — what's NOT in Phase 21

- ❌ **Hardware ray-traced GI.** Revisit when wgpu RT ships stable.
- ❌ **Path tracer / offline reference renderer.**
- ❌ **Hair / fur rendering.** Strand-based hair is its own phase.
- ❌ **Cloth simulation rendering / cloth shading.**
- ❌ **Sky / atmosphere authoring.** Phase 27. The existing skybox stays.
- ❌ **Terrain-specific renderer upgrades.** Clipmap terrain keeps its Phase 1.0 renderer; Nanite-lite does not eat terrain.
- ❌ **Virtual texturing / megatextures.** Bindless is not virtual texturing.
- ❌ **Cluster streaming.** Meshlets stay in RAM.
- ❌ **Software rasterization for sub-pixel triangles.** The Nanite feature that matters most for film-quality geometry; not worth the implementation weight at 1.x scale.
- ❌ **Denoised soft shadows.** VSM is hard-shadowed in this phase; PCF / PCSS compose on top but ship later.
- ❌ **DLSS or FSR direct integration.** Vendor SDKs, vendor headaches, wgpu layer mismatch. Our TAA-U is the answer.
- ❌ **HDR display output / scRGB swapchain.** Nice to have, different phase.
- ❌ **Multi-GPU / mGPU.** No.

## 14. Risks

- **Bindless portability.** wgpu's bindless story on Metal argument buffers and D3D12 descriptor heaps has sharp edges — non-uniform indexing limits differ, SPIR-V translation differs. Every High-tier build must be tested on all three backends during development, not just "Vulkan on NVIDIA." Budget a week for backend-specific bug hunts.
- **VSM page-pool sizing.** Too small and popping; too large and you've spent more memory than the CSM path. Pick 64 MB of pool on High as the default and expose the knob. A replay-based benchmark suite from a few representative scenes should drive the default, not one dev's test map.
- **VCT memory.** A 256³ × RGBA8 clipmap is 64 MB per cascade, × 4 cascades = 256 MB. Compress to R11G11B10 to halve. Mind the compute cost of re-voxelizing dynamic objects — budget one ms/frame on the target High-tier GPU and gate the dynamic list when it exceeds.
- **VCT leaks.** Thin walls, flat objects. Mitigate with anisotropic voxel encoding (six directions → one per face) at 2× memory. Don't paper over with "art direction" — it's a real artifact, call it out in release notes.
- **Meshlet LOD popping.** DAG LOD without smooth cluster transitions pops at cluster boundaries. A per-cluster alpha dither across one frame on transition is the cheap fix; exposure in the graph node "LOD transition" lets projects pick dither or hard.
- **TAA-U ghosting.** Disocclusion rejection is the whole game. Over-aggressive → flicker; under-aggressive → smear. Ship with conservative defaults, expose variance clip sigma in the graph node, document "if you see ghosting on fast characters, raise this."
- **Post-graph regressions.** Rebuilding the stack on Phase 20 infrastructure will break somebody's project. The default graph must be bit-identical to the hardcoded 1.0 chain — compare frames in an automated test on every commit to the post module.
- **Tier-detection false negatives.** A driver bug hides a feature and the whole project falls to Medium. Provide an override in editor prefs ("force tier: High") and a log line on detection that says exactly which limit failed.
- **Phase 22 coupling.** Mobile tiers map to Low / Medium here. Phase 22 must not need renderer changes beyond what this phase exposes. Keep the tier API the integration surface; no mobile-specific rendering shims.
- **Phase 20 coupling.** The post graph depends on Phase 20's graph runtime. If Phase 20 slips, §9 either slips with it or ships as a hardcoded-but-reorderable chain. Decide the fallback before merging §9's PR.
- **Budget UI underselling the cost.** The GPU budget panel should show the cost *before* the user commits a tier change, not after. Dry-run the new tier against the current scene; display "if you switch, these buffers shrink / grow" with numbers.
- **High-tier feature creep in samples.** Marketing will want the sample scenes on High everywhere. Resist — keep a full set of Medium-tier sample scenes so Medium is never a "demo afterthought" and the fallback paths get real exercise.

## 15. Exit criteria

Phase 21 is done when all of these are true:

- [ ] `RealismTier::detect()` returns deterministic tiers on Vulkan, D3D12, Metal, GL (WebGL2); unit tests assert the tier for mocked limit sets.
- [ ] Every render feature implements `RenderFeature` with a declared `min_tier` and a passing fallback path.
- [ ] Bindless renders the bring-up scene on High with one material bind group total; Medium renders the same scene per-draw-bound; both match within a per-pixel SSIM threshold on a reference frame.
- [ ] TAA-U at 66% render scale on a 4k target measures ≥ 40% frame-time reduction vs. native 4k on the reference High-tier GPU, with no flicker on the canonical "spinning camera / moving character" test clip.
- [ ] Virtual Shadow Maps render the open-world sample scene with no visible cascade streaks and ≤ 8 ms shadow pass on the reference High GPU at 1440p. Pool exhaustion warning fires correctly on a forced-undersize pool.
- [ ] VCT GI renders the Cornell-analog test scene with indirect bounce visible and agreeing with an offline reference to within a per-channel ΔE target. Dynamic voxelization of a single moving character costs ≤ 1.2 ms / frame.
- [ ] Screen-space probe GI (Medium) renders the same scene with coarser bounce and no crashes / NaNs.
- [ ] Meshlet cluster pipeline renders a 12M-triangle source mesh at stable 60 fps on the reference High GPU with no visible LOD pops during a canonical fly-through (dither transition enabled).
- [ ] Reflection composite shows SSR hits overlaying probe fills with a smooth boundary; glossy surfaces use probes only; no artifacts on moving objects.
- [ ] Volumetric light shafts render through a tree-canopy test scene and cost ≤ 0.6 ms / frame on the reference Medium GPU.
- [ ] Post-processing graph loads, evaluates, and renders bit-identical output to the 1.0 hardcoded chain using the default graph. A graph editor roundtrip (open, save, close, reopen) preserves node state.
- [ ] Tonemap, bloom, DoF, motion blur, color grading, vignette ship as stock post graph nodes and each individually round-trips serialized.
- [ ] High-tier master material inputs (subsurface, coat, anisotropy, sheen) render correctly when the project targets High; connecting a High-only input on a Medium project emits a clear compile error at save time.
- [ ] GPU budget panel shows live allocation per feature and fires a warning at 80% of `max_buffer_size`. Dry-run tier switch shows before/after deltas before apply.
- [ ] Every feature in this phase is exercised in an automated visual-regression test (reference frame + ΔE budget).
- [ ] Rendering features respect the `editor`-feature gate exactly as Phase 2 required — a game build without the editor feature still renders identically.
- [ ] Phase 22 mobile/web build runs green on Low / Medium tiers with no Phase-21-specific changes needed in Phase 22.
- [ ] Phase 20 material graph compiles High-tier inputs through the same pipeline used for the post graph — no duplicated node-runtime code.
