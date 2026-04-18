# Phase 42 — Deep Profiling & Telemetry

Phase 8 gave RustForge a frame-time graph and wgpu timestamp queries. Phase 10 gave it structured logging, a console, an entity debugger, and a 300-frame physics rewind. Those two phases covered the "is this frame slow?" and "what on earth is this script doing?" questions that show up in the first week of real production. They did not cover the question that shows up in month six, the one that every shipping studio eventually asks, and the one the Unreal-vs-RustForge scorecard flagged as the biggest remaining Dev Workflow gap: *across a thirty-minute session, at this exact moment of the boss fight, why did we drop three frames and who is responsible?*

Unreal Insights answers that question. It captures hours of trace data across every engine subsystem, correlates CPU threads with GPU queues with asset loads with network traffic, and lets you click a red bar and walk backwards to the cause. Phase 42 builds the RustForge equivalent — a scoped trace framework, an `.rtrace` binary capture format, a timeline panel that cross-cuts every subsystem, causal dependency arrows, a memory profiler, asset-load tracing, flame graphs, differential analysis, and a live streaming view for connecting to a running build. All of it reuses Phase 10's `tracing` facade at the source level and extends it with a high-throughput binary sink that Phase 10 deliberately did not attempt.

Phase 42 also finally closes the game-side telemetry story. The editor-side policy from Phase 13 — no first-party telemetry, no analytics, no phone-home — stays exactly where it was; that is non-negotiable and this phase reaffirms it. What Phase 42 adds is a framework *for games* to emit their own telemetry to servers *the game's developer* runs, reusing the Phase 29 DDC server shape: self-host only, boring HTTP, bearer tokens, no first-party SaaS. Games can emit `level_complete` and `boss_defeated` events to their own analytics box. Players must consent. The editor never participates. This is the only honest way to offer game telemetry from a studio that refuses to ship it itself.

+12 scorecard points. Largest single Dev Workflow delta remaining.

## Goals

By end of Phase 42:

1. **Scoped trace framework** — zero-overhead-when-inactive trace macros across every engine subsystem, building on Phase 10's `tracing` facade but with a dedicated binary sink sized for hours of capture.
2. **`.rtrace` binary format** — zstd-compressed, chunked, self-describing, append-only, safe to kill mid-capture.
3. **Insights-equivalent panel** — timeline across CPU threads, GPU queues, network, asset I/O, audio, physics, script ticks, and user-emitted game events.
4. **Causal analysis** — click a slow frame, see dominant subsystem, blocked threads, memory spikes, asset loads, with dependency arrows between events.
5. **GPU profiler** — per-pass timings extended with pipeline-state capture, draw-call-level breakdown, shader permutation identity, one-click RenderDoc hand-off.
6. **Memory profiler** — per-subsystem allocation timelines, leak detection, peak-vs-steady graphs, per-type histograms.
7. **Asset-load tracing** — which asset, why it loaded, how long it took, who triggered it, with Phase 31 world-partition integration.
8. **User event timeline** — game code emits `trace!(category="gameplay", "boss_spawned")` and it lands on the timeline.
9. **Flame graph + call tree** — aggregate slicing of a captured session.
10. **Streaming live trace** — TCP connection to a running game, low overhead, read-only view.
11. **Differential analysis** — compare two captures and surface perf deltas.
12. **Exports** — CSV, JSON, RenderDoc `.rdc` hand-off.
13. **Game-side telemetry SDK** — opt-in `telemetry::emit()`, manifest declaration, consent on first launch, self-hosted server only.
14. **Overhead budget** — trace infrastructure costs < 1% of frame time when inactive, < 5% when fully active on all subsystems.

## 1. Trace framework — scope, macros, and the buffer

Phase 10 already has `tracing` as the logging facade. We do not replace it. We add a second **trace** layer that sits beside the log ring buffer with different priorities: log entries are read by humans and need string formatting; trace events are read by the timeline panel and need microsecond timestamps and cheap binary payloads.

```
crates/rustforge-trace/
├── src/
│   ├── lib.rs              # public macros, init(), shutdown()
│   ├── scope.rs            # TraceScope RAII guard
│   ├── buffer.rs           # per-thread SPSC ring, 1 MiB default
│   ├── writer.rs           # background thread: drains rings -> .rtrace file
│   ├── format.rs           # TraceEvent, payload variants
│   ├── category.rs         # CpuTick, Gpu, Asset, Physics, Script, Net, Audio, Gameplay
│   └── stream.rs           # TCP live-stream server (§8)
```

The core macro is shaped to be a no-op when tracing is disabled at the `ENABLED` atomic, and cheap enough when enabled that peppering every engine call site is defensible:

```rust
#[macro_export]
macro_rules! trace_scope {
    ($category:expr, $name:expr $(, $k:literal = $v:expr)* $(,)?) => {
        let _guard = if $crate::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
            Some($crate::scope::TraceScope::enter(
                $category,
                $name,
                &[$(($k, $crate::payload::Field::from($v))),*],
            ))
        } else {
            None
        };
    };
}
```

`TraceScope::enter` writes an `Enter` event to the current thread's ring; `Drop` writes the matching `Leave` with the elapsed nanoseconds. The ring is a lock-free SPSC queue owned by the thread; the background writer owns the consumer ends for all registered threads. No mutex on the hot path. Dropped events on ring overflow are counted, not blocked on — we would rather lose a span than stall the render thread.

Usage at call sites follows the pattern Phase 8 set for the profiler, but with category tags:

```rust
fn tick_physics(world: &mut World, dt: f32) {
    trace_scope!(Category::Physics, "tick", dt = dt, bodies = world.bodies.len());
    // ...
}
```

Instant (non-scope) events are a separate macro for things like "boss spawned" or "asset evicted":

```rust
trace_instant!(Category::Gameplay, "boss_spawned", id = boss_id.0, difficulty = "hard");
```

Opinion: do *not* reuse `tracing::span!` as the trace-scope primitive. `tracing`'s dispatcher is too general for microsecond budgets — it allocates, it walks subscribers, it supports structured filtering. Phase 10 keeps it for logs; Phase 42 writes its own because the cost model is different.

### 1.1 Zero-overhead-when-inactive

`ENABLED` is a global `AtomicBool`. In a hot loop the macro resolves to a single relaxed load and a branch — under 1 ns on every platform we care about. Measurements from the CI perf harness (see Exit Criteria) must prove this stays under 1% of frame time on the standard-scene test.

In a shipped game with `trace` feature off at compile time, the macros expand to nothing and the buffer/writer crates are not linked at all. Zero bytes of trace infrastructure in a retail binary unless the developer opts in.

## 2. `.rtrace` file format

Trace captures are binary, chunked, and streamed. A session can run for hours; the format has to survive a process kill mid-capture without corrupting the whole file.

```
.rtrace layout:
┌─────────────────────────────────────────────────────────┐
│ Header (64 B): magic, version, session GUID, start ts   │
├─────────────────────────────────────────────────────────┤
│ StringTable chunk (interned category/name strings)      │
├─────────────────────────────────────────────────────────┤
│ ThreadInfo chunk (thread id -> name, core affinity)     │
├─────────────────────────────────────────────────────────┤
│ EventChunk #0  [zstd-compressed, ~4 MiB payload]        │
│ EventChunk #1  [zstd-compressed]                        │
│ EventChunk #N  [zstd-compressed]                        │
├─────────────────────────────────────────────────────────┤
│ Index footer (chunk offsets, time ranges) — last 4 KiB  │
└─────────────────────────────────────────────────────────┘
```

Each `EventChunk` is self-contained: header, compressed event stream, CRC32. The writer fsyncs after each chunk so a crash loses at most the tail chunk. The index footer is rewritten at the end; if the file is truncated, the reader scans chunks forward and reconstructs an index.

Events are varint-packed within a chunk. A typical `Enter`/`Leave` pair lands around 12 bytes after zstd — an hour of 60 Hz game capture with 200 events per frame is roughly 500 MB compressed, well within disk tolerance.

## 3. Timeline panel — the Insights-equivalent view

The panel is the centerpiece. It loads an `.rtrace` (or attaches live, §8) and renders a horizontal timeline with one track per thread, per GPU queue, per subsystem category.

```
┌─ Trace: session-2026-04-16-1413.rtrace ──────────────────────────────────┐
│ [File] [Live][Diff]  t=14.213s  zoom: 1ms/px   filter: [gameplay     ]   │
│                                                                          │
│ Frame  │ 842       843       844        845 (!)      846        847     │
│        ├─────────┬─────────┬─────────┬───────────┬─────────┬─────────    │
│ Main   │[tick ][render     ][tick ][render           ][tick ][render ]   │
│ Worker1│[phys     ][anim ][phys     ][phys                ][phys ]       │
│ Worker2│[ai ][path   ][ai ][path ][ai ][path           ][ai ]            │
│ GfxQ   │      [shadow][gbuf][lit ][post]  [shadow][gbuf][lit          ]  │
│ Comp Q │                            [ssao][bloom]                        │
│ Assets │                                   [load: boss_mesh.rmesh ]      │
│ Script │[upd][upd][upd][upd][upd   ][upd                ][upd ][upd ]    │
│ Phys   │[step ][step ][step ][step     ][step              ][step ]      │
│ Audio  │[mix ][mix ][mix ][mix ][mix ][mix ][mix ][mix ][mix ][mix ]     │
│ Events │               ▼boss_spawned                ▼checkpoint          │
│                                                                          │
│ Selected: frame 845 (41.2ms, budget 16.7ms)  [Analyze ▸]                 │
└──────────────────────────────────────────────────────────────────────────┘
```

Frames that exceed budget get a `(!)` flag and red tint. Clicking a frame opens the Analyze drawer (§4). Clicking any span pops a detail card with category, name, duration, fields, parent span, and — crucially — a "go to source" link if the span was emitted from an attributed call site. Bookmarks and tags let a developer mark "here's where the slowdown is" and share the `.rtrace` with a colleague who opens it and sees the same bookmark.

Filter box narrows to categories or name substrings. Zoom is mouse-wheel, pan is middle-drag, `F` frames the selected span, `[` and `]` step frame-by-frame. Exact same input idioms as the viewport — consistency beats novelty.

## 4. Causal analysis — "why was this frame slow?"

This is the feature that makes the panel more than a pretty picture. Clicking a slow frame opens the Analyze drawer:

```
┌─ Frame 845 — 41.2 ms (budget 16.7 ms, over by 24.5 ms) ──────────────┐
│ Dominant cost:   Render / gbuffer pass        18.3 ms  44%           │
│ Second:          Physics / solver             9.8 ms   24%           │
│ Third:           Asset / decompress           7.1 ms   17%           │
│                                                                      │
│ Blocked threads: Main waited on AssetLoader   6.2 ms                 │
│                  Worker2 waited on Physics    3.1 ms                 │
│                                                                      │
│ Memory:   +48 MiB (asset loader staging, freed frame 846)            │
│ Assets:   boss_mesh.rmesh streamed in (trigger: world-partition)     │
│ GPU:      shader permutation miss → compile 4.2 ms (first draw)      │
│                                                                      │
│ [Jump to dominant span]  [View dependency graph]  [Copy summary]     │
└──────────────────────────────────────────────────────────────────────┘
```

The dependency graph view shows arrows between spans: "Main-tick waited here because Worker2 hadn't finished physics-step, which was waiting on AssetLoader which was decompressing boss_mesh". This is the mechanism users actually debug with. Arrows are computed from blocking-wait events emitted at every `Mutex::lock`, `Condvar::wait`, and task-join that we've instrumented — call-site coverage is gradual, but the big ones (job system, asset barrier, GPU fence) are in for the first release.

Causal links are stored as event pairs in the `.rtrace` (`BlockBegin`/`BlockEnd` with a target span ID) so the analysis is offline-replayable, not a live-only heuristic.

## 5. GPU profiler extension

Phase 8 reported per-pass timings via wgpu timestamp queries. That's the floor. Phase 42 extends it:

- **Pipeline-state capture** — each draw records its pipeline ID, bound resources, vertex/index buffer ranges, scissor, blend, depth state. The `.rtrace` carries a compact delta of state changes.
- **Draw-call breakdown** — per-pass you can expand to see every draw, its GPU time (approximated from timestamp queries on the enclosing pass scaled by vertex count, with a note that this is an estimate unless per-draw timestamps are enabled — cost: non-trivial, opt-in).
- **Shader permutation identity** — every draw records the shader's content hash and permutation key. Spikes from shader-compile stalls (first-use compilation) show up as a distinct category in the timeline.
- **RenderDoc hand-off** — right-click a frame, "Capture in RenderDoc". We launch RenderDoc via its injection API (on supported platforms) or emit a `.rdc` stub and tell the user to open it manually. We do not reimplement RenderDoc. We hand off.

```rust
pub struct GpuPassEvent {
    pub pass: &'static str,
    pub pipeline_hash: u64,
    pub draw_count: u32,
    pub triangles: u64,
    pub ts_begin_ns: u64,
    pub ts_end_ns: u64,
    pub state_delta: StateDelta,
}
```

## 6. Memory profiler

Per-subsystem allocation tracking over time, with three views:

1. **Timeline** — stacked area chart of bytes allocated per subsystem (Renderer, Physics, Assets, Script, ECS, Audio, Scratch). The same timeline that the trace panel uses, with memory as the Y axis.
2. **Peak vs steady** — for each subsystem, the 99th-percentile peak vs the median, with a "pressure" indicator when peak/steady > 3.
3. **Per-type histogram** — for types marked with `#[derive(Tracked)]`, live count and byte totals over time. No global heap walk; opt-in per type.

```rust
#[global_allocator]
static ALLOC: TracedAlloc = TracedAlloc::new(std::alloc::System);

// Subsystems wrap allocations in a scope:
let _mem = mem_scope!(Subsystem::Physics);
let bodies = Vec::<Body>::with_capacity(10_000);
```

`mem_scope!` is a RAII guard that routes subsequent allocations on this thread into the named bucket. Thread-local state; no synchronization on the hot path.

Leak detection works by pairing allocation scopes with expected-free scopes: at scope exit, any allocation that escaped is tagged. This is not a full heap walker — we are not building Valgrind — but it catches the 80% case where "this function was supposed to release its temporaries." The full expensive check runs under `--profile leak-check`, which enables tracking-allocator metadata for every allocation and flags matched-but-never-freed entries at shutdown.

Non-goal: replacing dedicated tools like `heaptrack`, `dhat`, or the Windows Performance Analyzer heap profiler. We cover the common 80% case in-editor; for the long tail we point users at those.

## 7. Asset-load tracing

Every asset load emits:

```rust
pub struct AssetLoadEvent {
    pub guid: AssetGuid,
    pub path: Arc<str>,
    pub trigger: LoadTrigger,      // Manual, Dependency(parent), Streaming(cell)
    pub t_begin_ns: u64,
    pub t_ready_ns: u64,
    pub size_bytes: u64,
    pub from_cache: bool,          // hit local DDC (Phase 29)
}
```

`LoadTrigger::Streaming` carries the world-partition cell ID from Phase 31 so the timeline can show "camera crossed into cell (32, 14), loaded 47 assets, 120 ms on worker thread 3." This is the mechanism that lets level designers see their streaming budget at a glance without reading the logs.

The Assets track on the timeline renders each load as a span, colored by trigger type, with tooltip showing asset path and size. Filter by trigger — "show me everything world-partition loaded in the last 5 seconds of this capture."

## 8. User game events

Game code emits events through the same `trace!` entry point that Phase 10 scripts already use, but with a new `category` parameter that puts them on the timeline:

```rust
use rustforge::trace;

fn on_boss_defeated(ctx: &mut Ctx, boss: Entity, time_ms: u64) {
    trace!(category = "gameplay", "boss_defeated",
           boss = boss.id(), time_ms = time_ms, player_hp = ctx.player.hp);
    // ...
}
```

These appear on the Events track as labeled markers. Designers can drop markers at gameplay milestones and then visually align perf spikes with game state: "we always lose frames right after the boss spawns its minions — here's the trace proving it."

## 9. Flame graph & call tree

Aggregated views of a capture (or a selected time range):

```
Flame graph (time range: 14.20s – 14.30s):

main_tick                                                                  █████████████████  100%
├─ render                                                                  ████████████        68%
│  ├─ gbuffer_pass                                                         ██████              32%
│  │  ├─ batch_submit                                                      ████                22%
│  │  └─ state_sort                                                        ██                  10%
│  ├─ shadow_pass                                                          ████                18%
│  └─ post                                                                 ██                  12%
├─ physics_step                                                            ████                22%
│  ├─ broadphase                                                           ██                  10%
│  └─ solver                                                               ██                  12%
└─ script_tick                                                             ██                  10%
```

Call tree is the same data shown as a collapsible tree with self-time, total-time, call-count, and a context menu to jump to the source span. Both views aggregate over any selection on the main timeline — drag-select a region, the flame graph redraws for that window.

## 10. Streaming live trace

Attach to a running game and watch events flow in real time. Intended for "run the game on the console devkit, debug from the editor on the desktop."

```rust
// In the game, gated on --trace-stream CLI flag:
rustforge_trace::stream::serve("0.0.0.0:7842", TraceStreamConfig {
    token: load_token_from_env(),
    max_events_per_sec: 50_000,
}).unwrap();
```

Editor side: Trace panel → Live → enter host:port and token. The editor opens a TCP connection, pulls framed events, and renders them on the timeline as they arrive (with a small buffer to keep the view smooth). All the categories, filters, and bookmarks work on a live feed the same way they work on a file — because the file format is just the serialized stream written to disk.

Overhead on the game side is single-digit percent at the event rates above; the gated `--trace-stream` flag means it is opt-in per run. The token is required; never stream without auth, even on a LAN, because a malicious peer could snapshot your trace and learn a lot about your codebase.

## 11. Differential analysis

Load two `.rtrace` captures — "before.rtrace" and "after.rtrace" — and diff them:

```
┌─ Diff: before.rtrace vs after.rtrace ─────────────────────────────┐
│ Scenario: same benchmark scene, 1000 frames each                  │
│                                                                   │
│ Mean frame time:     16.8 ms  →  14.2 ms    -2.6 ms  (-15.5%)  ✓  │
│ 99th percentile:     28.4 ms  →  19.1 ms    -9.3 ms  (-32.7%)  ✓  │
│ GPU gbuffer:          8.1 ms  →   6.9 ms    -1.2 ms  (-14.8%)  ✓  │
│ CPU physics:          3.2 ms  →   3.4 ms    +0.2 ms  (+6.3%)   ✗  │
│ Memory peak:          512 MB  →   498 MB    -14 MB  (-2.7%)    ✓  │
│ Asset loads (total):  847     →  847         (same)                │
│ Shader compiles:      42      →  3           -39  (cache warmed)  │
│                                                                   │
│ [Show per-span breakdown]  [Export CSV]                           │
└───────────────────────────────────────────────────────────────────┘
```

Matching spans across captures is name + category + parent-chain; unmatched spans show up as "new in B" or "missing in B." This is the tool engineers use to justify a perf PR — attach before and after `.rtrace` and the diff tells the reviewer what actually changed.

## 12. Game-side telemetry — opt-in, self-hosted

Editor-side telemetry: **none**. Zero. Phase 13 wrote the policy and this phase reaffirms it, in case anyone forgot: the editor does not phone home, does not count opens, does not count crashes-without-consent, does not submit feature usage. Full stop.

What Phase 42 adds is a framework games built on RustForge can use to emit their own telemetry to their own server. Two moving parts:

### 12.1 Game-side SDK

```rust
use rustforge::telemetry;

telemetry::emit("level_complete", telemetry::payload! {
    level: "world-3-5",
    time_seconds: 135.2,
    deaths: 3,
    final_score: 18_400,
});
```

Events are batched on a background thread, sent via HTTPS to the configured endpoint, retried on failure with exponential backoff, dropped after 1 hour of offline. Events are buffered to disk during retry so a brief network outage doesn't lose data.

### 12.2 Self-hosted server — `rustforge-telemetry-server`

Reuses the Phase 29 DDC server shape: small Rust binary, HTTP/S, bearer token, stores events to sqlite or postgres. The developer runs one. We ship no first-party hosted version. Ever.

```
Game (emits events)  ──HTTPS──▶  rustforge-telemetry-server  ──▶  sqlite/postgres
                                    (dev-owned box)                (dev-owned)
```

### 12.3 Privacy model — load-bearing

1. **Declared in manifest** — the game's `Cargo.toml` must have `[package.metadata.rustforge.telemetry]` with the server URL and categories of data collected. Without this, the SDK refuses to initialize.
2. **Consent on first launch** — the SDK shows a dialog (or asks the game's UI to) enumerating what is collected and asking the player to opt in. Decline = off, no retry for 30 days. No pre-checked box. No dark patterns.
3. **No first-party SaaS** — we do not operate a telemetry service. Full stop. The data goes to the developer's server and nowhere else.
4. **Player-visible toggle** — every game that enables telemetry must expose an in-game toggle to turn it off after the fact. SDK helper: `telemetry::set_enabled(false)`.
5. **No PII by default** — the SDK provides an event payload macro that refuses to serialize types tagged `#[pii]` unless the game explicitly calls `with_pii_consent()`.
6. **Editor is uninvolved** — the SDK's network stack is not linked into the editor. Build-gated on `cfg(not(feature = "editor"))` for the transport layer.

This is the only honest posture. Studios that want analytics get a tool that works. Players get a bright line. Anthropic — sorry, RustForge — never operates the pipeline.

## 13. Exports

- **CSV** — span rows (name, category, start_ns, dur_ns, thread, fields) for spreadsheet analysis.
- **JSON** — same, more verbose, for custom scripts.
- **Chrome Trace Format** — an `.rtrace → chrome-trace.json` converter. `chrome://tracing` and Perfetto both consume it. Free tooling.
- **RenderDoc `.rdc`** — GPU capture hand-off (§5).

Export is a menu action on the Trace panel; no scripting required.

## 14. Build order

1. Trace framework core — macros, per-thread ring, background writer, `ENABLED` gate.
2. `.rtrace` format — header, chunks, zstd, index footer, recovery from truncation.
3. Timeline panel — tracks, frames, zoom/pan/filter, span detail card.
4. Causal links — blocking-wait instrumentation, Analyze drawer, dependency graph.
5. Memory profiler — tracked allocator, per-subsystem scopes, leak-check mode.
6. Asset-load tracing — Phase 31 integration, trigger classification, Assets track.
7. GPU profiler extension — pipeline state, draw-call, shader permutation, RenderDoc hand-off.
8. Flame graph + call tree — aggregate views over selections.
9. Streaming live trace — TCP server, editor attach flow, token auth.
10. Differential analysis — span matching, diff drawer, per-span breakdown.
11. Game-side telemetry SDK + `rustforge-telemetry-server` + manifest + consent dialog.
12. CI perf harness — measure trace overhead on standard scene, enforce budgets.

Each step independently useful. Step 1 alone gives us better-than-Phase-10 spans. Step 3 is already a usable Insights-lite. Step 11 is a separate product axis and can slip to 42.5 without blocking the rest.

## Scope ❌

- ML-driven anomaly detection ("this frame looks weird"). Deterministic thresholds and user-set budgets only.
- Hosted telemetry SaaS. We do not operate analytics infrastructure for other people's games. The SDK talks only to self-hosted servers.
- Cross-studio benchmarking service. No shared leaderboards, no "your game compared to average." That is a privacy hazard we decline.
- Always-on production tracing at AAA shipping scale. Traces are dev-time primarily; shipped games can enable them, but we do not claim the overhead profile of a dedicated APM.
- Replacement for external APM tools (Datadog, New Relic, Sentry). Developers who want those should use those. We integrate via standard export formats (JSON/CSV); we don't reinvent.
- Full heap profiling. We cover the 80% case; `heaptrack` and friends cover the tail.
- Symbolication of optimized production binaries. Use `addr2line`/`llvm-symbolizer` on your PDBs/DWARF as usual.
- First-party editor telemetry of any kind. Not negotiable.

## Risks

- **Overhead creep** — trace scope at every hot call site is only free if the `ENABLED` load stays cached. Micro-benchmarks in CI, budget enforced at < 1% inactive, < 5% active. Regress this budget, the PR doesn't land.
- **Ring drops on spikes** — 1 MiB per thread holds roughly 80 k events. A pathological frame with nested spans can overflow. Drops are counted and surfaced; document the failure mode; allow `TRACE_RING_BYTES` env override.
- **Binary format churn** — `.rtrace` version is in the header; bump carefully, keep a reader for the previous version, document migration. Never break opens on week-old captures.
- **Live stream security** — no auth = no stream. Token required. Localhost-only by default; remote-enable is explicit. Document that a trace reveals subsystem structure an attacker might find useful.
- **Telemetry policy erosion** — every future contributor will feel pressure at some point to add "just a crash ping" to the editor. The answer is no. Write it on the door. CI has a test that greps the editor crate for HTTP client references and fails if new ones appear without a whitelist entry.
- **Consent dialog UX** — studios will be tempted to make it a pre-checked box or bury it. The SDK rejects that at runtime; no opt-in string = no init. Make the failure loud.
- **Memory-profiler accuracy** — per-thread allocator scopes miss allocations on foreign threads (native libraries, OS callbacks). Document the limitation; for the long tail, point at `heaptrack`.
- **GPU capture compatibility** — RenderDoc hand-off depends on platform and graphics API combinations we don't control. Graceful fallback to "save `.rdc` stub and open manually."

## Exit criteria

- Trace overhead with `ENABLED = false` on the standard CI scene: **< 1%** of frame time, measured over 1000 frames at 1080p/1440p/4K. Enforced by perf harness.
- Trace overhead with all categories active and writing to disk: **< 5%** of frame time, same scene, same measurement.
- Capture a 30-minute session → `.rtrace` file < 1.5 GB on disk → open in timeline panel → first-frame render < 3 s → pan/zoom stays above 60 Hz.
- `.rtrace` files survive `kill -9` of the process mid-capture: reader recovers all chunks up to the last fsync.
- Click a frame over budget → Analyze drawer populates in < 200 ms → dominant span, blocked threads, memory delta, and asset loads all shown.
- GPU pass timings accurate to within ±5% of external ground truth (RenderDoc timer query) on reference scene.
- Memory profiler leak-check mode detects seeded leaks of 10 allocations across 10 k scopes with zero false positives on the reference regression suite.
- Asset-load tracing reports every load in a 10-minute streaming session; trigger classification (manual vs dependency vs streaming) is correct on 100% of a scripted test run.
- Live stream: attach from editor to running game, < 500 ms first-event latency, < 3% added overhead on game side vs offline capture.
- Differential analysis: two captures of the same scene recorded 5 minutes apart report < 2% variance on all per-span metrics.
- Telemetry SDK: games without the manifest entry fail to call `telemetry::emit()` with a clear error. Consent dialog must fire on first launch of a telemetry-enabled game; declining persists for 30 days. No first-party server exists.
- Editor crate contains zero outbound network calls other than the DDC client (Phase 29) and Git LFS (Phase 27); enforced by a CI grep test.
- Export to Chrome Trace Format opens in `chrome://tracing` and Perfetto without warnings on a reference capture.
- Documentation: "Profile your first frame" tutorial, "Diagnose a streaming stall" walkthrough, and "Set up telemetry for your game" guide (with the consent-dialog sample UI) all shipped.
