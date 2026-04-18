# Phase 49 — Visual Scripting (Blender-inspired)

Every previous phase, at least forty times across the plan, has a line like *"RustForge does not ship Unreal Blueprints-style visual scripting."* That sentence is imprecise, and a late-arc phase is the right place to correct it. RustForge already ships Blender-style **data-flow** visual scripting in eight domains: materials (Phase 20), audio routing (Phase 17), VFX (Phase 18), PCG (Phase 40), dialogue (Phase 44), quests (Phase 44), behavior trees (Phase 26), and the animation state graph (Phase 23). All of them reuse the same `rustforge-node-graph` crate from Phase 20. What the plan rejected — and still rejects — is a separate, imperative, control-flow visual-programming system positioned as *the* authoring surface for gameplay logic in place of code. Blender itself never shipped that, and neither do we.

Phase 49 adds a general-purpose **event-wiring and prototyping tool**: Scratch meets Blender Geometry Nodes, not Unreal Blueprints. A visual script compiles to the same WASM runtime Phase 7 hot-reloads, runs under the same Phase 11 capability sandbox, and uses the same Phase 20 node-graph widget as every other graph in the engine. It is a sibling of hand-written WASM, not a replacement. Designers wire "gate opens when lever pulled" without touching Rust; programmers keep authoring systemic code in Rust; the two interoperate through typed function calls in both directions. When a visual script outgrows the tool, an **Eject to Rust** button writes a WASM-ready Rust skeleton equivalent to the current graph, and the script continues life as code.

## Goals

By end of Phase 49:

1. **`.rvscript` asset format** — RON node graph, round-trip stable, reuses the Phase 20 `rustforge-node-graph` DAG model.
2. **Event + function graphs** — entry nodes from engine events; reusable pure/effectful function subgraphs callable from other scripts.
3. **Node palette** covering event entry, control flow (bounded), variables, data ops, integration with other node-graph domains, ECS ops, and WASM-script interop.
4. **Typed pins** identical to Phase 20's type system; authoring-time type errors with red badges.
5. **Graph → IR → WASM compilation** — incremental per-subgraph, hot-reloaded at end-of-frame via the Phase 7 swap.
6. **Capability-sandboxed** — a compiled visual script is indistinguishable from a Phase 7/11 WASM module at runtime.
7. **Bounded execution** — no unbounded loops; `WhileN` and `ForEach` only; per-tick instruction budget enforced.
8. **Live debugger** — step-through, pin watches, node breakpoints, PIE trace recording with reverse-scrub.
9. **`VisualScript` component** — attach N scripts per entity, configurable execution order.
10. **BT task bridge** — any `.rvscript` function graph can serve as a Phase 26 BT leaf task.
11. **Plugin-authored nodes** — Phase 11 plugins extend the palette, same extension shape as Phase 20 materials.
12. **Eject to Rust** — one-way codegen from a visual script to a WASM Rust source skeleton.
13. **Accessibility** — keyboard graph navigation and screen-reader labels via Phase 13 AccessKit.

## 1. Positioning, honestly

Role, in one line: **a reactive event-wiring layer, not an authoring surface for full game logic.**

In scope: event wiring ("gate opens when lever pulled", "enemy spawns when player enters trigger"); prototyping game-loop reactions before writing Rust; designer-authored overlays on systemic Rust code; Phase 38 tutorial hookup glue.

Out of scope: primary authoring surface for a shipping commercial game; Blueprints-parity (we curate the palette, not expose every subsystem); replacement for Rust knowledge; low-level rendering or shader authoring (Phase 20 owns materials); IDE features like refactor or find-all-references.

The palette-curation test, in one line: "would a Blender user expect this node to exist in a Geometry Nodes-style tool?" If yes, it can ship. If only Blueprints has it, it does not.

## 2. Graph model — reusing Phase 20

The model is not new. `rustforge-node-graph` already carries pan/zoom/selection/routing/minimap and a generic `NodeDomain` trait. Phase 49 is one more domain implementation.

```
crates/rustforge-vscript/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── model.rs          # VScriptGraph, VScriptNode, VScriptEdge
    ├── ports.rs          # PinType, PinValue (superset of Phase 20)
    ├── palette.rs        # curated node catalog + plugin registration
    ├── ir.rs             # lowering: graph -> SSA-ish IR
    ├── codegen.rs        # IR -> wasm-encoder module
    ├── compile.rs        # public API: compile_graph(&VScriptGraph) -> Wasm
    ├── runtime.rs        # VisualScript component + instance state
    └── debug.rs          # breakpoints, trace buffer, pin watch hooks

crates/rustforge-vscript-editor/
├── Cargo.toml
└── src/
    ├── panel.rs          # AssetEditor-style tab
    ├── palette_ui.rs     # searchable contextual palette
    ├── compile_log.rs    # diagnostics panel
    ├── stepper.rs        # live single-step harness
    └── eject.rs          # Eject-to-Rust codegen
```

```rust
pub struct VScriptGraph {
    pub nodes: SlotMap<NodeId, VScriptNode>,
    pub edges: Vec<VScriptEdge>,
    pub locals: Vec<Local>,              // script-instance variables
    pub functions: Vec<FunctionGraph>,   // reusable subgraphs
    pub meta: GraphMeta,
}

pub struct VScriptNode {
    pub id:     NodeId,
    pub kind:   NodeKind,
    pub params: NodeParams,
    pub pos:    Vec2,
    pub group:  Option<GroupId>,         // comment-box grouping
}

pub struct VScriptEdge {
    pub from: (NodeId, PinIdx),
    pub to:   (NodeId, PinIdx),
    pub kind: EdgeKind,                  // Data | Exec
}
```

Data-flow sections are pure DAGs. Control-flow uses **explicit exec pins** — a white triangle on the left and right of control-flow nodes. Exec edges carry no value, only "run next". This is the one concession to imperative shape; it is unavoidable if the tool is to express sequencing and branches, and Blender Geometry Nodes recently adopted a similar distinction with its simulation zones.

## 3. Node categories

The palette is the product. A palette with two hundred nodes is a failed product; a palette with thirty well-chosen nodes is a usable one. v1 ships six categories.

### 3.1 Event entry

Entry nodes have no input pins; each one is the root of a separate exec subgraph within the script.

| Node | Fires on |
|---|---|
| `OnSpawn`         | entity spawned |
| `OnDespawn`       | entity despawning |
| `OnTick`          | every tick (rate configurable) |
| `OnInputAction`   | Phase 16 action fires |
| `OnCollision`     | Phase 22 contact enter/stay/exit |
| `OnDialogEvent`   | Phase 44 custom event fires |
| `OnQuestStep`     | Phase 44 quest transition |
| `OnCustomEvent`   | named event from Rust or other scripts |

### 3.2 Control flow (all bounded)

`Branch`, `Switch(enum)`, `Sequence{N}`, `Select`, `ForEach(iter)`, `WhileN(maxN, cond)`, `Wait(duration)`, `Timer(interval, maxFires)`.

There is no `While(true)` node. The runtime also enforces a per-tick instruction budget (default 50 000 Wasm instructions per `OnTick` invocation per script); exceeding it raises a `BudgetExceeded` diagnostic and halts that script's tick without killing others. Rationale is stated in the UI: **a visual-script author writing an infinite loop must see an error, not a frozen editor.**

### 3.3 Variables

Typed locals (script-instance lifetime) and component-field get/set via Phase 2 reflection. The palette offers `Get<T>(component, field)` / `Set<T>(component, field, value)`; if reflection says the field does not exist or type mismatches, the node is authoring-time invalid.

### 3.4 Data ops

Math (`Add`, `Sub`, `Mul`, `Div`, `Min`, `Max`, `Clamp`, `Lerp`, `Abs`, `Sqrt`), comparison, logical (`And`, `Or`, `Not`), string (`Concat`, `Format`, `Eq`), vector (`Vec3::new`, `Dot`, `Cross`, `Normalize`, `Length`). The set deliberately matches the Phase 20 palette name-for-name so a user who has authored a material recognizes the nodes.

### 3.5 Integration (call into other domains)

- `TriggerPcgGraph(asset, inputs)` — Phase 40
- `PlayDialogue(asset, actor)` — Phase 44
- `PlayAnimClip(asset, blend)` — Phase 23
- `PlayAudio(asset, bus)` — Phase 17
- `ApplyMaterial(entity, asset)` — Phase 20
- `RunEqsQuery(asset) -> results` — Phase 26

### 3.6 ECS ops

`SpawnPrefab`, `Despawn`, `GetComponent<T>`, `SetComponent<T>`, `HasComponent<T>`, `Query<(A, B)>` (returns an iterator for `ForEach`).

### 3.7 Script interop

- `CallWasmFn(path, args)` — call a user's hand-written WASM function as a node. Signature is reflected from the exported fn.
- `ReceiveCallback(name)` — entry node that fires when Rust-side code raises a named callback against this script instance.

These two nodes are how a visual script cooperates with Rust instead of competing with it.

## 4. Typed pins

Pin types are a superset of Phase 20's: `Bool`, `Int`, `Float`, `Vec2`, `Vec3`, `Vec4`, `String`, `Entity`, `AssetRef<T>`, `Enum<T>`, `Struct<T>` (reflection-derived), `List<T>`, plus exec. Implicit conversions exist only where lossless (`Int -> Float`, `Vec3 -> Vec4` zero-extended). Anything else requires an explicit cast node.

```rust
pub enum PinType {
    Exec,
    Bool, Int, Float,
    Vec2, Vec3, Vec4,
    String,
    Entity,
    Asset(TypeId),
    Enum(TypeId),
    Struct(TypeId),
    List(Box<PinType>),
}

pub fn is_assignable(src: &PinType, dst: &PinType) -> bool { /* ... */ }
```

Connection attempts between incompatible pins are rejected by the widget and show a transient red pulse. Already-stored edges that become invalid after a refactor (e.g., a field type changed via reflection) are marked with a red error badge on the node; compilation fails until the author resolves them.

## 5. Lowering — graph to IR to WASM

The compile pipeline is three stages. Nothing novel.

```
VScriptGraph ──lower──▶ VScriptIR ──codegen──▶ wasm bytes ──▶ wasmtime module
```

### 5.1 IR shape

IR is SSA-ish, per event entry and per function graph:

```rust
pub struct IrFunction {
    pub name:    String,
    pub params:  Vec<IrType>,
    pub ret:     Option<IrType>,
    pub locals:  Vec<IrType>,
    pub blocks:  Vec<IrBlock>,            // control-flow blocks
}

pub struct IrBlock {
    pub id:   BlockId,
    pub ops:  Vec<IrOp>,
    pub term: IrTerminator,               // Br, BrIf, Switch, Return
}

pub enum IrOp {
    Const(IrValue),
    BinOp(BinOp, Value, Value),
    Call(FnRef, Vec<Value>),
    HostCall(HostFn, Vec<Value>),         // engine API, gated by capabilities
    LocalGet(LocalIdx),
    LocalSet(LocalIdx, Value),
    Phi(Vec<(BlockId, Value)>),
}
```

Data-flow DAG subgraphs lower to topologically sorted `IrOp` sequences. Exec-flow nodes lower to block terminators. `WhileN` lowers to `loop + br_if` with an explicit counter compared against `maxN`. `Wait(d)` lowers to a suspend point — the script is a state machine whose resume index lives in a script-instance local, and the scheduler re-enters it when the timer elapses. No threads, no fibers, just a `match` on `resume_at`.

### 5.2 Codegen

`wasm-encoder` writes one module per script. Host imports match the Phase 7 script ABI exactly — the visual script cannot call anything a hand-written script cannot. Bounds, types, and capability checks all funnel through the same host shims.

```rust
pub fn compile_graph(
    graph: &VScriptGraph,
    caps: &Phase11Capabilities,
) -> Result<CompiledScript, CompileError> {
    let ir = lower::to_ir(graph)?;
    typecheck::verify(&ir, caps)?;
    let bytes = codegen::emit_wasm(&ir)?;
    Ok(CompiledScript { bytes, source_map: codegen::build_map(&ir) })
}
```

Compilation is **incremental**: each function graph and each event entry is a compilation unit. A `.rvscript` edit recomputes only the units whose node/edge set changed; untouched units are copied from the previous module. Typical latency target: under 30 ms for an event subgraph, under 100 ms for a full script.

### 5.3 Hot reload

The output `CompiledScript` is handed to Phase 7's end-of-frame swap, the same path hand-written WASM uses. Script-instance state (locals, resume index) is preserved via the same snapshot mechanism if the function signatures did not change; otherwise the instance restarts. The author sees no difference between editing a `.rs` script and editing a `.rvscript`.

## 6. ASCII editor mockup

```
┌──────────────────────────────── Visual Script Editor ────────────────────────────────┐
│ File  Edit  View  Debug                              [Compile ✓]  [Play ▶]  [Step ⏭] │
├──────────────┬───────────────────────────────────────────────────────────┬───────────┤
│  Palette     │                        Graph Canvas                      │  Details  │
│              │                                                           │           │
│  Search: [__]│  ╔══════════╗        ╔═══════════╗        ╔═══════════╗  │ Selected: │
│              │  ║OnCollis  ║─▶── ──▶║  Branch   ║─T─▶ ──▶║SpawnPrefab║  │ Branch    │
│  ▼ Events    │  ║ (Player) ║        ║           ║        ║ (Pickup)  ║  │           │
│   OnTick     │  ╚═════╤════╝        ╚══════╤════╝        ╚═══════════╝  │ Condition:│
│   OnSpawn    │        │Other              │F                            │  [pin]    │
│   OnCollision│        ▼                    └─▶──╔═══════════╗           │           │
│   OnInputAct │  ╔══════════╗                    ║ PlayAudio ║           │ ExecOut T:│
│              │  ║HasComp   ║                    ║ (bump.ogg)║           │  1 wire   │
│  ▼ Control   │  ║ (Pickup) ║◀── cond            ╚═══════════╝           │ ExecOut F:│
│   Branch     │  ╚══════════╝                                             │  1 wire   │
│   Sequence   │                                                           │           │
│   ForEach    │  [watch: cond = true]   [breakpoint ● on Branch]          │           │
│   WhileN     │                                                           │           │
│   Wait       │  ── exec pin ▷   ── data pin ○   ── current frame: node 4 │           │
│              │                                                           │           │
│  ▼ Data      │                                                           │           │
│   Add        │                                                           │           │
│   Lerp       │                                                           │           │
│   Vec3       │                                                           │           │
│   ...        │                                                           │           │
├──────────────┴───────────────────────────────────────────────────────────┴───────────┤
│ Compile Log:  OK  3 events, 2 functions, 412 bytes wasm, incr=18ms                   │
│ Trace:        [◀◀] frame 41/60  Branch(cond=true) → SpawnPrefab(Pickup#12)  [▶▶]     │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

The palette on the left is **contextual**: if the entity in the outliner has no `Rigidbody`, `OnCollision` is greyed with a tooltip explaining why. Search is fuzzy; recently-used nodes float to the top.

## 7. Debugging

The debugger is the other half of the product. A visual script you cannot observe running is worse than Rust — at least Rust has `dbg!`.

Four tools ship. **Single-step** — with PIE paused, `Step` advances one node fire; active node is highlighted yellow, pending blue. **Pin watch** — hover any pin at runtime to see its last flowed value; watches can be pinned as floating labels. **Node breakpoints** — right-click to set; the next fire auto-pauses PIE and jumps the canvas. **Trace and scrub** — a per-script circular buffer records the last N seconds (default 10 s at 60 Hz, ≈40 KB); the transport bar scrubs backwards, replaying pin values and node highlights. This last feature is the one most likely to convert skeptical designers.

```rust
pub struct TraceRecord {
    pub frame:  u64,
    pub node:   NodeId,
    pub pins_in:  SmallVec<[(PinIdx, PinValue); 4]>,
    pub pins_out: SmallVec<[(PinIdx, PinValue); 4]>,
}
```

## 8. Attachment — the `VisualScript` component

```rust
#[derive(Component, Reflect)]
pub struct VisualScript {
    pub asset: AssetRef<VScriptAsset>,
    pub order: i16,                  // tie-break among multiple scripts on one entity
    pub enabled: bool,
}
```

An entity can carry any number of `VisualScript` components. Execution order within a single tick is `order` ascending, then asset GUID as tiebreak. Scripts communicate through the custom-event bus and through ECS component reads/writes; they are not directly connected to each other.

Instance state (locals + resume indices) lives in a separate `VisualScriptState` component, reflection-serialized like any other component, so PIE snapshot/restore (Phase 7) and save files (Phase 4) pick it up without special casing.

## 9. Function graphs and reuse

A `.rvscript` is not only an event-handler graph; it also contains a list of **function graphs**. Each function graph is pure-by-default (no exec pins unless it declares side effects), takes typed inputs, returns typed outputs, and is callable from any other visual script or from Rust.

```rust
pub struct FunctionGraph {
    pub name:   String,
    pub params: Vec<(String, PinType)>,
    pub ret:    Option<PinType>,
    pub effectful: bool,             // if true, gains exec pins
    pub graph:  VScriptGraph,
}
```

This is how the palette grows without bloating the engine. A project builds its own macro-nodes — "compute damage with resistances", "find nearest enemy within cone" — and those show up in the palette of every script in the project under a `Project` category. It is also how Phase 26 consumes visual scripts: a BT leaf task references a function graph by name.

## 10. BT integration

A behavior tree leaf node of kind `VScriptTask` holds an `AssetRef<VScriptAsset>` and a function-graph name. When the BT ticks it, the function graph runs; its return value (or a convention `BtStatus` out-pin) maps to `Success / Failure / Running`. Phase 26's BT editor gets a new palette entry under `Tasks → Custom → Visual Script`.

This closes the loop the two phases have been circling around. BT handles high-level decision-making; visual scripts handle low-level reactive logic. Between them they cover everything Unreal's AIModule + Blueprints covers for NPC behavior, with no Blueprints-class surface.

## 11. Plugin-authored nodes

Phase 11 already defines the plugin ABI for nodes (Phase 20 materials use it). Phase 49 registers the same shape for vscript nodes:

```rust
pub trait VScriptNodeProvider: Send + Sync {
    fn ident(&self) -> NodeIdent;
    fn category(&self) -> &'static str;
    fn pins(&self) -> PinSignature;
    fn compile(&self, inputs: &[IrValue], ctx: &mut IrCtx) -> Vec<IrValue>;
}
```

Plugins cannot add control-flow shapes (no custom exec semantics); they can add data-ops, host-side side effects subject to capability grants, and wrappers around plugin-exposed APIs. The palette shows plugin-authored nodes under a per-plugin category with a small plugin badge.

## 12. Eject to Rust

Every visual-scripting tool eventually hits the ceiling. Phase 49's explicit answer to "what then" is a one-way export. The button **Eject to Rust** opens a save dialog; the output is a Rust source file (WASM-compilable) whose public functions mirror the script's event entries and function graphs, whose control flow matches the graph's structure, and whose comments annotate each block with the originating node IDs.

```rust
// generated from Gate.rvscript @ 2026-04-16 — do not round-trip
#[no_mangle] pub extern "C" fn on_collision(other: Entity) {
    // node 7 — HasComponent<Pickup>
    if has_component::<Pickup>(other) {
        // node 11 — SpawnPrefab(Pickup)
        spawn_prefab(PICKUP_PREFAB, transform_of(other));
    } else {
        // node 13 — PlayAudio(bump.ogg)
        play_audio(BUMP_SFX, AudioBus::Sfx);
    }
}
```

Eject is **one-way by design**. A round-trip Rust→graph parser is a project of its own and not one we take on. The ejected file becomes the new source of truth; the original `.rvscript` is archived with a `.ejected` suffix.

## 13. Accessibility

Keyboard-first authoring is not optional for a node graph in 2026.

- Tab / Shift-Tab cycles focus across nodes in topological order.
- Enter opens the palette centered on focus; typing fuzz-searches; Enter inserts.
- Alt+Left/Right navigates along the exec chain; Alt+Up/Down along data edges.
- Space + direction starts a connection from the focused pin; Space again commits.
- All nodes carry AccessKit labels (Phase 13) with type, category, and current value on watched pins. Screen readers announce "OnCollision node, exec out connected to Branch, 2 inputs".

## 14. Performance

Visual scripts run the same wasmtime path as hand-written WASM. There is no interpreter, no tree-walker. The only overhead relative to hand-written Rust-compiled-to-WASM is codegen inefficiency: a naive graph author produces naive IR, and our lowering does not aggressively rewrite it. Specifically, a data-flow DAG that reuses a value across three nodes emits the computation three times unless the author extracts a local — there is no CSE pass in v1.

Target: a 50-node event graph ticking at 60 Hz should consume less than 0.1 ms per invocation on a 2021 mid-range desktop. Phase 42's profiler gains a **Visual Scripts** panel showing per-script tick time, per-event-entry breakdown, and hot-node counts. If a script is a frame-time problem, this panel names it.

## 15. Build order

1. `rustforge-vscript` crate skeleton + reuse Phase 20 `rustforge-node-graph` with a vscript `NodeDomain` impl.
2. Node categories 3.1–3.4 (event entry, control flow, variables, data ops).
3. Pin type system + authoring-time typechecker.
4. IR shape + lowering for DAG subgraphs and linear exec chains.
5. Codegen via `wasm-encoder`, module emission, host-import wiring to Phase 7.
6. Hot-reload glue through Phase 7's end-of-frame swap.
7. `VisualScript` + `VisualScriptState` components + scheduler with per-tick budget.
8. `rustforge-vscript-editor` panel (AssetEditor tab) with canvas, palette, compile log.
9. Debugging layer: single-step, pin watch, node breakpoints.
10. Trace buffer + scrub UI.
11. Function graphs + per-project palette category.
12. Integration nodes (category 3.5) for Phases 17/18/20/23/26/40/44.
13. Phase 11 plugin registration for custom nodes.
14. BT `VScriptTask` leaf node + Phase 26 editor palette entry.
15. Eject-to-Rust codegen.
16. AccessKit labels + keyboard navigation.
17. Phase 42 profiler panel.

## 16. Scope ❌

- ❌ Unreal-Blueprints-parity authoring surface.
- ❌ Visual scripting as a replacement for Rust/WASM hand-written scripts.
- ❌ Visual authoring of engine-level rendering, culling, or memory-management code.
- ❌ Visual shader/material authoring (Phase 20 owns that; the `rvscript` palette does not expose shader nodes).
- ❌ Live collaborative editing of visual scripts beyond what Phase 37 provides for assets.
- ❌ AI-generated visual scripts (adjacent to Phase 39; not owned by Phase 49).
- ❌ Round-trip Rust→graph parsing (Eject is one-way).
- ❌ JetBrains-class IDE features: refactor-rename-across-projects, find-all-references over a mixed Rust/vscript corpus, autocomplete over Rust symbols.
- ❌ Console platform backends (still blocked by wgpu).

## 17. Risks

- **Scope creep toward Blueprints.** Every user request will ask for "just one more node" until the palette is Blueprints. Mitigation: palette changes go through the same RFC review as public API; explicit "this would turn us into Blueprints" veto.
- **Performance cliff from naive graphs.** Users will author 500-node `OnTick` graphs and call them slow. Mitigation: instruction budget surfaces the problem loudly; profiler panel names the script; Eject-to-Rust is the upgrade path.
- **Capability-sandbox evasion.** A visual script feels safer than a WASM blob, so users may grant it more capabilities. Mitigation: capability prompts are identical in phrasing and scariness regardless of source; the runtime cannot tell the two apart.
- **Debugger complexity.** Trace scrub is the feature with the highest bug surface area. Mitigation: record-only fields are simple structs; the scrub UI is read-only; the live PIE session is authoritative.
- **Eject drift.** Users ship with partially-ejected scripts and expect the archived `.rvscript` to be current. Mitigation: archived file carries an editor banner "ejected on DATE, Rust is source of truth" and opens read-only.
- **Plugin-node safety.** Malformed plugin nodes can produce IR that miscompiles. Mitigation: plugin nodes run through the same typechecker and IR verifier as built-ins; failures show as red badges naming the plugin.

## 18. Exit criteria

1. A designer with no Rust experience authors the "lever opens gate, plays sound, advances quest" loop end-to-end in under ten minutes, including attaching the script to an entity.
2. The authored script hot-reloads on save during PIE with preserved script-instance state (where signatures are unchanged).
3. Capability sandbox denies a visual-script attempt to reach outside granted filesystem scope, identically to a hand-written WASM module.
4. `WhileN(1_000_000, true)` fires a budget-exceeded diagnostic within one tick, does not hang the editor, and names the offending node.
5. A `.rvscript` function graph serves as a Phase 26 BT leaf task in the sample AI project and returns `Success`/`Failure`/`Running` correctly.
6. Trace scrub replays the last 10 seconds of a 60 Hz script with pin values matching the live run.
7. Eject-to-Rust produces a compiling WASM Rust source for the five sample scripts; output runs identically to the graph.
8. A plugin from Phase 11's sample set registers a custom vscript node, it appears in the palette with a plugin badge, and compiles into a script.
9. AccessKit-driven screen-reader walk of a ten-node graph names every node, pin, and connection without visual inspection.
10. Phase 42 profiler's Visual Scripts panel shows per-script tick cost and a hot-node breakdown; disabling a script removes it from the report.
11. Documentation includes the honest re-framing: RustForge shipped Blender-style data-flow visual scripting from Phase 20 onward, and Phase 49 adds event wiring on top — not Blueprints, and explicitly not the primary authoring surface for a shipping game.
