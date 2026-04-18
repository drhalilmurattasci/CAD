# Phase 36 — Hardware Ray Tracing, Path Tracer & Neural Rendering

Phase 21 closed the Unreal rendering gap everywhere wgpu made it feasible. It explicitly refused to chase hardware ray tracing because wgpu's ray-query surface was still moving under our feet, DLSS/FSR were vendor-coupled and version-fragile, and a path tracer was out of scope for a 1.x renderer. Every one of those "no" bets was correct in the 1.0 timeline. By the 2.0+ timeline they are wrong. wgpu's ray-query extension is now stable on Vulkan, D3D12, and Metal; the vendor upscaler SDKs all expose a thin-enough C ABI to wrap behind an adapter; and a CPU-side offline path tracer fits cleanly next to the lightmap baker. Phase 36 picks up exactly where Phase 21 stopped.

Nothing here replaces Phase 21. VCT, SSR, VSM, TAA-U, meshlet clusters all keep working. Phase 36 layers hardware-accelerated paths on top and lets each one fall back to its Phase 21 cousin when the device, driver, or tier says no. The tier ladder gains a fourth rung — **Ultra** — for GPUs that meet the ray-query and cooperative-matrix feature sets. RT on Ultra, VCT on High, probe GI on Medium, lightmaps on Low. Same renderer; four disjoint quality paths per feature.

## Goals

By end of Phase 36:

1. **Ray-tracing backend** — wgpu ray-query adapter detected at startup; `RealismTier::Ultra` gated on the feature. No RT on Web or Mobile tiers.
2. **BVH management** — per-mesh BLAS built at import / bake; scene TLAS refit every frame for dynamic objects, full rebuild on topology change.
3. **Hardware RT global illumination** — multi-bounce diffuse + specular RT GI replacing VCT as the default Ultra-tier GI; spatiotemporal + ML denoiser.
4. **Hardware RT reflections** — full ray-traced reflections replacing the SSR + probe composite; falls back to SSR on tier downgrade.
5. **Hardware RT shadows** — ray-traced soft shadows with penumbra; directional RT, local lights stay VSM where cheaper (mixed mode).
6. **Offline CPU path tracer** — fully path-traced output for marketing shots, cinematics, and lightmap baking. "Render Path-Traced" menu; PNG / EXR sequence output.
7. **Path-traced lightmap baking** — per-scene offline bake producing the light atlas the runtime shaders already consume.
8. **Neural upscaler** — DLSS 3, FSR 3, XeSS behind one adapter trait; TAA-U from Phase 21 is the guaranteed fallback.
9. **Neural denoiser** — Intel OIDN (CPU) and/or NVIDIA OptiX denoiser (GPU) behind a trait; optional; the spatiotemporal denoiser remains the always-available path.
10. **RT preview in editor** — material editor preview (Phase 20) and cinematic sequencer preview (Phase 19) can opt into RT when available. User toggle, not automatic.
11. **Profiler integration** — Phase 10 GPU profiler surfaces RT timings per pass (TLAS refit, RT GI, RT reflections, RT shadows, denoise); overlay shows RT usage per pixel.
12. **Hybrid fallback matrix** — every RT feature is independently tier-gated. RT GI off but RT reflections on is a valid configuration, and vice versa.
13. **Performance target** — RTX 3060 at 1440p with RT GI + RT reflections + neural upscaler at 66% render scale: 60 fps on the reference scene. Documented and regression-tested.

## 1. The Ultra tier and RT feature detection

Phase 21's `RealismTier` enum grows one variant. Phase 36 does **not** reuse High for RT; High stays exactly what it was. Ultra is additive.

```rust
// crates/rustforge-core/src/render/tier.rs  (extended)
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum RealismTier {
    Low = 0,
    Medium = 1,
    High = 2,
    Ultra = 3,  // NEW: ray-query + cooperative-matrix class GPUs
}

impl RealismTier {
    pub fn detect(adapter: &wgpu::Adapter, surface: &wgpu::Surface) -> Self {
        let high = /* unchanged Phase 21 detection */;
        if high == RealismTier::High
            && adapter.features().contains(wgpu::Features::RAY_QUERY)
            && adapter.features().contains(wgpu::Features::RAY_TRACING_ACCELERATION_STRUCTURE)
            && adapter.limits().max_bind_groups >= 8
        {
            return RealismTier::Ultra;
        }
        high
    }
}
```

Rules:

- Tier is still detected **once** at startup. No runtime RT toggling — the acceleration-structure residency layout changes are too invasive.
- An explicit `project.target_tier = "Ultra"` on a non-RT device refuses to run. An `Auto` project with an RT-capable device picks Ultra unless the project opts out per-feature.
- Ultra *implies* High. Every High-tier feature must still work on Ultra; RT is strictly additive quality.
- Web and Mobile targets (Phase 22) never report Ultra. The detection short-circuits on those platforms before the RT feature query.

## 2. BVH management

We do not build BVHs. The driver does. Our job is managing two levels of it and keeping the TLAS coherent every frame.

```
  per mesh (import / bake time)        per frame (render time)
  ┌──────────────────────────┐         ┌──────────────────────────┐
  │ vertex + index buffer    │         │ list of visible instances│
  │         │                │         │         │                │
  │         ▼                │         │         ▼                │
  │   build_acceleration_    │         │  update_acceleration_    │
  │   structure (BLAS)       │         │  structure (TLAS refit)  │
  │         │                │         │         │                │
  │         ▼                │         │         ▼                │
  │   cached on disk next    │         │   bound into RT passes   │
  │   to the mesh asset      │         │                          │
  └──────────────────────────┘         └──────────────────────────┘
```

- **BLAS** is built once per mesh at asset import / bake time. The opaque handle is cached next to the mesh asset (Phase 5 pipeline gets a new serialized artifact kind). Rebuild on mesh change; otherwise load from cache.
- **TLAS** is rebuilt from scratch when the visible set's topology changes (instance added / removed / material changed), **refit** when instances only move. Refit is an order of magnitude cheaper; prefer it.
- Skinned meshes get a per-frame compute pass that writes deformed positions into a scratch vertex buffer; the BLAS for that mesh is then refit against the scratch buffer. Don't rebuild skinned BLAS from topology every frame — that's the whole RT performance cliff.
- Instance data in the TLAS mirrors the bindless material index (Phase 21 §2) so the RT shader can fetch materials through the same `textures[]` and `materials[]` arrays the raster path uses. One source of truth.
- Budget: one TLAS at a time, sized to the scene's visible instance count × 1.5 headroom. On overflow, evict non-visible-for-N-frames instances.

### 2.1 Deformables and particles

Particles and procedural geometry do **not** enter the TLAS in Phase 36. They render as a post-RT raster pass and composite. The alternative — per-frame BLAS rebuilds for thousands of particle quads — destroys the RT budget. Flag this clearly in docs.

## 3. Ray-traced shadows (first)

RT shadows are the easiest RT feature to bring up and the best validator that the backend works end-to-end. Ship this first.

```wgsl
// crates/rustforge-core/src/shaders/rt_shadow.wgsl
@group(0) @binding(0) var scene_tlas: acceleration_structure;

fn trace_shadow(origin: vec3<f32>, dir: vec3<f32>, tmax: f32) -> f32 {
    var rq: ray_query;
    let ray = RayDesc(0u, 0xFFu, 0.001, tmax, origin, dir);
    rayQueryInitialize(&rq, scene_tlas, ray);
    rayQueryProceed(&rq);
    let hit = rayQueryGetCommittedIntersection(&rq);
    return select(1.0, 0.0, hit.kind != RAY_QUERY_INTERSECTION_NONE);
}
```

- Trace one shadow ray per pixel per directional light.
- Soft-shadow penumbra via **cone sampling** — the ray direction is jittered within the light's angular radius, denoised spatiotemporally. Two rays per pixel gives good enough penumbra at 1440p; one ray + denoise for lower budgets.
- Local lights (point, spot) that are cheap to shadow with VSM (Phase 21 §4) stay on VSM. The mixed-mode decision is per-light: `light.shadow_method ∈ {RT, VSM, None}`, defaulting to RT for directional, VSM for local. An editor knob overrides per-light.
- Fallback: tier drops below Ultra → every `RT` shadow method silently maps to VSM (High) or CSM (Medium / Low), no scene edit required.

Budget: ≤ 1.5 ms / frame on an RTX 3060 at 1440p for one directional + four local RT lights.

## 4. Ray-traced reflections (second)

The Phase 21 SSR-plus-probes composite is good but fundamentally can't reflect off-screen geometry, and glossy-but-not-mirror materials fall back to blurry probes. RT reflections fix both.

```
                         ┌───────────┐
  G-buffer normal,   ──▶ │ per-pixel │ ─▶ one reflection ray
  roughness, depth       │ ray setup │     │
                         └───────────┘     ▼
                                        TLAS trace
                                           │
                                           ▼
                                    material fetch at hit
                                           │
                                           ▼
                                    spatiotemporal denoise
                                           │
                                           ▼
                                    composite into lit
```

- One reflection ray per pixel. Roughness modulates the ray cone; mirror surfaces get a perfect reflection ray, rough surfaces a cone-sampled ray into a distribution.
- Shade the hit with the same deferred lighting shader the primary pass uses — the hit produces a mini G-buffer sample that feeds the existing lighting resolve. No duplicated shading code.
- Denoise is mandatory. Half-res traces + full-res denoise composite; documented knob for projects that want to burn GPU on full-res traces.
- Rough-surface cutoff: at roughness > 0.8, skip the RT trace and sample the probe (same fallback as SSR). RT on diffuse surfaces is noise at any sample count we can afford.
- Fallback: no Ultra → fall through to Phase 21's SSR + probe composite. Projects notice a roughness-dependent fidelity drop only.

Budget: ≤ 2.5 ms / frame on RTX 3060 at 1440p half-res.

## 5. Ray-traced global illumination (third)

Replaces VCT as the default Ultra GI. Multi-bounce diffuse + specular.

```
  G-buffer ─▶ sample direction (GGX / cos)  ─▶  trace bounce 1
                                                    │
                                       (maybe) ─▶  trace bounce 2
                                                    │
                                                    ▼
                                             ML + spatial denoise
                                                    │
                                                    ▼
                                             inject into lit resolve
```

- **Bounce count**: two by default on Ultra; one on Ultra-with-tight-budget. Three is a path tracer (§6) — not real-time.
- **Sampling**: one diffuse ray per pixel + one specular ray per pixel; reuse reflection pass rays for specular where possible (share the trace, split the shading).
- **Temporal reservoir (ReSTIR-GI)** for noise reduction at a given sample count. The maths fit in a compute pass; no third-party library needed.
- **Denoiser pipeline**: spatial A-Trous denoise → temporal accumulation → optional neural denoiser (§8) at the end. The neural denoiser is a quality upgrade, not a correctness requirement.
- VCT stays compiled for High. Projects that ship to both High and Ultra get both GIs wired up; the frontend talks to a `GlobalIllumination` trait that each implements.
- **Dynamic re-voxelization cost from Phase 21 disappears on Ultra** — no voxel cascades to maintain. That 1+ ms / frame comes back to the budget.

Opinion: RT GI leaks less than VCT, nails contact, and scales with geometric complexity rather than voxel resolution. It also costs more at low poly counts. For scenes that VCT already handled well, the win is fidelity (no leaks through thin walls). For scenes VCT choked on, the win is correctness.

Budget: ≤ 4.0 ms / frame on RTX 3060 at 1440p with two bounces and temporal reservoir.

## 6. Offline CPU path tracer

Built for marketing stills, cinematic renders, and the lightmap baker. Not a real-time feature. CPU-only on purpose.

```rust
// crates/rustforge-pathtrace/src/lib.rs
pub struct PathTracer {
    pub scene: BvhScene,           // rebuilt from same meshes + mats
    pub samples_per_pixel: u32,    // 256 default, 4096 for marketing
    pub max_bounces: u8,           // 8 default
    pub integrator: Integrator,    // {PT, DirectOnly, AmbientOcclusion}
    pub output: PathTraceOutput,   // PNG8, PNG16, EXR32, sequence
}

pub fn render(&self, camera: &Camera, resolution: (u32, u32)) -> Image {
    // tiled, rayon-parallel over tiles, one thread per core
}
```

- **Why CPU**: a wavefront GPU path tracer is a whole phase by itself. The CPU path tracer reuses the same BLAS data (mirrored to a CPU-side BVH, not the driver's opaque one) and the same material graph evaluated via a WGSL → Rust cross-compile path already in Phase 20's toolchain. One month of work vs. one year.
- **Integrator**: pure path tracing with next-event estimation. No MIS, no bidirectional, no Metropolis — those are research projects and out of scope.
- **Editor hook**: `File → Render Path-Traced…` opens a dialog for resolution, SPP, output format, camera (current viewport or a named cine camera from Phase 19). Rendering runs in a worker thread; progress bar in the editor; cancelable.
- **Output**: single image or sequence driven by a Phase 19 cine timeline. EXR preserves HDR for post grading.
- **Determinism**: seed per tile. Two renders of the same scene at the same SPP produce pixel-identical output.

Non-goals (reiterate): no GPU path tracer, no MIS / BDPT / MLT, no spectral rendering, no subsurface random-walk (the rasterizer's SSS is good enough; path-traced SSS can come later).

## 7. Path-traced lightmap baking

The path tracer from §6 is repointed at static geometry UV atlases to produce runtime lightmaps. Same kernel, different camera, different output.

- **Input**: tagged-as-static scene geometry, each object carrying a lightmap UV channel (Phase 5 asset pipeline already supports this for the Phase 21 Low-tier lightmap path).
- **Output**: a light atlas texture per scene stored next to the scene file; loaded by the runtime shader's existing lightmap sampler.
- **SPP default**: 512 for a production bake, 64 for an iteration bake. Editor shows both buttons.
- **Denoise**: the neural denoiser (§8) runs on the output atlas before save. Baked noise in a runtime lightmap is the worst possible place for it.
- **Per-tile parallel**: one thread per atlas tile, rayon-driven. A 4k atlas at 512 SPP bakes in roughly the same time as an offline marketing still at the same SPP.
- The runtime path is **unchanged** — the shader that sampled the Phase 21 baked atlas samples the same atlas.

## 8. Neural upscaler

Vendor SDKs, wrapped one layer down. Phase 21 refused to ship these because "vendor SDKs, vendor headaches, wgpu layer mismatch." Those concerns haven't evaporated — they've just become worth paying.

```rust
// crates/rustforge-core/src/render/upscaler.rs
pub trait NeuralUpscaler {
    fn name(&self) -> &'static str;
    fn supported(device: &wgpu::Device) -> bool where Self: Sized;
    fn init(&mut self, ctx: &UpscalerCtx) -> Result<()>;
    fn dispatch(&self, frame: UpscalerFrame) -> Result<wgpu::Texture>;
}

pub struct Dlss3; impl NeuralUpscaler for Dlss3 { /* NVIDIA NGX */ }
pub struct Fsr3; impl NeuralUpscaler for Fsr3 { /* AMD FSR 3 source drop */ }
pub struct XeSS; impl NeuralUpscaler for XeSS { /* Intel XeSS SDK */ }

pub fn pick(adapter: &wgpu::Adapter) -> Box<dyn NeuralUpscaler> {
    if Dlss3::supported(...) { return Box::new(Dlss3::default()); }
    if XeSS::supported(...) { return Box::new(XeSS::default()); }
    if Fsr3::supported(...) { return Box::new(Fsr3::default()); }
    Box::new(TaaU::default())  // Phase 21 fallback
}
```

- Interop with wgpu happens through exported textures: we open wgpu textures as native handles (Vulkan external memory, D3D12 shared resources, Metal IOSurface) and hand those to the vendor SDK. This is the part that will break on every wgpu version bump — pin wgpu versions and vet upgrades.
- Each SDK is behind a Cargo feature (`upscaler-dlss`, `upscaler-fsr3`, `upscaler-xess`). Builds without any upscaler feature just have TAA-U.
- License: DLSS and XeSS are proprietary, FSR 3 source is MIT. The license of each backend is exposed as a `pub const LICENSE` on the upscaler trait so projects can audit what they ship.
- Post-graph integration: the upscaler node from Phase 21 §9 gains a `backend` socket; graph serializes the chosen backend name but loading a project authored for DLSS on a machine without DLSS falls back to the best available, logs a single line.

## 9. Neural denoiser

ML-based denoise for RT passes and the path tracer. Optional quality upgrade; the spatiotemporal denoiser is always the guaranteed path.

- **Intel OIDN** — CPU, MIT licensed, integrates trivially. Used by the offline path tracer and lightmap baker always (the quality delta is enormous; the latency doesn't matter for offline).
- **NVIDIA OptiX denoiser** — GPU, CUDA, only on NVIDIA. Used for real-time RT denoise on Ultra tier when enabled.
- **Runtime switching**: editor setting `rt.denoiser ∈ {Spatial, OIDN, OptiX}` picks at startup. Default Spatial on Ultra.
- **Interop gotcha (again)**: OptiX needs CUDA-side pointers to the textures. Same exported-memory dance as §8.

Do **not** ship an in-house trained denoiser in Phase 36. Training pipelines, model versioning, dataset rights — all of that is a separate skillset and phase.

## 10. RT in editor previews

Phase 19 (cinematic sequencer) and Phase 20 (material graph editor) both ship render previews. On Ultra, offer RT for those previews.

- **Material preview** — a single-sphere / cube / teapot preview with RT reflections on gets the user a correct mirror without authoring a probe. Very visible upgrade for metallic material authoring.
- **Sequencer preview** — the timeline scrubber can optionally render each frame with RT shadows + reflections for a WYSIWYG cine preview. Off by default (users scrub at full speed); toggle in the preview toolbar.
- **Latency**: RT preview costs the editor more per-frame than the raster preview. If the editor frame budget (Phase 10 editor perf budget) blows past threshold, auto-downgrade with a toast message.

## 11. Profiler integration

Phase 10 surfaces CPU and GPU timings per subsystem. Phase 36 adds:

- **Per-RT-pass timestamps** — TLAS refit, RT shadows, RT reflections, RT GI, denoise — each is a named range in the GPU profiler.
- **RT usage overlay** — a debug visualization that colors each pixel by the number of rays cast against it (primary + secondary). Finds performance hotspots like "everyone's reflection ray hits the sky except for this one corridor."
- **Denoiser cost breakdown** — spatial vs. temporal vs. neural time separately. ML denoise that eats the budget is the #1 failure mode.
- **Budget warnings** — if any RT pass exceeds its allocated budget (§3–5) for 30 consecutive frames, fire a profiler warning. Do not auto-downgrade at runtime.

## 12. Hybrid fallback matrix

Every RT feature is tier-gated independently. The configuration space is:

```
                 RT GI       RT Reflect   RT Shadow    Upscaler
  Ultra          on/off      on/off       mixed/off    DLSS/FSR/XeSS/TAA-U
  High           VCT         SSR+probe    VSM          TAA-U
  Medium         probe GI    SSR+probe    VSM/CSM      TAA-U/off
  Low            lightmap    probe        CSM          off
```

- Each RT feature has its own `enable: bool` in the project's `rendering.rt` section, independent of tier. Ultra + `rt.gi = false` is valid and sensible (use VCT on Ultra for a cheaper GI).
- Each feature's fallback is deterministic: RT GI off → VCT (if High-capable else probe GI else lightmaps). The fallback ladder is hardcoded, not user-authored.
- Project save files record the intended RT configuration. Loading on a lower-tier machine does not *lose* the configuration — it renders with fallbacks and restores RT when next loaded on an Ultra machine.

## 13. Build order within Phase 36

1. **RT backend & Ultra tier detection (§1)** — no visual change, feature flag + wgpu adapter surface. Merge first.
2. **BLAS / TLAS management (§2)** — geometry-only; render a debug view of the TLAS as wireframe. No RT shading yet.
3. **RT shadows (§3)** — simplest RT feature, closes the loop end-to-end. When this works, the backend works.
4. **RT reflections (§4)** — builds on §3's shader scaffolding. Graceful fallback to Phase 21 SSR.
5. **RT GI (§5)** — the quality jump of the phase. Lands last among the real-time RT features because it depends on §3 / §4 denoisers being robust.
6. **Spatiotemporal + neural denoiser (§9)** — the optional neural path lands after real-time RT settles. Spatial denoiser has to work without it.
7. **Offline CPU path tracer (§6)** — independent from the real-time RT features; can begin in parallel once §2 lands. Gated on no runtime feature.
8. **Neural upscaler (§8)** — separable; begins in parallel with §3–5. Vendor SDK wrangling is its own thread.
9. **Path-traced lightmap bake (§7)** — reuses §6. Ships after §6 + §9 are solid.
10. **Editor RT previews (§10)** — requires all the above to be stable. Nice-to-have, last.
11. **Profiler integration (§11)** — lands alongside each feature, consolidated at the end.

## 14. Scope ❌ — what's NOT in Phase 36

- ❌ **Custom BVH construction.** The driver builds BVHs. We do not chase SAH tuning, TLAS compaction heuristics, or custom intersectors.
- ❌ **GPU wavefront path tracer.** A separate phase, minimum. CPU is enough for offline here.
- ❌ **Monte Carlo light transport research.** No MIS, no bidirectional path tracing, no Metropolis light transport, no guided sampling. Pure PT with NEE.
- ❌ **Spectral rendering.** RGB tristimulus throughout.
- ❌ **Lightfield rendering.** Not a product feature in our target space.
- ❌ **Neural radiance caches / NeRF-style real-time GI.** Experimental, unstable training pipelines, no vendor alignment. Revisit in a later phase if the field settles.
- ❌ **In-house trained denoiser.** Use OIDN / OptiX.
- ❌ **RT transparency / refraction.** Phase 37+. Opaque materials only in Phase 36.
- ❌ **Hair / fur RT shading.** Strand-based hair is a different phase. Non-trivially interacts with RT.
- ❌ **Particle BVH inclusion.** Particles raster-composite after RT.
- ❌ **Multi-GPU RT.** Still no.
- ❌ **RT on Web / Mobile.** Tier cap prevents it.
- ❌ **Real-time caustics via RT.** Photon mapping variant. Out of scope.
- ❌ **Ray-traced ambient occlusion as a separate pass.** RT GI already nails contact darkening; a separate RTAO pass is redundant.
- ❌ **Vendor SDK version upgrade automation.** Manual pin + vet per release.

## 15. Risks

- **wgpu RT surface stability.** "Stable" is a moving target. A wgpu minor-version bump might change ray-query binding layout. Pin wgpu and gate upgrades on running the Phase 36 visual regression suite.
- **BLAS build memory spikes.** A scene with 500 unique meshes doing first-time BLAS builds on load can spike peak VRAM. Stream BLAS builds over multiple frames during load; warn if any single build exceeds 256 MB scratch.
- **Skinned BLAS cost.** Refit is cheap per-mesh but multiplies by character count. Budget two milliseconds for 30 characters on reference Ultra GPU; gate the skinned RT set when exceeded (characters beyond threshold render with VSM shadows).
- **TLAS refit thrash.** If "dynamic" is over-tagged in authoring (e.g., every prop marked dynamic "just in case"), the TLAS refit becomes a full rebuild every frame. Lint authoring: warn when > 10% of scene instances are dynamic.
- **RT GI flicker on fast camera.** Temporal reservoir accumulation fights with disocclusion. Same tuning game as TAA-U; conservative default, knob exposed.
- **Denoiser ghosting on moving characters.** Spatiotemporal denoise treats history as truth; fast character motion produces tails. Neural denoiser is better here but not available on every machine. Tight motion-vector dilation in the denoiser, documented knob.
- **Vendor SDK interop fragility.** DLSS / FSR / XeSS each expect specific resource states, barrier patterns, and NHWC layouts. The adapter layer has to translate every time. Integration tests must exercise all three on a matrix CI, not just "it worked on my RTX."
- **OptiX CUDA coupling.** OptiX demands the CUDA runtime. Bundling CUDA ballons the installer. Ship OptiX as an optional download the user installs once, not in the main package.
- **Path tracer divergence from real-time.** CPU PT shading and real-time GPU shading can produce materially different results if the material graph has any branch that differs between the WGSL and Rust codegen. Golden-image tests per stock material node, no exceptions.
- **Editor preview thrash.** Turning on RT material preview on a cold scene triggers a per-mesh BLAS build for every material ball. Preview uses a cached primitive set; no scene BLAS builds.
- **Lightmap bake times regress from Phase 21.** The VCT path had no bake. Projects migrating to Ultra + path-traced lightmaps will see longer bakes. Document. Iteration-bake mode (64 SPP) exists for this reason.
- **Upscaler license surprise.** A team ships with DLSS, is later blocked by NVIDIA license review for a platform release. The `LICENSE` surfacing at build time catches this; but enforce in CI that the shipped binary's upscaler match the declared license whitelist.
- **Profiler overhead from per-ray counters.** The "rays cast per pixel" overlay instruments every ray. In release builds the instrumentation compiles out; debug builds disable it on low-end Ultra GPUs where the overhead skews measurements.
- **Fallback drift.** "Works on Ultra" and "works on High" become different code paths that nobody looks at simultaneously. Automated visual regression must run both tiers on every commit to rendering code, not just Ultra.

## 16. Exit criteria

Phase 36 is done when all of these are true:

- [ ] `RealismTier::Ultra` is detected correctly on RTX 2000+, RDNA2+, Arc Alchemist+, Apple Silicon with Metal 3 RT. Unit tests assert tier on mocked adapter features.
- [ ] Per-mesh BLAS builds at asset import, caches to disk, and loads in under 50 ms for a 500k-triangle mesh on the reference GPU.
- [ ] TLAS refit for a 5000-instance scene runs in ≤ 0.5 ms / frame on the reference GPU; full rebuild on topology change in ≤ 3.0 ms.
- [ ] RT shadows render the open-world sample scene with soft penumbra and no visible banding; ≤ 1.5 ms / frame on RTX 3060 at 1440p for the reference light configuration.
- [ ] RT reflections render a polished-floor sample scene with correct off-screen geometry and smooth roughness transitions; ≤ 2.5 ms / frame on RTX 3060 at 1440p half-res.
- [ ] RT GI renders the Cornell-analog test scene with two bounces and matches the offline path tracer output within the established per-channel ΔE target; ≤ 4.0 ms / frame on reference GPU.
- [ ] Spatiotemporal denoiser eliminates visible noise on the canonical reference frames; neural denoiser improves SSIM by ≥ 5% at equal sample count when enabled.
- [ ] **Performance target:** RTX 3060, 1440p, RT GI + RT reflections + RT directional shadow + neural upscaler at 66% render scale, reference scene: 60 fps sustained across the canonical camera path.
- [ ] Offline CPU path tracer renders the Cornell-analog test at 1024 SPP to a target convergence metric in documented time on a 16-core reference CPU; pixel-identical on repeat runs with a fixed seed.
- [ ] Path-traced lightmap baker produces atlases that the existing runtime shader consumes unchanged; bake is resumable and parallelized across tiles.
- [ ] Neural upscaler: at least two of {DLSS 3, FSR 3, XeSS} wired through the adapter and produce correct frames on corresponding vendor hardware; TAA-U fallback triggers automatically on unsupported devices.
- [ ] Neural denoiser (OIDN for path tracer / bake; OptiX for real-time) is feature-gated, optional, and produces cleaner frames than the spatiotemporal baseline at equal sample count.
- [ ] Material editor and cinematic sequencer preview both support optional RT on Ultra, with a user toggle and automatic downgrade on editor frame-budget overrun.
- [ ] Profiler surfaces per-RT-pass timings; the "rays per pixel" overlay visualizes RT cost distribution; automated warning fires at persistent budget overrun.
- [ ] Every RT feature can be independently disabled; the fallback matrix renders the scene without panics or NaNs on all 16 combinations of {GI, Reflect, Shadow, Upscaler} × {on, off}.
- [ ] Phase 21's VCT, SSR, VSM, TAA-U render identically on non-Ultra devices; Phase 36 adds no regressions to High / Medium / Low paths.
- [ ] Visual regression suite runs both Ultra and High tier on every commit touching the renderer; per-commit ΔE budget is enforced.
- [ ] The `editor`-feature gate from Phase 2 still excludes the RT preview UI from game builds; game builds with RT enabled render correctly without any editor code.
- [ ] Documented performance table covers RTX 3060, RX 7700 XT, Arc A770, Apple M3 Pro at 1080p / 1440p with the reference scene, recording per-pass costs and the full frame time.
