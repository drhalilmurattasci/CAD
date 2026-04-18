# Phase 1 — RustForge Engine Game Editor Plan

Given the size of the engine (~47k LOC) and the patch cadence, the editor should be treated as a **separate crate** that links against the engine as a library, not bolted into it. Here's a concrete, modular plan.

## 1. Workspace layout

Add a new crate alongside the engine:

```
rustforge-engine/
├── Cargo.toml                    # workspace root
├── crates/
│   ├── rustforge-core/           # existing engine as lib
│   ├── rustforge-editor/         # NEW — the editor binary
│   └── rustforge-editor-ui/      # NEW — reusable UI widgets (optional split)
```

Rationale: the editor is a *client* of the engine. Keeping it in its own crate prevents editor-only deps (egui, rfd, notify, etc.) from polluting runtime builds and lets you ship `rustforge-core` standalone for games.

## 2. Editor crate internal structure

```
crates/rustforge-editor/
├── Cargo.toml
└── src/
    ├── main.rs                   # entry, window + wgpu surface setup
    ├── app.rs                    # EditorApp: top-level state machine
    ├── config.rs                 # editor prefs, layout persistence
    │
    ├── panels/                   # each dockable panel = one file
    │   ├── mod.rs
    │   ├── viewport.rs           # 3D scene view, gizmos, camera
    │   ├── hierarchy.rs          # scene tree / entity list
    │   ├── inspector.rs          # component editor (per-entity)
    │   ├── content_browser.rs    # asset browser (files on disk)
    │   ├── console.rs            # log output + command input
    │   ├── profiler.rs           # frame stats, GPU timings
    │   ├── material_editor.rs    # PBR material tweaker
    │   ├── terrain_tools.rs      # clipmap sculpt/paint
    │   ├── animation.rs          # skeletal anim preview
    │   └── script_editor.rs      # WASM script editing hooks
    │
    ├── gizmos/                   # manipulation widgets
    │   ├── mod.rs
    │   ├── translate.rs
    │   ├── rotate.rs
    │   ├── scale.rs
    │   └── picking.rs            # GPU-based entity picking
    │
    ├── commands/                 # undo/redo system
    │   ├── mod.rs                # Command trait + CommandStack
    │   ├── transform.rs
    │   ├── entity.rs             # spawn/despawn/reparent
    │   ├── component.rs          # add/remove/modify
    │   └── asset.rs
    │
    ├── selection/
    │   ├── mod.rs                # SelectionSet resource
    │   └── marquee.rs            # box-select in viewport
    │
    ├── assets/                   # editor-side asset pipeline
    │   ├── mod.rs
    │   ├── importer.rs           # drag-drop dispatch
    │   ├── thumbnails.rs         # async thumbnail generation
    │   ├── watcher.rs            # notify-rs file watching
    │   └── meta.rs               # .meta sidecar files (GUIDs)
    │
    ├── scene/                    # scene save/load (editor format)
    │   ├── mod.rs
    │   ├── serializer.rs
    │   └── prefab.rs
    │
    ├── play_mode/                # PIE (play-in-editor)
    │   ├── mod.rs
    │   ├── snapshot.rs           # ECS state snapshot/restore
    │   └── controls.rs           # play/pause/step
    │
    ├── input/
    │   ├── mod.rs
    │   ├── shortcuts.rs          # configurable hotkeys
    │   └── viewport_nav.rs       # fly/orbit cam in editor
    │
    ├── rendering/                # editor-only draw passes
    │   ├── mod.rs
    │   ├── outline.rs            # selection outline
    │   ├── grid.rs               # infinite ground grid
    │   ├── debug_draw.rs         # wireframes, bounds, colliders
    │   └── picking_pass.rs       # entity-ID render target
    │
    ├── docking/                  # window layout
    │   ├── mod.rs
    │   └── layout.rs             # serialize dock state
    │
    └── util/
        ├── mod.rs
        └── paths.rs              # project paths helper
```

Rule of thumb: **one responsibility per file; folder when it grows past ~3 tightly-related files.**

## 3. Tech choices (minimal, Rust-native)

- **UI:** `egui` + `egui-wgpu` + `egui_dock` for docking. Pure Rust, integrates with your existing wgpu surface, no C++ deps. Alternative if you want retained-mode later: `iced`. Avoid Dear ImGui bindings — not worth the FFI for a Rust codebase this size.
- **File watching:** `notify`
- **File dialogs:** `rfd`
- **Serialization:** stick with whatever `rustforge-core` uses (likely `serde` + `ron` or `bincode`); scenes as `.ron` for readability, assets as binary.
- **Window/input:** `winit` (almost certainly already a core dep).

## 4. Engine-side hooks needed

The editor should not reach into engine internals. Add these to `rustforge-core` as a stable editor-facing API, ideally behind an `editor` feature flag:

- **Reflection:** some way to enumerate components on an entity and edit their fields generically. Given hecs, this probably means a registry: `ComponentRegistry` mapping `TypeId → (name, serialize_fn, deserialize_fn, inspect_fn)`. Each component opts in via a macro or manual registration.
- **Headless tick control:** ability to advance the world N frames, pause physics, snapshot/restore ECS state (for Play-in-Editor).
- **Render-to-texture:** viewport panel needs the engine to render into an offscreen texture that egui samples, not directly to the swapchain.
- **Picking support:** an entity-ID GBuffer attachment the editor can read back.
- **Event bus / hooks:** asset reload notifications, scene-changed events.

If those don't exist yet, they're the first patches before any UI work.

## 5. Suggested build order

1. **Foundation** — editor crate compiles, opens a window, renders engine viewport into an egui texture, docking works.
2. **Hierarchy + Inspector** — component reflection registry in core, basic field editors (f32, Vec3, Color, asset refs).
3. **Gizmos + Selection + Picking** — translate/rotate/scale, GPU picking pass.
4. **Scene I/O** — save/load scene to `.ron`, new/open/save menu.
5. **Content Browser + Asset Pipeline** — file watcher, thumbnails, drag-drop import (you already have drag-drop import — wire it into the browser).
6. **Undo/Redo** — Command trait, wrap all inspector/gizmo/hierarchy mutations.
7. **Play-in-Editor** — ECS snapshot, play/pause/step.
8. **Specialized editors** — material, terrain, animation, script. These are the long tail; each is its own patch cycle.

## 6. Things worth deciding up front

- **Editor = same process as game, or separate?** Same process is simpler and what UE/Unity do. Stick with same-process.
- **Scene format:** `.ron` (human-readable, diff-friendly) vs binary. Recommend `.ron` for scenes, binary for cooked assets.
- **Asset GUIDs:** every asset needs a stable ID independent of path, stored in a `.meta` sidecar. Do this from day one — retrofitting is painful.
- **Reflection approach:** derive macro (`#[derive(Reflect)]`) vs manual registration. Derive macro is more work upfront, huge payoff later. Worth a dedicated `rustforge-reflect` crate.
