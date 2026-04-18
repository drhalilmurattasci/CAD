# Phase 13 — Polish, Preferences & Release

Twelve phases in, the editor is feature-complete. Every panel works, Play-in-Editor is trustworthy, assets import, scripts hot-reload, the profiler reports honest numbers, plugins load, and collaboration doesn't lose work. What's left is the pile of small, cross-cutting items that nobody asked for directly but that *every* shipped creative tool eventually has: a preferences system, keyboard rebinding, theming, accessibility, localization, a welcome screen, crash recovery, and the release-engineering discipline that keeps the next twelve phases from regressing the first twelve.

Phase 13 is the ship-it phase. None of the items here invent new subsystems — they tie off loose ends in ones that already exist. The goal is to turn "works" into "1.0," not to chase new features.

## Goals

By end of Phase 13:

1. A unified **Preferences** resource persisted to disk, edited through a reflection-driven settings panel.
2. **Every** editor action bound by name through a customizable `Keybindings` map, with preset schemes and import/export.
3. **Theming** — dark / light / high-contrast built in, user themes via a RON palette file, hot-reload during development.
4. **Accessibility baseline** — UI scale, minimum font size, reduced motion, color-blind-safe palette, screen-reader labels.
5. **Localization infrastructure** in place even though only English ships in 1.0.
6. **Welcome window**, recent-projects list, and in-editor help with context-sensitive `F1`.
7. **Crash recovery** via periodic autosave; offered on the next launch after a crash.
8. **Opt-in crash reporting** only — no telemetry, no analytics. Policy written down.
9. **Release engineering policy** — version numbering, changelog, reproducible builds, signing, packaging.
10. **Performance budgets** measured by Phase 8's profiler and Phase 10's diagnostics and enforced in CI.
11. **Quality gates** for release — clippy, cargo-deny, doc-tests, an integration test that opens, plays, and closes the editor.

## 1. Preferences system

Everything that a user might flip without touching a project file belongs in `Preferences`. Reuse the reflection registry from Phase 2 §2.2 so the settings panel generates itself.

```
crates/rustforge-editor/src/prefs/
├── mod.rs                # Preferences resource, load/save
├── schema.rs             # #[derive(Reflect)] typed categories
├── migrate.rs            # version-to-version migration shims
└── panel.rs              # reflection-driven settings UI
```

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct Preferences {
    pub version: u32,                // for migrate.rs
    pub appearance: AppearancePrefs,
    pub editor:     EditorPrefs,
    pub viewport:   ViewportPrefs,
    pub keybindings: Keybindings,    // see §2
    pub scripts:    ScriptPrefs,
    pub profiler:   ProfilerPrefs,
    pub advanced:   AdvancedPrefs,
    pub recent_projects: Vec<PathBuf>,
}
```

Load path on startup: `~/.config/rustforge/prefs.toml` (XDG on Linux, `%APPDATA%\rustforge\prefs.toml` on Windows, `~/Library/Application Support/rustforge/prefs.toml` on macOS). Use the `directories` crate — do not roll this yourself.

### 1.1 Auto-generated settings UI

Walk the reflected struct, emit an egui widget per field by type:

- `f32` with `#[range(0.5..=2.0)]` → slider.
- `bool` → checkbox.
- `Color` → color picker (same one the material editor uses).
- `PathBuf` → file-pick row.
- `enum` → combo.

Custom widgets (keybindings table, theme selector) register overrides per field path, same pattern the inspector used in Phase 3. The settings panel is a tab in the editor, not a modal — users want to pop it open while trying things.

### 1.2 Migration

Bump `Preferences::version` when the schema changes. `migrate.rs` holds a `Vec<Fn(&mut toml::Value)>` indexed by from-version. Unknown fields are logged and dropped; missing fields fall back to defaults. Never delete the user's prefs file on a parse failure — rename it to `prefs.toml.broken-<timestamp>` and start fresh.

## 2. Keybindings — rebindable everything

Phase 7 hard-coded `Ctrl+P`, Phase 3 hard-coded `F` to frame, etc. Phase 13 pulls them all behind an action registry.

```rust
pub struct ActionId(pub &'static str);  // e.g. ActionId("play.toggle")

pub struct ActionRegistry {
    actions: HashMap<ActionId, ActionMeta>,   // label, category, default chord
}

pub struct Keybindings {
    bindings: HashMap<ActionId, KeyChord>,
}
```

Every action used anywhere in the editor — menu items, toolbar buttons, shortcut handlers — resolves through `ActionRegistry::trigger(id)`. Panels never match raw `KeyEvent`s. This is the invariant: **if an action has a keybinding it has an `ActionId` first.**

### 2.1 Rebinding UI

```
┌─ Keybindings ──────────────────────────────────────────┐
│ Category: [ Play      ▼ ]     [Reset category]         │
│                                                         │
│  play.toggle          Ctrl+P       [●Record]  [Clear]  │
│  play.pause.toggle    Ctrl+Shift+P [●Record]  [Clear]  │
│  play.step            F10          [●Record]  [Clear]  │
│                                                         │
│ Preset: [ Default ▼ ]   [Import…] [Export…]            │
└─────────────────────────────────────────────────────────┘
```

- **Record** swallows the next key event, converts it to a `KeyChord`, asks for confirmation if it collides with another binding.
- **Conflict detection** is global across categories; show both offenders and let the user decide.
- **Reset category** / **Reset all** — just re-copy defaults from `ActionRegistry`.
- **Presets** — ship Default, Unity-like, Unreal-like. These are just keymap RON files living in the binary's resources. Users can author their own and import.

### 2.2 Chord model

```rust
pub struct KeyChord {
    pub key: Key,           // Key::P
    pub mods: Modifiers,    // ctrl | shift | alt | meta
}
```

Two-chord sequences (`Ctrl+K, Ctrl+S`) are tempting; skip them for 1.0 — VS Code users will want them, nobody else will. Leave `KeyChord` as a single combo so the data model doesn't need rework when sequences land later.

## 3. Theming

```
assets/themes/
├── dark.ron
├── light.ron
└── high-contrast.ron
```

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub palette: Palette,       // 24 named colors: bg_0, bg_1, fg_0, accent, …
    pub fonts: FontOverrides,   // family, base size
    pub borders: BorderStyle,
}
```

On startup, resolve `Preferences::appearance.theme` to a file; apply to egui via a central `apply_theme(ctx, &theme)` function. Custom themes live next to prefs (`~/.config/rustforge/themes/*.ron`); the theme picker lists built-ins plus discovered user files.

### 3.1 Hot-reload

Only meaningful to theme authors, but cheap: when the `editor` binary is built with `debug_assertions`, watch the active theme file via the same `notify` file-watcher used in Phase 5 and re-apply on change. Ship release builds without the watcher — no reason to eat the inotify handle in production.

### 3.2 Per-panel overrides — discouraged

`Theme` exposes a `overrides: HashMap<PanelId, PartialPalette>` field. Nothing in the editor sets it; it exists so plugin authors (Phase 11) who need their panel to look different have an escape hatch. Document: overrides break theme coherence and should be a last resort.

## 4. Accessibility

Four concrete requirements, each with a prefs entry:

1. **Minimum font size.** `appearance.min_font_pt` defaults to 11. The auto-generated settings UI reads this and clamps every `TextStyle`.
2. **UI scale factor.** `appearance.ui_scale` in `0.75..=2.0`, applied to egui's `pixels_per_point`. Hot-applied on change — no restart. Interacts with theme font sizes; scale multiplies them.
3. **Screen-reader hints.** Enable egui's `accesskit` bridge. The rule: **every interactive widget must have a label or a `hint`.** Add a debug-only pass that fails CI if a `Button::new("")` / unlabeled `DragValue` is emitted in the editor tree. This is cheap and prevents accessibility regression.
4. **Reduced motion.** `appearance.reduced_motion`: disable gizmo hover bounce, panel slide-in, selection pulse, toast animations. Everything becomes instant. The visible-change rule: if reduced motion changes state, the new state appears in one frame.

### 4.1 Color-blind-safe palette

`appearance.color_blind_safe: bool`. When true, re-remap status hues: profiler over-budget bars (Phase 8 §6.2), selection outline (Phase 3), dirty markers (Phase 6), PIE banner (Phase 7 §4). Use an Okabe-Ito-style palette that's safe for deuteranopia and protanopia. Theme files may also opt in via a `palette.color_blind_safe: true` field for user-authored themes that want it baked in.

Accessibility is not a panel; it's a property of every panel. Each reviewer pass must ask *"does this work at 2.0x scale with `reduced_motion=true` and `color_blind_safe=true`?"*

## 5. Localization (i18n)

Ship English, build for everyone else.

```
crates/rustforge-editor/locales/
├── en-US.ftl
├── en-US.toml           # plural + metadata
└── README.md            # "how to contribute a translation"
```

Use `fluent-rs`. Every editor-visible string goes through a macro:

```rust
let label = t!("menu.file.open");
let msg   = t!("dialog.unsaved", name = scene_name);
```

Rules:

- **No bare string literals in UI code.** CI grep: `egui::Label::new\("[A-Z]` and friends fail the build. Errors and log strings are exempt (they're for developers).
- **Fallback is English.** If a key is missing in the loaded locale, fall back to `en-US` and log once per missing key.
- **String freeze** before the 1.0 ship — no new keys without a discussion. After freeze, changing an English string requires a new key, not editing the old one, so translators' work doesn't silently go stale.

RTL layout (Arabic, Hebrew) is out of scope; document it as a future phase. The Fluent infrastructure does not preclude it, but egui's RTL support is thin and fixing it is a project of its own.

## 6. Startup & welcome

On launch, if no project is specified on the command line and no project was open last session (tracked in prefs), show a welcome window:

```
┌─ Welcome to RustForge ─────────────────────────────┐
│                                                     │
│   [  New Project…           ]                       │
│   [  Open Project…          ]                       │
│                                                     │
│   Recent                                            │
│    • my-roguelike            2 hours ago            │
│    • jam-entry-2026          yesterday              │
│    • prototype               last week              │
│                                                     │
│   Samples                                           │
│    • First-person starter                           │
│    • 2D platformer starter                          │
│                                                     │
│   [Open Documentation]                              │
│                                                     │
│   [x] Show on startup                               │
└─────────────────────────────────────────────────────┘
```

Welcome is a regular editor window, not a special dialog — when the user clicks "New Project," the welcome closes and the main editor takes over. Recent-projects list is the same `Preferences::recent_projects` (capped at 10, stale entries pruned on load).

## 7. Crash recovery

The editor's save model is explicit (Ctrl+S). The autosave is a *scratch* file for recovery only — never confused with the project scene file.

```
<project>/.rustforge/autosave/
├── <scene_guid>.ron          # last autosave of each open scene
└── session.ron               # open tabs, selection, viewport cam
```

- Write every N seconds (prefs, default 60) **only if scenes are dirty**.
- On clean shutdown, delete the autosave directory.
- On next launch, if `session.ron` exists, show a modal: *"RustForge did not exit cleanly last time. Restore unsaved changes from \<timestamp\>?"* with Restore / Discard / Show files.

Restore reconstructs the scene by loading the autosave, marking it dirty so the user still has to Save intentionally. Restoring must not overwrite the user's real scene file — this is the whole point of the scratch split.

Autosave is throttled when the disk is slow and paused entirely during Play (Phase 7) since scene state is snapshotted anyway.

## 8. Help & documentation

```
Help menu
├── Documentation…          opens https://docs.rustforge.dev/
├── Keybinding Cheatsheet…  in-editor window, generated from ActionRegistry
├── Report a Bug…           opens the issue tracker with system info template
└── About                   version, commit hash, license list
```

`F1` is context-sensitive:

- Focused panel → open `docs.rustforge.dev/panels/<panel_id>`.
- Hovered reflected field → open `docs.rustforge.dev/api/<type_path>#<field>`.

The field-level jump requires the reflection registry to carry **docstrings**. Phase 2 registered types by name; Phase 13 extends that: the `#[derive(Reflect)]` macro captures `///` doc comments into a `doc: &'static str` slot on each `FieldInfo`. Call this out as a Phase 2 extension and land it early in Phase 13 so the help links have something to resolve.

## 9. Telemetry & privacy

**No usage analytics in 1.0.** Not "off by default" — not shipped. The moment an engine editor phones home, people stop trusting it, and the data gathered rarely justifies the trust loss. If a future team wants analytics they can propose a separate opt-in phase.

**Opt-in crash reporting only.** Off by default, togglable in `Preferences::advanced.crash_reporting`. When enabled and a panic occurs, symbolicate, scrub paths (`/home/<user>/…` → `~/…`), and POST to a minidump endpoint. The report includes editor version, OS, GPU adapter name, and the backtrace. It does **not** include project paths, scene contents, or asset names.

Write the policy into `CONTRIBUTING.md` so nobody silently instruments a panel with "just a tiny ping."

## 10. Release engineering

Policy only — the actual installer scripts are a side quest worth their own track.

- **Version numbering.** Editor version follows `rustforge-core`. A tag `v1.0.0` builds both. Editor-only fixes bump the patch (`1.0.1`).
- **Changelog.** `CHANGELOG.md` in Keep-a-Changelog format. Every PR that changes user-visible behavior appends an entry.
- **Reproducible builds.** Pin toolchain in `rust-toolchain.toml`. `cargo vendor` the deps for release tags. Record build flags.
- **Signing.**
  - Windows: Authenticode-sign the `.exe` and MSI.
  - macOS: developer-ID-sign and notarize `.app` and `.dmg`.
  - Linux: GPG-sign `.deb`, `.rpm`, and `.AppImage`.
- **Packaging targets.** Windows MSI + portable zip, macOS DMG, Linux `.deb` / `.rpm` / `.AppImage`. Flatpak and Snap are community-maintained, not shipped first-party.

None of this is implementation work for Phase 13 — it's the checklist a release engineer picks up.

## 11. Performance budgets

Document the targets, then write checks that fail CI when they regress.

| Scenario                               | Budget    | Measured by             |
|----------------------------------------|-----------|--------------------------|
| Editor idle (empty project)            | < 2 ms   | Phase 8 profiler panel  |
| Scene with 10k entities, Edit mode     | < 16 ms  | Phase 8 profiler panel  |
| Scene with 10k entities, Play mode     | < 16 ms  | Phase 8 profiler panel  |
| Cold start to usable window            | < 3 s    | boot-time sampler        |
| Memory floor (empty project)           | < 500 MB | RSS at idle              |
| Autosave write (10k-entity scene)      | < 500 ms | Phase 10 diagnostics     |

A small `perf_gate` binary runs each of these headlessly in CI on a pinned reference machine. Regressions > 10% fail the PR. This is the enforcement teeth; without it the budgets drift within two releases.

## 12. Quality gates for release

The editor ships as 1.0 when CI is green on all of these:

- `cargo clippy --workspace --all-features -- -D warnings`.
- `cargo deny check` — no yanked deps, no GPL contamination, no known advisories.
- `cargo test --workspace` including doc-tests.
- **Editor integration test.** Headless build that:
  1. Launches `rustforge-editor --project samples/first-person-starter`.
  2. Waits for the scene to load.
  3. Sends `Ctrl+P` via synthetic input.
  4. Waits 5 seconds.
  5. Sends `Ctrl+P` again to stop.
  6. Sends `Ctrl+Q` to quit.
  7. Asserts exit code 0 and autosave directory empty.
- **Accessibility smoke test.** Loads the editor at `ui_scale = 2.0`, `reduced_motion = true`, `color_blind_safe = true`, takes snapshots of every panel, asserts they render without overflow.
- Perf-gate (see §11).

## 13. Build order within Phase 13

Each step is independently testable — land them in order, verify each, then move on.

1. **Preferences resource + load/save** (§1) — empty schema, just the file on disk.
2. **Reflection docstring extension** for Phase 2 (§8) — enables F1-to-docs later without retrofits.
3. **Action registry + migrate existing hotkeys** (§2) — no UI yet; every hard-coded shortcut now goes through `ActionRegistry`. Ship.
4. **Theming engine** (§3) — built-in dark/light/high-contrast, no hot-reload yet.
5. **Auto-generated settings panel** (§1.1) — read/write prefs through reflection.
6. **Keybinding UI + conflict detection + presets** (§2.1–2.2).
7. **Accessibility baseline** (§4) — ui_scale, min_font, reduced_motion, color-blind palette.
8. **i18n infrastructure** (§5) — move strings behind `t!`, ship `en-US.ftl`.
9. **Welcome window + recent projects** (§6).
10. **Autosave + crash recovery** (§7).
11. **Help menu + F1 context links** (§8).
12. **Crash reporting opt-in + policy doc** (§9).
13. **Theme hot-reload in dev builds** (§3.1).
14. **Performance budget gate** (§11) — pinned CI runner, `perf_gate` binary.
15. **Release quality gates** (§12) — integration test, accessibility smoke, clippy/deny in CI.
16. **Release engineering policy write-up** (§10) — docs only, no code.

## 14. Scope boundaries — what's NOT in Phase 13

- ❌ **Right-to-left (RTL) layout.** Infrastructure allows it; egui support isn't ready. Future.
- ❌ **Auto-downloader for editor updates.** A notification with a link is as far as Phase 13 goes. Silent updaters need a trust model this project doesn't have.
- ❌ **Usage analytics / telemetry.** Not off-by-default; not shipped. Opt-in crash reports only.
- ❌ **Installer authoring implementation.** Policy and target list; actual MSI/DMG/AppImage scripts are their own track.
- ❌ **Multiple shipped locales.** Infrastructure and `en-US`; translations arrive post-1.0 via community PRs.
- ❌ **Two-chord keyboard sequences** (`Ctrl+K, Ctrl+S`). Single chord only for 1.0.
- ❌ **Node-graph theme editor / live palette picker.** Edit the RON file.
- ❌ **Plugin-authored settings categories.** Phase 11's plugin API can register prefs fields in a future extension; not Phase 13.
- ❌ **Scene autosave recovery across *project* moves.** If the project directory is renamed, autosave files stay where they were.

## 15. Risks & gotchas

- **Prefs migration drift.** The schema changes across editor versions; users who skip versions hit compound migrations. Always test `0 → N` migration, not just `N-1 → N`. Keep `migrate.rs` exhaustive and under unit test.
- **Theme / palette drift.** Built-in themes get a new field (`palette.warning_bg`), user-authored themes don't have it, UI falls back to black. Every palette field must have a default; validate themes on load and log missing fields.
- **Keybinding preset maintenance.** Shipping Unity-like + Unreal-like presets means every new action needs three default bindings forever. Budget this: new actions get a default chord and `None` for both presets unless the preset has a clear analog. Document the policy so contributors don't sprawl.
- **Accessibility regressions.** A panel lands with an unlabeled icon button; nobody notices until a user with `accesskit` reports it. The CI accessibility smoke-test is the single best line of defense; don't skip it to green a release.
- **i18n string freeze coordination.** Translators start working from a snapshot; English drifts; their files go stale silently. The "new key on change" rule prevents this but only if enforced. Add a CI check: modifying an existing `en-US.ftl` key fails the build; you must add a new one and retire the old.
- **Autosave eating disk.** A user opens a 200 MB scene, hits dirty constantly, autosave writes every 60 s → gigabytes per hour. Cap total autosave size per project (default 1 GB); rotate oldest. Also: autosave must be atomic (`tmp + rename`) so a crash mid-write doesn't corrupt the previous autosave.
- **Crash-report symbolication.** Shipping stripped binaries means stack traces are useless without symbols on the server. Upload debug symbols as a separate artifact during release; keep them forever. Don't ship symbols inside the installer.
- **Performance gate flakiness.** CI runners have variable load; a real 10% regression and a noisy runner look the same. Use a dedicated perf runner; run each budget 5 times and take the median; only fail on sustained regression across 3 PRs, not a single spike.
- **Welcome on CI / headless runs.** The integration test opens a project explicitly; welcome must not steal focus. Gate welcome strictly on "no project argument AND no last session AND TTY is interactive."
- **Color-blind palette clashes with user themes.** A user ships a custom theme; the color-blind toggle overrides part of its palette. Document precedence: color-blind overrides win. Theme authors can annotate their theme as already color-blind-safe to opt out.
- **`accesskit` platform coverage.** Linux screen-reader support is the thinnest. Ensure all labels are in place even if a given platform renders them less well — the data is portable, the backend isn't.

## 16. Exit criteria

Phase 13 is done when all of these are true:

- [ ] `Preferences` loads, saves, and migrates cleanly across at least one schema bump test.
- [ ] Settings panel is entirely auto-generated from the reflected schema; no hand-written forms.
- [ ] Every editor hotkey in Phases 1–12 resolves through `ActionRegistry`; none are hard-coded in panel code.
- [ ] Keybinding UI records chords, detects conflicts, imports/exports keymaps, and ships Default / Unity-like / Unreal-like presets.
- [ ] Dark, Light, and High-Contrast themes ship built in; user `.ron` themes load from the config dir.
- [ ] Theme hot-reload works in debug builds and is absent from release builds.
- [ ] UI scale factor, minimum font size, reduced motion, and color-blind-safe palette are prefs-controlled and applied live.
- [ ] `accesskit` is enabled; every interactive widget has a label (enforced in CI).
- [ ] All editor-visible strings go through `t!`; `en-US.ftl` is complete; CI forbids raw string literals in UI code.
- [ ] Welcome window appears on first launch and with no open project; recent-projects list is prefs-backed.
- [ ] Autosave writes periodically while dirty and is offered on next launch after an unclean exit; never overwrites the user's scene file.
- [ ] `F1` opens context-sensitive documentation for the focused panel or hovered reflected field.
- [ ] Crash reporting is opt-in, off by default, and the privacy policy is documented in `CONTRIBUTING.md`.
- [ ] Release engineering policy (versioning, changelog, signing, packaging) is written down.
- [ ] Performance budgets from §11 are enforced by `perf_gate` on a pinned CI runner.
- [ ] Quality gates (clippy `-D warnings`, `cargo deny`, doc-tests, integration test, accessibility smoke) are green.
- [ ] `rustforge-core` still builds without the `editor` feature.
- [ ] Editor ships as 1.0.

---

This completes the 13-phase editor series. Phases 1–12 built the machinery; Phase 13 put a shell around it fit for real users. What remains is incremental work driven by users — plugin authors finding gaps in the reflection API, translators contributing locales, artists asking for the node-graph material editor that Phase 8 deferred, studios pushing on collaboration beyond Phase 12. None of that belongs in a phased design document anymore. From here, the editor evolves the way every living tool does: one PR at a time, from people using it.
