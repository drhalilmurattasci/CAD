# Phase 30 — Retrospective, Remaining Gaps & 2.0 Roadmap

Thirty phases ago, Phase 1 was four pages of boxes-and-arrows naming the crates an editor would need. The claim was modest: a game editor for a 47k-LOC Rust engine, shipped as a separate crate behind a feature flag, built one patch cycle at a time.

Twenty-nine phases later the surface area covers an editor, a retained-mode game UI layer, three different node graph domains, a QUIC replication stack, a particle and a material system, a timeline and a sequencer, an animation graph and a physics refit, AI, weather, XR, and a shared content pipeline. Every one of those phases was a single, opinionated design document written against the plan that preceded it. None of them were community RFCs. All of them said no to at least one thing users wanted. The cumulative word count is somewhere north of 400,000 words, the cumulative build order is somewhere north of 300 discrete slices, and the cumulative set of explicit non-goals is its own document.

This final phase does not design a subsystem. It looks back. A retrospective has three jobs, and the order matters.

The first is to be honest about what got built, what got deferred, and why — not a marketing pass, not a celebration, but an audit. The second is to name the bets the series made — which ones compounded, which ones turned out to be closer calls than they felt at the time — and score them against the current state. The third is to sketch what a successor series *would* cover if anyone were to start one, while resisting the temptation to design it here.

This document is a lookback, a gap analysis, and a forward map. It is not a new implementation plan, and the items named in §5 are candidates, not commitments. Readers looking for a checklist at the end of this phase will not find one. There is no "Exit criteria" section. The thirty-phase arc exits when the last paragraph ends.

## 1. The arc in review

Three acts, twenty-nine phases. Each paragraph below is the compressed memory of a document five to fifteen thousand words long. The compression is lossy on purpose — the goal is to restate each phase in one or two sentences dense enough to trigger the right memory in a reader who was there, not to substitute for re-reading the source.

### Act I — 1.0: the editor itself (Phases 1–13)

Thirteen phases, one deliverable: an editor users would trust to ship a game with. Each phase left the editor in a working state; no phase was a prerequisite that delivered no user-visible value on its own.

**Phase 1 — Editor plan.** Named the crates: `rustforge-core`, `rustforge-editor`, `rustforge-editor-ui`. Locked in the "editor is a client of the engine, never bolted in" rule that every later phase inherited. Picked `egui` + `egui_dock` for the shell. Listed the engine-side hooks (reflection, headless tick, render-to-texture, picking) as prerequisites before any UI code.

**Phase 2 — Reflection & engine hooks.** Built the `Reflect` derive macro, the `ComponentRegistry` keyed on `TypeId`, and the five API surfaces (reflection, headless tick, render-to-texture, picking, event bus). This is the phase the other 28 lean on; every later inspector, serializer, undo command, scripting binding, and replication field resolves through it.

**Phase 3 — Hierarchy, inspector, gizmos, picking.** The first phase with pixels. Translate/rotate/scale gizmos, GPU-picking pass, the reflection-driven inspector that generates its widgets per-type.

**Phase 4 — Scene I/O and `SceneId`.** Committed to `.ron` for scenes and baked the stable `SceneId(u64)` that every merge, replication, and undo operation would later key on. Text scenes + structural merge was the single most consequential format decision in the series.

**Phase 5 — Content browser and asset pipeline.** `notify`-based file watching, thumbnail generation, `.meta` sidecar GUIDs, drag-drop import. The asset pipeline from here forward is one-way: disk → watcher → importer → registry → UI.

**Phase 6 — Undo/redo.** The `Command` trait, the `CommandStack` resource, the invariant that *every* mutation routes through a command. No shortcuts. No "this one's small." The reason most of the editor's later features compose cleanly is this phase's stubbornness.

**Phase 7 — Play-in-Editor.** ECS snapshot-and-restore, play/pause/step, the PIE banner, the rule that nothing the user does in Play persists after Stop unless they explicitly promote it. The snapshot invariant is the one later phases (networking, sequencer, audio, physics) each had to honor.

**Phase 8 — Specialized editors (baseline).** Material property sheet, terrain sculpt/paint, animation preview, script editor. Three of these later grew teeth: material got its node graph in Phase 20, animation got state machines in Phase 24, script stayed a text editor and that was correct.

**Phase 9 — Build & packaging.** Cook pipeline, feature-gate split, `rustforge-core` builds green without the `editor` feature. The wall between editor and shipped game was drawn here and has not cracked.

**Phase 10 — Debugging & diagnostics.** Frame graph, profiler tier, stat HUD, structured logs. Set the bar for the per-subsystem diagnostics every runtime phase from 14 on had to meet.

**Phase 11 — Plugins.** Capability-gated WASM plugins with a narrow host API. The plugin security story is the category where RustForge decisively beats Unreal — because Unreal chose not to have one.

**Phase 12 — Version control & structural merge.** The payoff phase for Phase 4's text-scene bet. `git merge` on a scene where two authors edited different entities just works, keyed on `SceneId`. No locking, no check-outs, no `.umap` binary conflicts.

**Phase 13 — Polish & release.** Preferences, keybindings, theming, accessibility, localization scaffolding, welcome, crash recovery, quality gates. The ship-it phase. The moment 1.0 was declared.

### Act II — Post-1.0 Wave A: runtime and authoring breadth (Phases 14–23)

Ten phases filling in the runtime systems that a shipped 1.0 editor had exposed as gaps. The users who showed up for 1.0 asked for multiplayer, for game UI, for audio, for a real input system, and for authoring tools that closed the deferrals from Phase 8. Act II answered them in order.

**Phase 14 — Networking & replication.** Client-server authoritative architecture. `#[replicate]` tied to the reflection registry, QUIC via `quinn`, client-side prediction with server reconciliation, `NetId` for runtime entities layered over the `SceneId` baked by Phase 4.

Three RPC channels (reliable-ordered, reliable-unordered, unreliable). Two `World`s in PIE for testing without a LAN. Peer-to-peer and deterministic lockstep were rejected by name in §1.1 and §1.2. Anti-cheat and matchmaking were deferred by name in §13.

**Phase 15 — Game UI framework.** `.rui` retained-mode assets. `taffy` flexbox for layout. WYSIWYG designer panel following the Phase 8 `AssetEditor` pattern. Data binding through reflection. State-transition animation. Gamepad focus navigation with `AccessKit` integration.

Drew the line that `egui` is editor-only and never ships in a game binary. The enforcement is a build check that fails if `egui` appears as a dependency of `rustforge-core` with the `editor` feature off — a convention enforced by CI rather than by reviewer memory.

**Phase 16 — Enhanced input.** Runtime action/context stack distinct from the editor's keybindings. `gilrs` gamepad support. Modifier/trigger/composite model. `.rinput` assets. Per-player save slots. Recording-and-replay of the action stream for determinism-sensitive features (replays, netcode, tests).

The invariant: gameplay code reads actions, never raw devices. Script hot-reload (Phase 7) does not lose a player's remap mid-session.

**Phase 17 — Audio.** `cpal` device. Mixer graph with typed busses (Music / SFX / Dialog / UI). 2D and 3D sources with distance attenuation and optional HRTF. Streaming-vs-decoded split for memory. `.raudio` DSP graph. Opus cook.

First phase to land the reusable `rustforge-node-graph` widget — initially prototyped inside the audio crate, then extracted in Phase 20 for Phases 18, 24, 26 to share.

**Phase 18 — Particles & VFX.** Four-module emitter pipeline (spawn, init, update, render). GPU-compute simulation path with CPU fallback for emitters below a threshold and for WebGL2 targets. `.rvfx` graphs authored in the shared node widget. Distance LOD. Depth-buffer collision for cheap environmental bouncing. Seeded determinism per emitter. Trails and ribbons as first-class render types.

Explicitly scope-limited: no fluid, no cloth, no volumetric clouds — each named as out of scope with a reason.

**Phase 19 — Timeline & sequencer.** One `.rtimeline` format for both animation clips (bound to a skeleton) and cinematic cuts (bound to entities and properties). Keyframe + curve editor with tangent modes. Transform / reflected-property / subsequence / camera-cut / event / audio tracks. Event fires into WASM. Take Recorder that auto-keyframes reflected-field changes during PIE.

Scrubs are transactions on the Phase 6 command stack; sequencer overrides restore on Stop exactly like PIE snapshots. One of the phases that most cleanly demonstrated the payoff of the invariant discipline — scrub-and-revert composed with PIE-and-revert without either knowing about the other.

**Phase 20 — Material node graph.** Closed Phase 8's deferral. Typed DAG with `float`/`Vec2`/`Vec3`/`Vec4`/`texture`/`sampler` ports. WGSL codegen with hash-keyed compiled-shader cache.

Source-mapped errors back to originating nodes. Master + instance materials (parameter override only, no topology change in instances). Feature-bit permutations capped at 32. Plugin-authored node palette. Subgraphs as `.rmatfunc` assets. Extracted `rustforge-node-graph` as a standalone crate for future domains.

**Phase 21 — Advanced rendering.** Clustered-forward refit. Volumetric fog and height fog. Virtual shadow maps equivalent at wgpu fidelity. Temporal upscaling (FSR-style). Screen-space GI approximation. Reflection probes. Meshlet-based LOD and occlusion culling.

The "go wide on fidelity without inventing Nanite or Lumen" phase. Named where the gap to UE5 rendering remains structural (HW-RT, Nanite virtualized geometry) and where it doesn't (everything else).

**Phase 22 — Mobile & web targets.** iOS and Android builds via wgpu's Metal/Vulkan backends. WebGPU target. Touch-input pathway through the Phase 16 action stack (touch events become gesture actions). Texture-format variants in the cook (ASTC for mobile, BCn for desktop). Package size budgets enforced in CI. Shader permutation cap per platform so mobile doesn't blow up compile times.

Console ports remained out of scope and named as such. The phase that proved the `wgpu` bet by shipping the same renderer to three new platforms with a cook-time texture reformat rather than a renderer rewrite.

**Phase 23 — Ecosystem close.** Documentation site auto-built from reflection docstrings. RFC process with a template and review cadence. Governance charter. Plugin registry with capability-manifest surfacing. `rustforge-core` API stability policy (SemVer-strict, breaking changes gated on majors). Release cadence.

The point where design-by-one-author ends and community-driven evolution begins. This is the phase that licensed Phases 24–29 to exist as smaller, more surgical documents instead of grand designs.

### Act III — Post-1.0 Wave B: depth & high-end (Phases 24–29)

Six phases pushing the engine into the depth categories: animation state machines rather than clip playback, physics that includes characters and vehicles rather than rigid bodies, AI that includes perception and EQS rather than just navmeshes, sky and weather as a subsystem rather than a skybox texture, XR as a first-class target rather than an afterthought, and a shared content pipeline that makes team-scale projects viable.

**Phase 24 — Animation graph.** State machines with transition conditions. 1D and 2D blend spaces. Layered blends with per-bone masks. IK nodes (two-bone, FABRIK, look-at). Notify events firing into WASM at timed points in clips. Runtime parameter bindings exposed as reflected component fields.

Reused the Phase 20 node widget for its fourth domain. Closed the long-standing animation-authoring gap that Phase 8 had explicitly deferred. Runtime parameter changes drive state transitions; transitions are deterministic given the same parameter stream, which matters for Phase 14 replication.

**Phase 25 — Advanced physics.** Refit on top of Rapier. Kinematic character controller with slope handling and step-up. Vehicle model with suspension and tire friction. Joint library (hinge, slider, ball-socket, generic 6DOF). Trigger volumes with on-enter/on-exit events into reflected callbacks.

Destruction via pre-fractured proxy meshes (cluster-break on impulse threshold). Cloth via position-based dynamics with wind field reads from Phase 27. No FEM, no large-scale fluid, no continuous fracture simulation — those were named and deferred in §9.

**Phase 26 — AI.** Behavior trees in the shared node widget (domain five), navmesh generation via Recast and query via Detour, EQS-style environment queries with weighted scoring, perception (sight cones, hearing with sound events), utility scoring for decision nodes, blackboards as reflected components so AI state is inspectable and replicable. Enough to build an RPG or a stealth game, not enough to claim GOAP or HTN parity — those were named as out of scope.

**Phase 27 — Sky, weather, atmospherics.** Physical sky (Hillaire-style precomputed scattering), time-of-day with sun and moon angles, precipitation particles hooked to Phase 18's emitter system, wind as a global field that cloth (Phase 25), grass, and VFX all read, weather state as a scriptable component so gameplay can react to rain, snow, fog. Deliberately stopped short of a clouds-as-a-subsystem build — volumetric clouds get a screen-space approximation and no simulation authoring tool.

**Phase 28 — XR (VR/AR).** OpenXR integration, stereo viewport in the editor for authoring, hand and controller abstractions routed through the Phase 16 action stack so gameplay code doesn't care which XR runtime is active, comfort settings (vignette, snap turn, teleport locomotion), foveated rendering hook for adapters that expose it. VisionOS and Apple-specific XR were called out as not in scope and remain candidates for 2.0 (§5).

**Phase 29 — Shared DDC.** Content-addressed cook cache with team-shared tier (on a local network share or an S3-compatible bucket) and CI-shared tier. Integrity verification via content hashes. Garbage collection policy with configurable age and size caps. Integration with the build pipeline from Phase 9.

The performance-last-mile phase: the cook that took ninety seconds on a cold project now takes two when one teammate has already paid the cost. First phase to make team-scale RustForge projects viable rather than merely possible. The one phase of Act III where the main benefit is felt by non-author teammates before it's felt by the author.

## 2. What the series deliberately didn't build

Being honest about gaps matters more than being exhaustive about features. The following are named gaps — things users of comparable engines expect, which RustForge does not have, and which were considered and deferred rather than forgotten. Each carries a reason, not an apology.

**Hair and fur grooming.** A real grooming pipeline requires strand authoring, clump simulation, skin attachment, dynamics that don't blow up on low-frame-rate spikes, and LOD down to cards for distant views. It's a multi-year investment, and the audience is narrow enough that most mid-tier engine users never touch it. RustForge ships cards-only for stylized hair via the Phase 18 particle system (ribbons) and standard skinned meshes. Groom authoring goes to 2.0 or never.

**FEM destruction and large-scale fluid simulation.** Phase 25 shipped pre-fractured proxy destruction (Chaos's cheap layer) and position-based-dynamics cloth. Finite-element destruction with continuous fracture, plus SPH/FLIP fluids, plus Chaos-tier rigid body counts — these are specialist workloads that deserve their own physics track led by someone who has shipped one before. Not attempted; named as not attempted.

**Motion capture authoring pipeline.** Import of `.fbx` / `.abc` mocap clips works through Phase 8's importer and the Phase 24 animation graph can play them. A cleanup suite — retiming, rig solving, trajectory blending, mocap-to-keyframe conversion, noise reduction — does not ship. Third-party tools (MotionBuilder, Cascadeur, Blender) fill the gap, and the import-cook-play path supports their outputs.

**Ray-traced global illumination.** wgpu's hardware-RT surface is still young and the ecosystem is non-uniform across vendors as of the phases' writing. Phase 21 shipped screen-space GI, light probes, and reflection probes — the same approximations shipped engines used before HW-RT. HW-RT GI waits on wgpu's ray-tracing backend maturing and on the hardware floor rising. This is a timing issue, not a principled rejection.

**Console platform ports.** PS5, Xbox Series, and Switch SDKs are NDA-gated and require dedicated backend teams per platform. Phase 22 drew the line at mobile and web, where the SDKs are open. Consoles are a business-model question more than a technical one: a studio that wants to ship RustForge on a console needs to licence the SDK, staff a backend team, and maintain the port. That is a company's job, not a phase's.

**Visual scripting for gameplay logic.** Deliberate rejection, repeated. The scripting story is Rust (native) and WASM (sandboxed), and a Blueprints-equivalent would fragment the gameplay code authoring surface into two languages. The refactoring leverage Rust gives — rename a component field and every caller either updates or fails to compile — halves the moment a visual script references that field by name through a runtime reflection lookup. The Phase 7 rejection and the Phase 26 restatement both name this cost explicitly.

**Facial performance capture.** Adjacent to mocap. Blend-shape authoring and playback work; a facial-capture solve from video or depth sensor input does not. This is specialist-tool territory where the upstream tools (Faceware, LiveLinkFace, MetaHuman Animator) are industry-specific enough that integrating rather than reinventing is the right move.

**Cinematic-quality path tracer.** For in-editor preview and offline renders. wgpu's compute-path-tracer story is buildable but it's a standalone track that would compete for renderer engineering time with the real-time pipeline. The current offline path renders through the real-time pipeline with long accumulation, which produces credible stills for marketing but is not a path tracer. Named as such.

**VisionOS and Apple-specific XR.** Phase 28 ships OpenXR. Apple's XR stack (RealityKit / visionOS) is not OpenXR-compatible at the time of these phases and would require a separate backend behind the XR abstraction. Candidate for 2.0 (§5).

**Procedural content generation tools.** A Houdini-style procedural graph for levels, vegetation scatter, city generation, asset variation. The shared node widget could host it; the command stack integration is the hard part; no phase scoped the domain. Candidate for 2.0.

**Full Nanite-parity virtualized geometry.** Phase 21 shipped meshlet-based LOD and occlusion culling, which covers the 80% case of "artists want to import higher-poly meshes without manual LOD." A software-rasterizer cluster pipeline in the Nanite style — letting artists import unconstrained triangle counts with subpixel error — is a renderer research project that took Epic years to ship. Not a phase; a product line.

**Dedicated animation-retargeting UI at Unreal's depth.** Phase 24 ships a basic retarget via a bone map and simple IK adjust, enough for most humanoid-to-humanoid transfers. The interactive retarget pose editor with correctives, compensated rotations per bone, and side-by-side skeleton preview is not in there. Named as a depth gap rather than a structural one.

Each of these is defensible, and each is a gap. The job of the retrospective is to say both.

## 3. Architectural bets in retrospect

A bet is a decision made early with incomplete information whose consequences compound across later decisions. The series made seven big ones. Scoring them in hindsight:

### Text scenes + structural `SceneId` merge (Phase 4 / Phase 12)

Payoff: high. This is the category where RustForge decisively beats Unreal.

Git workflows on binary `.umap` files are the state of the art for a reason — they don't work — and every user who wanted that to change got a tool that delivers it. The structural merge driver paid for the cost of `.ron` parsing a hundred times over. Two designers editing different entities in the same scene file now merge without a phone call, without a lock, and without either one throwing away work.

The only real cost is scene load time at 50k+ entities, which Phase 9's cooked format hides from shipped games entirely and which Phase 29's DDC shortens substantially for editor sessions. The worst-case scene that ever showed up in testing took four seconds to parse and was not anyone's authoring workflow. Bet paid.

### Reflection-as-one-job vs UObject-as-everything (Phase 2)

Payoff: high but asymmetric. This is the closer call of the seven.

UObject does reflection, GC, serialization, replication, and transactions in one type. That is simultaneously the source of UE's greatest strength — everything composes through one reflection system — and its greatest cost: UObject dispatch overhead, GC pressure, and a single type that accretes every concern anyone ever needed from an engine-owned object.

RustForge's choice — reflection is for properties, GC is absent because Rust, replication (Phase 14) and transactions (Phase 6) compose on top of reflection rather than through it — is cleaner by an order of magnitude and gives up deep integration in exchange. In practice, the compositions worked: `#[replicate]` slots into the reflection registry, the command stack serializes through reflection, the UI designer binds through reflection. But there is a category of features (the kind that assume "give me every property of every object and let me do X to it") that is marginally harder to write against a registry than against a base class.

Net: the bet paid, and it was closer than Phase 2 acknowledged at the time.

### `editor` feature flag gating (Phase 1 / Phase 9)

Payoff: very high. This might be the single most compounding decision of the series.

Shipped games pay zero cost for editor code. No editor deps, no editor types, no editor code paths in the binary. `rustforge-core` without `editor` is a plausible library to embed in a non-editor product — a server, a headless renderer, a test harness.

The discipline this required was real and recurring. Every later phase had to answer, in CI, "does this still build without `editor`?" Every convenience function that slipped into the wrong crate got bounced. The inspector panels could not reach into runtime types without going through a feature-gated trait. Reflection itself had to split cleanly between the always-compiled `Reflect` derive and the editor-only inspection machinery.

That discipline paid three ways: game binaries stay small, the engine is reusable outside games, and the architecture never quietly tangled the way long-lived engines do. Bet paid with interest.

### Command stack as a cross-cutting concern (Phase 6)

Payoff: high. A good substrate is worth more than any feature built on it.

Every mutation through a command means undo/redo, multi-select operations, scripted edits, sequencer-driven changes (Phase 19), and network-replicated edits (Phase 14 hooks) all share one substrate. The first inspector tweak, the last cinematic scrub, the first plugin-authored bulk rename — all commands.

The cost is that every new panel has to learn to route through commands, and the occasional feature had to bend the model. Phase 19 scrubbing as a single large transaction rather than per-frame commands was the most visible bend. Take Recorder is another: it synthesizes keyframes as a single command at stop-recording time rather than streaming per-sample commands that would drown the undo history.

Overall: the bet compounds. Later phases got undo for free as long as they routed mutations properly, and the ones that didn't caught themselves in review. Paid.

### WASM scripting with capability-sandboxed plugins (Phase 7 / Phase 11)

Payoff: high, especially in the ecosystem lens.

The plugin safety category is where RustForge scores 9 vs Unreal's 3 on the scorecard. Users can install a third-party plugin without auditing its source because the capability manifest bounds what it can touch: a plugin that asks for `fs.read` gets file read, and nothing else. A plugin that tries to open a socket without declaring network capability fails at load time, not at runtime. Unreal cannot retrofit this; its plugin API is C++ with full engine access and no sandbox.

The cost was a narrower plugin API — some plugins that would have been trivial in Unreal were impossible or awkward in RustForge — and a one-time implementation cost for the capability gate. The payoff is a trust model that actually exists. For plugin authors it means a clear surface to write against; for users it means `cargo install` equivalents for plugins are safe by default. Bet paid.

### Shared node-graph widget (Phases 17 / 18 / 20 / 24 / 26)

Payoff: very high, in retrospect higher than Phase 17 predicted.

Audio, VFX, material, animation state machines, and behavior trees are five node-graph domains that all use one widget with per-domain node palettes. Pan, zoom, connection routing, minimap, multi-select, box-select, keyboard navigation, undo hooks, accessibility labels: written once, in `rustforge-node-graph`, and consumed five times.

The temptation to fork at each new domain — "our domain is special, our graph needs a custom routing algorithm" — was real every time and correctly resisted every time. The widget's per-domain extension hooks (custom node renderers, custom port-type validators, domain-specific context menus) carried every legitimate variation without requiring a fork.

The hidden win: bug fixes compound. A fix to connection routing in the material graph fixes the same bug in the behavior tree editor. A keyboard accessibility improvement lands across all five domains at once. Bet paid five times.

### wgpu everywhere (Phase 2+)

Payoff: high, with one caveat. This is the other closer call.

One rendering pathway covers Windows (D3D12), macOS (Metal), Linux (Vulkan), Android (Vulkan), iOS (Metal), and WebGPU. The mobile and web targets in Phase 22 existed at all because of this choice — neither target would have been viable if the renderer had been D3D12-native, and the cost of multi-backend support on a hand-rolled renderer would have been prohibitive.

The caveat is the HW-RT gap noted in §2: wgpu's ray-tracing surface lagged D3D12 and Vulkan native for much of the series. Any feature that required HW-RT (Phase 21 had to route around this with screen-space and probe-based approximations; a future path tracer blocks on it) was constrained by what wgpu exposed, not what the hardware could do.

Net: still paid, but the ceiling on rendering fidelity is partly set by wgpu's pace, not RustForge's.

### Summary

The closer calls were Phase 2 (reflection scope) and wgpu (HW-RT timing). The four bets that clearly compound without qualification are the feature flag, the command stack, the text scene format, and the shared node widget. Those are the ones a future engine built from scratch should copy even if it copies nothing else.

## 4. Remaining Unreal gap

The scorecard at `A:\GAMECAD\doc\unreal-vs-rustforge-scorecard.md` was written against the 13-phase 1.0. It put UE at 552 and RustForge at 356, a 196-point gap, with the delta breaking down roughly as 30 points from post-1.0 runtime subsystems, 80 from specialized-authoring long tail, 50 from ecosystem/maturity, and 35 from platform support.

Re-scoring conceptually against the 29-phase plan:

### Runtime subsystems gap (38 pts in original scorecard) — largely closes

Networking (Phase 14) closes the replication gap to a credible level for the games the engine targets. UMG equivalent (Phase 15) closes the game UI gap structurally. Enhanced Input (Phase 16) closes input parity. Audio (Phase 17) closes the entire audio category that was a blank line on the original scorecard. Rendering and physics close partially — the baseline rose, the fidelity ceiling did not match UE5's Nanite/Lumen. Remaining: console deployment, HW-RT GI, Nanite-parity geometry, FEM-grade destruction.

### Specialized-authoring gap (50 pts) — largely closes

Material node graph (Phase 20) closes the single largest category-level gap on the original scorecard (material editor: 10 vs 3 became 10 vs 8). Animation graph (Phase 24) closes the animation authoring category from 2 to 7 or 8. Particles (Phase 18) and audio graph (Phase 17) fill two blank rows outright. Sequencer (Phase 19) fills another. Remaining: hair/fur, facial capture, procedural generation — all genuinely specialist categories that most engine users never touch.

### Ecosystem/maturity gap (50 pts) — does not close

Documentation can be written, an RFC process can run (Phase 23), a plugin registry can exist — and none of that is the same as twenty-five years of shipped titles, a talent pool that interviews can draw from, a YouTube index of tutorials covering every gotcha, or a Stack Overflow tag with fifty thousand answered questions.

The only thing that closes this gap is time and users. No design document can earn maturity; maturity is the name for the difference between a thing that was designed well and a thing that was lived with. Phase 30 does not close this gap by a point. It names it, it builds the infrastructure (Phase 23) that lets the community close it, and it accepts that this category will move on a ten-year clock no matter what the engine does.

### Platform gap (35 pts) — partially closes

Mobile and web land in Phase 22, worth roughly 12 points. Consoles remain structurally blocked on NDA SDKs — that's 10 points locked out of any plan-level attempt. Mac and Linux improve marginally through the shared DDC work. Net: maybe 15 of the 35 close.

### Specialized new wins

XR (Phase 28), shared DDC (Phase 29), sky/weather (Phase 27), structural merge (already counted), and accessibility-from-day-one (Phase 13, already counted) each close small additional gaps the original scorecard didn't enumerate. A refreshed scorecard would add rows for XR, atmospherics, and DDC.

### Bottom line

A reasonable re-score puts RustForge at roughly 480 against Unreal's 552 — about 87% of Unreal as designed, up from 64%. That number is not rigorous; the original scorecard's categories were pitched against 1.0, and a clean redo of it against 29 phases would shuffle weights.

The honest reading: the series closed the design gap almost as far as design can close it. What remains is the gap that only shipping, using, fixing, documenting, and living with the engine in production can close. That is the handoff to §7.

## 5. 2.0 roadmap — candidates, not commitments

If a Phase 31+ series ever gets written, these are the candidates. Each is named, each is bounded by a short description, and none is scoped here. A future author or RFC would scope them properly.

❌ **None of the following are committed in any future phase.** They are candidates for a successor design series that does not exist yet. Naming them here is not a promise; it is a map of the terrain a successor might choose to survey.

### Hair and fur system

Strand grooming, clump simulation, skin attachment, card-based LOD that falls back gracefully on mobile. Adjacent needs: integration with the Phase 28 XR path (stereo rendering of strands is non-trivial), wind-field reads from Phase 27, and a new cook step in Phase 29's DDC for strand data. A full grooming tool is a one-year project on its own. Candidate.

### Procedural generation tools

A Houdini-style procedural graph hosted on the shared node widget, making this its sixth domain. Use cases: level layout, vegetation scatter, city generation, asset variation. Interaction with the command stack is non-trivial — procedural outputs are not user-authored entities but materialized on demand, and the undo model for "re-run the graph with different parameters" is not the same as "undo this edit." A real phase, not a panel. Candidate.

### Path-traced global illumination

Once wgpu's HW-RT surface matures across vendors, a real-time path tracer as a lighting mode — both in-editor preview and offline high-quality bakes. Phase 21's screen-space and probe-based approximations would remain the default on HW that can't afford it. The blocker is not this engine; it's wgpu. Candidate, contingent.

### Cinematic-grade facial animation

Blend shapes already work through the Phase 8 importer and Phase 24 animation graph. What does not work: a facial capture solve from video or depth sensor, a correctives graph for combining blend shapes without popping, a lipsync pipeline from audio to viseme tracks. All of these are adjacent to each other and worth one coordinated phase rather than three. Candidate.

### Hot-reload for native Rust plugins

Currently, WASM plugins hot-reload (Phase 11) and native Rust code does not. A stable Rust ABI (which does not exist at the language level) or a `cdylib`-with-shim pattern could enable native reload without a full editor restart. The design exists in the open-source Rust ecosystem (`hot-lib-reloader` and similar); integrating it into the plugin model without breaking Phase 11's capability gate is the real work. Candidate, contingent on ABI work outside RustForge's control.

### AI-assisted authoring

LLM-powered asset generation, scene suggestion, shader assistant, behavior-tree first-draft. A real subsystem, not a button. The interesting design questions are where the model runs (local? cloud? user choice?), how it interacts with the capability sandbox, how its suggestions flow through the command stack so they're undoable, and how it integrates with reflection so it can describe the editor's state without the user hand-transcribing it. Candidate. (Arguably the category most likely to ship first, for market reasons rather than technical ones.)

### Cloud collaboration / live multi-user editing

The Phase 12 structural-merge story is async-first: authors work independently, merge via git. A real-time collab layer in the Figma or Google Docs mold would be a different beast — operational transforms or CRDTs on the scene, per-cursor presence, shared PIE sessions. Would integrate with but not replace the merge story. Worth its own phase. Candidate.

### VisionOS + WebXR

Phase 28 shipped OpenXR. Apple's XR stack is not OpenXR-compatible at the time of these phases; it requires a separate backend in the XR abstraction. WebXR is closer to OpenXR conceptually but runs in a browser sandbox with different input and rendering constraints. Either would be a Phase 28-sized addition. Candidate.

### Summary

Eight candidates. A successor series would pick three to five and scope them; it would not ship all eight. The point of naming them here is so that a future author reading Phase 30 sees both what was considered and what was left out of even the candidate list.

The following remain in the not-a-candidate tier from §2: full offline path-tracer for cinematic render, full Nanite-parity streaming virtualized geometry, console platform ports (PS5/Xbox/Switch), visual scripting for gameplay logic, FEM-grade destruction, large-scale SPH/FLIP fluid simulation, motion-capture authoring pipeline. These are deferred not because they don't matter but because each is a business or NDA or decade-scale technical problem that no single author's design phase can usefully scope.

## 6. Lessons and principles

Across twenty-nine phases a handful of principles recur. None were declared at Phase 1; all emerged. Listing them here is worth more than re-deriving them.

### 1. Opinionated design beats a menu of options

Every time a phase offered the user three ways to do one thing, the phase was worse for it. Pick the way; justify it; ship it. Users change engines rarely, and they appreciate a strong default more than a weak choice. A menu of options is the designer admitting they could not decide. Users notice.

### 2. Lock decisions early; compound them later

The `SceneId` decision in Phase 4 paid for itself in Phase 12 (merge), Phase 14 (net identity), Phase 19 (sequencer bindings), Phase 24 (retarget bone map keys). A late-locked decision is a late-paid one. When a bet looks consequential, take it early even if the evidence is thin — waiting does not improve the evidence as much as committing and iterating does.

### 3. Reject features with named reasoning, not silence

Every phase had an "NOT in this phase" section. That list is as important as the feature list. An un-named rejection reads to a future author as an oversight, and they will try to fix it. A named rejection reads as a boundary, and future authors work with the boundary or explicitly revisit it. Users respect boundaries they can see.

### 4. Plan and ship in slices a single patch cycle can cover

Phase 1's build order had seven steps; every later phase had one too. A phase that cannot be broken into a month of independently testable slices is not a phase — it's a research project in phase's clothing. Research projects are fine, but they should be labeled correctly and not interleaved with shipping work.

### 5. Editor and runtime are different audiences; separate their concerns

The `editor` feature flag is the cleanest expression of this, but it shows up everywhere: Phase 13 (editor keybindings) vs Phase 16 (runtime input), Phase 15 (`egui` never ships in games) vs the editor (`egui` is the only UI), the reflection registry that serves both vs the inspector that serves only one. Mixing the two audiences' concerns is where engines accrete weight that never comes off. Ask early, repeatedly: is this editor work or runtime work? The answer dictates the crate, the deps, the testing model, and the release cadence.

### 6. Honor invariants ruthlessly

The PIE snapshot-and-restore invariant shaped Phase 14 (networking must restore), Phase 17 (audio must stop), Phase 19 (sequencer overrides must revert), Phase 24 (animation state must reset), Phase 25 (physics must rewind). The command-stack invariant shaped every mutating panel. The "gameplay reads actions, not devices" invariant shaped Phase 14, Phase 16, Phase 28. An invariant is worth more than any single feature because it bounds an infinite family of future decisions and lets reviewers reject bad patches by citing the invariant rather than re-deriving the argument.

### 7. One widget, many domains

The node-graph widget saved the series months. Look for the widget that five future phases will want, extract it once, and accept the upfront cost. The cost is a one-time loan; the savings are compound. The corresponding anti-pattern is the five almost-identical widgets written for five domains that can never share a bug fix. If you see two phases specifying the same widget, your next phase is a refactor.

### 8. Reflection is the keel

Every cross-cutting subsystem attaches to reflection: inspector, serializer, undo, scripting, replication, UI data binding, preferences, docstring-to-docs links, settings panel, `#[replicate]`. If the reflection registry is weak, every one of those systems develops its own ad-hoc fields table and they drift. Pay the reflection tax in Phase 2; cash the refund every phase after. The tax is: one derive macro, one registry, one set of carefully chosen metadata fields. The refund is: every later phase that needs "give me the fields on this type" has an answer.

### 9. Text formats where humans look; binary where machines ship

`.ron` scenes, `.rmat` graphs, `.rui` layouts, `.rinput` bindings, `.rtimeline` sequencers, `.rvfx` emitters — all human-diffable. Cooked assets, replication streams, hot caches, baked meshes, compiled shaders — all binary. The distinction is about who reads the bytes, not about performance. A human who reads the bytes wants to diff, merge, grep, and hand-edit in an emergency. A machine that reads the bytes wants throughput. Pick the format to serve the reader.

### 10. Document the no

The scorecard is a love letter. It names where UE is better and where RustForge is. A retrospective that only talks about wins is a brochure; a retrospective that talks about losses honestly is a map. The same principle applies within phases: the Goals section and the "NOT in scope" section together define the phase. Either one alone does not.

### 11. Shared invariants beat shared code

The `editor` feature flag, the command-stack rule, the reflection-first rule — these propagate through the codebase as invariants that any author can restate in one sentence. They are cheaper to maintain than a shared library of utility functions because they live in reviewer brains, not in dependency edges. Shared code is good; shared invariants are how shared code stays good.

### 12. Stop when the design format stops serving

Phase 30 exists because the design-by-one-author format had delivered what it could, and the next best thing for the project was to hand it to a community of users. Knowing when to stop designing is the last principle and the hardest one to follow. A thirty-first phase in this style would have been less valuable than the first RFC written by someone who tried to ship a game on the engine and hit a wall.

## 7. Handoff to community

The thirty-phase arc is the end of design-by-one-author. From here, the engine evolves the way every living tool does: through contributors who use it, hit its limits, and file RFCs.

Phase 23 put the infrastructure in place:

- An RFC process with a template, review cadence, and a merge criterion based on consensus rather than central authority.
- A governance charter naming the maintainer roles, the decision-making model for disputes, and the code-of-conduct enforcement path.
- The documentation site, auto-built from `///` docstrings via the reflection machinery Phase 13 extended.
- A plugin registry where third-party plugins publish with their capability manifests visible, so users decide what to trust.
- A published API-stability policy distinguishing `rustforge-core` (SemVer-strict, breaking changes gated on majors) from editor panels (looser, annotated, community-contributed panels can evolve faster).
- A release cadence — quarterly minors, monthly patches, annual majors — committed to in writing.

Everything a community needs to propose, debate, land, and ship a change without a central design doc.

This means the 2.0 roadmap in §5 is not a to-do list. It is a set of candidate directions any community member can pick up and propose as an RFC. Some of them will ship; some will be rejected; some will split into five smaller RFCs and merge incrementally; some will wait years for a contributor with the right problem to show up.

That is correct. The series stops here because at this point a single author's design decisions are worth less than a hundred users' reports. The author can guess what a studio of eight animators needs from the animation graph; a studio of eight animators actually using it will know. Twenty-nine phases of guessing is enough.

A monolithic design document is the right artifact when nobody is using the tool yet. It is the wrong artifact once they are. Phase 30 is the last one not because the design work is done — it is not, it never is — but because the format has served its purpose and the next phase of the engine's life belongs to people who will have tried to ship something with it, hit the friction, and written down where it hurt. That kind of document looks different. It is shorter, more specific, usually angrier, and far more useful.

## 8. Closing

Thirty phases is a lot of planning for a tool that, as of this writing, still measures its shipped scope in ambition more than in binaries.

That is fair criticism and an honest one. A plan is not a program. The scorecard's reminder that "planned" overestimates reality by twenty percent was never wrong, and a realistic delivery against even this plan would fall short of the paper score. None of the phases shipped a line of code; none of them ran on a user's machine; none of them caught a bug the way a bug report from a stranger catches one. The distance between a coherent design and a living engine is not a formality.

What the plan did earn, if it earned anything, is a shape.

A clear answer to what goes in `rustforge-core` and what does not. A consistent rule about feature gates that every phase had to honor. A reflection registry with a single responsibility that twenty-nine later phases could lean on. A scene format that respects the humans who will merge it. A command stack that respects the users who will undo. A plugin system that respects the users who will install. A node-graph widget that respects the engineers who would otherwise have written it five times. An editor-runtime split that respects the game binaries that will ship without carrying editor weight.

A retrospective's job is not to claim these things were inevitable. They were not. Every one of them could have gone the other way, and at several points almost did. The UObject-as-everything pattern is tempting for good reasons; `egui`-in-games is tempting for good reasons; a fork of the node widget per domain is tempting for good reasons. The phases that said no to those temptations said no under pressure, and the coherence of the whole is made of those refusals.

The job of this phase, the last one, is to notice that taken together they form something. Whether that something survives contact with production is a question no design document can answer, and the only honest way to find out is to ship, use, break, and fix.

The phases end here. The engine, if it is to be one, begins.
