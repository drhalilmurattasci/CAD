# Phase 18 — Particles & VFX

Post-1.0, RustForge has material graph authoring (Phase 16) and an animation authoring tool (Phase 17). The remaining authoring gap for any modern real-time renderer is visual effects — smoke, fire, dust, sparks, trails, magic, weather, impacts. Without a first-class particle system, every game built on the engine ends up either hand-rolling emitter code per effect or importing ad-hoc meshes baked in a DCC. Phase 18 closes this gap with a Niagara-equivalent: GPU-compute-driven simulation, a modular emitter model, and a node-graph authoring tool that reuses the widget infrastructure Phase 17 built for its state-machine editor.

Scope is deliberately limited to **particle/sprite/ribbon/mesh effects**. Simulation domains that share surface tooling (fluid, cloth, destruction, volumetric clouds) are called out in §5 and deferred.

## Goals

By end of Phase 18:

1. **Emitter model** — spawn + init + update + render modules, composable, data-driven.
2. **GPU compute path** — WGSL compute shaders drive per-particle state for emitters above a threshold; CPU path for small emitters (<10k live particles).
3. **`.rvfx` asset format** — RON-serialized emitter graphs, diff-friendly, hot-reloadable (Phase 5).
4. **VFX Graph Editor panel** — AssetEditor-pattern (Phase 8 §1) using the node-graph widget from Phase 17.
5. **Shader codegen** — emitter graph compiles to a WGSL compute shader with hash-keyed cache.
6. **Isolated preview viewport** — same pattern as material/animation preview (Phase 8 §2.1, §4.3), with HDRI backdrop and timeline scrubber.
7. **`ParticleEmitter` component** — runtime-facing, spawnable like any other component, parameters scriptable.
8. **Per-emitter profiling** — extends Phase 8 §6 with GPU+CPU breakdown per active emitter.
9. **Distance-based LOD** — spawn-rate and tick-rate fall-off by camera distance.
10. **Collision** — depth-buffer collision for cheap environmental bouncing, optional analytic colliders (sphere, plane) for gameplay-critical interactions.
11. **Determinism** — seeded RNG per-emitter, advancing per-frame, reproducible given fixed dt.
12. **Trails and ribbons** — first-class renderer type alongside sprite/mesh, with sub-UV flipbook support on sprites.
13. **Platform fallback** — WebGL2 target (no compute shaders) falls back to CPU path.

## 1. Data model — the four-module emitter

Every emitter is a pipeline of four module categories executed in order per tick:

```
┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐
│  Spawn  │ → │  Init   │ → │ Update  │ → │ Render  │
└─────────┘   └─────────┘   └─────────┘   └─────────┘
  rate/burst    one-shot      per-tick      each frame
                 (new)        (all live)
```

```rust
pub struct EmitterDef {
    pub name:       String,
    pub max_particles: u32,
    pub sim_space:  SimSpace,          // World | Local
    pub spawn:      Vec<SpawnModule>,
    pub init:       Vec<InitModule>,
    pub update:     Vec<UpdateModule>,
    pub render:     RenderModule,      // exactly one
    pub seed:       u64,               // determinism (§10)
}
```

Each module is a concrete enum variant, not a trait object — this keeps the data plain, serializable, and visible to the shader codegen. Plugin-defined modules can live in a separate registry and generate into the graph via code, but shipping Phase 18 with a closed set keeps the WGSL generator tractable.

### 1.1 Module catalogue (initial)

- **Spawn:** `Rate { per_second }`, `Burst { count, at_time }`, `Continuous { curve }`
- **Init:** `Position { shape: Point|Sphere|Box|Cone|Mesh }`, `Velocity { mode: Directional|Radial|Orbital }`, `Lifetime { min, max }`, `Size { min, max, curve }`, `Color { gradient }`, `Rotation { min, max }`
- **Update:** `Gravity`, `Drag { linear, angular }`, `CurlNoise { scale, strength }`, `PointForce`, `VortexForce`, `Collision { source }`, `ColorOverLife`, `SizeOverLife`, `RotateOverLife`, `SubUVAnimation { fps }`
- **Render:** `Sprite { material, mode: Billboard|Velocity|Facing, sub_uv }`, `Mesh { mesh, material }`, `Ribbon { material, max_verts, tessellation }`, `Beam { start, end, segments }`

One render module per emitter — effects with multiple renderers use multiple emitters in the same `.rvfx` asset. This dodges a surprising amount of dispatch complexity.

## 2. GPU path — structure of the compute pipeline

The GPU path is the primary one for any serious effect. Per emitter:

```
Per frame:
  1. spawn_count = compute_spawn_cpu_side(time, dt)        // tiny CPU work
  2. alloc_slots.dispatch(spawn_count)                     // compact free-list
  3. init.dispatch(spawn_count)                            // write new particles
  4. update.dispatch(live_count)                           // advance all live
  5. sort_or_draw.dispatch(live_count)                     // optional sort + draw
```

Particle state is a `array<Particle>` SSBO with a companion free-list. A dead particle zeroes its `lifetime_remaining` — the update kernel writes it to the free-list. `alloc_slots` pops `spawn_count` free indices and seeds them for `init`.

```rust
#[repr(C)]
struct Particle {
    pos: Vec3, life_remaining: f32,
    vel: Vec3, life_total: f32,
    color: [u8; 4],   // packed
    size: f32, rot: f32, sub_uv_frame: u32,
    seed: u32,        // per-particle RNG state for init-time randomness
}
```

32 bytes per particle is the target; a million particles is 32 MB of SSBO. Document the ceiling; budget-conscious projects will want to keep per-emitter caps low.

### 2.1 When to go CPU

Below the threshold the CPU path wins on latency (no upload round-trip, no compute dispatch overhead). Threshold of 10k live particles is a reasonable cutover for 60Hz; make it configurable per emitter and per platform. WebGL2 targets force CPU regardless (§9).

## 3. Shader codegen

An emitter graph compiles to three WGSL compute shaders: `init`, `update`, `alloc`. `alloc` is boilerplate and baked. `init` and `update` are generated by concatenating per-module snippets in declaration order.

```
crates/rustforge-core/src/vfx/
├── mod.rs
├── emitter.rs        # EmitterDef, SimSpace, modules enum
├── codegen.rs        # EmitterDef -> String (WGSL)
├── cache.rs          # blake3-keyed shader cache, disk + mem
├── runtime.rs        # VfxSystem: allocates buffers, issues dispatches
└── cpu_sim.rs        # fallback path, same module semantics
```

```rust
pub fn compile(def: &EmitterDef) -> CompiledEmitter {
    let key = blake3::hash(&postcard::to_vec(def).unwrap());
    if let Some(cached) = CACHE.get(&key) { return cached; }

    let init   = emit_wgsl_init(def);
    let update = emit_wgsl_update(def);
    let pipes  = build_pipelines(&init, &update);

    CACHE.insert(key, pipes.clone());
    pipes
}
```

Cache key: hash of the serialized `EmitterDef`. Asset-level hot-reload (Phase 5) is automatic — a changed `.rvfx` produces a new hash and a recompile. Disk cache lives under `.rustforge/shader-cache/vfx/{hash}.wgsl` so iteration after a restart is instant.

### 3.1 Why codegen and not interpretation

A per-module dynamic dispatch inside the compute kernel would be catastrophic for GPU occupancy. Codegen lets the compiler inline everything, strip dead code, and keeps register pressure predictable. The cost is build-time complexity, not runtime complexity — worth it.

## 4. VFX Graph Editor panel

The editor-side surface is an `AssetEditor` implementation (Phase 8 §1) backed by the node-graph widget Phase 17 built for its state-machine editor. Each node is a module; edges carry bound parameters (curves, gradients, scalar expressions); a sink node represents the render module.

```
┌─ VFX: muzzle_flash.rvfx ─────────────────────────────────────────┐
│ ┌──────────────┐  ┌────────────────────────────────────────────┐ │
│ │              │  │ Emitter: Flash               [+ Emitter]   │ │
│ │              │  │ ┌─────────────┐   ┌─────────────┐          │ │
│ │              │  │ │ Spawn: Burst│──▶│ Init: Pos   │          │ │
│ │   Preview    │  │ │   count 24  │   │  sphere r.1 │          │ │
│ │              │  │ └─────────────┘   └──────┬──────┘          │ │
│ │              │  │                          ▼                 │ │
│ │              │  │                   ┌─────────────┐          │ │
│ │              │  │                   │ Init: Vel   │          │ │
│ │   [HDRI ▼]   │  │                   │  radial 4.0 │          │ │
│ │              │  │                   └──────┬──────┘          │ │
│ │              │  │                          ▼                 │ │
│ │              │  │                   ┌─────────────┐          │ │
│ │              │  │                   │ Upd: Drag   │          │ │
│ │              │  │                   │   linear 2  │          │ │
│ │              │  │                   └──────┬──────┘          │ │
│ │              │  │                          ▼                 │ │
│ │              │  │                   ┌─────────────┐          │ │
│ │              │  │                   │ Render:     │          │ │
│ │              │  │                   │  Sprite     │          │ │
│ │              │  │                   └─────────────┘          │ │
│ └──────────────┘  └────────────────────────────────────────────┘ │
│ [▶] [⏸] [⟲]  t = 0.34 / 1.00   ├──────●───────────────────────┤  │
│ Live:   247     Spawned: 312    Killed: 65     GPU: 0.08 ms      │
└───────────────────────────────────────────────────────────────────┘
```

### 4.1 Parameter binding

Each numeric field has three states: constant, curve (lifetime-parameterized), or external (driven by `ParticleEmitter` component parameter). The inspector for a selected node edits the field and routes through Phase 6's command stack (same `EditAssetFieldCommand` pattern as the material editor).

### 4.2 Preview viewport

Isolated `hecs::World`, as with animation preview (Phase 8 §4.3). Lives under `asset_editors/vfx/preview.rs`. HDRI dropdown and preview shape picker stored in editor prefs, not the asset. Timeline scrubber drives a `PreviewTime` resource; while scrubbing, the emitter runs on the CPU path regardless of size — determinism is cheaper than reproducible GPU rewind.

### 4.3 Multi-emitter `.rvfx`

One `.rvfx` asset holds an array of emitters plus shared parameters. The editor shows each emitter as a collapsible section with its own node graph; all share the preview viewport.

## 5. Scope ❌

- ❌ **Fluid / SPH / grid fluid sim** — separate future phase.
- ❌ **Cloth simulation** — Phase 23 (physics-adjacent).
- ❌ **Destruction / fracture (Chaos-equivalent)** — Phase 25.
- ❌ **Volumetric clouds / sky** — Phase 27.
- ❌ **GPU-driven hair/fur** — out of scope, separate rendering feature.
- ❌ **Particle-driven lights** — per-particle point lights tank performance at scale. Revisit with clustered forward (Phase 14).
- ❌ **Mesh-emitter skinning of individual particles** — mesh render module uses static meshes only in Phase 18. Animated-mesh particles are a future extension.
- ❌ **Scripted module authoring inside the editor** — module catalog is fixed per-phase; plugins add modules via code, not graph edits.
- ❌ **Networked emitter sync** — emitters are visual. Authoritative gameplay should not depend on particle state. Determinism (§10) covers lockstep visuals; that's the extent.
- ❌ **GPU raytraced collision** — depth-buffer collision plus analytic shapes is the Phase 18 ceiling.

## 6. `ParticleEmitter` component & scripting

```rust
#[derive(Reflect, Clone)]
pub struct ParticleEmitter {
    pub asset:       AssetRef<VfxAsset>,
    pub playing:     bool,
    pub time_scale:  f32,
    pub loop_mode:   LoopMode,           // Once | Loop | PingPong
    pub params:      SmallVec<[VfxParam; 4]>,  // external bindings (§4.1)
}

impl ParticleEmitter {
    pub fn play(&mut self);
    pub fn stop(&mut self);
    pub fn set_param(&mut self, name: &str, value: VfxValue);
}
```

Scripts (Phase 11 WASM) call `emitter.set_param("intensity", 2.5)` and the compute kernel picks it up via a per-emitter uniform. No script callbacks into per-particle logic — that would cross the CPU/GPU boundary on the hot path.

## 7. Per-emitter profiling

Extends Phase 8 §6. The profiler's GPU pass list gets a collapsible **VFX** section:

```
VFX
  muzzle_flash              0.08 ms    247 live
  campfire_smoke            0.41 ms   1842 live
  rain                      1.92 ms  18432 live    [LOD: 50%]
```

Metrics captured via the existing `Sampler::scope` infrastructure plus wgpu timestamp queries bracketing each emitter's dispatch group. Displayed in the emitter preview viewport header as well (see ASCII mockup above).

## 8. LOD

Distance-based, two levers:

- **Spawn scaling** — `spawn_rate *= lod_curve(distance)`; at far distance drops to zero, emitter sleeps until distance recovers.
- **Tick stride** — tick every Nth frame beyond a distance threshold; integrate with a catch-up dt. Don't stride `init` work; the spawn count already handles that.

```rust
pub struct LodCurve {
    pub near:   f32,   // full rate up to this distance
    pub far:    f32,   // zero rate past this distance
    pub stride: [(f32, u32); 3],  // (distance, tick_every_n_frames)
}
```

Per-emitter LOD curve in the asset. Global multiplier in project settings for quality presets. A hidden emitter (frustum-culled AND behind camera for more than N frames) sleeps entirely — free-list state preserved, dispatch skipped — and resumes without a pop by spawning its steady-state population in a single burst on wake-up.

### 8.1 Screen-coverage LOD (optional)

For billboards, screen coverage is a stricter metric than distance: a huge emitter far away may still dominate the screen. Compute projected radius at the emitter's bounding sphere and scale spawn by the inverse. This is a later refinement — Phase 18 ships distance-only LOD and leaves the coverage hook in the module API.

## 9. Platform fallback

WebGL2 has no compute. The CPU path exists for the <10k case; for WebGL2 targets it becomes the *only* path. Hard-cap per-emitter max at 10k and per-project VFX particle budget at 50k when the target is WebGL2. The compiler front-end (same `codegen.rs`) generates equivalent per-module snippets for the CPU path — module semantics are defined once, emitted twice.

Emit a warning during cook if a `.rvfx` would exceed the CPU-budget on WebGL2 targets.

## 10. Determinism

Each emitter has a seed. Each frame, the RNG state is advanced as `state = hash(seed, frame_index)`. Per-particle randomness derives from `hash(state, particle_slot)`. Given the same seed, frame index, and spawn count, the init phase produces identical particles.

This is necessary for:

- **Replay** — Phase 15 recording replays VFX faithfully.
- **Deterministic test captures** — the VFX editor's golden-image tests rely on a fixed seed producing the exact same preview frame.
- **Networked lockstep visuals** — not required, but nice to have.

Determinism is GPU-path-sensitive: wave-level ordering in compute can vary. Keep per-particle work independent; avoid cross-lane operations.

## 11. Trails and sub-UV

Ribbons and beams are first-class renderers, not a special case shoehorned into sprite. A ribbon's geometry is rebuilt each frame from a sliding buffer of per-spawn trail points; tessellation and width are module parameters. Beams tie two transforms (or an emitter and a target) with parameterized segmentation and noise offset.

Sprite flipbook: an `Nx M` sub-UV grid, frame advances via the `SubUVAnimation` update module at a configurable fps. Frame blending between adjacent cells is a render-module flag — cheap, big visual win.

```rust
pub enum RenderModule {
    Sprite {
        material: AssetRef<Material>,
        mode: SpriteMode,
        sub_uv: Option<SubUv>,
        blend: Blend,      // Additive | Alpha | Premultiplied
    },
    Ribbon {
        material: AssetRef<Material>,
        max_verts_per_strand: u32,
        tessellation: u32,
        uv_mode: RibbonUvMode,  // Stretch | Tile | PerParticle
    },
    Beam { /* ... */ },
    Mesh { /* ... */ },
}
```

## 12. Build order

Each step is independently shippable.

1. **CPU emitter + `ParticleEmitter` component.** Four-module pipeline, Sprite render, no graph editor — authored by code. De-risks the module semantics.
2. **`.rvfx` format + RON round-trip.** Asset loader, hot-reload hook, no editor yet.
3. **GPU compute path.** Codegen, cache, dispatch. Feature-gate — CPU stays the reference semantics for tests.
4. **VFX Graph Editor panel.** AssetEditor, node-graph widget reuse, command-stack integration.
5. **Preview viewport.** HDRI, scrubber, live metrics.
6. **Ribbons / beams / sub-UV.** Once core + graph are stable, renderer variety is additive.
7. **Depth collision + analytic colliders.**
8. **LOD + per-emitter profiler integration.**
9. **WebGL2 fallback cooker pass.**

Authoring the GPU path *before* the graph editor is deliberate: the editor becomes a view over a known-correct runtime, not the definition of it. Reversing the order would bake UX decisions into runtime data.

## 13. Risks

- **Codegen complexity.** A module's WGSL snippet must compose with any other module's — ordering, bindings, register use. Early rule: modules read/write a fixed `Particle` struct in-place only; no module introduces new SSBOs. Breaking this rule unlocks features but destroys composability.
- **Sort cost for translucent sprites.** Back-to-front sort per frame is expensive above 100k particles. Phase 18 ships bitonic sort on the GPU as the default; additive-blend emitters skip sort entirely — document the perf delta prominently.
- **Depth-buffer collision ghosts.** Collision against the scene depth buffer misses anything outside the camera frustum. For gameplay-critical collisions (player walks through snow and kicks it up) use analytic colliders. The editor should warn when collision is enabled on a long-lived particle that might leave frustum.
- **Hot-reload stalls.** A changed `.rvfx` triggers shader recompile. Naga + wgpu pipeline creation can take tens of ms. Compile in a worker; keep the old pipeline alive until the new one is ready (same pattern as material hot-reload).
- **Determinism on GPU.** Claimed above, but in practice atomic ordering in `alloc_slots` is non-deterministic across drivers. Either use per-slot ownership (fixed mapping from dispatch index to slot) or accept that determinism holds for init-time randomness but not for slot reuse order. Go with per-slot ownership.
- **Editor memory with many open previews.** Each VFX editor tab allocates an offscreen preview target and a CPU simulation world. Pause simulation when tab is unfocused — same policy as Phase 8 §9.
- **Ribbon buffer churn.** Rebuilding ribbon geometry per frame on CPU and uploading is fine at one emitter; at ten, it's the hot path. Investigate a GPU ribbon path in a later phase; Phase 18 ships CPU-built ribbons and documents the scaling wall.
- **WebGL2 cooker lying.** Silent CPU fallback at runtime is worse than a cook-time error. Fail the cook if an emitter's declared cap exceeds the CPU budget, don't auto-clamp.

## 14. Exit criteria

Phase 18 is done when all of these are true:

- [ ] `ParticleEmitter` component spawns, plays, loops, and stops correctly on CPU and GPU paths.
- [ ] A `.rvfx` asset round-trips losslessly (save, reload, re-save — identical file).
- [ ] Hot-reload of a `.rvfx` swaps the running emitter without a visible hitch under 10k particles.
- [ ] VFX Graph Editor opens from Content Browser, renders the node graph, edits route through the command stack, Ctrl+Z undoes slider drags as a single unit (Phase 6 coalescing).
- [ ] Preview viewport runs the selected `.rvfx` with HDRI lighting, scrubber, and live metrics without touching the scene world.
- [ ] WGSL codegen output is byte-identical for byte-identical `EmitterDef`s (hash cache hit rate ≥ 99% during steady-state editing).
- [ ] At least one emitter of each render type (sprite, mesh, ribbon, beam) runs in the sample project.
- [ ] Sub-UV flipbook animation plays on a sprite emitter.
- [ ] Depth-buffer collision bounces particles off scene geometry; a configured analytic sphere collider does the same independently.
- [ ] Seeded emitter with fixed dt produces identical init-time particle state across runs (regression-tested).
- [ ] Per-emitter CPU and GPU timings appear in the profiler panel (Phase 8 §6) and the VFX editor header.
- [ ] Distance-based LOD reduces spawn rate to zero past the configured distance and recovers on re-entry.
- [ ] WebGL2 build cooks with the CPU path and refuses to cook assets exceeding the CPU budget.
- [ ] `rustforge-core` still builds without the `editor` feature; runtime VFX works in shipped builds.
