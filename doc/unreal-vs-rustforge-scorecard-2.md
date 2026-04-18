# Scorecard — Unreal Engine 5 vs RustForge 38-phase plan

Bold = winner. Δ positive = UE leads. "New" = row added since 30-phase scorecard.

| § | Category | UE | RF | Δ |
|---|---|---|---|---|
| **A. Foundation & architecture** | | | | |
| | Workspace/module boundaries | 6 | **9** | -3 |
| | Reflection coverage | **10** | 5 | +5 |
| | Reflection performance | 6 | **9** | -3 |
| | Language safety | 4 | **9** | -5 |
| | Build times | 4 | **5** | -1 |
| | Code cleanliness | 5 | **9** | -4 |
| | **Subtotal** | **35** | **46** | **-11** |
| **B. Scene & asset data** | | | | |
| | Scene diffability | 2 | **9** | -7 |
| | Scene load perf at scale | **8** | 6 | +2 |
| | Stable ID system | **9** | 8 | +1 |
| | Asset format variety | **10** | 4 | +6 |
| | Prefab/template system | **9** | 7 | +2 |
| | Hot reload | 8 | 8 | 0 |
| | **Subtotal** | **46** | **42** | **+4** |
| **C. Editor UX** | | | | |
| | UI framework maturity | **9** | 5 | +4 |
| | Viewport / gizmos | **9** | 6 | +3 |
| | Hierarchy / inspector | **8** | 7 | +1 |
| | Undo/redo coverage | 7 | **8** | -1 |
| | Play-in-Editor | **9** | 6 | +3 |
| | Live edit in play | 8 | 8 | 0 |
| | Accessibility | 4 | **8** | -4 |
| | **Subtotal** | **54** | **48** | **+6** |
| **D. Specialized authoring** | | | | |
| | Material editor | **10** | 9 | +1 |
| | Terrain / landscape *(P32)* | 9 | **9** | 0 |
| | Animation authoring *(P35)* | 10 | 10 | 0 |
| | Particle system | **10** | 8 | +2 |
| | Audio / DSP | **10** | 8 | +2 |
| | Cinematic / sequencer | **10** | 8 | +2 |
| | Sky / weather | **9** | 7 | +2 |
| | **Subtotal** | **68** | **59** | **+9** |
| **E. Scripting & logic** | | | | |
| | Native code path | **9** | 8 | +1 |
| | Visual scripting | **10** | 0 | +10 |
| | Sandboxed scripting | 3 | **8** | -5 |
| | Hot reload robustness | 6 | **8** | -2 |
| | **Subtotal** | **28** | **24** | **+4** |
| **F. Runtime subsystems** | | | | |
| | Rendering features *(P36)* | 10 | 10 | 0 |
| | Rendering scalability *(P36)* | 9 | 9 | 0 |
| | Physics | **10** | 9 | +1 |
| | Animation runtime | 9 | 9 | 0 |
| | Networking / replication *(P34)* | **10** | 9 | +1 |
| | Input system | 8 | **9** | -1 |
| | UI framework for games | **9** | 8 | +1 |
| | AI / Navigation | **9** | 8 | +1 |
| | **World/scene streaming (new, P31)** | **10** | 9 | +1 |
| | **Subtotal** | **84** | **80** | **+4** |
| **G. Developer workflow** | | | | |
| | Profiler | **10** | 5 | +5 |
| | Debugging tooling | **8** | 6 | +2 |
| | Frame debugger / GPU capture | **8** | 5 | +3 |
| | Logging | 8 | 8 | 0 |
| | Crash reporting | **8** | 6 | +2 |
| | **Replay / record (new, P33)** | 9 | 9 | 0 |
| | **Subtotal** | **51** | **39** | **+12** |
| **H. Version control & collaboration** | | | | |
| | Perforce integration | **10** | 0 | +10 |
| | Git integration | 6 | **8** | -2 |
| | Scene merge *(P31 OFPA)* | 4 | **10** | -6 |
| | Binary-asset handling | 7 | 7 | 0 |
| | Asset locking | **9** | 3 | +6 |
| | **Multi-user live editing (new, P37)** | 9 | 9 | 0 |
| | **Subtotal** | **45** | **37** | **+8** |
| **I. Build & platforms** | | | | |
| | Build tooling | **9** | 8 | +1 |
| | Incremental cooking | 8 | **9** | -1 |
| | Windows | **10** | 8 | +2 |
| | Linux | 7 | **8** | -1 |
| | macOS | **8** | 7 | +1 |
| | iOS / Android | 8 | 8 | 0 |
| | Consoles (PS5/Xbox/Switch) | **10** | 0 | +10 |
| | Web (WebGPU/WASM) | 2 | **9** | -7 |
| | **Subtotal** | **62** | **57** | **+5** |
| **J. Extensibility** | | | | |
| | Plugin system (editor) | **9** | 6 | +3 |
| | Plugin safety / sandboxing | 3 | **9** | -6 |
| | Editor customization | **8** | 7 | +1 |
| | Marketplace / ecosystem | **10** | 0 | +10 |
| | **Modding runtime (new, P38)** | 8 | **9** | -1 |
| | **Subtotal** | **38** | **31** | **+7** |
| **K. Polish & release** | | | | |
| | Localization | **10** | 3 | +7 |
| | Theming | 7 | **8** | -1 |
| | Keybinding customization | 8 | 8 | 0 |
| | Preferences system | 8 | 8 | 0 |
| | Welcome / onboarding | 7 | 7 | 0 |
| | Auto-update | **8** | 3 | +5 |
| | **Subtotal** | **48** | **37** | **+11** |
| **L. Maturity & ecosystem** | | | | |
| | Production track record | **10** | 0 | +10 |
| | Documentation | **8** | 0 | +8 |
| | Community / talent pool | **10** | 0 | +10 |
| | Long-term stability | **9** | 0 | +9 |
| | Commercial viability | **10** | 0 | +10 |
| | **Subtotal** | **47** | **0** | **+47** |
| **M. XR (VR/AR)** | | | | |
| | VR/AR runtime | 8 | 8 | 0 |
| | XR authoring / dev workflow | 7 | 7 | 0 |
| | Advanced XR features (eye/face) | **7** | 4 | +3 |
| | WebXR | **3** | 1 | +2 |
| | **Subtotal** | **25** | **20** | **+5** |

## Grand totals

| Plan stage | UE | RF | Δ | RF % of UE |
|---|---|---|---|---|
| 1.0 (phases 1–13) | 552 | 356 | +196 | 64% |
| Post-1.0 (phases 1–30) | 595 | 474 | +121 | 80% |
| **2.0+ (phases 1–38)** | **631** | **520** | **+111** | **82%** |

## Categories RF wins at 38-phase scope

| Category | UE | RF |
|---|---|---|
| Web target | 2 | **9** |
| Scene merge | 4 | **10** |
| Scene diffability | 2 | **9** |
| Plugin safety | 3 | **9** |
| Modding runtime | 8 | **9** |
| Language safety | 4 | **9** |
| Reflection performance | 6 | **9** |
| Code cleanliness | 5 | **9** |
| Workspace modularity | 6 | **9** |
| Input system | 8 | **9** |
| Accessibility | 4 | **8** |
| Sandboxed scripting | 3 | **8** |
| Script hot reload | 6 | **8** |
| Incremental cooking | 8 | **9** |
| Git integration | 6 | **8** |
| Linux | 7 | **8** |
| Theming | 7 | **8** |
| Undo/redo | 7 | **8** |

## Categories still uncloseable

| Gap | Δ | Reason |
|---|---|---|
| L. Maturity/ecosystem | +47 | Can't be planned; earned over years. |
| G. Dev workflow | +12 | Unreal Insights moat. |
| K. Polish | +11 | Locale breadth, installers, auto-update. |
| Visual scripting | +10 | Philosophical — never planned. |
| Perforce | +10 | Rejected by design. |
| Console support | +10 | Platform SDKs + wgpu backend. |
| Marketplace | +10 | Rejected — plugin index only. |

## What phases 31–38 bought

- **+46 RF points** (474 → 520). UE gained +36 from new rows added to its columns (world streaming, replay, multi-user editing, modding runtime).
- RF moved from **80% → 82%** of UE.
- **3 more categories pulled to parity**: terrain (4→9), animation authoring (9→10), scene streaming (tied at near-parity).
- **Final rendering gap closed**: Phase 36 HW RT + path tracer brings rendering features to 10/10.
- **1 new category where RF leads**: modding runtime (capability sandbox + live patch = safer than UE Mod SDK).

## Verdict — 38-phase plan

RustForge as specified across 38 phases would be **technically competitive with Unreal 5 across 82% of measured surface area**. Remaining 18% is:
- **~7%** earned-only ecosystem maturity
- **~4%** philosophical exclusions (visual scripting, Perforce, paid marketplace)
- **~4%** console platforms
- **~3%** tooling depth (Insights, installers, locale breadth)

RF leads Unreal decisively on **18 categories** — up from 12 at 30-phase. The architectural bets (safety-by-design, Git-native, capability sandbox, web-first) compound: each new phase adds to RF's lead rather than chipping at UE's lead, because those categories were already won.

For the target audience (indie-to-mid-studio, Rust developers, Git-native teams, cross-platform including web) the 38-phase plan describes a **technically superior editor**. Unreal's remaining moat is console/AAA/ecosystem — real, but narrow enough that RF is the better tool for the 70% of game development that isn't AAA console.
