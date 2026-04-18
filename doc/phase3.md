# Phase 3 — Hierarchy, Inspector, Gizmos

Phase 2 gave you a live viewport and the plumbing (reflection registry, picking). Phase 3 is where the editor starts feeling like an *editor*: you can see the scene's entities, select them, edit their components, and move them around with gizmos.

## Goals

By end of Phase 3:

1. **Hierarchy panel** lists every entity in the scene as a tree, supports selection, reparenting via drag-drop, and rename.
2. **Inspector panel** shows all components of the selected entity, with auto-generated UI for each field via the reflection registry.
3. **Gizmos** (translate / rotate / scale) work in the viewport on the selected entity, with hotkey switching (W/E/R) and local/world space toggle.
4. **Selection** is a first-class concept shared across all panels — click in viewport → highlights in hierarchy → populates inspector.

## 1. Selection system (foundation for everything else)

Everything in Phase 3 depends on a clean selection model. Build it first.

```
crates/rustforge-editor/src/selection/
├── mod.rs                # SelectionSet: Vec<Entity>, primary, active
└── marquee.rs            # (stub for Phase 3; real impl later)
```

API:

```rust
pub struct SelectionSet {
    entities: Vec<Entity>,
    primary: Option<Entity>,   // last-clicked, used by inspector
}

impl SelectionSet {
    pub fn set(&mut self, e: Entity);
    pub fn add(&mut self, e: Entity);      // shift-click
    pub fn toggle(&mut self, e: Entity);   // ctrl-click
    pub fn clear(&mut self);
    pub fn primary(&self) -> Option<Entity>;
    pub fn iter(&self) -> impl Iterator<Item = Entity>;
}
```

Store it on `EditorApp` as a single source of truth. Panels read it; commands mutate it. No panel owns its own selection state.

## 2. Parent/child relationships in the engine

The hierarchy panel needs a parent-child graph. If `rustforge-core` doesn't have one yet, add it now — it's a core ECS concept, not editor-specific.

```
crates/rustforge-core/src/scene/
├── hierarchy.rs          # Parent, Children components + helpers
└── transform_system.rs   # world-from-local propagation
```

Components:

```rust
#[derive(Reflect)] pub struct Parent(pub Entity);
#[derive(Reflect)] pub struct Children(pub Vec<Entity>);  // or SmallVec
#[derive(Reflect)] pub struct Name(pub String);           // editor-facing label
```

A system each frame walks roots → leaves to compute `GlobalTransform` from `Transform + Parent`. Standard stuff; `bevy_hierarchy` is a good reference but don't pull in bevy.

Invariants to enforce in helpers (don't let users touch the raw components directly from the inspector):
- `Parent(p)` ⟺ `p`'s `Children` contains self.
- No cycles.
- Removing an entity removes it from its parent's `Children`.

Put these behind `scene::hierarchy::{set_parent, remove_parent, despawn_recursive}`.

## 3. Hierarchy panel

```
crates/rustforge-editor/src/panels/
└── hierarchy.rs
```

Responsibilities:

- Walk roots (entities with no `Parent`), recursively draw `CollapsingHeader` per entity.
- Display name from `Name` component, or fall back to `Entity({id})`.
- Click = `selection.set(entity)`. Shift-click extends, ctrl-click toggles.
- Right-click → context menu: Rename, Duplicate, Delete, Create Child, Create Empty.
- Drag-drop for reparenting (egui has `dnd_drag_source` / `dnd_drop_zone`).
- F2 or double-click to rename inline.
- Scroll-to-selected when selection changes from viewport picking.

Implementation notes:

- Use a stable ordering. Either sort children by insertion order (store index in `Children`) or alphabetically — pick one and stick to it. Unordered ECS iteration will cause the tree to reshuffle every frame, which is disorienting.
- Drag-drop reparenting goes through the command system (Phase 6) once it exists; for now, call `scene::hierarchy::set_parent` directly and accept no-undo.
- Entity count can be large. If iteration gets slow, cache the tree and invalidate on structural changes — but don't optimize until measured.

## 4. Inspector panel

This is the Phase 3 centerpiece. It consumes the reflection registry from Phase 2 and renders editable UI per component.

```
crates/rustforge-editor/src/panels/
└── inspector.rs

crates/rustforge-editor/src/inspect/     # UI generators per type
├── mod.rs                # Inspect trait: fn ui(&mut self, ui: &mut egui::Ui) -> Response
├── primitives.rs         # f32, i32, bool, String
├── math.rs               # Vec2, Vec3, Vec4, Quat (as euler), Mat4 (readonly)
├── color.rs              # Color with color picker
├── asset_ref.rs          # Handle<Mesh>, Handle<Texture> etc. — drag-drop target (wires up in Phase 5)
└── fallback.rs           # generic struct walker for unknown types
```

### 4.1 Inspect trait

Separate from `Reflect`. `Reflect` is *structural* (core crate, no UI deps); `Inspect` is *presentational* (editor crate, egui deps). This split lets the core crate stay headless.

```rust
pub trait Inspect {
    fn inspect(&mut self, ui: &mut egui::Ui) -> egui::Response;
}
```

Implement it for primitives directly. For user components with `#[derive(Reflect)]`, provide a blanket implementation that walks fields via the reflection API and dispatches each field to its `Inspect` impl (looked up through the registry).

### 4.2 Inspector panel flow

```
for each component on selection.primary:
    draw CollapsingHeader with component type_name
    look up ComponentVTable in registry
    call vtable.inspect(component_ptr, ui)
    track if response.changed() → mark scene dirty
```

Bottom of the panel: "Add Component" button → popup with searchable list of all registered components. Clicking inserts a `Default::default()` instance.

### 4.3 Quat editing

Quaternions should display as euler angles (pitch/yaw/roll in degrees) in the UI while staying quats in storage. This is a surprisingly common source of confusion if done wrong — convert on display, convert back on edit, and don't round-trip through euler every frame (that accumulates drift). Only write back to the quat when the user actually changes a value.

### 4.4 Multi-edit (optional for Phase 3)

If multiple entities are selected, show only components common to all, and edit all of them simultaneously. Nice to have. Skip if it balloons scope — put it in Phase 3.5.

## 5. Gizmos

```
crates/rustforge-editor/src/gizmos/
├── mod.rs                # GizmoMode enum, GizmoState
├── translate.rs          # 3 axis arrows + 3 plane handles + center screen-space
├── rotate.rs             # 3 axis rings + screen-space ring
├── scale.rs              # 3 axis boxes + uniform center box
└── raycast.rs            # ray vs handle intersection
```

### 5.1 Build vs buy

Two real options:

- **Use `egui-gizmo` or `transform-gizmo-egui`** — works out of the box, draws into egui's painter on top of the viewport image. Fast to integrate. Less control.
- **Roll your own in wgpu** — draw gizmos as a post-pass in the engine's offscreen target. More work, but matches the engine's rendering style and gives you depth-aware gizmos.

Recommendation: **start with `transform-gizmo-egui`** for Phase 3. It's actively maintained, correct, and unblocks the rest of the editor. You can swap for a custom implementation in a later phase if you want gizmos that occlude with scene geometry or draw with engine materials.

### 5.2 Gizmo state machine

```rust
pub enum GizmoMode { Translate, Rotate, Scale }
pub enum GizmoSpace { Local, World }

pub struct GizmoState {
    pub mode: GizmoMode,
    pub space: GizmoSpace,
    pub snap: Option<f32>,          // grid snap for translate, degree snap for rotate
    pub active_drag: Option<DragState>,
}
```

Hotkeys (match industry convention):
- `Q` — select (no gizmo)
- `W` — translate
- `E` — rotate
- `R` — scale
- `X` — toggle local/world
- `Ctrl` held — snap
- `Shift` during drag — duplicate-and-drag (nice-to-have)

### 5.3 Gizmo → scene mutation

Gizmo manipulation must go through a *pending transform* pattern:

1. Drag starts → snapshot original transform.
2. Drag continues → write delta to `Transform` every frame for visual feedback (no undo entry yet).
3. Drag ends → push a single `TransformCommand { before, after }` to the (future) command stack.

This keeps the command history clean — one undoable operation per drag, not one per frame. The command stack doesn't exist until Phase 6, but design the gizmo code to produce a before/after pair now, and just apply the after value directly. When Phase 6 lands, you only change the drag-end path.

## 6. Viewport integration

The viewport panel (from Phase 2) needs three additions:

1. **Picking click** — on left-click inside viewport rect, call the picking pass readback and update `SelectionSet`. Shift/Ctrl modifiers honored.
2. **Gizmo overlay** — after the scene image is drawn, overlay the gizmo using the same screen rect. Hand the gizmo the panel's rect and the camera matrices.
3. **Outline pass** — selected entities get a colored outline. Cheapest approach: mask pass (draw selected entities to a 1-channel texture) + edge-detect in a fullscreen pass, composite on top. Add it to the engine's editor-only passes:

```
crates/rustforge-core/src/render/passes/
└── outline.rs            # #[cfg(feature = "editor")]
```

Engine exposes `render(target, &EditorOverlay)` where `EditorOverlay { selection: &[Entity], gizmo: Option<...> }`. Keeps editor data out of the core render loop while giving the editor what it needs.

## 7. Panel layout & docking

By end of Phase 3 you have 3 real panels. Update the default dock layout:

```
┌─────────────────────────────────────┬────────────────────┐
│                                     │                    │
│           Viewport                  │     Hierarchy      │
│                                     │                    │
│                                     ├────────────────────┤
│                                     │                    │
│                                     │     Inspector      │
│                                     │                    │
└─────────────────────────────────────┴────────────────────┘
```

Classic Unity-style layout. Persist the dock state to `config.rs` so it survives restarts. Phase 3 is a good time to actually implement `config.rs` — serialize `egui_dock::DockState` + window size/position to `~/.config/rustforge/editor.ron` (or platform equivalent via `directories` crate).

## 8. Build order within Phase 3

1. **`SelectionSet`** — lives on `EditorApp`, no UI yet. Wire up viewport picking from Phase 2 to call `selection.set(entity)`. Log selection changes.
2. **Hierarchy components in core** — `Parent`, `Children`, `Name`, helper functions, transform propagation system. Unit tests for invariants.
3. **Hierarchy panel (read-only)** — just draws the tree with selection highlighting. No drag-drop, no context menu yet.
4. **Hierarchy panel (mutations)** — context menu, rename, delete, create. Drag-drop reparenting.
5. **`Inspect` trait + primitives** — get f32, Vec3, bool, String, Color editing working with a hardcoded test component.
6. **Inspector panel** — iterate components of `selection.primary`, dispatch through registry. "Add Component" popup.
7. **Gizmo integration** — drop in `transform-gizmo-egui`, hotkey switching, local/world toggle. Translate first, then rotate, then scale.
8. **Outline pass** — editor-only render pass, visually confirms selection.
9. **Layout persistence** — save/load dock state and window geometry.

## 9. Scope boundaries — what's NOT in Phase 3

- ❌ Scene save/load (Phase 4) — in-memory scenes only for now.
- ❌ Prefabs (Phase 4).
- ❌ Content browser / asset drag-drop into inspector (Phase 5). Asset-ref fields can show a placeholder: `[Asset: MeshHandle(42)]`.
- ❌ Undo/redo (Phase 6). Document the before/after pattern in gizmos and inspector, but don't build the stack.
- ❌ Multi-edit in inspector (optional; Phase 3.5 if desired).
- ❌ Custom per-component inspectors (e.g., a curve editor for `AnimationCurve`). Default field-walker UI is enough for Phase 3.

## 10. Risks & gotchas

- **Reflection edge cases.** `Vec<T>`, `Option<T>`, and `HashMap<K, V>` will show up in real components. If Phase 2's derive only handles plain structs, these components will panic or silently fail in the inspector. Decide upfront: either expand `Reflect` to handle collections (small scope bump in Phase 2 retrospective) or have the fallback UI print `<unsupported>` and move on.
- **Borrow checker pain with ECS + UI.** The inspector needs `&mut Component` while iterating the component list. With `hecs`, grab the entity's components once per frame into a temp list or use `World::entity(e).get_mut::<T>()` per component. Design the panel function signature deliberately — this is a place where bad ergonomics early will haunt you.
- **Gizmo + viewport coordinate mismatch.** Gizmo library expects screen-space coordinates relative to the viewport rect, not the window. Easy to get wrong; verify with a hardcoded entity at origin and a known camera.
- **Selection lag.** Picking readback is async (GPU → CPU). Either stall for one frame (simple, imperceptible) or do a 1-frame-delayed selection (invisible but surprising when scripted). Stall is fine.
- **Derive macro maintenance.** Every new component needs `#[derive(Reflect)]`. When you forget, it doesn't appear in the inspector — silent failure. Add a debug-build lint or a `#[register_component]` attribute that enforces registration at startup.

## 11. Exit criteria

Phase 3 is done when all of these are true:

- [ ] Hierarchy panel shows all entities as a tree with names.
- [ ] Clicking an entity in hierarchy or viewport selects it; selection is visible in both places.
- [ ] Inspector shows all components of the selected entity with editable fields for at least: `f32`, `Vec3`, `Quat` (as euler), `Color`, `bool`, `String`.
- [ ] Changing a value in the inspector updates the viewport immediately.
- [ ] "Add Component" and "Remove Component" work from the inspector.
- [ ] Translate, rotate, and scale gizmos all work on the selected entity via W/E/R.
- [ ] Local/world space toggle works.
- [ ] Selected entity has a visible outline in the viewport.
- [ ] Reparenting via hierarchy drag-drop works and preserves world-space transform.
- [ ] Dock layout and window size persist across restarts.
- [ ] `rustforge-core` still builds without the `editor` feature.
