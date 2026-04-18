# Phase 20 — Material Node Graph

Phase 8 §2.2 made an explicit call: the material editor would ship a property sheet, and a node-based graph — "parsing, layout, topological evaluation, code generation into WGSL" — was deferred to a future phase. This is that phase. It closes the deferral and brings material authoring to parity with the node graphs users expect from Unreal, Unity Shader Graph, and Blender — without inventing a second graph UI for each future node-based domain. Phases 17 (audio graph) and 18 (VFX graph) already pointed at the same widget problem; Phase 20 extracts it once, pays the cost, and lets those phases reuse it.

This phase ships a full material node graph, a WGSL code generator, master materials and parameter-override instances, a plugin-authored node API, and feature-bit shader permutations. The underlying graph widget becomes a standalone crate so audio and VFX do not reinvent pan/zoom/connection routing for the third and fourth time.

## Goals

By end of Phase 20:

1. **Shared `rustforge-node-graph` crate** — domain-agnostic DAG widget: pan, zoom, multi-select, box-select, connection routing, minimap, undo-friendly mutation API.
2. **Typed material DAG** — nodes with `float / Vec2 / Vec3 / Vec4 / texture / sampler` ports; Master node with Phase 8 PBR inputs.
3. **v1 node palette** — constants, UV, sampler, math, noise, gradient, time, camera, fresnel, vertex attrs, texture sample, Custom WGSL.
4. **WGSL codegen** — topological sort, dead-code elimination, hash-keyed compiled-shader cache.
5. **Source-mapped compile errors** — WGSL diagnostics route back to originating nodes with red badges; compile failure never crashes the editor.
6. **Master + instance materials** — parameter-only overrides; no topology change in instances.
7. **Exposed parameters** — marked parameters become runtime-scriptable component fields.
8. **Live preview** — debounced-on-release recompile into the Phase 8 material preview viewport.
9. **Subgraphs / functions** — reusable fragments saved as `.rmatfunc` assets.
10. **Plugin-authored nodes** — extend the palette through Phase 11's plugin API.
11. **Feature-bit permutations** — skinned vs static, forward vs deferred, etc., capped at 32 per material.
12. **Migration** — existing `.rmat` property materials auto-generate an equivalent graph on first open.

## 1. The shared `rustforge-node-graph` crate

Three upcoming phases need the same widget: material (Phase 20), audio routing (Phase 17), VFX (Phase 18). Writing it three times is the failure mode. Writing it once, poorly factored, is the other failure mode. Extract it as its own crate with the domain knowledge held out.

```
crates/rustforge-node-graph/
├── Cargo.toml
└── src/
    ├── lib.rs              # re-exports
    ├── model.rs            # NodeId, PortId, Edge, Graph<N, P>
    ├── view.rs             # pan/zoom state, viewport transform
    ├── interact.rs         # select, box-select, drag, connect
    ├── routing.rs          # bezier connection paths + hit testing
    ├── minimap.rs          # scaled overview panel
    ├── palette.rs          # PaletteTrait — per-domain node menu
    └── compile.rs          # CompileTrait — per-domain compile hook
```

The generics are the contract. Material provides its own `MaterialNode` and `MaterialPort` types; audio provides `AudioNode` / `AudioPort`; both share pan/zoom/selection/routing.

```rust
pub trait NodeDomain: 'static {
    type Node: Clone + Send;
    type PortType: PortType;          // equality + color + compatibility
    type CompileOutput;
    type CompileError: NodeError;

    fn palette() -> &'static dyn Palette<Self>;
    fn compile(graph: &Graph<Self>) -> Result<Self::CompileOutput, Vec<Self::CompileError>>;
}
```

Nothing inside the widget crate knows what a "texture" or "frequency" is — it holds positions, draws bezier curves between compatible ports, and forwards edit events. Undoability is achieved by having domain code push `Phase 6` commands in response to widget events (`NodeMoved`, `EdgeCreated`, `NodeRemoved`). The widget itself never writes to the command stack — it is below that layer.

```
┌─────── rustforge-node-graph (domain-agnostic) ───────┐
│                                                       │
│   pan · zoom · select · box-select · routing · minimap│
│                                                       │
└───────────┬───────────────┬───────────────┬───────────┘
            ▼               ▼               ▼
       Phase 20         Phase 17        Phase 18
       Material         Audio           VFX
       palette          palette         palette
       WGSL compile     DSP compile     particle compile
```

## 2. Material graph model

The material-side data lives in `rustforge-core` so the runtime can instantiate it; the editor lives in `rustforge-editor` on top.

```
crates/rustforge-core/src/material/graph/
├── mod.rs                # MaterialGraph, MaterialNode, MaterialEdge
├── ports.rs              # PortType, PortValue
├── master.rs             # Master node: PBR pins (Albedo, Metallic, ...)
├── instance.rs           # MaterialInstance { master: Guid, overrides }
└── serialize.rs          # .rmat graph format v2
```

```rust
pub enum PortType {
    Float, Vec2, Vec3, Vec4,
    Texture2D, Sampler,
}

pub struct MaterialNode {
    pub id:     NodeId,
    pub kind:   NodeKind,            // enum tag: Constant, UV, Sampler, Math, Noise, ...
    pub params: NodeParams,          // per-kind inline data (e.g., Constant value)
    pub pos:    Vec2,
}

pub struct MaterialEdge {
    pub from: (NodeId, PortIdx),
    pub to:   (NodeId, PortIdx),
}
```

The master node is a fixed node kind with Phase 8's PBR pin set: Albedo (Vec3), Metallic (Float), Roughness (Float), Normal (Vec3), Emissive (Vec3), AO (Float), Alpha (Float). Exactly one per graph; cannot be deleted.

Type coercion: a `Float` connecting into a `Vec3` broadcasts; a `Vec3` into a `Float` requires an explicit swizzle node. Fail validation on incompatible connections and refuse to route the edge at the widget layer (the domain callback `can_connect(from, to)` says no, the bezier snaps back).

## 3. The v1 node palette

Shipping too few nodes turns the feature into a demo; shipping too many turns it into a scope hole. The v1 palette is the list that covers 90% of real shaders.

| Category | Nodes |
|----------|-------|
| Constants | `Float`, `Vec2`, `Vec3`, `Vec4`, `Color` |
| Inputs | `UV0`, `UV1`, `VertexColor`, `WorldPos`, `WorldNormal`, `Time`, `CameraPos`, `CameraDir` |
| Texture | `Texture2D` (asset ref), `Sampler`, `SampleTexture2D(uv, tex, sampler)` |
| Math | `Add`, `Sub`, `Mul`, `Div`, `Saturate`, `Clamp`, `Lerp`, `Step`, `Smoothstep`, `Pow`, `Sqrt`, `Abs`, `Sign`, `Min`, `Max` |
| Vector | `Dot`, `Cross`, `Normalize`, `Length`, `Reflect`, `Refract`, `Swizzle` |
| Noise | `PerlinNoise`, `SimplexNoise`, `VoronoiNoise` (all 2D and 3D variants) |
| Gradient | `GradientRamp`, `LinearGradient`, `RadialGradient` |
| Utility | `Fresnel`, `OneMinus`, `Remap`, `Panner`, `Rotator` |
| Flow | `IfGreater`, `IfLess` (compile-time branch, not runtime) |
| Custom | `CustomWGSL` — multiline WGSL body with declared in/out ports |

`CustomWGSL` is the escape hatch for everything we do not ship. It compiles its body as a WGSL function with a signature derived from its declared ports; if compilation fails, it shows the WGSL error on the node. This is how a studio with a bespoke skin shader does not need a plugin — they paste the snippet in.

## 4. WGSL code generation

Compilation is a four-stage pipeline. Each stage is unit-testable in isolation.

```
graph edit ─▶ topo sort ─▶ DCE ─▶ WGSL emit ─▶ naga parse ─▶ wgpu module
                                                    │
                                                    └─▶ hash-keyed cache hit?
```

1. **Topological sort** — Kahn's algorithm on the DAG. Cycle detection returns a compile error pointing at the cycle's back-edge nodes; the widget highlights them.
2. **Dead-code elimination** — start from the Master node, walk backwards along edges, mark reachable. Unreachable nodes do not emit code (but still draw in the editor with a faded badge — "not contributing").
3. **WGSL emit** — visit nodes in topo order, each node emits a SSA temporary line.

   ```rust
   pub trait EmitWgsl {
       fn emit(&self, ctx: &mut EmitCtx) -> Result<(), EmitError>;
   }
   ```

   `EmitCtx` holds a `String` builder, a `HashMap<(NodeId, PortIdx), Ident>` for port → variable names, and counters for uniform/texture binding slots. Naming is deterministic: `n{node_id}_{port_idx}`.

4. **Hash cache** — hash the normalized graph (nodes sorted, positions stripped) plus the feature-bit set → content-addressed WGSL cache at `<project>/.rustforge/shader-cache/<hash>.wgsl` and compiled pipeline in memory. Opening the same graph twice is a cache hit; rebuilding from scratch on every edit would stall editing at graph-growth time.

Example emitted WGSL for a trivial graph (`Texture2D → Sample → Master.Albedo`):

```wgsl
// generated from material graph 0c3e..9f4b
@group(2) @binding(0) var n3_tex: texture_2d<f32>;
@group(2) @binding(1) var n3_smp: sampler;

fn material_main(in: VertexOut) -> MaterialOutput {
    let n2_0: vec2<f32> = in.uv0;
    let n4_0: vec4<f32> = textureSample(n3_tex, n3_smp, n2_0);
    var out: MaterialOutput;
    out.albedo = n4_0.rgb;
    // ... remaining master pins fall back to defaults
    return out;
}
```

### 4.1 Source-mapped errors

Naga errors carry WGSL spans. We keep a `Vec<(Range<usize>, NodeId)>` during emission — each node records the byte range it wrote. A naga error span intersects at most one range, and that's the offending node.

```rust
pub struct NodeCompileError {
    pub node:    NodeId,
    pub message: String,     // from naga
    pub kind:    ErrorKind,  // TypeMismatch | UndeclaredBinding | ...
}
```

The editor draws a red dot on those nodes and surfaces the message on hover. Compile failure disables preview update and toasts, but the editor never crashes — a malformed `CustomWGSL` body is a user error, not an editor bug.

## 5. Master materials and instances

A master material is a graph. An instance is a reference to a master plus a parameter override table — nothing else.

```rust
pub struct MaterialInstance {
    pub master:    AssetGuid,
    pub overrides: HashMap<ParamId, PortValue>,
}
```

Changing the master's topology propagates to every instance. Changing an instance's override only affects that instance. Instances do not have nodes, edges, or a WGSL body; they share the master's compiled pipeline and just feed a different uniform buffer. This is what keeps the permutation count finite.

Parameters are declared by right-clicking a node and choosing "Expose as parameter." The exposed value becomes an entry in a flat parameter table shown at the graph root; instances can override any entry.

### 5.1 Runtime-scriptable

Exposed parameters are reflected (Phase 2 §2.2) onto a synthetic component type derived from the master material:

```rust
#[derive(Reflect)]
pub struct MaterialParams_Brick {
    pub tint:       Color,
    pub roughness:  f32,
    pub uv_scale:   Vec2,
}
```

Scripts (WASM or native) mutate this like any other component; the material system writes the uniform buffer next frame. No special API — reflection already handles it.

## 6. Preview and live update

Reuse Phase 8's material preview viewport verbatim. The preview scene binds the compiled pipeline and uniform buffer produced by the graph compile; on successful recompile, swap them atomically.

Recompile policy: **debounced on edit release**, not on every keystroke. Node drag end, slider drag-stop, edge connect, edge delete — these trigger a compile. Dragging a node in progress, or sliding a constant value, does not. The same `drag_stopped()` rule Phase 8 §2.3 uses for slider coalescing applies here.

Failed compile keeps the previous pipeline bound — the preview continues to show the last working state with a red banner ("Compile failed — previous material still in use").

## 7. Subgraphs / `.rmatfunc` assets

Selecting a set of nodes and choosing "Collapse to Function" extracts them into a `.rmatfunc` asset with declared input and output ports. The original graph keeps a single function node that references the asset.

```
┌─ GraphEditor: brick.rmat ────────┐   ┌─ Function: triplanar.rmatfunc ─┐
│                                  │   │                                │
│   [UV0] ──▶ [Triplanar] ──▶ ...  │   │  [WorldPos] ──▶ ... ──▶ [Out]  │
│                │                 │   │  [WorldNorm] ──▶              │
└────────────────┼─────────────────┘   └────────────────────────────────┘
                 │
                 ▼ (references)
       triplanar.rmatfunc
```

Function assets are themselves master-style graphs with no Master node, a `FunctionInputs` node, and a `FunctionOutputs` node. They compile to a WGSL `fn`. The graph compiler inlines them at call sites (simpler than managing WGSL function imports across hash boundaries). Recursion is forbidden — detected and rejected at save time.

## 8. Plugin-authored nodes

Phase 11's extension system gets one new extension point:

```rust
pub trait MaterialNodeDef: Send + Sync + 'static {
    fn id(&self) -> &'static str;          // "com.acme.ocean.wave"
    fn display_name(&self) -> &str;
    fn category(&self) -> &str;
    fn inputs(&self) -> &[PortDef];
    fn outputs(&self) -> &[PortDef];
    fn emit(&self, args: &[Ident], ctx: &mut EmitCtx) -> Result<Ident, EmitError>;
}

// in the plugin:
ctx.register_material_node(Box::new(WaveNode));
```

Plugins append to the palette under their declared category. Their `emit` method writes the WGSL snippet; the host enforces that bindings declared by the plugin node only come from ports the plugin declared. A plugin cannot reach past its node boundary into the engine's global bind groups.

Plugins cannot register node types that allocate bind-group slots outside the material's reserved range (group 2, Phase 8 convention). This keeps the renderer's binding layout stable.

## 9. Feature-bit shader permutations

A single graph produces multiple pipelines depending on how it is used: skinned vs static, forward vs deferred, with/without motion vectors, with/without tessellation. These are compile-time flags, not runtime branches.

```rust
bitflags! {
    pub struct MaterialFeatures: u32 {
        const SKINNED            = 1 << 0;
        const DEFERRED           = 1 << 1;
        const MOTION_VECTORS     = 1 << 2;
        const SHADOW_PASS        = 1 << 3;
        const DOUBLE_SIDED       = 1 << 4;
        // ... cap at 32 bits
    }
}
```

Each unique `(graph_hash, features)` pair gets its own WGSL compile and wgpu pipeline. The combination count grows multiplicatively, and with 32 bits the worst case is 4 billion — obviously unusable. Two mitigations:

1. **Lazy compile** — pipelines are built on first use, not upfront. Typical scenes exercise a handful of combinations.
2. **Hard cap at 32 permutations per material** — if the Nth unique `(graph, features)` for one material arrives past the cap, log an error and fall back to the most-recently-used pipeline. 32 is enough to cover skinned/static × forward/deferred × shadow/no-shadow × motion/no-motion × double-sided/single-sided with room to spare.

The cap is per-material, not global. A project with 500 materials can still allocate up to 16,000 pipelines in the worst case, but each material's contribution is bounded.

## 10. Migration from `.rmat` property materials

Existing property-sheet materials from Phase 8 must continue to open without user action. The loader detects the v1 property format and synthesizes an equivalent graph on first open:

```
Albedo = Color(0.8, 0.2, 0.1)       ──▶   [Color] ──▶ Master.Albedo
Metallic = 0.3                      ──▶   [Float] ──▶ Master.Metallic
Roughness = Texture(scratched.png)  ──▶   [Texture2D + Sampler + SampleTexture2D] ──▶ Master.Roughness
```

The synthesized graph saves back as v2 `.rmat`; the v1 file is kept as `.rmat.bak` for one editor session in case the user wants to revert. Opening a v1 `.rmat` in a post-Phase-20 editor is a one-way door on save, and the save dialog says so.

Property-sheet fallback UI stays available for simple materials — the graph editor has a "Simplify to property sheet" action that works if and only if every input is a constant, color, or single-texture node. This is the exit ramp for users who opened the graph by accident and want the old view back.

## 11. Build order within Phase 20

Each step is independently shippable, and the order is chosen so that the riskiest extraction (shared widget) lands first and reveals integration issues before they multiply.

1. **Extract `rustforge-node-graph` crate** — pan/zoom/select/route/minimap on a generic `NodeDomain` trait. Ship with a `examples/toy-graph` binary that drives it with a dummy domain. This is the load-bearing abstraction; do it first so §2+ build on solid ground.
2. **Material graph model + serialize** — `MaterialGraph`, `MaterialNode`, `.rmat` v2 format. No UI yet; round-trip tests only.
3. **WGSL codegen** — topo sort, DCE, emit, naga parse, cache. No editor integration yet; drive from a unit test that emits WGSL for a handful of hand-built graphs.
4. **Source-mapped errors** — span tracking through emission, error routing back to `NodeId`.
5. **Material graph editor** — bind the shared widget to the material domain; implement the v1 palette; wire edits through the Phase 6 command stack.
6. **Master / instance split** — parameter exposure, instance override table, synthetic reflected component for exposed params.
7. **Preview integration** — swap compiled pipeline into Phase 8's material preview; debounced recompile on release.
8. **Feature-bit permutations** — the 32-cap and lazy compile.
9. **Plugin-authored nodes** — Phase 11 extension point, palette integration, binding-slot sandbox.
10. **Subgraphs / `.rmatfunc`** — extract/inline, recursion check.
11. **Migration from v1 `.rmat`** — property-to-graph synthesizer, `.rmat.bak` safety, "Simplify to property sheet" reverse action.

## 12. Scope boundaries — what's NOT in Phase 20

- ❌ **Post-processing shader graph.** Deferred to Phase 21 — post stack has a different port set (screen textures, multi-pass history) and a different compile contract.
- ❌ **Compute shader authoring.** Material graphs compile to fragment WGSL only. GPGPU needs its own graph domain and is not attempted here.
- ❌ **HLSL / GLSL export.** WGSL is the only backend. Naga can translate, but productionizing cross-backend export would double the test matrix for no Phase 20 user gain.
- ❌ **Ray-tracing shaders.** RT needs hit/miss/any-hit shader stages that do not fit the fragment-centric master node shape.
- ❌ **Visual debugger for WGSL.** Step-through, variable inspection, live probe — not now. Compile errors and the preview viewport are the debugging surface for Phase 20.
- ❌ **Shader variants beyond feature bits.** No material-instance-level topology variants; override parameters only.
- ❌ **Procedural mesh generation from the graph.** Geometry shader / mesh shader surface is its own phase.
- ❌ **Graph diffing / merge UI for version control.** Text diff of `.rmat` JSON is the Phase 20 answer; three-way graph merge is a research problem.

## 13. Risks & gotchas

- **Widget crate API churn during Phase 17/18 intake.** Audio and VFX will need ports of types material never imagined (ring-buffers for audio, emitter streams for VFX). Pin `rustforge-node-graph` at 0.x until all three domains have shipped once, then freeze. A v0 crate consumed by first-party code has fewer compat obligations than a published one.
- **Permutation explosion hitting the 32-cap in real projects.** A vegetation material needs skinned + wind + shadow + motion — already 4 bits, 16 permutations. Add a LOD bit and a per-biome variant and users run out. Mitigation: make the cap adjustable in project settings with a loud warning past 32, and profile before raising it.
- **Custom WGSL nodes + binding collisions.** Two `CustomWGSL` nodes declaring `@binding(0)` in their bodies. Rewrite their bindings during emit using the SSA rename mechanism; never trust user-written `@group`/`@binding` in a snippet. Document this rewrite in the node's help text.
- **Subgraph inlining cost.** A `.rmatfunc` used 200 times in a graph inlines 200 copies of the WGSL. Naga is fast but not infinite. Measure; if compile time becomes visible, emit a real WGSL function and call it. Defer this optimization until a user hits it.
- **Cache invalidation on plugin updates.** A plugin node's `emit` changes → existing cached shaders for graphs using that node are stale but still hash-match on graph content. Include plugin-node-impl hashes in the cache key, not just the graph content.
- **Preview recompile thrash during multi-edit commands.** Ctrl+Z undoes a ten-node paste → ten removal events → naively, ten recompiles. Coalesce recompile requests to one per frame; the debounce-on-release rule already covers typical editing, but command-stack paths need the same gate.
- **Feature-bit set drift between material and renderer.** Renderer adds a new pass that needs a new feature bit → every material's permutation set changes. Treat `MaterialFeatures` as part of the shader cache key. Old caches invalidate. Document the cache bust on renderer changes.
- **Master node accidentally deleted.** Delete-selection with the Master in it. Guard at the widget layer via a `NodeKind::is_removable()` hook on the domain trait; the shared crate respects it.
- **Instance overrides referencing a removed parameter.** Master material loses a parameter; instances still have an entry for it. Prune on load with a warning toast naming the parameter and instance; never silently drop a user's override without telling them.
- **Reflection drift on exposed params.** Renaming an exposed parameter breaks scripts that reference it by name. Give parameters a stable `ParamId` (uuid) separate from their display name; scripts bind by id via a generated accessor, renames are cosmetic.
- **`CustomWGSL` as a malware vector.** WGSL cannot escape the GPU sandbox, but a snippet that infinite-loops in the fragment stage hangs the GPU. wgpu's device-lost recovery handles this, but the editor must survive — catch device-lost during preview compile and fall back to the previous pipeline instead of crashing.
- **Widget minimap re-rendering on every pan.** A large graph (500 nodes) re-rasterizing its minimap every frame will stutter. Cache the minimap texture; invalidate only on node add/remove/move.

## 14. Exit criteria

Phase 20 is done when all of these are true:

- [ ] `rustforge-node-graph` crate compiles standalone, is consumed by the material editor, and has a toy-domain example demonstrating pan / zoom / multi-select / box-select / connection routing / minimap.
- [ ] A material graph can be authored end-to-end: drop nodes from the palette, connect ports, watch the Phase 8 preview viewport update on drag release.
- [ ] The v1 palette (§3) is fully implemented and each node has at least one emit test covering the WGSL it produces.
- [ ] Topological sort detects cycles and surfaces them as per-node errors; DCE marks unreachable nodes faded.
- [ ] Hash-keyed shader cache hits on repeat opens; cold-cache and warm-cache compile times are both logged.
- [ ] A malformed `CustomWGSL` body produces a red badge on the owning node with the naga error message, and the editor does not crash.
- [ ] Master materials and instances coexist: editing the master topology propagates to instances; editing an instance override does not.
- [ ] Exposed parameters appear as reflected fields on a synthetic component type and are scriptable from WASM.
- [ ] Subgraph extract + inline round-trip preserves the generated WGSL byte-for-byte (after SSA renaming).
- [ ] Recursive `.rmatfunc` references are rejected at save with a clear error.
- [ ] A Phase 11 plugin can register a material node; it appears in the palette, participates in codegen, and its bind-group usage stays within the material's reserved range.
- [ ] Feature bits compile independently; the per-material 32-permutation cap logs an error and does not crash when exceeded.
- [ ] Opening a v1 `.rmat` property material produces an equivalent graph that compiles to the same uniform values; `.rmat.bak` is written once per session.
- [ ] "Simplify to property sheet" reverse action works for any graph whose inputs are all constants/colors/single-textures.
- [ ] Undo/redo (Ctrl+Z / Ctrl+Y) on every graph operation routes through the Phase 6 command stack with no special-casing inside `rustforge-node-graph`.
- [ ] `rustforge-core` still builds and runs without the `editor` feature; the node-graph crate and editor-side graph UI are absent from shipped games, but the compiled pipelines and parameter buffers are loaded at runtime.
