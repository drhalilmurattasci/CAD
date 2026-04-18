# Scorecard — Unreal Engine 5 vs RustForge plan

Scale: **1–10** per category. Unreal scored against its shipped state (UE 5.4). RustForge scored against the 13-phase plan *as specified* (speculative — nothing is built). "—" means not covered in the plan.

Bold = category winner.

## A. Foundation & architecture

| Category | UE | RF | Note |
|---|---|---|---|
| Workspace/module boundaries | 6 | **9** | UE: 200+ modules, heavy header coupling. RF: clean crate split, explicit `editor` feature gate. |
| Reflection system coverage | **10** | 5 | UE UObject does serialization + GC + replication + RPC + transactions. RF Reflect: properties only. |
| Reflection performance | 6 | **9** | UObject dispatch overhead + GC. Rust trait-based reflection is ~free. |
| Language safety | 4 | **9** | C++ vs Rust. Decades of UE security advisories. |
| Build times | 4 | 5 | Both bad. UE has unity builds + Incredibuild; RF has cargo incremental + workspace. |
| Code quality / cleanliness | 5 | **9** | UE is a 25-year codebase; lots of cruft. RF clean slate, but unproven at scale. |
| **Subtotal** | **35** | **46** | RF +11 |

## B. Scene & asset data

| Category | UE | RF | Note |
|---|---|---|---|
| Scene format (diffable) | 2 | **9** | `.umap` binary vs `.ron` text. |
| Scene format (load perf at scale) | **8** | 6 | Binary beats `ron` parsing on 50k+ entity scenes. |
| Stable ID system | **9** | 8 | FGuid + FSoftObjectPath vs `SceneId` + `AssetGuid`. Both solid; UE's is battle-tested. |
| Asset format variety | **10** | 4 | UE imports everything. RF plans gltf/png/wav at start. |
| Prefab/template system | **9** | 7 | Blueprints + Level Instances vs prefab subtrees. UE's is deeper. |
| Hot reload | 8 | 8 | Directory Watcher vs `notify`. Same mechanism. |
| **Subtotal** | **46** | **42** | UE +4 |

## C. Editor UX

| Category | UE | RF | Note |
|---|---|---|---|
| UI framework maturity | **9** | 5 | Slate (15+ years) vs egui (newer, less battle-tested at scale). |
| Viewport / gizmos | **9** | 6 | UE's are polished; RF's planned but unproven. |
| Hierarchy / inspector | **8** | 7 | UE deeper (component hierarchy, archetypes); RF cleaner via reflection. |
| Undo/redo coverage | 7 | **8** | UE transactions have known gaps. RF snapshot model cleaner by design. |
| Play-in-Editor | **9** | 6 | PIE + SIE + standalone + multi-PIE networking. RF single-mode. |
| Live edit in play | 8 | **8** | Both solid; RF's runtime-override badge is a nice UX touch. |
| Accessibility | 4 | **8** | UE historically weak. RF plans AccessKit, UI scale, color-blind palettes from day one. |
| **Subtotal** | **54** | **48** | UE +6 |

## D. Specialized authoring

| Category | UE | RF | Note |
|---|---|---|---|
| Material editor | **10** | 3 | Node graph, industry-leading, vs property sheet. Largest single gap. |
| Terrain / landscape | **9** | 4 | Landscape + layers + grass + splines + Nanite vs basic sculpt/paint. |
| Animation authoring | **10** | 2 | Persona (state machines, blend spaces, IK, retargeting, Sequencer) vs preview-only. |
| Particle system | **10** | — | Niagara. RF not planned. |
| Audio / DSP | **10** | — | MetaSounds + spatial audio + DSP. RF not in scope. |
| Cinematic / sequencer | **10** | — | Unrivaled. RF not planned. |
| **Subtotal** | **59** | **9** | UE +50 |

## E. Scripting & logic

| Category | UE | RF | Note |
|---|---|---|---|
| Native code path | **9** | 8 | C++ with Live Coding vs Rust recompile. |
| Visual scripting | **10** | 0 | Blueprints are the defining UE feature. RF: no plans. |
| Sandboxed scripting | 3 | **8** | UE has Verse (new, uneven). RF WASM with capability gating. |
| Hot reload robustness | 6 | **8** | Live Coding hits edge cases. WASM module swap cleaner. |
| **Subtotal** | **28** | **24** | UE +4 |

## F. Runtime subsystems

| Category | UE | RF | Note |
|---|---|---|---|
| Rendering features | **10** | 6 | Lumen, Nanite, Virtual Shadow Maps, TSR vs baseline PBR + SSR + volumetrics. |
| Rendering scalability | **9** | 6 | UE scales from indie to AAA. RF untested. |
| Physics | **10** | 6 | Chaos (destruction, vehicles, fluids) vs Rapier baseline. |
| Animation runtime | **9** | 6 | Same pattern. |
| Networking / replication | **10** | — | UE's RPC + replication is a major feature. RF: not planned. |
| Input system | **8** | 3 | Enhanced Input is mature. RF: winit basics. |
| UI framework for games | **9** | — | UMG. RF: no game-facing UI framework planned. |
| **Subtotal** | **65** | **27** | UE +38 |

## G. Developer workflow

| Category | UE | RF | Note |
|---|---|---|---|
| Profiler | **10** | 5 | Unreal Insights is best-in-class. RF basic frame graph + timings. |
| Debugging tooling | **8** | 6 | UE has rich runtime inspector. RF P10 covers the basics. |
| Frame debugger / GPU capture | **8** | 5 | UE has built-in + RenderDoc integration. RF defers to RenderDoc. |
| Logging | **8** | 8 | FLog/UE_LOG vs tracing-style. Comparable. |
| Crash reporting | **8** | 6 | UE ships Crash Reporter; RF plans it. |
| **Subtotal** | **42** | **30** | UE +12 |

## H. Version control & collaboration

| Category | UE | RF | Note |
|---|---|---|---|
| Perforce integration | **10** | — | First-class in UE; RF explicitly Git-only. |
| Git integration | 6 | **8** | UE's Git plugin is weak. RF plans structural merge. |
| Scene merge | 4 | **9** | UE relies on locking (binary umap). RF structural merge keyed on SceneId. |
| Binary-asset handling | **7** | 7 | Comparable; LFS in both. |
| Asset locking | **9** | 3 | UE has it; RF rejects it by design. Depends on team model. |
| **Subtotal** | **36** | **27** | UE +9 (but RF wins for small Git teams) |

## I. Build & platforms

| Category | UE | RF | Note |
|---|---|---|---|
| Build tooling | **9** | 7 | UBT + UAT mature. RF uses cargo + custom cook. |
| Incremental cooking | **8** | 7 | DDC + shared-network cache vs content-hash. |
| Windows target | **10** | 8 | UE ships polished installers. RF plans it. |
| Linux target | 7 | **8** | UE Linux is second-class. RF treats it equally. |
| macOS target | **8** | 7 | UE has notarization story. RF plans it. |
| iOS / Android | **8** | 2 | UE ships to mobile. RF: wgpu can, plan won't. |
| Consoles (PS5/Xbox/Switch) | **10** | 0 | UE is the industry standard. RF: not viable. |
| Web (WebGPU/WASM) | 2 | **5** | UE deprecated HTML5. RF: wgpu supports it, plan excludes but unblocked. |
| **Subtotal** | **62** | **44** | UE +18 |

## J. Extensibility

| Category | UE | RF | Note |
|---|---|---|---|
| Plugin system | **9** | 6 | UE mature + marketplace. RF planned. |
| Plugin safety / sandboxing | 3 | **9** | UE plugins run with full engine access. RF capability system is a real differentiator. |
| Editor customization | **8** | 7 | Editor Utility Widgets vs custom egui panels. |
| Marketplace / ecosystem | **10** | 0 | UE has a thriving marketplace. RF: none. |
| **Subtotal** | **30** | **22** | UE +8 |

## K. Polish & release

| Category | UE | RF | Note |
|---|---|---|---|
| Localization | **10** | 3 | Dozens of shipped locales + RTL. RF: infra only, English 1.0. |
| Theming | 7 | **8** | Slate theming vs RON theme files. |
| Keybinding customization | **8** | 8 | Comparable. |
| Preferences system | **8** | 8 | Comparable. |
| Welcome/onboarding | 7 | 7 | UE has Epic Launcher. RF plans welcome window. |
| Auto-update | **8** | 3 | Launcher. RF: notification only. |
| **Subtotal** | **48** | **37** | UE +11 |

## L. Maturity & ecosystem

| Category | UE | RF | Note |
|---|---|---|---|
| Production track record | **10** | 0 | Fortnite, Senua's Saga, Robocop, hundreds of AAA shipped. RF: plan. |
| Documentation | **8** | 0 | RF doesn't exist. |
| Community / talent pool | **10** | 0 | Massive. |
| Long-term stability | **9** | ? | 25 years. RF unknown. |
| Commercial viability | **10** | 0 | Royalty model proven. RF no business model stated. |
| **Subtotal** | **47** | **0** | UE +47 |

---

## Grand total

| | UE | RF | Delta |
|---|---|---|---|
| A. Foundation | 35 | 46 | **RF +11** |
| B. Scene/assets | 46 | 42 | UE +4 |
| C. Editor UX | 54 | 48 | UE +6 |
| D. Specialized authoring | 59 | 9 | UE +50 |
| E. Scripting | 28 | 24 | UE +4 |
| F. Runtime subsystems | 65 | 27 | UE +38 |
| G. Dev workflow | 42 | 30 | UE +12 |
| H. VCS | 36 | 27 | UE +9 |
| I. Build/platforms | 62 | 44 | UE +18 |
| J. Extensibility | 30 | 22 | UE +8 |
| K. Polish | 48 | 37 | UE +11 |
| L. Maturity | 47 | 0 | UE +47 |
| **Total** | **552** | **356** | **UE +196** |

**UE 552 / RF 356** — UE wins by ~55% as specified.

## Categories RustForge wins

1. **Scene diffability** (9 vs 2) — structural text merge.
2. **Scene merge** (9 vs 4) — if the merge driver works.
3. **Plugin safety** (9 vs 3) — capability sandbox.
4. **Reflection performance** (9 vs 6) — no GC, cheap dispatch.
5. **Code cleanliness** (9 vs 5) — fresh codebase, Rust.
6. **Language safety** (9 vs 4) — memory + thread safety.
7. **Workspace modularity** (9 vs 6) — clean crate boundaries.
8. **Accessibility** (8 vs 4) — planned from day one.
9. **Undo/redo model** (8 vs 7) — cleaner invariants.
10. **Linux parity** (8 vs 7) — RF treats Linux as a first target.
11. **Script hot reload** (8 vs 6) — WASM swap is cleaner than Live Coding.
12. **Sandboxed scripting** (8 vs 3) — WASM capability model.

## Where UE is uncatchable

- **Maturity, docs, ecosystem** (47 pts gap). No plan closes this without years of real use.
- **Specialized authoring** (50 pts gap). Materials, animation, particles, audio, cinematics are 20-year efforts.
- **Runtime subsystems** (38 pts gap). Networking/replication, UMG, Enhanced Input — entire subsystems RF doesn't plan.
- **Console support** (10 pts gap in one row). Structural blocker requiring platform SDKs + backend work.

## Verdict

The plan scores a credible **64% of Unreal** across all categories — remarkable for a 13-phase document, unrealistic as a 1-team deliverable. The delta breaks down as:

- **~30 points** from things that *will* get built but aren't in the plan (networking, UMG, Enhanced Input equivalent).
- **~80 points** from the specialized-authoring long tail Phase 8 explicitly defers (material graph, animation authoring, particles, audio).
- **~50 points** from ecosystem/maturity that can't be planned, only earned.
- **~35 points** from platform support (consoles, mobile) that's scope or technically gated.

Where RustForge wins, it wins on **architectural cleanliness and safety-by-design** — scene merging, plugin sandboxing, reflection simplicity, accessibility, Rust itself. These are the *interesting* categories: they're choices Unreal can't easily retrofit, and they're where a new engine can actually beat an incumbent.

A realistic 1.0 delivered against this plan would score maybe **280/600 real-world** (plan scores overestimate by ~20% because "planned" isn't "shipped") — enough to be genuinely useful to a narrow audience (indie + technical teams on Git workflows doing straightforward 3D games), nowhere near enough to compete with Unreal on AAA or broadly.
