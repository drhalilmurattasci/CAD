# Phase 11 — Extensibility & Plugins

Phases 1–10 built a complete first-party editor: every panel, importer, command, asset editor, build target, and debugging tool shipped as RustForge code compiled into one binary. That model scales to the core team; it does not scale to the long tail of studios and hobbyists who want to bolt on a FBX tweaker, a company-internal asset linter, a custom terrain brush, or a Lua-like console DSL — without forking the editor.

Phase 11 opens the editor. It defines a stable plugin API, two plugin runtimes (compile-time Rust and sandboxed WASM), a manifest format, a capability system, discovery, lifecycle, settings UI, and the versioning policy that keeps it all from collapsing under churn. This is the shift from *closed editor* to *platform*.

## Goals

By end of Phase 11:

1. **Stable plugin API crate** (`rustforge-plugin-api`) that third-party code depends on, with strict 0.x semver policy.
2. **Two plugin runtimes**: compile-time Rust plugins (trusted, first-class performance) and WASM plugins (sandboxed, hot-reloadable).
3. **`plugin.toml` manifest** describing name, version, runtime, entry point, and declared capabilities.
4. **Extension points** for panels, importers, asset editors, inspector widgets, commands (undoable), console commands, gizmos, and Tools-menu entries.
5. **Capability system** — plugins declare what they touch; the editor enforces grants per runtime.
6. **Discovery** of project-local and user-global plugins with per-project enable/disable.
7. **Lifecycle** (`on_load` / `on_unload` / `on_scene_open` / `on_play_start` / `on_play_stop`) with crash isolation: a failing plugin disables itself, the editor stays up.
8. **Plugin settings UI** driven by reflection, surfaced in Project Settings.

## 1. Plugin model — the three candidates

The editor's extension story has exactly three viable shapes. Each has tradeoffs worth being explicit about.

### 1.A Compile-time Rust plugins

A plugin is a workspace crate that the user adds to their project's `rustforge-editor` binary build. The editor is recompiled; the plugin links statically.

**Pros:** zero ABI risk, full performance, full access to the editor's Rust types, easiest to author. Fits how Bevy handles its `App::add_plugin` pattern.
**Cons:** requires a Rust toolchain and a recompile; no hot-reload; no sandboxing — a malicious plugin has full process privileges.

### 1.B Dynamically loaded Rust libraries (`.dll` / `.so`)

A plugin is a Rust `cdylib` loaded at runtime via `libloading` or `abi_stable`.

**Pros:** no editor recompile per plugin; faster iteration than 1.A in theory.
**Cons:** the Rust ABI is not stable. Trait objects, `String`, `Vec`, generics, panic-unwind across the boundary — all undefined behavior unless you pin the exact same compiler and flags on both sides. `abi_stable` mitigates but forces a separate C-like API surface on every shared type. Debug/release mismatch alone will produce weird crashes that waste hours. **Reject this option.** The class of bugs it generates undermines every other stability promise in the editor.

### 1.C WASM-sandboxed plugins

A plugin compiles to `.wasm`, runs inside the Phase 7 WASM host, and talks to the editor through explicitly declared host functions.

**Pros:** real sandboxing (no filesystem or network unless granted); hot-reloadable; language-agnostic (AssemblyScript, Rust-to-wasm, Zig); safe to distribute untrusted.
**Cons:** every host function is API surface that must be versioned; no zero-cost access to editor types; serialization at every boundary.

### 1.D Recommendation: hybrid 1.A + 1.C

- **Compile-time Rust plugins** for first-party, studio-internal, and trusted community extensions. Performance and ergonomics matter more than sandboxing here.
- **WASM plugins** for untrusted, marketplace-distributed, or user-script extensions. Sandboxing matters more than the last 2% of performance.
- **Explicitly reject dynamic Rust library loading.** Document the reasoning in `doc/adr/plugin-runtimes.md` so this decision isn't relitigated every six months.

Both runtimes implement the same trait surface (`Plugin`) from `rustforge-plugin-api`. Host code treats them identically above the runtime boundary.

## 2. `rustforge-plugin-api` crate

The public, versioned crate that plugin authors depend on. Nothing else.

```
crates/rustforge-plugin-api/
├── Cargo.toml               # published to crates.io; pinned minor
└── src/
    ├── lib.rs               # re-exports
    ├── plugin.rs            # Plugin trait + PluginMeta
    ├── context.rs           # PluginContext — handle to editor services
    ├── capability.rs        # Capability enum, grant checks
    ├── lifecycle.rs         # LifecycleEvent
    ├── ext/
    │   ├── panel.rs         # register_panel
    │   ├── importer.rs      # register_importer
    │   ├── asset_editor.rs  # register_asset_editor
    │   ├── inspector.rs     # register_inspector_widget
    │   ├── command.rs       # register_command_factory
    │   ├── console.rs       # register_console_command
    │   ├── gizmo.rs         # register_gizmo
    │   └── menu.rs          # register_menu_entry
    └── settings.rs          # PluginSettings via reflect
```

Core trait:

```rust
pub trait Plugin: Send + 'static {
    fn meta(&self) -> &PluginMeta;
    fn on_load(&mut self, ctx: &mut PluginContext) -> Result<()>;
    fn on_unload(&mut self, ctx: &mut PluginContext) {}
    fn on_lifecycle(&mut self, _ev: LifecycleEvent, _ctx: &mut PluginContext) {}
}
```

`PluginContext` is the **only** way a plugin touches the editor. It exposes narrow methods that internally check the calling plugin's granted capabilities before dispatching. This is the enforcement seam; everything else is policy.

### 2.1 Versioning

`rustforge-plugin-api` follows strict semver-pre-1.0: any breaking change bumps the minor. Plugins compiled against `0.3` do not load against a `0.4` editor. The editor reads the plugin's declared API version from `plugin.toml` and refuses mismatches with a clear error — never a crash, never silent degradation. Every minor bump ships with a `CHANGELOG.md` migration note.

Patch releases (`0.3.1 → 0.3.2`) are additive-only and do not break plugins.

## 3. `plugin.toml` manifest

Every plugin ships a `plugin.toml` at its root.

```toml
[plugin]
id          = "com.acme.brush-extras"
name        = "Brush Extras"
version     = "0.2.1"
api_version = "0.3"             # rustforge-plugin-api minor
runtime     = "wasm"            # or "rust-static"
entry       = "brush_extras.wasm"   # or, for rust-static, crate name
authors     = ["Acme <devs@acme.io>"]
description = "Extra terrain brushes: erosion, ridge, plateau."

[capabilities]
panel            = true
asset_import     = false
asset_edit       = false
scene_read       = true
scene_write      = false
command_register = true
console_command  = true
gizmo            = true
fs_read          = ["assets/brushes/"]    # path allowlist
fs_write         = []
network          = false

[dependencies]
# Inter-plugin deps. Optional; resolved alphabetically after declared deps.
"com.acme.core-utils" = "^0.4"
```

Keys are explicit and boolean-or-allowlist. No catch-all "permissions: full" — that flag would become a meme.

## 4. Extension points

Each point has one registration method on `PluginContext`. Registration returns an opaque `Handle` that the editor uses to tear down the extension on `on_unload`.

### 4.1 Custom panels

```rust
ctx.register_panel(PanelSpec {
    id: "brush_extras.gallery",
    title: "Brush Gallery",
    default_dock: DockSlot::Right,
    factory: Box::new(|| Box::new(BrushGalleryPanel::new())),
});
```

Panel trait matches Phase 3's first-party panel trait. The `egui::Ui` reference handed to WASM plugins is a proxy that marshals draw commands across the boundary.

### 4.2 Custom asset importers

Extends Phase 5's importer registry.

```rust
ctx.register_importer(ImporterSpec {
    id: "brush_extras.svg",
    extensions: &["svg"],
    kind: AssetKind::Texture,
    import: Box::new(|src, settings| svg_to_cooked_texture(src, settings)),
});
```

### 4.3 Custom asset editors

Extends Phase 8's `AssetEditor` registry. The plugin supplies a factory; the editor hosts the tab exactly like a first-party editor.

### 4.4 Custom inspector widgets

Targeted at user-authored reflected types. Keyed by `TypeId`, consulted before the generic reflection-driven fallback.

```rust
ctx.register_inspector_widget::<MyCurveField>(|ui, value, ctx| { /* draw */ });
```

### 4.5 Custom commands

Extends Phase 6's command stack. Plugin-authored commands are fully undoable because they push through the same `CommandStack` — there is no second stack.

```rust
pub trait PluginCommand: Send + 'static {
    fn apply(&mut self, ctx: &mut CommandContext) -> Result<()>;
    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()>;
    fn label(&self) -> &str;
}

ctx.register_command_factory("brush_extras.erosion", |args| Box::new(ErosionCommand::from(args)));
```

Invoking is via `ctx.dispatch("brush_extras.erosion", args)`. Coalescing (Phase 6 §4.2) is available to plugin commands on opt-in.

### 4.6 Custom console commands

Extends Phase 10's console.

```rust
ctx.register_console_command("brush.reload", |argv, ctx| { /* ... */ });
```

The console autocomplete picks up plugin commands alphabetically after first-party.

### 4.7 Custom gizmos / debug draw

```rust
ctx.register_gizmo(GizmoSpec {
    id: "brush_extras.radius",
    draw: Box::new(|frame, ctx| { /* immediate-mode lines/spheres */ }),
});
```

Debug draw routes through the existing editor debug-draw queue — plugins don't hold a renderer handle.

### 4.8 Menu entries

Append to the Tools menu only. Plugins cannot add to File/Edit/View/Window/Help.

```rust
ctx.register_menu_entry(MenuEntry {
    path: &["Tools", "Brush Extras", "Regenerate gallery"],
    shortcut: None,
    action: Box::new(|ctx| { /* ... */ }),
});
```

## 5. Capability system

Capabilities declared in `plugin.toml` are parsed into a `CapabilitySet` attached to the plugin's `PluginContext`. Every `PluginContext` method that touches a resource class performs a grant check against that set before dispatching.

```rust
impl PluginContext<'_> {
    pub fn read_entity(&self, e: Entity) -> Result<EntityView<'_>> {
        self.require(Capability::SceneRead)?;
        self.world.view(e)
    }
    pub fn push_command(&mut self, cmd: Box<dyn PluginCommand>) -> Result<()> {
        self.require(Capability::CommandRegister)?;
        self.stack.push_plugin_command(self.plugin_id, cmd)
    }
}
```

Enforcement differs per runtime:

- **Rust-static plugins**: the `require` check is honor-based. A malicious statically linked plugin can bypass it by calling the underlying `World` directly. This is accepted — Rust-static plugins are a trusted tier. The capability check still exists so that bugs (not malice) are caught early and so that the editor has one uniform code path.
- **WASM plugins**: enforcement is real. Host functions that touch filesystem, network, scene writes, or command dispatch inspect the calling plugin's grants and reject unprivileged calls before any engine state is read or mutated. There is no "escape hatch" host function.

A plugin declaring only `panel = true` that attempts `ctx.write_entity(...)` receives `Err(CapabilityDenied)` at runtime and a toast is shown to the user: "Plugin X attempted SceneWrite without grant."

## 6. Plugin discovery

```
<project>/
├── rustforge-project.toml
├── assets/
└── plugins/
    ├── brush-extras/
    │   ├── plugin.toml
    │   └── brush_extras.wasm
    └── scene-linter/
        ├── plugin.toml
        └── src/...

%APPDATA%/RustForge/plugins/          # Windows
~/.config/rustforge/plugins/          # Linux / macOS
```

Discovery order:

1. Project-local `plugins/` (highest priority; project overrides user-global on id conflict).
2. User-global plugins folder.
3. Alphabetical by `id` within each tier. Deterministic ordering is load-order-as-API; anything less guarantees bugs that reproduce on one machine and not another.

Per-project enable/disable lives in `rustforge-project.toml`:

```toml
[plugins]
enabled = ["com.acme.brush-extras", "com.studio.scene-linter"]
disabled = ["com.acme.legacy-thing"]
```

A plugin discovered but not listed is surfaced in the Plugin Manager panel (§10) as "installed, not enabled."

### 6.1 URL-based install path

The Phase 11 primitive for distribution is a `git+https://...` reference in `rustforge-project.toml`. A CLI subcommand resolves it into `plugins/`.

```toml
[plugin-sources]
"com.acme.brush-extras" = { git = "https://github.com/acme/brush-extras", tag = "v0.2.1" }
```

`rustforge plugins fetch` clones the listed sources into `plugins/`. That's the entire package manager in Phase 11. No registry, no marketplace, no signing — all explicitly out of scope.

## 7. Lifecycle

```rust
pub enum LifecycleEvent {
    SceneOpen { path: PathBuf },
    SceneClose,
    PlayStart,
    PlayStop,
}
```

Every lifecycle dispatch is wrapped in `catch_unwind` for Rust-static plugins and the WASM host's trap boundary for WASM plugins. On error:

1. Log the panic / trap with plugin id and backtrace (file:line if available — Phase 10 §4 surfaces these).
2. Mark the plugin disabled in memory for the rest of the session.
3. Show a toast: "Plugin X crashed during *SceneOpen*. Disabled. See log."
4. Continue dispatch to remaining plugins. One plugin failing must never block the rest.

Ordering guarantees: `on_load` fires for all plugins before any `SceneOpen`; `on_unload` fires in reverse load order on editor shutdown. Inter-plugin `dependencies` form a topological order within the alphabetical tier.

## 8. Hot-reload

### 8.1 Rust-static plugins — no hot-reload

A Rust-static plugin is statically linked into the editor binary. Swapping its code requires recompiling the editor. Phase 11 does **not** implement Rust plugin hot-reload — see §1.B for why dynamic loading is rejected. The UX is: edit plugin source, `cargo build`, restart editor. Document this clearly; attempting to paper over it with any dynamic-load mechanism reintroduces every failure mode that §1.B exists to avoid.

### 8.2 WASM plugins — reuse Phase 7

WASM plugin reload rides on Phase 7 §7's script hot-reload path. A `.wasm` file under `plugins/` is watched; on change:

1. Fire `on_unload` for the old instance (catch failures).
2. Reinstantiate from the new module.
3. Fire `on_load` on the new instance.
4. If `on_load` fails, keep the old instance disabled and toast the error.

All previously registered extensions (panels, importers, gizmos, commands) are torn down and re-registered. The editor's registries are keyed by `(plugin_id, extension_id)` so teardown is O(extensions) and has no ghost entries.

During Playing state, defer reload to end-of-frame (Phase 7 §7.2) — same rule as script hot-reload.

## 9. Plugin settings UI

Each plugin can expose a settings struct via reflection (Phase 2 §2.2):

```rust
#[derive(Reflect, Serialize, Deserialize, Default)]
pub struct BrushExtrasSettings {
    pub gallery_path: PathBuf,
    pub max_thumbnails: u32,
    pub highlight_color: Color,
}

impl Plugin for BrushExtras {
    fn on_load(&mut self, ctx: &mut PluginContext) -> Result<()> {
        ctx.register_settings::<BrushExtrasSettings>()?;
        // ...
    }
}
```

The Project Settings panel grows a "Plugins" section with one page per enabled plugin, rendered by the same reflection-driven inspector as component editing. Settings persist to `rustforge-project.toml` under `[plugins.settings."com.acme.brush-extras"]`. Defaults are computed from `Default`.

## 10. Plugin Manager panel

A new first-party panel (`crates/rustforge-editor/src/panels/plugin_manager.rs`):

```
┌─ Plugins ──────────────────────────────────────────────────────┐
│ [x] com.acme.brush-extras         0.2.1  wasm   [Settings] [..]│
│ [x] com.studio.scene-linter       0.1.0  rust   [Settings] [..]│
│ [ ] com.acme.legacy-thing         0.9.0  wasm   [Enable]   [..]│
│ [!] com.third.broken              0.3.0  wasm   DISABLED — load│
│                                                    failed: see │
│                                                    log.        │
├────────────────────────────────────────────────────────────────┤
│ [Reload all]   [Fetch from manifest]   [Open plugins folder]   │
└────────────────────────────────────────────────────────────────┘
```

Clicking a row expands capabilities, declared dependencies, and last-error (if any). No install-from-URL dialog in the panel itself — that's `rustforge plugins fetch` on the CLI, by design.

## 11. Build order within Phase 11

Each step is independently testable.

1. **`rustforge-plugin-api` crate skeleton** — `Plugin` trait, `PluginContext` stub, `PluginMeta`, `Capability` enum. No extension methods yet. Publish nothing; iterate in-tree.
2. **Manifest parsing** — `plugin.toml` → typed `PluginManifest`. Test round-trips and all capability forms.
3. **Rust-static runtime** — a hardcoded `Vec<Box<dyn Plugin>>` registered at editor startup. Validates the trait and context shapes before any loading logic exists.
4. **Discovery + enable/disable** — walk folders, build in-memory plugin list, honor project settings, expose alphabetical ordering.
5. **Lifecycle dispatch with crash isolation** — `catch_unwind` wrapper, disable-on-fail, toast on error. Test with a deliberately panicking plugin.
6. **Extension points, one at a time** — panel, then menu, then gizmo (all read-only), then importer, then asset editor, then inspector widget, then command, then console command. The order widens capability surface gradually.
7. **Capability enforcement** — add `require` checks to every context method. Honor-mode for Rust-static.
8. **WASM runtime** — reuse Phase 7 host. Wire `Plugin` trait to WASM exports; implement host functions with capability checks. Marshal `egui::Ui` proxy for panels.
9. **WASM hot-reload** — hook into Phase 7's reimport path; tear down and re-register extensions cleanly.
10. **Plugin settings UI** — reflection-driven Project Settings page.
11. **Plugin Manager panel** — enable/disable/reload, show capabilities and errors.
12. **`rustforge plugins fetch` CLI** — git-based resolution into `plugins/`.
13. **Docs + migration** — `CHANGELOG.md` for the plugin API, an example plugin per runtime in `examples/plugins/`.

## 12. Scope boundaries — what's NOT in Phase 11

- ❌ **Marketplace UI inside the editor.** No browsing, searching, rating, or one-click install. The primitive is `rustforge plugins fetch`.
- ❌ **Billing, paid plugins, license key plumbing.** Not now, probably not ever in first-party code.
- ❌ **Plugin signing / signature verification.** Trust is by convention (Rust-static) or by sandbox (WASM). Signing is a separate phase if ever.
- ❌ **Compile-on-demand C++ plugins.** Rust and WASM only.
- ❌ **Sandboxing for compile-time Rust plugins.** Rust-static is a trusted tier. Users who need isolation use the WASM runtime.
- ❌ **Dynamic Rust library (`.dll`/`.so`) loading.** Rejected in §1.B; do not add later without an ADR reversing the decision.
- ❌ **Cross-plugin IPC or pub/sub bus.** Plugins interact with the editor, not each other, in Phase 11. If two plugins need to coordinate, they do it via shared reflected components or a mutually agreed asset format.
- ❌ **Plugin-authored render passes or shader injection.** The renderer's pipeline is first-party in Phase 11. Plugins draw via gizmos/debug-draw only.
- ❌ **Hot-reload for Rust-static plugins.** Restart the editor.

## 13. Risks & gotchas

- **Plugin API version churn.** Every minor bump strands plugins that haven't updated. Mitigate by freezing the API surface aggressively — a new extension point is additive and patch-safe; only shape changes to existing types are breaking. Require a written justification in `CHANGELOG.md` for every minor bump.
- **WASM host surface creep.** Each host function is forever API. Resist one-off "just add a helper" additions. Every new host function needs a ticket, a capability mapping, and a versioning note.
- **Lifecycle crash cascades.** A plugin panicking during `on_unload` during editor shutdown must not block exit. `catch_unwind` around every lifecycle call, with a hard timeout on WASM traps.
- **Capability escape via undeclared Rust-static plugin.** A Rust-static plugin can call `rustforge-core` directly, bypassing `PluginContext`. Accepted as the cost of the trusted tier; document it in the plugin author guide so nobody thinks static plugins are sandboxed.
- **Load-order surprises.** Plugin A registers an importer for `.svg`; plugin B also registers for `.svg`. Alphabetical order decides. Log a clear warning on duplicate registrations with both plugin ids and the winner. Expose an override in project settings for power users.
- **Inter-plugin dependency cycles.** Two plugins each depending on the other. Detect on load; disable both; toast. Never silently pick one.
- **Settings persistence corruption.** A plugin adds a field, old `rustforge-project.toml` lacks it. Use `#[serde(default)]` on every plugin settings field; never fail to load a project because of a missing plugin setting.
- **`egui::Ui` proxy cost for WASM panels.** Every draw call marshals across the WASM boundary. A pathological panel (grids of thousands of cells) will be slow. Document expected perf; suggest batching; accept that truly perf-sensitive panels should be Rust-static.
- **Plugin uninstall leaves orphaned asset-editor tabs open.** Closing a plugin's asset editor tab must succeed even if the plugin that registered it is gone. Tab teardown routes through first-party code, not the plugin.
- **Duplicate `plugin.id` across tiers.** Project-local plugin shadows a user-global one with the same id. Intended behavior, but surprising. Plugin Manager must show both and clearly mark which is active.
- **Command-stack pollution.** A plugin pushes thousands of undo entries. Phase 6's 500 MB cap already protects memory; per-plugin quotas are overengineering for Phase 11 but worth revisiting if abuse is seen.
- **WASM plugin using up host memory.** The host caps each WASM instance's linear memory; misbehaving plugins trap, get disabled, toast. Do not share one instance across plugins.

## 14. Exit criteria

Phase 11 is done when all of these are true:

- [ ] `rustforge-plugin-api` crate compiles standalone and is consumed by at least one example plugin per runtime.
- [ ] `plugin.toml` parses all declared capability forms; malformed manifests produce actionable errors, never panics.
- [ ] Rust-static plugins load, register extensions, receive all lifecycle events, and unload cleanly.
- [ ] WASM plugins load through the Phase 7 host, register extensions, receive all lifecycle events, and unload cleanly.
- [ ] Project-local and user-global plugin folders are discovered; alphabetical load order is deterministic.
- [ ] Per-project enable/disable in `rustforge-project.toml` is honored at load time.
- [ ] A plugin panicking in any lifecycle hook is disabled with a toast; the editor stays up and other plugins continue.
- [ ] A WASM plugin without `scene_write` that attempts `ctx.write_entity` receives `CapabilityDenied` and does not mutate state.
- [ ] Editing a `.wasm` plugin file triggers hot-reload; all registered extensions are torn down and re-registered without ghost entries.
- [ ] Plugin Manager panel lists discovered plugins with status, capabilities, and last-error; enable/disable toggles persist.
- [ ] Plugin settings surface as reflection-driven pages in Project Settings and persist across editor restarts.
- [ ] Custom commands registered by a plugin undo/redo through Ctrl+Z/Ctrl+Y with no special-casing in the command stack.
- [ ] `rustforge plugins fetch` resolves `git+https://` sources into `plugins/` and logs version mismatches against `api_version`.
- [ ] Bumping `rustforge-plugin-api` minor version refuses to load a plugin manifest declaring the old minor, with a clear error.
- [ ] `rustforge-core` still builds and runs without the `editor` feature; nothing in the plugin API leaks into shipped games.
