# Phase 2 — Foundation & Core Editor Hooks

Phase 1 was the architectural plan. Phase 2 is the first real code: get the editor crate compiling, get the engine rendering into it, and land the three engine-side hooks that everything else depends on (render-to-texture, reflection, picking). No panels yet beyond a working viewport — those come in Phase 3.

## Goals

By end of Phase 2:

1. `cargo run -p rustforge-editor` opens a window with a docked viewport showing the engine rendering a test scene.
2. `rustforge-core` exposes a stable `editor` feature with render-to-texture, a component reflection registry, and entity-ID picking.
3. Editor has a frame loop that drives the engine forward each tick and samples the engine's offscreen target into egui.

## 1. Workspace split

Convert the current single-crate layout into a workspace.

```
rustforge-engine/
├── Cargo.toml                    # [workspace] only
├── crates/
│   ├── rustforge-core/           # existing code moves here
│   │   ├── Cargo.toml            # features = ["editor"]
│   │   └── src/
│   └── rustforge-editor/
│       ├── Cargo.toml
│       └── src/main.rs
```

Root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/rustforge-core", "crates/rustforge-editor"]

[workspace.dependencies]
wgpu = "..."        # pin shared versions here
winit = "..."
glam = "..."
hecs = "..."
serde = { version = "...", features = ["derive"] }
```

CI (GitHub Actions) should build `--workspace --all-features` and run clippy on both crates.

## 2. Engine-side hooks (`rustforge-core`)

These three are the minimum for a useful editor. Gate all of them behind `#[cfg(feature = "editor")]` so shipped games don't pay the cost.

### 2.1 Render-to-texture

The engine currently (presumably) renders to the swapchain. The editor needs it to render into an offscreen `wgpu::Texture` that egui can sample.

Add to `rustforge-core`:

```
src/render/
├── target.rs             # RenderTarget enum: Surface | Offscreen
└── frame.rs              # Frame descriptor: target, size, camera
```

- `RenderTarget::Offscreen { color: TextureView, depth: TextureView, size: UVec2 }`
- The main render graph takes `&RenderTarget` instead of assuming a surface.
- Resize: editor tells engine when the viewport panel resizes; engine reallocates the offscreen textures.
- The GBuffer, SSR, volumetric fog, etc. all need to respect the new size — audit each pass for hardcoded swapchain references.

This is the single biggest engine change in Phase 2. Budget accordingly.

### 2.2 Reflection registry

Needed for the Inspector panel (Phase 3) but the *registry* infrastructure lands now.

```
crates/rustforge-reflect/          # NEW proc-macro crate
├── Cargo.toml
└── src/lib.rs                     # #[derive(Reflect)]

crates/rustforge-core/src/reflect/
├── mod.rs                         # Reflect trait, FieldInfo
├── registry.rs                    # ComponentRegistry (TypeId -> VTable)
├── vtable.rs                      # serialize_fn, deserialize_fn, inspect_fn
└── primitives.rs                  # Reflect impls for f32, Vec3, Color, etc.
```

API sketch:

```rust
pub trait Reflect: 'static {
    fn type_name() -> &'static str;
    fn fields() -> &'static [FieldInfo];
    fn get_field(&self, idx: usize) -> &dyn Reflect;
    fn get_field_mut(&mut self, idx: usize) -> &mut dyn Reflect;
}

pub struct ComponentRegistry { /* HashMap<TypeId, ComponentVTable> */ }
```

Each component opts in:

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct Transform { pub position: Vec3, pub rotation: Quat, pub scale: Vec3 }
```

Start with derive for plain structs only. Enums, generics, and `#[reflect(skip)]` attributes can come later — don't let scope creep delay Phase 2.

### 2.3 Entity-ID picking pass

An extra render pass (or GBuffer attachment) that writes `entity.to_bits() as u32` per fragment into an R32Uint texture. Editor reads a single pixel on click → selected entity.

```
crates/rustforge-core/src/render/passes/
└── picking.rs            # editor-only pass, #[cfg(feature = "editor")]
```

Keep it feature-gated. Simpler than reusing the GBuffer — a dedicated pass at quarter-res is fine for Phase 2; optimize later if needed.

### 2.4 Tick control

Small but important. Expose:

```rust
impl Engine {
    pub fn tick(&mut self, dt: f32);           // advance one frame
    pub fn render(&mut self, target: &RenderTarget);
    pub fn set_paused(&mut self, paused: bool); // freezes physics + scripts
}
```

Editor drives these explicitly instead of the engine owning the main loop. The game binary still has its own loop calling the same functions — no duplication.

## 3. Editor crate skeleton

Phase 2 only fills in a subset of the Phase 1 layout. Create empty `mod.rs` stubs for the rest so imports don't break later, but only implement:

```
crates/rustforge-editor/src/
├── main.rs               # winit event loop, wgpu surface, egui setup
├── app.rs                # EditorApp { engine, dock_state, viewport_texture }
├── config.rs             # stub — just a struct with defaults for now
│
├── panels/
│   ├── mod.rs            # Panel trait
│   └── viewport.rs       # renders engine offscreen texture via egui::Image
│
├── docking/
│   ├── mod.rs
│   └── layout.rs         # egui_dock::DockState setup, hardcoded default
│
├── input/
│   ├── mod.rs
│   └── viewport_nav.rs   # fly cam when viewport focused
│
└── rendering/
    ├── mod.rs
    └── grid.rs           # infinite ground grid — nice visual proof of life
```

Dependencies (`crates/rustforge-editor/Cargo.toml`):

```toml
[dependencies]
rustforge-core = { path = "../rustforge-core", features = ["editor"] }
egui = "0.28"
egui-wgpu = "0.28"
egui-winit = "0.28"
egui_dock = "0.13"
winit = { workspace = true }
wgpu = { workspace = true }
```

Pin egui/egui-wgpu/egui_dock to matching versions — mismatched egui versions across these crates is the #1 setup pain.

## 4. The egui ↔ wgpu integration detail

The tricky part of Phase 2 is the render order per frame:

1. Engine renders scene to `offscreen_color_texture`.
2. Register that texture with egui-wgpu: `egui_renderer.register_native_texture(...)` → get `egui::TextureId`.
3. Viewport panel does `ui.image((texture_id, size))`.
4. Egui renders its own UI (including the viewport image) to the swapchain.

Gotchas:
- Re-register the texture every time it's resized (or use `update_egui_texture_from_wgpu_texture`).
- Viewport camera aspect ratio must follow the panel's available rect, not the window.
- Input events inside the viewport rect go to the fly cam; everything else goes to egui. `response.hovered()` + `response.dragged()` is the gate.

Write this as one file (`panels/viewport.rs`) with clear comments — future-you will thank present-you.

## 5. Build order within Phase 2

Do these in order; each is independently testable:

1. **Workspace split** — just move files, no behavior change. Confirm `cargo build -p rustforge-core` still works.
2. **Tick control API** — refactor engine main loop into `tick` + `render`. Game binary adapts. No editor yet.
3. **Render-to-texture** — `RenderTarget` enum, offscreen path. Test by rendering to a texture and dumping it to a PNG from the game binary.
4. **Editor skeleton** — main.rs opens a window with egui, shows "Hello" panel. No engine integration yet.
5. **Viewport panel** — wire engine offscreen texture into egui. This is the "it works" moment.
6. **Fly cam in viewport** — input routing, WASD + RMB look.
7. **Grid + clear-color** — visual polish, proves the editor renders a live scene.
8. **Reflection registry** — derive macro, register a few components. No UI consumer yet (that's Phase 3), but the infrastructure exists and is tested with unit tests.
9. **Picking pass** — feature-gated, readback works, returns an `Entity` from a screen-space click. Log the entity ID on click — no selection UI yet.

## 6. Scope boundaries — what's NOT in Phase 2

To keep this finite:

- ❌ Inspector panel (needs reflection UI generators — Phase 3)
- ❌ Hierarchy panel (Phase 3)
- ❌ Gizmos (Phase 3)
- ❌ Scene save/load (Phase 4)
- ❌ Content browser (Phase 5)
- ❌ Undo/redo (Phase 6)
- ❌ Play-in-Editor (Phase 7)

The temptation will be to slip "just the hierarchy panel" into Phase 2 because it looks easy. Don't — it needs reflection-driven component lists, which drags in half of Phase 3.

## 7. Exit criteria

Phase 2 is done when all of these are true:

- [ ] Workspace builds clean with `cargo build --workspace`.
- [ ] `cargo run -p rustforge-editor` opens a docked window.
- [ ] Viewport panel shows the engine rendering a test scene (at least a grid + one mesh + lighting).
- [ ] Viewport resizes correctly when the panel is resized or undocked.
- [ ] Fly cam works inside the viewport and is ignored outside it.
- [ ] `ComponentRegistry` exists and has at least `Transform` + one other component registered via `#[derive(Reflect)]`.
- [ ] Clicking in the viewport prints the picked entity ID to the console.
- [ ] `rustforge-core` still builds and runs *without* the `editor` feature (headless/game build).
