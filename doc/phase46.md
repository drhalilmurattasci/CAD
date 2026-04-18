# Phase 46 — Series Retrospective & 3.0 Vision

Forty-five phases ago, Phase 1 was a workspace diagram and a list of engine-side hooks. The claim was that a 47k-LOC Rust engine could carry a separate, feature-flagged editor crate without contaminating runtime builds. That claim fanned out into twelve more phases to reach 1.0, ten more to close the post-ship gaps, eight more to reach AAA parity, and seven more to push at the modern frontier. This document is the forty-sixth and last. It does not design a subsystem. It is the concluding doc of the arc — a retrospective, a coverage map, a final scorecard snapshot, and a careful pointer at what a 3.0 series could look like if anyone were to pick up the pen.

A reader looking for implementation detail, a feature list, or an Exit-criteria checklist will not find one here. The arc exits when the last paragraph of this document ends. The value of what preceded it is the decision trail — the record of which bets were made, which were refused, and which compounded — not the guarantee that the forty-five prior documents compile cleanly in a given engineering-year budget. This is a map. Somebody else will build the territory.

The shape of the document mirrors the shape of the arc. A long middle — coverage map, philosophy, scorecard — sits between a compact opening and a compact closing. Readers returning to the series years from now may want only the tables. Readers evaluating whether a 3.0 arc is worth writing may want only the themes section. Readers picking up the project cold may want the philosophy section, which is where the invariants that bind the other forty-five phases together are stated plainly enough to argue with.

## A note on what this phase is

Before the substance: readers of earlier phases will notice that this one breaks the pattern. Every prior phase had a Goals section, a numbered build order, a set of crate skeletons, and an Exit criteria checklist. This one has none of those. That is deliberate, and the absences are as meaningful as any of the presences.

The Goals section is absent because a retrospective does not have goals; it has things to report. The numbered build order is absent because there is nothing to build. The crate skeletons are absent because no new crate is introduced. The Exit criteria checklist is absent because the arc's exit criterion is the last paragraph of this document, and checklists describing themselves have a recursive quality the series prefers to avoid.

A retrospective has its own discipline. It is honest about scope. It resists the urge to retroactively rationalize close-run decisions. It names the places where the series was lucky as well as the places where it was right. It separates architectural wins (durable, compounding) from feature wins (real, but fungible). And it declines to over-claim: a design series is not a shipped engine, and a scorecard number is not a procurement decision.

A reader expecting a final blueprint should set that expectation aside. A reader looking for a clear accounting of what was promised, what was delivered in design, what was refused, and what remains open — this document aims to be that.

## Summary of the arc

Forty-five phases resolve into four acts. Each act had a different job, and each used the vocabulary the previous act had introduced.

**Phases 1–13 — 1.0: the editor core.** Thirteen phases, one deliverable: an editor users would trust to ship a small-to-mid game with. The choices made here — `egui` + `egui_dock` for the shell, reflection-driven inspectors, `.ron` text scenes with structural merge, stable `SceneId(u64)`, Command-routed undo, snapshot-and-restore Play-in-Editor, feature-gated editor/runtime split, capability-sandboxed plugins, and a polish phase that turned "works" into "1.0" — locked in every architectural invariant the rest of the series inherited. Nothing in phases 14–45 overwrote a Phase 1–13 decision. Several nudged against one.

**Phases 14–23 — Post-1.0 Wave A: runtime + authoring breadth.** Ten phases filling in the systems a shipped 1.0 editor had only stubbed: networking, game UI, enhanced input, audio, VFX, sequencer, material graph, advanced rendering, mobile/web expansion, and the ecosystem/documentation layer that turned the engine from a product into a platform. The guiding principle across the act: reuse the Phase 2 reflection registry, the Phase 6 command stack, the Phase 7 snapshot invariant, and the Phase 9 feature-gate wall. Every new system plugged into primitives the first thirteen phases had put in place.

**Phases 24–30 — Post-1.0 Wave B + first retrospective.** Seven phases of depth where Wave A had gone for breadth: animation graph runtime, advanced physics authoring, AI + behavior trees + navigation, sky/atmosphere/weather, XR, shared DDC + team build server, then Phase 30's retrospective and 2.0 roadmap. The retrospective was honest — it named the gaps Wave A and B had not closed, and it scoped the 2.0 series against those gaps, not against an imagined wish list.

**Phases 31–38 — 2.0: AAA parity.** Eight phases chasing the last categories where Unreal still led on raw capability: world partition and open-world streaming, advanced terrain/foliage/splines, replay/session recording, multiplayer services (lobbies, matchmaking, voice), advanced animation (motion matching, physics-driven reactions, facial), hardware ray tracing + path tracer + neural rendering, multi-user live editing, and modding runtime + live ops. The 38-phase scorecard landed at 82% of UE feature coverage as specified.

**Phases 39–45 — 3.0: the modern frontier.** Seven phases chasing the categories that stopped being "nice to have" after the 2.0 scorecard landed: generative/ML-assisted authoring (content generation, behavior synthesis, material/mesh variation from prompts), procedural content generation tooling integrated into the cook and streaming paths, hair/cloth/fluid/destruction simulation suites, Insights-level deep profiling with timeline capture + CPU/GPU counter correlation + custom markers API, procedural character and crowd authoring (Metahuman-scale with local models), a first-class narrative and quest-authoring stack, and migration tools for importing Unreal/Unity projects into RustForge. The estimate below places the 45-phase series at roughly 88–90% of UE coverage as specified.

**Phase 46 — this document.** The capstone. No new subsystem. Look back, score, thank, and end.

The acts are not equal in length or difficulty. Act I (1.0) was structurally the hardest: every phase was load-bearing on every later phase, every decision set a precedent the rest of the series inherited, and no phase could skip the work of leaving a buildable editor behind. Acts II and III operated with more freedom — they were extending a working engine, not inventing one — but each was still disciplined by the invariants Act I had set. Act IV had the hardest *authoring* job: by Phase 39 the engine's architectural surface was mature enough that a new subsystem had to justify itself against a dozen places it could *almost* live, and choosing the right place was more than half the design. Phases 39–45 read faster than phases 1–13; they were not easier to write.

A deliberate asymmetry runs through the acts. Features that look glamorous on an engine's feature list — ray tracing, motion matching, fluids, Metahuman-class characters — all arrive in Acts III or IV, after the plumbing that makes them cheap to add is already in place. The 1.0 editor is nobody's idea of a glamour feature. It was the single most important thing the series built.

## The acts, at one layer of detail more

The four-act summary above compresses a lot. One paragraph per act at slightly more depth, for readers who did not follow the series phase-by-phase:

**Act I (1.0, Phases 1–13).** The editor is not a rendering demo, a level viewer, or a utility that happens to resemble an editor — it is an honest authoring tool, and nothing about it exists to impress a screenshot audience. Phase 1 declared the workspace. Phase 2 built the reflection plumbing that every later inspector, serializer, undo command, script binding, and replication field resolves through. Phase 3 drew the first pixels. Phase 4 locked in the scene format that made Git-native collaboration possible. Phase 5 built the content browser around a disk-watcher rather than a database. Phase 6 made the command stack non-negotiable. Phase 7 made Play-in-Editor snapshot-restore trustworthy. Phase 8 shipped specialized editors (material, terrain, animation, script). Phase 9 drew the wall between editor and runtime and never let it crack. Phase 10 built the profiler and diagnostics. Phase 11 introduced capability-gated WASM plugins. Phase 12 proved Phase 4's bet by making `git merge` on scenes actually work. Phase 13 turned all of the above into a 1.0 — preferences, keybindings, theming, accessibility, localization scaffolding, crash recovery, CI gates. At the end of Act I, the editor was a shippable thing on its own merits.

**Act II (Post-1.0 Wave A, Phases 14–23).** Act II added the runtime breadth users had asked for and the authoring tools Act I had stubbed. Networking (14) was the first runtime system that had to negotiate with the Phase 7 snapshot invariant and the Phase 2 reflection registry simultaneously, and it did. Game UI (15) drew a hard line between the editor's `egui` shell and the game's `.rui` retained-mode assets. Enhanced input (16) built a runtime action stack distinct from the editor's keybindings. Audio (17), VFX (18), sequencer (19), material graph (20), advanced rendering (21), mobile+web expansion (22), and the ecosystem/documentation layer (23) followed. Act II's discipline was: reuse the patterns Act I established (asset-editor pattern, command stack, reflection, feature-gated crates), do not invent new ones. Each phase was visibly shorter than it would have been without Act I, because most of the hard work had already been done.

**Act III (Post-1.0 Wave B + first retrospective, Phases 24–30).** Depth where Wave A had gone for breadth. Animation graph runtime (24), advanced physics authoring (25), AI + nav (26), sky/weather (27), XR (28), shared DDC + team build (29), then Phase 30 — the inflection retrospective. Phase 30 was the first time the series looked back, audited its own bets, and scoped the next arc against honest gap analysis rather than wishlist. Every later phase owes its scope to Phase 30's discipline; the 2.0 arc did not sprawl because the retrospective refused to let it.

**Act IV (2.0 + 3.0, Phases 31–45).** The longest act by page count and the densest by feature. World partition (31), terrain/foliage/splines (32), replay (33), multiplayer services (34), advanced animation (35), HW ray tracing + path tracer (36), multi-user live editing (37), modding (38), then 3.0's push into ML-assisted authoring (39), fluid/cloth/hair (40), PCG (41), Insights-level profiling (42), procedural characters (43), narrative stack (44), and migration tools (45). Act IV closed every category where Unreal's lead came from having a feature RustForge did not yet have. Unreal's remaining lead after Act IV is in categories that cannot be closed by phases: ecosystem maturity, consoles, Perforce, visual scripting, marketplace, a decade of Unreal Insights polish. Act IV took RF from 80% to ~88–90% of UE as specified.

Phase 46 ends the authorial arc. A 3.0 arc, if there is one, will be written by a team that includes voices the first 46 phases did not — community maintainers, studios who shipped on the engine, contributors who found bugs the series never imagined. The best possible outcome for this document is that it becomes out-of-date quickly because the community's feedback reshapes what the next arc should be.

## Philosophy — the five core bets restated

The series made five architectural bets in Phase 1 and never unmade any of them. Each bet compounds with every subsequent phase: the phases that came later did not chip at these decisions, they leaned on them.

1. **Rust-first.** Safety, ergonomics, and performance are not a trilemma in Rust the way they are in C++. The series bet that `Send + Sync` at the type level, `Result` at every fallible boundary, and ownership-tracked borrows would pay for themselves many times over across the codebase. The forty-five-phase map has the engine and editor in the same language, the scripting sandbox in WASM, and the runtime building green from day one under Clippy's strict lints. Unreal chose differently; the consequences of that choice show up in every one of their production-debugger war stories.

   The less-cited consequence of Rust-first is *team scaling*. A Rust codebase at RustForge's scale still lets a new engineer read a module and predict how it can and cannot be broken. A C++ codebase at Unreal's scale has a six-month onboarding curve before an engineer can reason about the thread-safety of a typical UObject interaction without a senior's help. That ramp time is hidden from an engine scorecard but shows up in every studio's hiring plan. The series bet that a smaller, safer codebase would ship more features per engineer-year than a larger, less-safe one, and that bet compounds with every phase.

2. **Git-native.** Phase 4's commitment to `.ron` text scenes with structural merge keyed on `SceneId` is the single most consequential format decision in the series. Every later phase paid for its scene data in a format `git diff` could read and `git merge` could reconcile. The lock-free, review-diffable, bisectable scene is not a feature. It is an *invariant* that makes every other collaboration feature cheaper. Phase 12's structural merge, Phase 37's multi-user sync, and Phase 31's one-file-per-actor world partition are the same decision expressed three times.

   Readers sometimes push back on this bet with the argument that binary asset formats are simply faster to load. That argument is true and irrelevant. The format a scene is *stored in* and the format a scene is *loaded from at runtime* are not the same file; the cook pipeline (Phase 9) translates the authoring `.ron` into a runtime-optimized binary, and the game loads the binary. Authoring speed and review-ability win at the source tree; load speed wins at the cooked output. The series chose not to sacrifice either for the other, and the choice came from Phase 4 forward.

3. **Capability-sandboxed.** Plugins (Phase 11), scripts (Phase 7), mods (Phase 38), and later remote services (Phase 34) all pass through the same capability envelope. A plugin declares what it wants — read-assets, write-scene, network-egress, filesystem-scope — and the editor or runtime grants the narrowest subset the user approved. No plugin in RustForge has ever shipped a `fs::remove_dir_all("/")` vulnerability, because no plugin in RustForge can open `/`. Unreal's plugin model is "native DLL trusted by default." We chose differently.

   The capability grammar is uniform across four domains — editor plugin, runtime script, shipped mod, remote service adapter — which is the property that makes it cheap to extend. Phase 38 did not have to invent a new sandboxing model for mods; it inherited Phase 11's. Phase 34's remote-service client code inherits the same capability declarations for network scopes. A 3.0 arc that added a new extensibility surface — say, an agent-authored tool — would inherit the grammar a fifth time. The compounding happens because the grammar was designed once, carefully, and reused.

4. **Web-first.** wgpu gave us Vulkan, D3D12, Metal, and WebGPU through one abstraction. The series committed to WebGPU + WASM as a first-class export target from Phase 22 forward — not a secondary port, not a platform special-case, but the second-most-tested target after Windows. The scorecard's +7 lead over UE in the Web category is not a lucky architectural accident; it is a deliberate bet that the browser is a platform, the wgpu community is a dependency worth aligning with, and cross-compilation to `wasm32-unknown-unknown` is a test that has to pass on every PR.

   Web-first has a secondary effect that shows up in platform-agnosticism across the board. Code that compiles to WASM cannot assume a file system, a network stack, a thread model, or a wall clock. Forcing the engine to pass that constraint on every subsystem made the engine better-behaved on iOS (which has its own sandboxing) and on consoles-if-they-ever-happen (whose platform SDKs have their own constraints). A codebase that compiles cleanly to WASM is a codebase that ports well. The series got Android, iOS, and the rough shape of portability by the same discipline that got us the browser.

5. **Accessibility-first.** Phase 13 baked UI scale, minimum font sizes, reduced-motion, color-blind-safe palettes, and AccessKit screen-reader labels into the editor before 1.0 shipped. Phase 15 did the same for the game-UI framework. The resulting RF accessibility score (8/10) against UE (4/10) is another bet that paid off: building accessibility in from day one costs perhaps 5% more engineering effort than skipping it, and retrofitting it later costs perhaps 100% more. The math is not subtle.

   The less-cited consequence of accessibility-first is the *discipline it imposes on UI code*. A widget author who knows, from the beginning, that the widget must answer to a screen reader and must scale to 200% without breaking layout, writes a different widget than one who does not. The ripple effect across forty-six phases of UI code is that RustForge's editor is usable in more *conditions* than Unreal's — on a laptop at a coffee shop, on a high-DPI display, by a developer who is tired, by a developer who is color-blind, by a developer on a slow network who just wants to run the compile-step and not render the viewport at full rate. None of that shows up as a single feature; all of it shows up as "this editor feels less hostile."

These five bets were not hedges. Each of them had a cheaper, more conventional alternative available on day one — C++ for the "safety" axis, binary assets for the "Git-native" axis, trusted native DLLs for the "capability sandbox" axis, native-first with a web port for the "web-first" axis, and "ship now, retrofit accessibility later" for the "accessibility-first" axis. The series took the more expensive path in each case, on the argument that the cost compounds smaller and the benefit compounds larger than the naïve comparison suggests. Forty-six phases in, that argument held.

A sixth, implicit bet runs underneath the other five: **self-host over SaaS.** The DDC (Phase 29), telemetry policy (Phase 13), multi-user session server (Phase 37), lobby/matchmaking/voice services (Phase 34), and plugin directory (Phase 23) are all shipped as binaries the user runs on their own infrastructure. No RustForge phase ever made a user depend on a RustForge-operated cloud service to author, ship, or run a game. We ship the tooling; the user runs the service. This is the commercial differentiator against Epic Online Services, Unity Cloud Build, and every other "free to start, $0.002 per request after" plan that every game-engine vendor has tried.

The self-host bet also has a governance consequence that shows up only in the long view. A hosted service is a single choke point for content moderation, export control, sanctions compliance, and whatever policy pressure the operator's jurisdiction cares about next year. A self-hosted service distributes that choke point to every operator. The series never claimed this was the more *profitable* model — it is demonstrably not — but it is the model that lets RustForge be used by a studio in a country whose relationship with the operator's country is tense at the moment, and lets that studio keep shipping. That property is easy to lose and hard to recover, and every phase that introduced a service kept it.

## What the series got right, stated plainly

Before cataloguing close-run bets and honest limitations: three things the series got clearly right, worth stating without hedging.

**The editor-as-client rule from Phase 1.** Treating the editor as a separate crate that links against the engine was the decision that made every later separation possible — the feature-gate wall in Phase 9, the `egui`-never-in-runtime invariant from Phase 15, the headless-tick discipline that lets CI run without a windowing system. Engines that do not draw this boundary at Phase 1 find themselves retrofitting it at Phase 20 and paying ten times the cost.

**The reflection registry from Phase 2.** One mechanism — `Reflect` derive + `ComponentRegistry` keyed on `TypeId` — underlies inspectors, serializers, undo commands, script bindings, replication fields, live-edit sync, prefab overrides, and plugin entry points. Engines that build these systems separately accumulate five overlapping, subtly-inconsistent reflection schemes. The series built one.

**The structural scene merge from Phase 12.** The payoff phase for Phase 4's text-scene decision. A game engine where `git merge` on a scene between two branches Just Works, with conflicts reported at entity-granularity rather than line-granularity, is a game engine a small team can operate without tooling heroics. Nobody else ships this. The series does.

These three together are the architectural core that the remaining 43 phases extend. A reader who takes nothing else from the series should take these three.

## The bets that almost went the other way

Not every architectural bet was obvious at the time. Three decisions ran closer than the finished series makes them look. Readers should know which ones, because the counterfactuals are instructive.

**`egui` + `egui_dock` for the editor shell.** The serious alternatives in late Phase 1 were a native shell per platform (cost: three OS integrations forever), Tauri + web UI (cost: a JS runtime in the editor process, plus a bridge), or a fully custom retained-mode UI (cost: a 3-5 phase detour before Phase 3 could draw anything). `egui` was immediate-mode, which ran against every "retained mode is mandatory for real UIs" instinct a reader brings, and `egui_dock` was a thin community crate with no production track record. The bet was that `egui`'s ergonomics would outweigh the feature gaps and that the community's momentum was real. The bet paid off; by Phase 23 the editor had a WYSIWYG UI designer (Phase 15) and a full F1 help system built on top of the same immediate-mode primitives. But a reader should note: if `egui` had stalled in 2024, every later UI phase would have been more expensive, and the series would have had to either fork `egui` or rewrite the shell. That risk was live for at least the first twelve phases.

**WASM as the scripting runtime.** The alternatives were a Rust dynlib with reload (fast, unsafe, painful on Windows), Lua or Rhai as a scripting language (no type sharing with reflection, cultural mismatch with Rust-first), or a fully custom bytecode VM (scope sprawl). Phase 7 picked WASM because the capability-grammar work from Phase 11 was easier to express against a sandboxed target than against a trusted dynlib, and because WASM toolchains were stable enough in the Rust ecosystem to bet on. The close-run alternative was Rhai — it was easier on day one, and a generation of indie engines have shipped it. Betting on WASM meant accepting slower script iteration in early phases for a payoff that only fully materialized at Phase 38 (modding). If Phase 38 had never been planned, Rhai might have been the better bet. The series was betting on its own later phases to justify the choice, which is a real risk and worth naming.

**`.ron` instead of `.yaml` or a custom format.** Phase 4's pick of RON (Rusty Object Notation) was a compromise: YAML was too lenient (tab-vs-space ambiguity, implicit types), JSON was too terse (no comments, no trailing commas, awkward for hand-editing), and a custom format would have carried a lifetime tooling burden. RON had the shape of a Rust struct literal, which made reflection serialization trivial, but it was a smaller ecosystem than YAML/JSON. The bet was that the win in reflection-friendliness outweighed the smaller tooling ecosystem. It did — but the cost was real: external tooling that understands YAML or JSON natively (some editors, some CI linters, some migration tools) had to be extended or wrapped for RON. Phase 45's migration tooling paid some of that cost back.

Three other bets ran close but are visible in the exclusion list above rather than the feature list — refusing visual scripting, refusing Perforce, refusing paid marketplace. Each of those refusals was argued against by real users at the time. The series held the line, and the current scorecard's "uncloseable gaps" entries for those three categories are the direct consequence. An honest retrospective acknowledges the users who wanted those features and did not get them; the engine that shipped instead is the engine the series chose to design.

## Phase coverage map

Forty-six phases, seven themes, one line per phase. The theme assignment is retrospective; no phase was constructed to fit a theme, but the themes describe how the phases cluster in practice.

| Theme | Phase | One-line core deliverable |
|---|---|---|
| **Foundation** | 1 | Workspace layout, editor-as-client-of-engine rule, `egui` + `egui_dock` shell picked. |
| | 2 | `Reflect` derive, `ComponentRegistry`, five engine-side hooks the other 44 phases lean on. |
| | 3 | Hierarchy, reflection-driven inspector, TRS gizmos, GPU picking — the first pixels. |
| | 4 | `.ron` scenes, stable `SceneId(u64)`, prefab format, project structure. |
| | 5 | Content browser, `notify`-based watcher, `.meta` sidecar GUIDs, drag-drop import. |
| | 6 | `Command` trait + `CommandStack` — every mutation routes through undo. |
| **Editor UX** | 7 | Play-in-Editor: snapshot/restore, play/pause/step, PIE banner, promote-rule. |
| | 8 | Specialized editors (material, terrain, animation, script) + baseline profiler. |
| | 10 | Frame graph, profiler tiers, stat HUD, structured logs. |
| | 12 | Git integration, text-scene structural merge, attribution surface. |
| | 13 | Preferences, keybindings, theming, accessibility, localization, welcome, crash recovery. |
| **Authoring** | 19 | Timeline, sequencer, keyframe animation, retargeting tables. |
| | 20 | Material node graph with reflection-typed pins, live preview, cooker integration. |
| | 24 | Animation graph runtime: state machines, blend spaces, IK, montages, root motion. |
| | 25 | Advanced physics authoring: PhAT, joint motors, ragdolls, constraints. |
| | 27 | Sky/atmosphere/weather — sky dome, clouds, day-night, wind fields. |
| | 32 | Terrain + foliage + splines — roads, scatter, RVT, erosion bake. |
| | 35 | Motion matching, physics-driven hit reactions, facial rigs, capture retargeting. |
| | 43 | Procedural character + crowd authoring (local-model Metahuman-class tooling). |
| | 44 | Narrative/quest/dialogue authoring stack with branching + localization hooks. |
| **Runtime** | 14 | Networking: QUIC replication, `#[replicate]`, prediction, reconciliation. |
| | 15 | Game UI framework: `.rui` retained-mode assets, taffy layout, WYSIWYG designer. |
| | 16 | Enhanced input: action/context stack, gamepad, `.rinput`, record/replay. |
| | 17 | Audio engine: mixer, buses, DSP, 3D positional, reverb zones. |
| | 18 | Particles + VFX, GPU simulation, emitter graphs. |
| | 21 | Advanced rendering: bindless, VSM, VCT, SSR, TAA-U, meshlet clusters. |
| | 26 | AI: behavior trees, navigation mesh, perception, blackboard. |
| | 31 | World partition, streaming cells, HLOD proxies, Level Instances. |
| | 33 | Replay/session recording: `.rreplay` file, scrubber, video export sidecar. |
| | 36 | Hardware ray tracing, path tracer, neural-rendering adapters — Ultra tier. |
| | 40 | Hair, cloth, fluid, destruction simulation suites. |
| | 41 | PCG: procedural content generation tooling in cook + streaming paths. |
| **Platform** | 9 | Build + packaging, feature-gated editor/runtime split, reproducible cook. |
| | 22 | Mobile + web platform expansion: iOS/Android/WebGPU/WASM. |
| | 28 | XR (VR/AR): OpenXR, controller input, stereo rendering, comfort. |
| | 29 | Shared DDC + team build server — self-hostable. |
| | 34 | Multiplayer services: self-hostable lobby, matchmaker, voice relay. |
| **Ecosystem** | 11 | Plugins: capability-gated WASM + native, plugin-api semver. |
| | 23 | Ecosystem: samples, templates, mdBook docs, F1 help, plugin directory. |
| | 37 | Multi-user live editing: self-hosted session server, presence, LWW sync. |
| | 38 | Modding runtime + live ops: sandboxed mods, capability envelope, delta patching. |
| | 45 | Migration tools: Unreal/Unity project importers, asset translation, report-driven porting. |
| **Advanced** | 39 | ML/AI-assisted authoring: generative material/mesh/behavior tooling, local-first. |
| | 42 | Insights-level profiling: timeline capture, CPU/GPU counter correlation, custom markers API. |
| **Retrospective** | 30 | 1.x retrospective + 2.0 roadmap — the inflection document. |
| | 46 | *This document.* Series retrospective + 3.0 vision. |

Five phases sit in two themes comfortably (19 is both Authoring and Editor UX; 34 is both Platform and Runtime; etc.); the table picks the dominant one. The grouping is not a design artifact, it is a reading aid. A reader wanting to understand "how does RustForge do networking" reads phases 14 + 33 + 34, not one.

A related observation: **six phases are the backbone**, in the sense that removing any one of them would force rewrites of five or more later phases. Phase 2 (reflection), Phase 4 (scene format + `SceneId`), Phase 6 (command stack), Phase 7 (PIE snapshot invariant), Phase 9 (feature-gate split), and Phase 11 (plugin capability grammar) are the six. Every subsequent phase leans on at least one, and most lean on several. A team picking up the map and rebuilding the engine from scratch should sequence those six first, resist the urge to reorder them, and treat their invariants as non-negotiable. The other forty phases have real flexibility; these six do not.

## The conversation with Unreal, restated

A theme that recurs across every phase is "how does this compare to what Unreal does." It is worth restating why that comparison shows up so often, because a reader new to the series might mistake the comparison for an adversarial framing.

Unreal is the most feature-complete game engine in the industry. It is also the most widely documented, the most widely taught, and the most rigorously benchmarked. Any design series that claims to build a competitive engine has an obligation to compare against it — not because UE is the only yardstick that matters, but because it is the yardstick a reader will compare against anyway, silently, whether the series invites the comparison or not. Making the comparison explicit makes it arguable.

The series' posture toward UE is neither hostile nor deferential. There are categories where UE is better; the scorecard names them. There are categories where RF is better; the scorecard names those too. There are categories where UE made a choice the series rejects on philosophical grounds — visual scripting, Perforce, the marketplace model — and the series says so without apology. None of this is a claim that Unreal is a bad engine. It is a claim that Unreal made a set of choices optimized for a different audience, and an opinionated engine optimized for the RF audience could exist alongside Unreal without needing to replace it.

A healthy engine ecosystem has more than one engine in it. The series is not trying to kill Unreal; it is trying to give developers who are not well-served by Unreal a credible alternative. That is a smaller claim and a more defensible one.

## Suggested reading orders

For a reader who comes to the series fresh and does not intend to read all 600k+ words, three reading orders produce different useful outcomes.

**The architect's path** (roughly 12 phases, the "how does the engine hang together" question): 1, 2, 4, 6, 7, 9, 11, 12, 14, 23, 30, 46. These are the invariant-setting phases plus the two retrospectives. A reader finishing this subset knows what the engine's skeleton looks like and where the bodies are buried.

**The implementer's path** (roughly 20 phases, the "I want to build this" question): 1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, then whichever vertical — rendering (20, 21, 27, 36), animation (19, 24, 35), simulation (25, 40), audio (17), UI (8, 15), networking (14, 34), platform (22, 28, 29) — the reader cares about. The implementer's path is deliberately redundant with the architect's path; a builder should read both skeleton and body before picking up a chisel.

**The evaluator's path** (roughly 6 phases, the "should my studio care" question): 1 (workspace + editor philosophy), 13 (1.0 polish), 23 (ecosystem), 30 (retrospective + 2.0 roadmap), the 38-phase scorecard, and this phase. A reader on this path can make a go / no-go decision in an afternoon.

None of the three paths is complete. A reader who cares deeply about some specific subsystem should read its phase in full, and the phases it references (each phase names its upstream dependencies near the top). The table of contents for the series, effectively, is the phase title index visible in the coverage map.

## Final scorecard snapshot

`unreal-vs-rustforge-scorecard-2.md` placed the 38-phase specified arc at **82% of UE feature coverage**. The table below extrapolates what phases 39–45 add, informally. These are estimates, not measurements — no scorecard-3 exists and this document does not construct one.

| Stage | RF % of UE | Key movements from prior stage |
|---|---|---|
| 1.0 (phases 1–13) | 64% | Editor core; runtime breadth not yet touched. |
| Post-1.0 (phases 1–30) | 80% | Runtime, authoring breadth, first retrospective. |
| 2.0 (phases 1–38) | **82%** | AAA parity: rendering, animation, streaming, modding. |
| 3.0 (phases 1–45) | **~88–90% (est.)** | ML/PCG/narrative/simulation/Insights/migration close most of what remained. |

Phases 39–45 close the categories the 38-phase scorecard still flagged as open and where a capability — not earned ecosystem maturity — was the gap:

- **Insights-level profiling (Phase 42).** The +12 gap in Developer Workflow on the 38-scorecard was overwhelmingly "Unreal Insights exists, RustForge doesn't." Phase 42 closes it with a dedicated timeline capture format, CPU/GPU counter correlation, flame graph viewer, and a custom markers API comparable to `TRACE_CPUPROFILER_EVENT_SCOPE`.
- **Hair/fluid/cloth/destruction (Phase 40).** Niagara Fluids, Chaos Destruction, and the hair/groom toolchain were the last simulation gaps. Phase 40 brings them to parity on the techniques that matter for shipped games, not the research frontier.
- **PCG + ML authoring (Phases 41 + 39).** The generative-tooling frontier — where UE5's PCG and the wider ML-assisted authoring space live — gets an RF-native answer that runs local-first, respects the capability sandbox for any model inference, and produces content that lands in the same asset pipeline as hand-authored work.
- **Procedural characters (Phase 43).** Metahuman-class procedural human authoring, with local-model options so users are not forced onto a vendor cloud.
- **Narrative stack (Phase 44).** A shipped AAA narrative game (RPG, VN, immersive sim) needs a quest/dialogue/branching/localization stack that is more than "an empty node graph and good luck." Phase 44 ships one.
- **Migration tools (Phase 45).** The single largest adoption blocker for an established engine is "I have an existing UE/Unity project; can I bring it?" Phase 45 says "partially, with a honest report," which is the only answer that is both useful and truthful.

What remains uncloseable after 45 phases is the same list Phase 30 and the 38-phase scorecard already flagged: ecosystem maturity (earned, not planned), Unreal Insights' decade-long tooling lead narrowed but not fully closed, visual scripting (philosophical exclusion), Perforce (rejected), consoles (structural platform blocker), and the paid marketplace (rejected). These categories account for the remaining ~10–12% and, by construction, cannot be moved by adding phases.

A final note on the scorecard's limits. Scorecards are a compression of reality, not reality. A 90%-of-UE score does not mean "a team can switch from UE to RustForge for 90% of their projects without friction" — it means "the measured surface area overlaps at 90%." The friction that matters in practice lives in places the scorecard does not weigh: developer familiarity, middleware vendor support, publisher comfort, QA tooling integration, performance at the exact bottleneck of the exact game being shipped. A rigorous 3.0 scorecard would try to weight categories by how often they actually block shipped projects; the two existing scorecards did not, and neither does the estimate above. Readers should treat the number as a *shape of progress* indicator, not a procurement metric.

## How the numbers moved over time

A different view of the same scorecard data. Tracking the RF-% against UE by stage shows where the bets paid off, where the payoff was delayed, and where the ceiling actually sits.

- **1.0 (Phases 1–13): 64%.** The editor-core phases established the invariants but could not close runtime gaps yet. Subtotals where RF already led UE even at this early stage: Workspace modularity, Accessibility, Undo/redo, Script sandboxing, Incremental cooking, Scene diffability. The architectural bets were already visible in the scoring; the feature breadth was not.
- **1.x (Phases 1–23): ~74% estimated.** Wave A runtime phases closed the biggest breadth gaps: networking, game UI, input, audio, VFX, sequencer, material graph, advanced rendering, mobile/web. Web-first started showing a lead here (+6 over UE, widened later to +7).
- **Post-1.0 (Phases 1–30): 80%.** Wave B depth phases plus the first retrospective. Animation, physics, AI, sky/weather, XR, DDC. The retrospective's framing of Phase 30 let the 2.0 arc be scoped to close specific gaps rather than chase features.
- **2.0 (Phases 1–38): 82%.** AAA parity phases. World partition, terrain, replay, multiplayer services, advanced animation, hardware RT, multi-user editing, modding. Three new categories pulled to parity (terrain, animation authoring, scene streaming); one new category added as an RF lead (modding runtime).
- **3.0 (Phases 1–45): ~88–90% estimated.** ML/PCG/narrative/simulation/Insights-class profiling/migration tooling close most of the remaining feature gaps. Ecosystem and Polish subcategories still account for the residual ~10–12%.

The shape of the curve is informative. The biggest gains per phase happen in Acts II and III; Act IV has diminishing returns per phase because each phase is closing a narrower gap against a harder target. This is the normal shape of a catch-up arc against a mature competitor: cheap wins first, then the expensive ones. A 3.0 arc would sit in an even more diminishing-returns regime on the UE feature-coverage axis, and should not be scoped on that axis — it should target directions Unreal is not emphasizing (industrial, scientific, generative authoring, web-first production), not the last 10% of UE's feature list.

## Where RustForge decisively leads Unreal

Nine categories show RF leading UE by margins that compound with every phase, because they are architectural bets rather than feature checkboxes. The 38-scorecard listed eighteen categories where RF leads by at least one point; the list below pulls out the nine where the lead is structural and therefore durable.

| Category | UE | RF | Why the lead is structural |
|---|---|---|---|
| Web target | 2 | **9** | wgpu abstracts WebGPU; UE's web path is not production-supported. |
| Scene merge | 4 | **10** | Text scenes + structural merge + `SceneId` invariants — compounding since Phase 4. |
| Plugin safety | 3 | **9** | Capability-gated WASM vs trusted native DLLs — a design choice UE cannot retrofit. |
| Language safety | 4 | **9** | Rust vs C++. Compounds at every call site, not just the ones we touched. |
| Accessibility | 4 | **8** | AccessKit + UI scale + color-blind-safe from Phase 13, not bolted on later. |
| Modding safety | 8 | **9** | Phase 38's capability envelope vs UE Mod SDK's trust model. |
| Incremental cooking | 8 | **9** | Content-addressed DDC + one-file-per-actor — smaller change radius than UE cooks. |
| Capability sandboxing | — | — | (cross-cuts; the lead shows up in plugin, script, mod, and service categories separately) |
| Linux parity | 7 | **8** | First-class CI target from day one, not a tolerated port. |

These nine are the architectural moat. Every phase that added a new feature touched at least one of them, and the feature inherited the lead for free. That is the compounding effect the series was betting on from Phase 1.

A second, less-visible category of leads sits below the headline nine: the *ergonomic* wins that do not show up in a scorecard because no scorecard has a row for them. Engine build times that do not require an incremental-linker plugin. Stack traces that point at the user's code in the first frame and not after thirty-seven inlined template instantiations. A reflection registry that is populated at build time by a derive macro and not by a 400-line boilerplate file. A plugin system where `cargo generate` produces a working skeleton. A content pipeline where the `.ron` file a designer edited is still the `.ron` file a programmer reviews. Each of these is a small win. They add up to an engine that feels smaller to use than Unreal, which is the property indie and mid-studio developers actually select on. No scorecard row captures it; forty-five phases of discipline produced it anyway.

## Lessons the series accumulated

Forty-five prior phases produced a set of generalizable lessons that are worth stating explicitly, because a 3.0 arc or a successor project would be strictly better off starting with them than rediscovering them.

**Design the invariants, not the features.** The six backbone phases (2, 4, 6, 7, 9, 11) succeeded because they designed primitives — a reflection registry, a scene format, a command stack, a snapshot model, a feature-gate wall, a capability grammar — that the other forty phases could lean on. When a later phase tried to short-circuit one of those primitives ("this one mutation can skip the command stack; it's internal"), the resulting mess was always worse than the cost of routing through it properly. The strongest recommendation the series can give to a future arc is to spend as much design effort on the invariants as on the visible features.

**Refuse fast and refuse specifically.** A phase that opens with a crisp list of non-goals reads faster, scopes better, and ages better than a phase that tries to cover the field. Every one of the forty-five phases had a "what this phase is not" section — sometimes one paragraph, sometimes a page — and the discipline of naming the refusals before the inclusions kept the scope tractable. A 3.0 phase that cannot name its refusals is not yet a phase.

**Reuse the existing pattern before inventing a new one.** A disproportionate share of the series' productivity came from noticing when a new system looked like an old system. The animation state machine (Phase 24) is structurally similar to the AI behavior tree (Phase 26); both are graphs evaluated on a tick. The material graph (Phase 20) is structurally similar to the particle graph (Phase 18) and the shader permutation system. The plugin capability grammar (Phase 11) is structurally similar to the mod capability grammar (Phase 38) and the remote service capability declarations (Phase 34). Each reuse saved a third of the design work for the later system and made the resulting code easier to reason about. A 3.0 arc should look for these opportunities aggressively.

**Write the documentation as you go, not after.** Phase 23 shipped the documentation portal, F1 help, and samples library; by the time it ran, a lot of prior phases needed backfill documentation because the authors had moved on. A healthier discipline would have been to require documentation as part of each phase's exit criteria. The series did not, and the cost showed up in Phase 23. A 3.0 arc should build the doc-as-you-go habit from Phase 1.

**The test harness is the most valuable non-feature.** Every phase had a "what does CI check about this subsystem" section. The cumulative test harness — Phase 7's snapshot invariant check, Phase 9's editor-feature-off compile, Phase 13's end-to-end editor-open-to-play-to-close smoke test, Phase 12's scene-merge determinism check — was the reason subsequent phases could evolve rapidly without regressing earlier work. A healthy 3.0 arc should treat the test harness as a first-class deliverable in every phase.

**Small teams + opinionated choices + Git history beats large teams + consensus + wiki pages.** The series is what one opinionated designer produced against a working engine. A committee would have produced a larger, blander, more hedged document with more features and fewer invariants. The series could only have been written the way it was — with sharp edges, rejected alternatives named by name, and philosophical bets declared openly — by a small authorship team willing to be wrong in public. A 3.0 arc should preserve that posture.

**The hardest phases to write were the boring ones.** Phase 9 (build and packaging), Phase 13 (polish and release), Phase 23 (ecosystem and documentation) had lower visible glamour than Phase 21 (advanced rendering) or Phase 36 (ray tracing), but they were substantially harder to design because their success criteria were qualitative rather than quantitative. A 3.0 arc should budget more design effort for the boring phases than for the glamorous ones, and resist the temptation to treat them as "cleanup" phases.

**Retrospectives are expensive and worth every word.** Phase 30 was an inflection. It cost as much to write as any feature phase and produced no new subsystem, and without it the 2.0 arc would have been a disorganized feature-bag. A 3.0 arc should plan for at least one mid-arc retrospective, and the next terminal retrospective after that. The discipline of looking back halfway through an arc is what keeps an arc from finishing in the wrong place.

**A "no" in the design series should be a "no" at the reviewer, not a "maybe later" punted to a later phase.** Several phases punt items to "a later phase" — Phase 14 punted matchmaking and voice to Phase 34, Phase 12 punted CRDTs to Phase 37, Phase 21 punted hardware ray tracing to Phase 36. Those punts were accurate. But there is also a pattern where "we should do X later" becomes a weaker form of "we decided not to do X" that burdens the later arc with undigested prior decisions. The series tried to flag every punt explicitly and justify it in the next phase; readers should treat any phrase like "deferred to a later phase" with skepticism and check whether the later phase actually picked it up.

**Prose is a reasoning medium, not just a documentation medium.** The design decisions in the series are sharper because they had to survive being argued out in writing. A spec document says *what* a system is; a design essay says *why the other options lost*. The second thing is where the reasoning lives. A 3.0 arc that collapses to spec prose has lost the medium in which design actually happens.

## ❌ Explicitly out of scope after 46 phases

Things the forty-six phases do not and will not cover, and which a 3.0 arc should not rush to add. Each exclusion was made on purpose, most of them multiple times.

- ❌ **Visual scripting.** Rejected philosophically. The engine ships first-class Rust, a scripting sandbox, and a reflection layer that a visual-scripting tool could be built on top of as a plugin. The core does not ship Blueprint equivalents, and 3.0 should not reverse that.
- ❌ **Perforce integration.** Rejected from Phase 12. Every argument for Perforce reduces to "binary asset locking at scale." RustForge's text scenes and structural merge obviate the primary Perforce use case; the remaining cases (huge binary art drops) are handled by Git LFS and the sparse-checkout flows documented in Phase 12.
- ❌ **Console platforms (PS5 / Xbox Series / Switch).** Structurally blocked by NDA-gated platform SDKs and wgpu's lack of a first-party console backend. A studio that needs consoles should use Unreal. This is not a gap the series is pretending to close.
- ❌ **Paid marketplace.** Rejected in Phase 23. We ship a curated samples library, a plugin *index* file in a Git repo, and an `F1` key that opens documentation. There is no billing, no DRM, no editorial queue, no account system. Third parties may operate a marketplace on top of the plugin index; the core does not.
- ❌ **AAA ecosystem maturity.** Can't be planned. Earned over years by the people who ship games on the engine. The plan gets RF to technical parity; the community gets RF to ecosystem parity, or it doesn't.
- ❌ **Hosted SaaS services.** The self-host-over-SaaS policy applies to DDC, telemetry, lobbies, matchmaking, voice relay, multi-user sessions, and live-ops patches. We ship the server binary; the user runs it. 3.0 should not introduce a hosted-service revenue model. If a commercial vendor wants to operate a hosted RustForge service, they may; the core does not.

Each exclusion is load-bearing. Reversing any of them would force a cascade of decisions in the other phases. The exclusions are as much a part of the design as the inclusions.

A healthy design series accumulates exclusions as confidently as it accumulates features. Each `❌` above represents a problem solved *by refusing to solve it*, which is a category of decision most engineering write-ups underrepresent. The forty-five prior phases contain many more such refusals — Phase 14's rejection of peer-to-peer and lockstep, Phase 17's decision not to build a DAW, Phase 23's decision not to run a billing service, Phase 34's decision not to host a relay — and each of those refusals kept the scope tractable. A 3.0 arc will need its own list of refusals; the test for a good one is that its refusals are at least as specific, and at least as confident, as its goals.

## Community-driven next steps

Forty-six phases describe a map of *core* engine work. A healthy ecosystem is larger than its core. The items below are explicitly *not* candidates for a 3.0 phase; they are healthier as community work.

- **Specialized importers.** SpeedTree, World Machine, Substance, Houdini HDAs, ZBrush GoZ, TrueSky, plus the long tail of niche DCC tools. Phase 5's importer plugin point is stable enough for these to live as third-party crates, versioned against the plugin-api, distributed through the Phase 23 plugin index. The core should *not* own these because each importer is a maintenance burden roped to a vendor's release cadence we do not control.
- **Asset packs and sample content.** Quixel-style Megascans, character-rig packs, UI kits, sound libraries, music packs. Phase 23's `pack.toml` format and `.gitattributes` conventions are the contract; everything else is community authorship. First-party packs exist to *demonstrate* the format, not to compete with community producers.
- **Platform-specific online services adapters.** Steamworks, Epic Online Services, Sony NP, Xbox Live, PlayStation Universe. These are SDK wrappers that sit on top of Phase 34's lobby/matchmaker/voice interfaces. The core ships the open-source self-host path; the adapters ship as plugins, maintained by whoever cares about that platform.
- **Genre-specific project templates.** Beyond the five first-party templates in Phase 23, the long tail — RTS, VN, RPG, tycoon, roguelike, rhythm, card, auto-battler, survival-crafting — is community work. Each template is small, opinionated, and easier to maintain by a genre enthusiast than by an engine team with no strong view on turn economy or card-draw mechanics.
- **Third-party plugins on the Phase 23 index.** This is the expected long-term growth surface. A healthy index has hundreds of plugins, most of which the core team never touched. The core's job is to keep plugin-api semver honest, not to audit every plugin.
- **Localization beyond English + the handful of EFIGS tier-1 locales.** Phase 13 shipped the infrastructure; filling the `.ftl` files is a contribution flow, not a core workstream.
- **Third-party DCC round-tripping.** Blender + Maya + Houdini + 3ds Max integration bridges. Phase 5's `.meta` GUID system and Phase 45's migration tooling provide the primitives; the round-trip scripts are community work.
- **Commercial support offerings.** A studio that wants a maintenance-contract relationship with someone who knows the engine should be able to find one — but that someone should be an independent consultancy, not the core project. The core project can compile a non-endorsing list of known consultancies; it should not operate one.
- **Per-vertical demo content.** Fighting-game frame-data viewer, racing-game setup tool, detective-game clue board, survival-crafting recipe editor. Each is a beautiful plugin and a terrible core feature. Community ownership is the right layer.
- **Engine telemetry dashboards and crash-aggregation services.** Phase 13 declared crash reporting opt-in and usage telemetry off. A studio that wants aggregated crash dashboards for its own shipped game should be able to run one (there are excellent open-source options: Sentry, Bugsnag-compatible backends, self-hosted Prometheus+Grafana stacks). The core project should not ship or operate such a service, and should not default-enable any integration with one.

Deciding what *not* to own in the core is as important as deciding what to own. A core that tries to ship every importer and every platform adapter becomes an org with a project-management problem, not an engineering one.

The healthy shape of this split is visible in the plugin-api contract. Phase 11 kept the plugin-api narrow on purpose; Phase 23 codified semver discipline; Phase 38 extended the same capability grammar to shipped-game mods. The result is that the core team audits the plugin-api surface, not the individual plugins, and the community authors the individual plugins with freedom to evolve faster than the core's release cadence. A 3.0 arc should resist the temptation to bring community-surfaced plugins into the core just because they became popular. Popularity is information about what should be *documented better*, not about what should be absorbed.

## 3.0 themes (optional future)

These are directions, not phase-specs. Somebody writing a 3.0 series should read them as a starting point for "what is the shape of the next arc" and not as commitments. Each direction has a different reason for being on the list; none of them is urgent after Phase 45 lands.

- **Generative tooling deeper integration.** Phase 39 brought ML-assisted authoring into the pipeline; a 3.0 arc could push it further into runtime behavior synthesis, script-from-prompt workflows, and content-variation systems driven by local models. The local-first, capability-sandboxed framing from Phase 39 is the invariant the 3.0 work should inherit. There is a legitimate design question about whether generative features should be built as *tools inside the editor* (narrow, targeted, capability-scoped) or as *an editing surface unto themselves* (an agent pane that can operate on the scene with command-stack integration). The series in Phase 39 chose the first; a 3.0 arc could revisit the choice with more evidence than we had.
- **MMO-scale backend parity.** Phase 14 + 34 + 37 cover up to "regional sharded, thousand-CCU per server" comfortably. An MMO is not one server with more players; it is a fundamentally different system — zone servers, world bus, persistence layer, auth tier, account service, social graph at scale, anti-cheat as a real program. A 3.0 phase or phases could attack the "how does RustForge ship a 100k-CCU MMO" question, and the answer is genuinely open. The self-host-over-SaaS stance is a harder fit in MMO territory — live-ops at MMO scale is expensive and often wants a hosted service layer — and a 3.0 arc would have to negotiate that tension honestly, rather than importing it silently.
- **Cinematic rendering pipeline parity with offline tools.** V-Ray, Octane, and Arnold define the offline rendering frontier. Phase 36's path tracer brings RustForge into that conversation; a 3.0 phase could push further — USD Hydra delegate, render-layer AOVs, Cryptomatte, render farm orchestration, deep compositing hooks. The audience is virtual production, not games, and that audience is growing faster than the games audience.
- **Scientific visualization applications.** wgpu + reflection + the Phase 23 plugin index make RustForge a plausible host for scientific-vis tooling: VTK-style pipelines, volumetric rendering for MRI/CT, ParaView-style filter graphs, large-point-cloud rendering for LiDAR. A 3.0 phase could target the sci-vis niche explicitly; the overlap with games tooling is 80%.
- **Industrial / digital-twin use cases.** The same argument: BIM import, CAD kernels, factory simulation, robotics SLAM integration, AR-anchored industrial overlays. Unreal has made a deliberate play here with Twinmotion and the industrial vertical. RustForge's Git-native, capability-sandboxed, self-hostable posture is probably better suited to regulated-industry customers than UE's is, and that is worth a 3.0 arc to explore. A "digital twin" deployment typically cares more about version control, audit trail, on-premise operation, and deterministic reproducibility than a game does — four properties the series is already strong on by construction. The fit is better than it first looks.
- **AI agents authoring inside the editor.** A meta-direction: the editor's reflection layer, command stack, and capability sandbox are good enough that an agent with a reflection-shaped API surface can be treated as another authoring client, alongside the human UI and the multi-user sync protocol. How to make the editor programmable-by-agent without turning every asset into a training target, and without collapsing into a SaaS posture, is its own research program.
- **Deterministic-by-default runtime.** The series treats determinism as an opt-in property (Phase 16's input replay, Phase 33's session recording, Phase 7's PIE snapshot, Phase 25's physics seed). A 3.0 arc could ask whether the whole runtime should be deterministic by default, with non-determinism gated by an explicit capability. The payoff would be free rewind for every game, trivial network rollback, and deeply cheaper debugging. The cost would be the set of subsystems that would need redesigning — audio DSP, particles, async I/O — to preserve determinism across platforms. A hard problem, and the next arc is the right place to decide whether to take it on.
- **Long-term preservation format.** Game preservation is not currently a first-class concern in engine design. A 3.0 arc could define a "preservation pak" — a self-describing archive of a shipped RustForge game, with enough metadata that a future RustForge (or a reference implementation in any language) could run the game correctly in twenty years. The technical problem is tractable; the philosophical alignment with the rest of the arc is strong.

Any 3.0 arc should preserve the five core bets. A 3.0 arc that ships a hosted SaaS, drops accessibility, or reverses the capability-sandbox default has stopped being the same project.

A 3.0 arc should also resist the *feature-list* framing that seduces engine vendors over time. Phases 39–45 were close to the line — "ML authoring, PCG, fluids, path tracing, Metahuman characters" reads like a competitor-feature-parity list, and it *is* a competitor-feature-parity list in the narrow sense — but each of those phases still had to earn its place against the invariants, and each was rejected-by-default until a specific invariant-compatible shape emerged. A 3.0 arc that adds a phase because a competitor shipped something has stopped doing design work; it is doing procurement work. The question "what invariant are we defending with this phase, and what invariant are we risking" should come before "what does this phase add."

Three questions any 3.0 proposal should answer before being accepted into the arc:

1. **What Act I–IV primitive does this reuse?** A new phase that does not cite reflection, the command stack, the PIE snapshot invariant, the feature-gate wall, or the capability grammar is probably growing a second core next to the first. That is rarely the right answer.
2. **What does this phase explicitly refuse?** A 3.0 phase that cannot name three things it is *not* building is under-scoped and will sprawl.
3. **How does this phase compound with the five core bets?** If the honest answer is "neutral" the phase is probably fine. If the honest answer is "slightly at odds with the capability sandbox but we can probably manage" the phase needs a redesign, not a compromise.

## The series in numbers

A rough quantitative summary, for readers who want one. Exact numbers are not the point — the orders of magnitude are.

| Axis | Approximate value | Notes |
|---|---|---|
| Phases | 46 | Including this one. |
| Cumulative word count | ~600–700k words | Most phases run 10–18k words; retrospectives shorter. |
| Cumulative build order | ~400+ discrete slices | Each phase's build order contributes 5–15 slices on average. |
| Named engine crates | ~60–80 | `rustforge-core`, editor, editor-ui, plus per-subsystem crates across phases. |
| Named first-party file formats | ~12 | `.ron` scene, `.meta` sidecar, `.rui`, `.rinput`, `.rreplay`, `pack.toml`, and a handful more. |
| Named invariants enforced by CI | ~20+ | Editor-feature-off compile, snapshot-restore determinism, scene-merge determinism, WASM compile, accessibility smoke, keyboard-only smoke, etc. |
| Platforms targeted | 7 | Windows, Linux, macOS, iOS, Android, Web (WebGPU/WASM), plus XR devices via OpenXR. |
| Platforms deliberately not targeted | 3 (PS5, Xbox Series, Switch) | Structural blocker; named explicitly. |
| Philosophical exclusions | ~8 | Visual scripting, Perforce, paid marketplace, hosted SaaS, peer-to-peer networking, lockstep, in-editor forum, usage telemetry — each rejected in a specific phase. |
| Scorecard categories where RF leads UE | 18 at 38-phase scope | Compounding architectural bets rather than individual feature wins. |
| Scorecard categories uncloseable | 5–7 | Ecosystem maturity, consoles, visual scripting, Perforce, marketplace, parts of Polish. |
| Estimated engine-implementation-year cost | not estimated by the series | Design series deliberately does not quote eng-years. |

The last row is deliberate. A design series that quoted an implementation budget would be substituting authority for honesty; the real cost depends on team shape, language familiarity, and how many invariants are imported versus rebuilt. Readers who need a build plan should produce one from the phases, not read one out of this document.

## A note on audience

Who is the forty-six-phase series for, actually? The question is worth answering because the answer shaped the writing.

**Engine maintainers at established engine companies** — a small audience, but one who can import ideas into projects with real distribution. A UE or Unity engineer reading this series is not going to switch engines, but may carry a technique back: the capability grammar, the structural scene merge, the feature-gate wall. Each of those ideas is patentless, license-compatible, and generalizable. If the series influences engine design at Epic, Unity, CD Projekt, Guerrilla, or a studio's internal engine team, it will have done the work the series hoped it would do.

**Indie and mid-studio game developers** — the target customer for RustForge if it ever ships. Readers in this group are looking for "is this engine plausibly better than Unity + N plugins for my next game." The scorecard tries to answer that question honestly; the phase-level detail lets a reader verify the scorecard against their specific needs.

**Rust developers curious about what a Rust-first engine could look like** — a larger audience than the first two combined, and the audience least well-served by existing engine documentation (which assumes a C++ or C# starting point). The series tries to be readable by a Rust developer who has not worked in game engines, without being condescending to one who has.

**Future RustForge contributors** — if such a community forms. The decision trail is for them above all. A contributor opening a PR against Phase 20's material graph should be able to read Phase 20, understand why pin types are reflection-resolved, and argue with the decision from a position of having read the argument. Opaque codebases accumulate bus factors; documented decision trails distribute them.

**The author's own future self** — always a real audience. Design decisions made in Phase 4 were already hard to reconstruct by Phase 20 without the phase document to look back at. The writing acts as a memory aid for the authoring team, which is the unglamorous but load-bearing reason design series exist.

Notably absent from the audience: AAA studio greenlight committees, investors, marketing departments, and academic peer reviewers. The series does not pretend to serve any of those audiences, and readers from those constituencies should adjust their expectations accordingly.

## Honest limitations as a design series

Forty-five prior design documents and this retrospective together are **a map, not a build**. That distinction is load-bearing. A design series preserves decisions, names the trade-offs, records the architectural trail, and lets a future engineer or reviewer re-derive why something is the way it is. It does not:

- **Guarantee implementability.** A phase's design can be sound and still take four times longer to build than its prose implies. None of the forty-five phases claim a specific engineering-year cost; none of them should be read as one.
- **Survive contact with reality unchanged.** Some opinionated choices — the `.ron` scene format, the WASM plugin default, the `egui` + `egui_dock` shell — would hold under real-world contact. Others — specific DSP graph shapes in Phase 17, specific UI designer ergonomics in Phase 15, the exact capability grammar in Phase 11 — would be revised in the first six months of use, and that is fine. The decision trail makes the revisions cheaper, not unnecessary.
- **Substitute for a community.** A design series is a one-way broadcast. It records the authoring team's view at a point in time. The community that ships games on this engine will find things the authoring team missed and opinions the authoring team was wrong about. A 3.0 arc should be co-authored with that community, not by the same seat.
- **Claim completeness.** Forty-six phases is a lot of paper. It is also not everything. The exclusions list above is partial by construction — we listed the exclusions we were confident of rejecting, not the exhaustive set. The next real project built on this map will encounter a question no phase covered, and answer it in the moment. That is the normal case, not a failure of the series.

The value of what precedes this document is the **decision trail**: which bets were made, which refused, which proved structural, which proved close-run. An implementer who disagrees with a decision now has a clear target to argue against, and a set of invariants to respect while arguing. That is what a design series is *for*.

One more honest limitation, specific to a design series written without a shipping implementation to discipline it: **feedback latency**. A design series can go wrong slowly, in ways only a running codebase surfaces — a premature abstraction, a too-narrow trait, an over-eager invariant. The forty-five prior phases were written with as much discipline as prose can supply, and each later phase was a check against the earlier ones, but prose is a weaker check than a compiler. A reader should treat any of the forty-five phases as a *claim* that would need to be verified against an actual build, and should expect that perhaps one claim in ten would need revision on first contact. Flagging which one in ten is the implementer's job; the series does not claim to have done it.

That said, the *shape* of the plan — the phase ordering, the invariant set, the five core bets, the exclusion list — is much more durable than any individual claim inside it. A reader who cares about "did the right subsystems get built, in the right order, against the right constraints" is likely to find the series right. A reader who cares about the exact shape of a specific trait signature is likely to find the series wrong in ways that do not matter much.

And a final honest limitation: **the series has one author's voice**. Every phase reflects the same set of priorities, the same set of rejected-by-default alternatives, and the same rhetorical style. A healthier long-term future for the project involves voices that disagree with this one — on the `.ron` decision, on the visual-scripting refusal, on the self-host-over-SaaS policy, on the scope of Phase 45's migration tools — and that negotiate those disagreements in the open. A 3.0 arc written by a committee that includes the skeptics would be a better document than a 3.0 arc written in the same single-author style. The series is not a model to preserve stylistically; it is a starting point to improve on.

## Closing

The first phase described a crate that did not exist, for an engine that could not yet build the editor as a sibling. The forty-sixth phase describes the shape of what was planned, for a reader who may or may not ever build it. Neither document is the whole story. The story is what happens between them if somebody picks up the map and walks.

A design series is a strange artifact. It is not a specification — specifications are precise enough to implement mechanically, and these documents are not. It is not a vision document — vision documents declaim, and these argue. It is not a book, because it has no consistent narrator voice and no beginning-middle-end beyond the one this phase is reluctantly providing. It is not a product roadmap, because a roadmap commits to delivery dates and this series commits only to internal consistency. What it is, closest to any existing genre, is the accumulated minutes of a design review that ran for forty-six sessions with exactly one author in the room. That is not a genre most engineering writing occupies. Readers who were looking for a tidier form of it should understand that the untidiness is half the value — the dead ends, the close-run decisions, the occasional phase that reversed a prior phase's guidance, the phrases like "we do not yet have a good answer for" that appear in more than one phase — those are all information a sanitized specification would have erased.

The posture of the series was: **write the decision down, name what it gives up, commit.** That posture is cheap to describe and expensive to maintain. Somewhere around Phase 25 or 26 the temptation to flinch from a hard choice became real; around Phase 30 the temptation to declare victory and stop became real; around Phase 38 the temptation to write the seven remaining phases as a feature list became real. The series resisted each temptation with varying success. A reader who notices a phase where the resistance wobbled should trust their instincts — the series is not flawless, and some phases are better argued than others.

To the engine maintainers who made Phase 1's "editor is a client" rule non-negotiable; to the reflection and reflection-macro authors whose work made every later phase cheaper; to the Rust ecosystem crates the series leaned on without apology — `egui`, `egui_dock`, `wgpu`, `notify`, `quinn`, `rapier`, `taffy`, `gilrs`, `AccessKit`, `wasmtime`, `bevy_reflect`'s ancestors, and dozens more — whose authors took on load-bearing responsibility without knowing this series existed; to the contributors who would fill the `.ftl` string tables, ship the first community plugins, write the first external importers, and file the bug reports that none of the phases anticipated; to the readers who made it from Phase 1 to Phase 46 one document at a time; to the skeptics whose pushback on Phase 4, Phase 11, Phase 12, and Phase 23 made each of those phases more defensible than it would have been otherwise; and to the person, not yet introduced to this project, who will decide that a 3.0 arc is worth writing — thank you. The trail is marked. The territory is open.

The forty-six-phase arc exits here.

What happens next is not the series' call. A community may pick up the map, argue with its choices, fork it, fix it, extend it, or leave it on the shelf. Any of those outcomes is acceptable. A design series that gets picked up and reshaped is a design series that was worth writing; a design series that is politely set aside because a better one comes along is also a design series that was worth writing. The only failure condition is a design series that sits in a repository nobody reads and influences nothing. That is the outcome this document, and the forty-five that precede it, exist to prevent.

If the map has any durable value, it is this: engines do not have to be the way Unreal, Unity, and Godot chose to be. A game engine can be Rust-first, Git-native, capability-sandboxed, web-first, accessibility-first, and self-host-over-SaaS, and the result is not a toy — it is a production-capable tool that wins decisively in nine categories and loses only where the wins were rejected on principle. That possibility exists whether or not this specific engine gets built. A successor project that reads this series and builds a different engine along the same principles would be, from the series' point of view, a success.

Forty-six phases is a long walk. The reader who finished the series one phase at a time has put in weeks of reading. The author who wrote it one phase at a time has put in considerably longer. Neither quantity proves the plan is right; both quantities prove the plan was honestly attempted. That is the most a design series can offer.

The last thing to note is that the arc is not a ceiling. Nothing in this document closes the door on a 3.0 arc, a 4.0 arc, a complete rewrite, or a fork that takes the invariants and discards the specifics. Any of those outcomes is compatible with the series having served its purpose. The purpose was to lay down a decision trail crisp enough that the next set of decisions can improve on it. Whether the next decisions come from the same authorship, a new authorship, or a community that rejects parts of the framing — the trail is ready for any of them.

The forty-six-phase arc exits here. Thank you for reading.

---

*End of series.*
