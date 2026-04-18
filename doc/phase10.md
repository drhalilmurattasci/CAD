# Phase 10 — Debugging & Diagnostics

Phase 8's profiler answered "how fast?" — frame times, per-system CPU, GPU pass budgets. Phase 9 answered "how do I ship it?" — build and packaging. Phase 10 answers the question users ask after both of those are in place and the game still isn't behaving: **why?** When a script panics, a collider tunnels, a draw call vanishes, or a component holds the wrong value, the profiler will not help. This phase is the diagnostic toolbox users reach for.

The design principle is the same one Phase 8 adopted for the profiler: lean on the `editor` feature flag, keep per-frame cost zero in shipped builds, and route everything user-visible through dockable panels that compose with the existing layout. Nothing in Phase 10 is a new engine subsystem — it is mostly *introspection of subsystems that already exist*, plus a small amount of render-side debug visualization.

## Goals

By end of Phase 10:

1. **Structured logging** — a `tracing`-backed facade in `rustforge-core`, filterable and searchable in a dedicated log panel with session persistence.
2. **Console panel** — registered commands (`ConsoleRegistry`) invoked from a REPL-style input; systems register their own commands at startup.
3. **Entity/Component debugger** — per-entity component list with type IDs, system-access map, and live numeric value graphs.
4. **Script tracepoints** — a `trace!(...)` macro in WASM scripts that emits log entries with entity context; panics surface as clickable rows that jump to source via the Phase 8 §5 external-editor launcher.
5. **Frame debugger** — single-frame capture of draw calls, pipeline state, and bound resources, with a documented fall-through to RenderDoc for deeper inspection.
6. **Physics timeline rewind** — bounded ring of physics snapshots (default 300 frames) scrubbable during Pause.
7. **Memory view** — asset cache totals, GPU buffer totals, entity/component counts. Not a heap profiler.
8. **Panic crash dump** — automatic capture of scene path, active entity, running script, and last N log lines, written to disk on panic.
9. **Diagnostic overlays** — in-viewport toggles: wireframe, normals, UV checker, lighting complexity, entity bounds.

## 1. Logging — `tracing` facade

`println!` is an accident. It locks stdout, allocates, and carries no structure. Replace it project-wide with a `tracing` facade in `rustforge-core`, then consume the event stream in the editor.

```
crates/rustforge-core/src/diag/
├── mod.rs                  # init(), public macros re-export
├── subscriber.rs           # custom Subscriber: ring buffer + file sink
└── entry.rs                # LogEntry: level, target, span path, ts, fields
```

```rust
pub struct LogEntry {
    pub ts: SystemTime,
    pub level: Level,                // Error | Warn | Info | Debug | Trace
    pub target: &'static str,        // module path
    pub span:   SmallVec<[SpanId; 4]>,
    pub msg:    String,
    pub fields: SmallVec<[(&'static str, FieldValue); 4]>,
    pub entity: Option<Entity>,      // set by script tracepoints (§4)
}
```

### 1.1 Sinks

Two sinks run in parallel:

- **Ring buffer** — last 10 000 entries held in a `parking_lot::Mutex<VecDeque<LogEntry>>`. The log panel reads from this; cheap to scan.
- **Rolling file** — `./.rustforge/logs/session-YYYYMMDD-HHMMSS.log` — JSON-per-line. Rolls at 50 MB, keeps last 10. This is the "persistent log across sessions" requirement — users can open last night's session.

Use a channel (`crossbeam`) between the `Subscriber` and the sinks so logging on the tick thread never blocks on disk. Drop oldest on channel overflow and emit one `warn!` per second indicating drops — *never* silently lose the fact that drops happened.

### 1.2 Panel

```
┌─ Log ─────────────────────────────────────────────────┐
│ [All][Err][Warn][Info][Dbg][Trc]  filter: [player   ] │
│ 12:01:04.113  INFO  scene     loaded  maps/level1     │
│ 12:01:04.240  WARN  physics   penetration depth 0.42  │
│ 12:01:04.441  ERROR script    panic at npc.rs:88  →   │
│ 12:01:04.442  DEBUG audio     voice stolen  id=42     │
│ [Clear]  [Export JSON]  [Pause]  [Follow tail ☑]      │
└───────────────────────────────────────────────────────┘
```

Level toggles are cumulative. Filter string is substring-match against `target || msg || fields`. Search (`Ctrl+F`) pops a find-bar that jumps between matches. The `→` on error rows is the source-jump affordance from Phase 8 §5.2 — Phase 10 doesn't invent it, just wires more events to it.

### 1.3 Zero-cost in shipped builds

The `diag` module compiles, but the editor-only sinks are gated. In a non-`editor` build, `tracing` still works (users may wire their own subscriber), but the ring buffer and log panel are absent. The facade is *not* gated — shipped games log too; they just log through a subscriber the user (not the editor) installs.

## 2. Console

Phase 1 reserved a `console.rs` stub. Phase 10 makes it real.

```
crates/rustforge-core/src/diag/console/
├── mod.rs                  # ConsoleRegistry, Command trait
└── builtins.rs             # scene.reload, entity.spawn, time.scale
```

```rust
pub trait ConsoleCommand: Send + Sync + 'static {
    fn name(&self) -> &'static str;             // "teleport"
    fn help(&self) -> &'static str;
    fn run(&self, args: &[&str], ctx: &mut ConsoleCtx) -> Result<String>;
}

pub struct ConsoleRegistry { /* DashMap<&'static str, Box<dyn ConsoleCommand>> */ }
```

### 2.1 Registration

Commands register in system init, not in the editor crate — systems own their commands. Example from a hypothetical gameplay crate:

```rust
registry.register("teleport", TeleportCmd { /* ... */ });
registry.register("spawn",    SpawnCmd);
```

The editor itself registers editor-only commands (`editor.save`, `editor.tab.close`) in its own crate. In a non-`editor` build those simply aren't registered; `ConsoleRegistry` itself lives in core so a shipped game can still expose commands (useful for dev builds).

### 2.2 Panel

```
┌─ Console ─────────────────────────────────────────────┐
│ > spawn Player at 0 10 0                              │
│ < entity 287 spawned                                   │
│ > time.scale 0.25                                     │
│ < time scale now 0.25                                  │
│ > help                                                │
│ < 37 commands: editor.save entity.spawn ...           │
│ > █                                                   │
└───────────────────────────────────────────────────────┘
```

Tab-complete walks the registry. `↑`/`↓` scroll history persisted to `./.rustforge/console_history` (last 500 lines). A command that mutates world state **must route through Phase 6's command stack** — `entity.spawn` pushes a `SpawnEntityCommand`, not a direct `world.spawn`. Anything else would silently bypass undo and PIE-snapshot restore. Commands that are read-only (`list entities`, `help`) skip the stack.

### 2.3 Cheat-console vs. debug-console

These are the same panel. Lock behind a config flag in shipped games; always open in the editor.

## 3. Entity / Component debugger

The inspector (Phase 2) shows reflected fields. The debugger shows the *shape of the data model* — which components the entity has, which systems touch them, and how a numeric value is moving over time.

```
crates/rustforge-editor/src/panels/entity_debugger.rs
```

### 3.1 Layout

```
┌─ Entity Debugger — entity 287 ("Player") ─────────────┐
│ Components (5):                                       │
│   Transform     TypeId(0x91a3..)   R/W by: Movement,  │
│                                    Render             │
│   Velocity      TypeId(0x3fe1..)   R/W by: Movement   │
│                                    R   by: AI         │
│   Health        TypeId(0x88b2..)   R/W by: Damage     │
│                                                       │
│ Graphed fields: velocity.x  velocity.y  health.hp     │
│   velocity.x  ▂▂▃▅▆▇██▇▆▅▄▃ range [-2.3, 7.8]         │
│   health.hp   ████████▇▆▅▃  range [42.0, 100.0]       │
│                                                       │
│ [+ Graph field…]   [Copy component list]              │
└───────────────────────────────────────────────────────┘
```

### 3.2 Reader/writer map

Systems declare their component access via hecs queries at schedule build time (or at register time for the custom scheduler). At startup the editor walks the schedule graph, indexes `TypeId → {readers, writers}`, and displays it. This only works if the schedule is introspectable — Phase 2's registry does not give it for free. Phase 10 adds a minimal `SystemDescriptor` (name, `TypeId` reads, `TypeId` writes) registered alongside each system. Gate under `editor`.

### 3.3 Live numeric graphs

Right-click any `f32`/`f64`/integer field in the inspector → "Graph this field." The debugger tails that field, keeps the last 240 samples (same horizon as Phase 8 profiler for consistency), and plots. Graphs pause with PIE pause. Implementation is a `Vec<(Instant, f32)>` ring — cheap. Non-numeric fields are not graphable.

## 4. Script debugger (WASM)

Real source-level breakpoint debugging of WASM scripts requires a DWARF pipeline, a stepper loop, and UI for variable inspection. That is a phase in itself and belongs to a future one. Phase 10 ships the minimum: **tracepoints and panic navigation.**

### 4.1 `trace!` in scripts

Scripts get a macro `trace!(fmt, args…)` that compiles to an imported host function. The host impl emits a `LogEntry` with the calling entity attached automatically (the script host knows which entity is currently ticking):

```rust
// inside a script:
trace!("jump kicked; vel_y={}", vel.y);
```

Emits:

```
12:01:04.301  TRACE  script:npc   jump kicked; vel_y=5.2   entity=287
```

In the log panel, clicking the `entity=287` field selects that entity in the hierarchy. This is the closest thing to print-debugging with context the user will have without a real debugger.

### 4.2 Step-one-frame

Reuse Phase 7's step action. No new mechanism. Users already have step; Phase 10 documents that combined with the log panel it covers most "what just happened?" cases.

### 4.3 Stop-at-panic

On a script panic, the WASM host already surfaces file:line (Phase 7 §7). Phase 10 wires two reactions:

- Auto-Pause the PIE session (respect the Phase 7 snapshot invariant — do not Stop, the user will want to inspect state).
- Emit an error-level `LogEntry` with the script's entity, file, and line. The row is clickable; click routes through Phase 8 §5's external-editor launcher.

### 4.4 Out of scope

Breakpoints, watches, variable inspection, step-into/step-over. These require DWARF + a stepper and are a dedicated future phase (§10).

## 5. Frame debugger

Capture one frame: record every draw call, its pipeline, its bind groups, and the textures it sampled. A miniature RenderDoc-inside-the-editor.

```
crates/rustforge-core/src/diag/framecap/
├── mod.rs                  # FrameCapture record/replay hooks
├── recorder.rs             # wgpu command-encoder intercept
└── view.rs                 # panel renderer
```

### 5.1 What we capture

For each draw in the frame:

- Pass name (already present from Phase 8 GPU timings).
- Pipeline handle + its WGSL entry point names.
- Bind group slot → resource handle map.
- Vertex/index buffer handles + counts.
- Texture previews for any bound sampled texture (thumbnail; open full view on click).

We do *not* capture every buffer's bytes — that's multi-hundred-MB. We store handles and let the panel resolve them from the asset cache on demand.

### 5.2 Panel layout

```
┌─ Frame Capture (frame 14223) ─────────────────────────┐
│ GBuffer  (47 draws, 3.2 ms)                           │
│   ▸ draw  tris=12480  pipe=opaque_pbr  tex=brick_a …  │
│   ▸ draw  tris=   864 pipe=opaque_pbr  tex=wood_a  …  │
│ SSAO     (1 draw,  0.9 ms)                            │
│ Lighting (8 draws,  4.1 ms)                           │
│ Post     (2 draws,  1.0 ms)                           │
│                                                       │
│ [Capture next frame]   [Open in RenderDoc…]           │
└───────────────────────────────────────────────────────┘
```

### 5.3 RenderDoc vs. in-editor

RenderDoc is better than anything we will build. **Ship the basic in-editor view for quick checks and explicitly support "capture with RenderDoc" via the existing RenderDoc inject on Linux/Windows.** The editor detects the RenderDoc dylib and enables the "Open in RenderDoc…" button when present; otherwise it's hidden. This is the honest tradeoff: quick data in-panel, deep data via the specialist tool.

### 5.4 Cost

Capture allocates per-draw records — not free. Capture is off by default; users press the button for a single frame. No capture, no cost.

## 6. Physics timeline rewind

Physics bugs are usually visual-once-and-gone: a capsule tunnels through a wall for three frames. The user's eye catches it; by the time they hit Pause the evidence is gone. Rewind fixes this.

```
crates/rustforge-core/src/diag/physics_rewind.rs
```

### 6.1 What we snapshot

Only physics state: rigid body transforms, linear/angular velocities, sleep flags. Not scripts, not rendering. This is a deliberate narrowing from Phase 7's full-scene snapshot — it has to be cheap enough to run every frame.

Ring buffer: 300 frames (5 s at 60 Hz). Per-frame size ≈ 64 B × bodies. 1 000 bodies × 64 B × 300 ≈ 19 MB. Acceptable.

### 6.2 Interaction with PIE

Rewind is only meaningful while Paused. While Playing the ring fills; while Paused a scrub bar appears at the bottom of the viewport:

```
│ Physics ring (240/300)  ├──────────────●─────┤  -0.35s │
```

Scrubbing writes the physics snapshot at frame *N* back into the world. **It does not advance scripts or rewind script state** — scripts see the current frame regardless. This is intentional: the user is inspecting geometry, not replaying gameplay. Mismatched script/physics state is acceptable *because the user triggered it* and the PIE snapshot invariant (Phase 7 §1) still holds — Stop restores to pre-Play, not to the scrubbed frame.

### 6.3 Cost in Playing

The snapshot is a `memcpy` of a packed per-body struct from Rapier's state. Target: <0.3 ms at 1 000 bodies. Gate the entire ring behind the `editor` feature. Non-editor builds never allocate.

## 7. Memory view

Coarse totals, updated once per second. Not a heap profiler.

```
┌─ Memory ──────────────────────────────────────────────┐
│ Asset cache         412 MB  (1 284 assets)            │
│   Textures          310 MB                            │
│   Meshes             71 MB                            │
│   Audio              18 MB                            │
│   Other              13 MB                            │
│ GPU buffers         148 MB                            │
│ World                 6 MB  (8 412 entities)          │
│ Command stack        31 MB  /  500 MB cap             │
│ Physics rewind ring  19 MB                            │
└───────────────────────────────────────────────────────┘
```

Sources are existing subsystems — the asset cache already tracks its size (Phase 5), `wgpu` exposes buffer sizes, hecs knows entity count. This panel is a reader; it does not allocate bookkeeping of its own. Heap profiling is out of scope — `dhat`/`valgrind` exist and we don't need to reinvent them.

## 8. Panic capture

A crash with no context is a waste. Install a `std::panic::set_hook` in editor startup that writes a crash dump before the default handler runs:

```
./.rustforge/crashes/2026-04-16-12-03-51.json
```

Contents:

- panic message, file:line, thread name
- current scene path and dirty flag
- selected entity id (if any)
- currently running script (if in PIE)
- last 200 log entries from the ring buffer
- editor version, engine version, wgpu backend, OS

Write synchronously so a second panic during unwind still leaves a file. Compress with `zstd` if size > 1 MB. **Never include asset contents or user source code** — crashes may be shared and we're not leaking arbitrary game data by default. Opt-in upload is explicitly out of scope (§9).

## 9. Diagnostic overlays

In-viewport debug visualizations, toggled from a dropdown in the viewport toolbar. Each is a render pass run after the main pass, before post.

| Overlay | What it does |
|---|---|
| Wireframe | Re-render opaque meshes with `PolygonMode::Line`. |
| Normals | Per-fragment `normalize(normal)*0.5+0.5` as color. |
| UV checker | Sample a procedural checker using mesh UV0. |
| Lighting complexity | Heatmap of lights affecting each tile (forward+ cluster count). |
| Entity bounds | World-space AABBs as line boxes, one color per archetype. |

All overlays are additive toggles — more than one may be on at once (wireframe + bounds is useful). Each overlay's shader lives in `rustforge-core/src/render/debug/`, behind `editor`. In a shipped build the whole module compiles out; the viewport toolbar itself doesn't exist anyway.

## 10. Build order within Phase 10

Each step is independently shippable and independently testable.

1. **Logging facade** (§1) — pure plumbing; no UI dependency; unlocks everything else because every later item logs. Ship the ring, the rolling file, and the panel in one slice.
2. **Console** (§2) — small surface, reuses the log panel for output. `ConsoleRegistry` lands in core so later systems can register into it.
3. **Memory view** (§7) — tiny reader panel. Good warm-up and validates the per-second update cadence used by §3 graphs.
4. **Entity/component debugger** (§3) — needs the `SystemDescriptor` registry addition but is otherwise a reader.
5. **Panic capture** (§8) — ten lines in `main` plus a dump writer; depends on the ring buffer from §1.
6. **Script tracepoints** (§4) — host import + macro; log entries flow through §1.
7. **Diagnostic overlays** (§9) — shader work; independent of everything above.
8. **Physics rewind** (§6) — last of the engine-side items; validates the ring-buffer pattern one more time.
9. **Frame debugger** (§5) — largest by surface area and lowest urgency; land last so the panel set is settled.

## 11. Scope boundaries — what's NOT in Phase 10

- ❌ **Native Rust source-level debugging.** Use `gdb`/`lldb`/`rust-analyzer` externally. The editor does not embed a debugger.
- ❌ **WASM source-level breakpoints, stepping, and variable inspection.** Deferred to a dedicated future phase (DWARF, stepper, UI).
- ❌ **Full heap profiling / allocation tracking.** Use `dhat`, `valgrind`, `heaptrack`. The memory panel is coarse totals only.
- ❌ **Multiplayer replay debugging / networked timeline scrubbing.** Single-process only.
- ❌ **Automatic crash-dump upload to a telemetry endpoint.** Dumps land on disk; the user decides what to do with them.
- ❌ **Shader debugger.** Fall through to RenderDoc; Naga validation covers authoring-time errors.
- ❌ **Deep GPU capture** (buffer byte contents, pipeline-internal state). Fall through to RenderDoc.
- ❌ **Script variable watches and hot-eval.** Tracepoints only.
- ❌ **Remote / attached debugger to a running shipped game.** Editor-process only (same scope line Phase 8 §8 drew).
- ❌ **Log correlation / distributed tracing UI.** One process; one ring buffer.

## 12. Risks & gotchas

- **Log channel backpressure.** A crate spamming `trace!` in a tight loop can overrun the channel. Drop-oldest with a rate-limited `warn!` per §1.1; do *not* silently drop or block the tick thread.
- **Tracing subscriber fighting user subscribers in shipped builds.** In the editor we install ours; in a shipped game the user may install theirs. Keep the editor subscriber gated behind `editor` so a non-editor `rustforge-core` never sets a global subscriber.
- **Console commands bypassing the command stack.** Easy mistake — a `spawn` command that calls `world.spawn` directly breaks undo and PIE. Lint at registration: any command that takes `&mut World` must be documented as routing through the Phase 6 stack; code-review on each new command.
- **Script tracepoint spam.** Users will leave `trace!` in hot scripts and ship builds will carry them. Add a build-time feature in the script crate to compile them to `()`. For development, rate-limit per-call-site (coalesce > 100/s to a single summary line).
- **Frame-debugger pipeline drift.** Pipelines and bind groups can be created mid-frame; the recorder must snapshot metadata at draw-time, not at frame-end (handle might be freed by then). Hold `Arc` clones of pipeline/bind-group metadata for the capture's lifetime.
- **RenderDoc auto-detection false positives.** Looking for the dylib on disk is brittle. Use RenderDoc's in-app API (`RENDERDOC_API_1_*` loader) to probe at runtime; absent → hide the button.
- **Physics rewind and sleeping bodies.** Rapier marks sleepers. Snapshotting must include the sleep flag; otherwise scrubbing a frame where a body was asleep wakes it permanently when you scrub back. Capture the flag; restore it.
- **Diagnostic overlay ordering.** Wireframe + bounds + lighting-complexity stacked can obscure each other. Document a fixed order (bounds → wireframe → color overlays) and render accordingly so the result is deterministic.
- **Panic hook and `tracing` interleaving.** If the panic occurs inside a tracing emit, the hook's attempt to read the ring under the same lock deadlocks. Use a try-lock with a 100 ms timeout in the hook; degrade to "logs unavailable" in the dump rather than hang the unwind.
- **Entity debugger holding stale `Entity` ids.** Entities can be despawned between frames; graphing a despawned entity's field must detect the missing archetype and stop the graph cleanly (not panic). Same rule as the inspector — guard every read.
- **Reflection drift for system reader/writer map.** New systems must register a `SystemDescriptor`. Until they do, the debugger shows "(unknown)". Enforce in code review; fail CI if a `System` impl in `rustforge-core` lacks a descriptor.
- **Console history file corruption.** Append-only text is fine, but an interrupted write leaves a partial line. Read with `lines().filter_map(Result::ok)`; never crash on bad history.

## 13. Exit criteria

Phase 10 is done when all of these are true:

- [ ] `rustforge-core::diag` exposes a `tracing`-backed facade and no `println!` remains outside of examples and CLI entry points.
- [ ] Log panel shows the last 10 000 entries with level toggles, substring filter, and Ctrl+F find.
- [ ] Rolling session log files are written to `./.rustforge/logs/` and persist across editor restarts.
- [ ] `ConsoleRegistry` lives in `rustforge-core`; editor panel accepts input, supports tab-complete, and persists history.
- [ ] All built-in mutating console commands route through the Phase 6 command stack (verified by unit test: undo after `entity.spawn` restores prior state).
- [ ] Entity debugger lists components with `TypeId`s and reader/writer systems for a selected entity.
- [ ] Right-click "Graph this field" on a numeric inspector field produces a 240-sample live graph that pauses with PIE Pause.
- [ ] `trace!` in a WASM script emits a log entry tagged with the ticking entity; clicking the entity field selects it.
- [ ] A script panic Pauses PIE (does not Stop), surfaces a clickable row, and opens the configured external editor at `file:line`.
- [ ] Frame capture records one frame's draws with pass grouping, pipeline names, and sampled textures; "Open in RenderDoc…" appears only when RenderDoc is attached.
- [ ] Physics rewind ring captures 300 frames at <0.3 ms/frame for 1 000 bodies; scrubbing during Pause restores body transforms/velocities; Stop still honors Phase 7 §1.
- [ ] Memory panel totals match subsystem self-reports within 1 MB and updates at 1 Hz.
- [ ] Panic anywhere in the editor produces a JSON dump under `./.rustforge/crashes/` containing scene, entity, script, and last 200 log entries.
- [ ] Diagnostic overlays (wireframe, normals, UV checker, lighting complexity, entity bounds) toggle independently and can be combined.
- [ ] `cargo bloat` confirms no `diag` editor-only code is present in a non-`editor` build.
- [ ] `rustforge-core` still builds, runs, and can install a user-provided `tracing` subscriber without the `editor` feature.
