# Phase 8 — Specialized Editors & Profiler

Phases 2–7 built the general-purpose editor: viewport, inspector, hierarchy, content browser, undo/redo, and Play-in-Editor. Every one of those panels works on *any* entity or asset generically. Phase 8 adds the **long tail**: focused tools for specific asset types (materials, terrain, animation, scripts) plus the profiler panel that Phase 7 deferred.

These tools share a pattern — dockable panel, reflection-driven UI for simple fields, live preview where relevant — so Phase 8 starts with the scaffolding for all of them, then implements each as an independently shippable sub-phase.

## Goals

By end of Phase 8:

1. **Asset-editor scaffolding** — double-clicking an asset in the Content Browser opens a specialized editor if one is registered, otherwise the generic inspector.
2. **Material editor** — live PBR preview (sphere / cube / teapot) alongside a property sheet, edits routed through the command stack.
3. **Terrain tools** — brush-based sculpt/paint over the clipmap, one undo entry per stroke.
4. **Animation preview** — timeline scrubber, play/pause, skeleton overlay; playback-only, not keyframe authoring.
5. **Script editor integration** — one-click handoff to the user's external editor, plus in-editor log tail and file:line navigation on panics.
6. **Profiler panel** — frame-time graph, per-system CPU timings, GPU pass timings via wgpu timestamp queries.

## 1. Shared scaffolding — the `AssetEditor` trait

Every specialized editor in this phase is a dockable panel that owns an asset. The pattern is regular enough to share:

```
crates/rustforge-editor/src/asset_editors/
├── mod.rs                # AssetEditor trait, EditorHost
├── registry.rs           # TypeId<Asset> -> AssetEditor factory
├── host.rs               # dock placement, tab lifecycle, dirty tracking
└── open.rs               # Content Browser dispatch: double-click -> open()
```

```rust
pub trait AssetEditor: Send + 'static {
    fn asset_type(&self) -> TypeId;
    fn asset_guid(&self) -> AssetGuid;
    fn title(&self) -> String;                  // shown as tab label
    fn dirty(&self) -> bool;                    // for * marker and save prompt
    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut EditorContext);
    fn save(&mut self, ctx: &mut EditorContext) -> Result<()>;
    fn close(&mut self, ctx: &mut EditorContext) -> CloseAction;
}
```

`EditorContext` bundles the same things a `CommandContext` does (Phase 6 §2) plus a mutable reference to the asset registry. Keep the APIs shaped alike — specialized editors push commands exactly like panels in earlier phases.

### 1.1 Registration

```rust
registry.register::<MaterialAsset, _>(|guid| Box::new(MaterialEditor::open(guid)));
registry.register::<AnimationClip, _>(|guid| Box::new(AnimationEditor::open(guid)));
```

Generic fallback: if no editor is registered for the asset's `TypeId`, dispatch to the generic inspector (Phase 3) targeting the asset's component-like fields via the reflection registry (Phase 2 §2.2). This means every `#[derive(Reflect)]` asset gets a minimum editor for free.

### 1.2 Tab lifetime

- Opening an already-open asset focuses the existing tab, doesn't spawn a new one.
- Closing a dirty tab prompts save/discard/cancel.
- On scene or project close, all asset-editor tabs prompt once in bulk, then close.
- Asset deletion on disk (Phase 5 file watcher) closes the matching tab with a toast.

## 2. Material editor

Materials are the highest-return target: everyone uses them, the preview is obvious, and the UI maps cleanly to reflection.

```
crates/rustforge-editor/src/asset_editors/material/
├── mod.rs                # MaterialEditor
├── preview.rs            # preview scene: sphere + HDRI + directional light
└── properties.rs         # reflection-driven property panel
```

### 2.1 Layout

```
┌─ Material: brick.mat ─────────────────────────────────┐
│ ┌──────────────┐ ┌──────────────────────────────────┐ │
│ │              │ │ Albedo    [color picker] [tex..] │ │
│ │              │ │ Metallic  [====o------]  0.35    │ │
│ │   Preview    │ │ Roughness [=========o-]  0.80    │ │
│ │   (sphere)   │ │ Normal    [.....] [tex..]        │ │
│ │              │ │ Emissive  [color] * 2.0          │ │
│ │              │ │ HDRI    [ studio ▼ ]             │ │
│ │              │ │ Shape   [ sphere  ▼ ]            │ │
│ └──────────────┘ └──────────────────────────────────┘ │
└────────────────────────────────────────────────────────┘
```

Preview is an offscreen render-to-texture (reuse Phase 2 §2.1). Its camera orbits on drag. HDRI and preview shape are editor-only state, stored in editor prefs (Phase 1 §2 `config.rs`), not in the material.

### 2.2 Property sheet vs. node graph

A node-based material graph (Unreal, Blender) is a significantly larger project than a property sheet — parsing, layout, topological evaluation, code generation into WGSL. The material system that exists today is PBR property-driven. **Phase 8 ships a property sheet only.** Node graph is a candidate future phase, flagged in §8.

### 2.3 Commands

Edits go through a Phase 6–style command:

```rust
pub struct EditMaterialFieldCommand {
    guid: AssetGuid,
    field: FieldPath,
    before: ReflectValue,
    after: ReflectValue,
}
```

Time-window coalescing (Phase 6 §4.2) applies to slider drags and color-picker drags exactly like inspector fields do. A drag-end event (from egui `response.drag_stopped()`) closes the coalescing window immediately.

Saving writes the whole asset through the existing material serializer. The Content Browser's dirty indicator (Phase 5) flips based on `editor.dirty()`.

## 3. Terrain tools

Terrain uses the engine's clipmap. The editor-side work is a brush UI and an undo strategy for heightmap edits.

```
crates/rustforge-editor/src/panels/terrain_tools.rs
crates/rustforge-core/src/terrain/edit.rs     # editor-facing write API
```

### 3.1 Brushes

- **Raise / Lower** — additive height delta falling off with a radial kernel.
- **Smooth** — average with neighbors.
- **Flatten** — pull toward a reference height.
- **Paint layer** — write a texture-layer weight.

Shared brush state: radius, strength, falloff curve, target layer for paint.

### 3.2 Stroke transactions

Every stroke is one undo entry. This maps directly to Phase 6 §4.1:

- Mouse-down on the terrain with a brush selected → `stack.begin_transaction("Raise terrain")`. Snapshot the bounding-box of clipmap tiles the brush could touch.
- Mouse-drag → live writes via `TerrainEdit::paint(region, kernel)`. No commands pushed yet.
- Mouse-up → capture the same bounding-box's after-state, build one `TerrainStrokeCommand { tile_id, before_bytes, after_bytes }` per touched tile, wrap in `CompositeCommand`, `end_transaction()`.

This keeps undo memory proportional to *touched area per stroke*, not total stroke length.

### 3.3 Memory

A 256×256 tile of R32F height data is 256 KB. A large stroke across 16 tiles is 4 MB of before-state plus 4 MB of after-state per command. This fits under Phase 6's 500 MB command-stack cap at reasonable editing rates, but document the number and let users tune it in editor prefs.

### 3.4 Engine hook

`rustforge-core` needs a targeted write API on the clipmap that dirties the right GPU upload regions:

```rust
impl TerrainEdit {
    pub fn paint(&mut self, region: Aabb2, kernel: &dyn BrushKernel);
    pub fn snapshot_region(&self, region: Aabb2) -> TerrainSnapshot;
    pub fn restore_region(&mut self, snap: TerrainSnapshot);
}
```

Gate behind `editor` feature — shipped games never edit terrain.

## 4. Animation preview

Animations are read-heavy at authoring time (validate the import, confirm retarget, scrub to find a bad frame). Authoring is someone else's tool. Phase 8 builds the reader.

```
crates/rustforge-editor/src/asset_editors/animation/
├── mod.rs                # AnimationEditor
├── timeline.rs           # scrubber + play controls
└── skeleton.rs           # bone overlay in preview viewport
```

### 4.1 Layout

Live preview viewport on top (reuse material preview scene with a skinned mesh substituted in), timeline below:

```
┌─ Animation: run_cycle.anim ─────────────────────────┐
│  [Preview mesh: humanoid]                            │
│                                                      │
├──────────────────────────────────────────────────────┤
│ [▶] [⏸] [🔁] 0:00.32 / 0:01.00   speed [1.0x ▼]     │
│ ├────────────●──────────────────────────────────┤    │
└──────────────────────────────────────────────────────┘
```

Scrub drags the playhead. Loop toggles wrap-around playback. Speed scales dt.

### 4.2 Non-goals

**No keyframe editing.** The timeline is a read-only curve view at most. Inserting/removing keyframes is a full authoring tool — out of scope (§8). If the preview reveals a bad clip, the user fixes it in their DCC and reimports; Phase 5's hot-reload picks it up.

### 4.3 Interaction with Play-in-Editor

If the user opens an animation while Phase 7's play mode is Playing, the preview still runs — it renders into an isolated offscreen target with its own mini-world, not the PIE world. Animation preview must never touch the PIE scene world, since Stop would fail to restore it. Enforce by construction: the editor owns a dedicated preview `hecs::World`.

## 5. Script editor integration

There is a well-worn trap in engine editors: writing your own code editor. It ends as a perpetual pit of autocomplete, rust-analyzer integration, and LSP glue that no one finishes. Do not enter the pit.

### 5.1 Policy: hand off to the user's editor

Double-click a `.rs` or `.wasm` script in the Content Browser → launch `$EDITOR` (or platform-configured default; fall back to `rustforge-editor` prefs). VS Code users get VS Code; RustRover users get RustRover; `vim` users get vim. That's the feature.

Per-user config:

```toml
# .rustforge/editor.toml
[scripts]
command = "code"
args    = ["--goto", "{file}:{line}"]
```

### 5.2 What the editor does provide

- **Log tail panel** (`panels/script_console.rs`) — follows the WASM host's stdout / stderr with basic filter. Reuses the Phase 2 `console.rs` widget if it exists; otherwise a thin tail view.
- **Panic navigation** — when the hot-reload system (Phase 7 §7) surfaces a panic with a file:line, clicking the panic opens the source at that location in the external editor using the command template above.
- **Hot-reload indicator** — small "script module reloaded" toast and a timestamp next to each script in the Content Browser. Uses the reimport events already flowing through Phase 5.

### 5.3 Optional: read-only inline view

If an inline "peek at the script without switching windows" view is desired, wrap an existing egui syntax-highlighting widget (`egui_code_editor` or similar) in read-only mode. Do not wire editing — editing belongs to the external tool. This is a nice-to-have; skip if it compromises the phase schedule.

## 6. Profiler panel

Phase 7 §11 deferred this. It fits Phase 8 because it's a specialized panel with its own engine-side data source, and it's most useful while PIE is running.

```
crates/rustforge-editor/src/panels/profiler.rs
crates/rustforge-core/src/telemetry/
├── mod.rs                # FrameTelemetry, Sampler
├── cpu.rs                # per-system CPU timings
└── gpu.rs                # wgpu timestamp queries, resolve-and-read
```

### 6.1 Data

- **Frame times** — ring buffer, last 240 frames (~4 s at 60 Hz).
- **Per-system CPU** — each ECS system's `tick` wrapped in `Sampler::scope("name")`. Zero cost in game builds via `#[cfg(feature = "editor")]`.
- **GPU pass timings** — a timestamp query per pass. Resolve happens one frame late; the panel displays N-1 to avoid stalls.

Memory: a ring of 240 frames × ~32 systems × ~10 GPU passes ≈ a few hundred KB. Cheap.

### 6.2 Panel layout

```
┌─ Profiler ─────────────────────────────────────────┐
│ Frame ▁▂▂▁▂▃▃▂▁▂▂▂█▅▂▂▁▂▁  16.9 / 16.6 ms          │
│                                                     │
│ CPU                                   GPU           │
│  TransformSys   1.1 ms                 GBuffer 3.2  │
│  PhysicsStep    4.3 ms                 SSAO    0.9  │
│  ScriptTick     2.7 ms                 Light   4.1  │
│  RenderSubmit   0.4 ms                 Post    1.0  │
│                                                     │
│ [Pause]  [Clear]  [Export JSON]                     │
└─────────────────────────────────────────────────────┘
```

Bars over the 16.6 ms budget highlight red. Export writes a JSON file for offline analysis.

### 6.3 Play-mode interaction

The profiler is most useful during PIE. While `PlayState::Playing`, telemetry reflects `tick_play`; while Edit, it reflects `tick_edit` (much shorter). Make this explicit in the panel header: "CPU — play" vs "CPU — edit". No gating — the panel works in both states.

### 6.4 Zero cost when shipped

All telemetry types live in `rustforge-core` but the `Sampler::scope` macro expands to a no-op without the `editor` feature. Verify with `cargo bloat` that a game build doesn't carry the timing calls.

## 7. Build order within Phase 8

Each step is an independently shippable sub-phase.

1. **Asset-editor scaffolding** (§1) — `AssetEditor` trait, registry, Content Browser dispatch, tab lifecycle. Validates the pattern before any specific editor depends on it.
2. **Profiler panel** (§6) — smallest, no new commands, independent of the other editors. Good warm-up.
3. **Material editor** (§2) — highest-return for users; exercises the preview-viewport pattern that §4 reuses.
4. **Animation preview** (§4) — reuses the preview viewport from §2 with a skinned mesh. Confirms the preview-world isolation design before anything mutates world state.
5. **Terrain tools** (§3) — largest engine-side change (`TerrainEdit`), landing last on the client side so the command/undo patterns are settled.
6. **Script editor integration** (§5) — tiny in surface area but depends on Phase 7 hot-reload signals; best landed after everything else is stable.

## 8. Scope boundaries — what's NOT in Phase 8

- ❌ **Node-based material graph.** Property sheet only.
- ❌ **Keyframe animation authoring.** Preview and scrub only.
- ❌ **In-editor code editor with IntelliSense / LSP.** Hand off to external editor.
- ❌ **Shader graph / WGSL authoring tools.** Edit shader files in the external editor like scripts.
- ❌ **Terrain biomes, foliage scatter, erosion simulation.** Separate future phase.
- ❌ **Remote / attached profiler** (connect to a running game binary). Editor-process profiling only.
- ❌ **Flamegraph / call-tree profiler UI.** A bar panel is plenty for Phase 8.
- ❌ **Audio mixer / sound-bank editor.** Future specialized editor.
- ❌ **Particle system editor.** Future specialized editor.

## 9. Risks & gotchas

- **Preview viewports chewing GPU memory.** Each open specialized editor with a live preview allocates an offscreen color + depth target. Ten tabs × 512² × f16 ≈ manageable, but a pathological user could open 50. Cap: pause rendering in inactive (non-focused) tabs; resume on focus. Resize preview targets to panel size, not a fixed resolution.
- **Terrain undo memory.** A broad stroke over many tiles consumes MB of snapshot. Phase 6's 500 MB stack cap can get hit by a power user. Expose the cap in prefs and warn when a single command exceeds, say, 50 MB.
- **Animation preview tick bleed.** Preview world and PIE world sharing a renderer is fine; sharing systems is not. The animation preview must tick only its own world. If a global `Time` resource is used, inject a scoped clone.
- **Script-editor launch vs. hot-reload race.** User opens a script, edits it, saves in external editor; Phase 5 file watcher fires reimport; WASM host swaps the module. If this happens mid-tick, Phase 7 §7 already says "defer to end of frame." Phase 8 adds nothing here but must respect it — don't reload on any event besides the one Phase 7 defined.
- **Profiler overhead skewing readings.** GPU timestamp queries have cost. Keep them to pass granularity (not draw-call granularity) in Phase 8. CPU samplers are cheap if implemented as `Instant::now` / `elapsed` at scope boundaries.
- **Profiler data sync with PIE snapshot restore.** On Stop, the engine resets. The profiler ring buffer should persist across the boundary (user wants to see what just happened), but it should mark the transition visually — a vertical line at the Stop frame.
- **Reflection drift.** A material adds a field; the material editor doesn't update. Reflection-driven UI is specifically meant to prevent this, but custom widgets (color picker, texture slot) bypass reflection. Every custom widget must have a reflection-path fallback so new fields at minimum appear with a default editor.
- **Asset deletion while tab is open.** Content Browser deletes `brick.mat`; the material editor has it open with unsaved edits. File watcher fires. Policy: close tab immediately, discard edits, toast the user. Don't try to preserve in-memory edits for a file that no longer exists.
- **Tab tabs in dock layout.** `egui_dock` has opinions about dynamic tabs. Pin the set of asset-editor tabs to a dedicated dock area to avoid fighting the layout system.
- **External editor launcher on Windows.** `start` vs. `cmd /c start` vs. direct PATH lookup. Use a crate like `open` or handle the platform split explicitly; do not shell out via `cmd /c` with user-supplied paths (quoting hazard).

## 10. Exit criteria

Phase 8 is done when all of these are true:

- [ ] `AssetEditor` trait, registry, and Content Browser double-click dispatch exist and unit-test green.
- [ ] Opening an unregistered asset type falls back to the generic inspector.
- [ ] Tab lifecycle: open, focus, dirty prompt on close, bulk prompt on project close.
- [ ] Material editor renders a live preview with HDRI, shape choice, and reflects edits in real time.
- [ ] Material edits route through the command stack; Ctrl+Z undoes slider drags as a single unit.
- [ ] Terrain brushes (raise/lower/smooth/flatten, paint) produce exactly one undo entry per mouse-down / mouse-up stroke.
- [ ] Animation preview plays, pauses, loops, and scrubs a selected clip on a skinned mesh in an isolated preview world.
- [ ] Animation preview does not touch the Play-in-Editor scene world (verified with a determinism test analogous to Phase 7 §13).
- [ ] Double-clicking a script opens it in the user's configured external editor via the prefs-driven command template.
- [ ] Hot-reload events from Phase 7 surface as an indicator in the Content Browser and the script console.
- [ ] Profiler panel shows frame-time graph, CPU system timings, and GPU pass timings with over-budget highlighting.
- [ ] Profiler telemetry is zero-cost in a build without the `editor` feature (verified via `cargo bloat` or equivalent).
- [ ] All specialized editors honor Phase 6's command stack (no direct world writes bypassing it).
- [ ] All specialized editors respect Phase 7's play-mode freeze — destructive actions disabled while Playing/Paused.
- [ ] `rustforge-core` still builds and runs without the `editor` feature.
