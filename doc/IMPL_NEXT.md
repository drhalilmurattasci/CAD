# RustForge — Implementation Phases I-2 → I-10

Detailed spec for the phases that follow I-1 (which has landed — see `IMPL_PHASES.md` §I-1 and the `viewport_3d` module on disk). Each phase here is **not yet implemented**; this file is a work-order, not a post-hoc record.

## Ground rules (restated)

1. **No fake code.** Every Rust snippet below is the exact shape of the code that should land when the phase is implemented. It references real files on disk and real crate versions.
2. **Each phase ends at a runnable milestone.** Mid-phase states are not phases.
3. **Observable deliverables.** Either a visible change in the editor, a new passing test, or a `cargo` exit code.
4. **Mock debt shrinks monotonically.** Each phase either eliminates a Mock call site or is explicitly labelled scaffolding.
5. **Scope is one commit.** If a phase exceeds a reasonable workday, split it.

## Current landing after I-1

| Subsystem | State |
|---|---|
| Workspace | `rustforge-core`, `rustforge-editor`, `rustforge-editor-ui` compile clean. |
| eframe backend | `wgpu` (switched from `glow`). |
| Viewport render | Real `egui_wgpu::CallbackTrait` pipeline fills viewport rect via WGSL shader. |
| ECS | None (`hecs` not yet a dep). |
| Camera | 2D pan/zoom state only; no perspective matrix. |
| Picking | `MockEngine` returns slot-based fake IDs. |
| Save/load | RON round-trip works for `SceneDocument` tree; no entity-level round-trip. |

All phases below build on this exact state.

---

## I-2 — First triangle with vertex buffer

**Status:** not yet implemented.

**Prereq:** I-1.

**Goal:** replace the fullscreen-fill clear shader with a colored triangle rendered from a real vertex buffer. The triangle sits centered in the viewport and colors interpolate per-vertex. Resizing the viewport does not distort the triangle's shape.

### Files

- Edit `crates/rustforge-editor-ui/src/components/viewport_clear.wgsl` — rename to `viewport_triangle.wgsl` (or add new file; old one is unused after this phase).
- Edit `crates/rustforge-editor-ui/src/components/viewport_3d.rs` — add vertex buffer creation, change pipeline vertex state.
- No Cargo changes (`bytemuck` already a workspace dep).

### Shader delta

```wgsl
struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec3<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(in.pos, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
```

### Rust delta (`viewport_3d.rs`)

Add:

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 3],
}

const TRIANGLE: [Vertex; 3] = [
    Vertex { position: [ 0.0,  0.6], color: [1.0, 0.3, 0.3] },
    Vertex { position: [-0.6, -0.5], color: [0.3, 1.0, 0.4] },
    Vertex { position: [ 0.6, -0.5], color: [0.4, 0.5, 1.0] },
];
```

`ViewportRenderer::new` gains:

```rust
let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
    label: Some("rustforge.viewport.vbo"),
    contents: bytemuck::cast_slice(&TRIANGLE),
    usage: wgpu::BufferUsages::VERTEX,
});
```

(wgpu 27 `create_buffer_init` comes from `wgpu::util`; that module ships with the default features we already pull in.)

Pipeline `vertex` state becomes:

```rust
vertex: wgpu::VertexState {
    module: &shader,
    entry_point: Some("vs_main"),
    buffers: &[wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x3],
    }],
    compilation_options: wgpu::PipelineCompilationOptions::default(),
},
```

`paint` gains:

```rust
render_pass.set_vertex_buffer(0, renderer.vertex_buffer.slice(..));
```

### Verification

- `cargo check --workspace` clean.
- `cargo test --workspace` unchanged (5/5).
- `cargo run -p rustforge-editor` — triangle visible, colors gradient-interpolated.
- Dock splitter test: dragging the viewport panel narrower does **not** distort the triangle — the triangle is NDC-aligned, so it appears to zoom with the rect, not stretch. (Confirms vertex buffer is truly fed to pipeline.)

### Risks

- `create_buffer_init` requires `wgpu::util`. If the feature isn't on, switch to `create_buffer` + `queue.write_buffer`.
- `vertex_attr_array!` macro is re-exported from `wgpu`; if using `eframe::wgpu` re-export it's still available.

### Done when

Removing one vertex from `TRIANGLE` and recompiling yields a still-rendering but degenerate draw (proves the vertex buffer is the source of truth, not the shader).

---

## I-3 — Spinning cube with depth

**Status:** not yet implemented.

**Prereq:** I-2.

**Goal:** a solid-shaded 3D cube rotating in place, with a real depth buffer and a perspective camera. Resizing the viewport keeps the aspect ratio correct.

### Files

- New: `crates/rustforge-editor-ui/src/components/viewport_cube.wgsl`.
- Edit: `crates/rustforge-editor-ui/src/components/viewport_3d.rs`.
- `Cargo.toml` workspace — add `glam`:

```toml
glam = { version = "0.29", features = ["bytemuck"] }
```

- `crates/rustforge-editor-ui/Cargo.toml` — add `glam.workspace = true`.

### Geometry

24 vertices (4 per face × 6 faces), 36 indices (6 per face × 6 faces). Each vertex carries position + face-normal color so the cube is readable without lighting.

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CubeVertex {
    position: [f32; 3],
    color: [f32; 3],
}

const CUBE_VERTICES: [CubeVertex; 24] = [ /* 6 faces × 4 verts, +X red, -X dark red, +Y green, -Y dark green, +Z blue, -Z dark blue */ ];
const CUBE_INDICES: [u16; 36] = [ /* 6 quads, 2 tris each */ ];
```

(Actual literal tables written out in the commit; omitted here for brevity — the data is mechanical.)

### Uniform buffer

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    model:     [[f32; 4]; 4],
}
```

Updated each frame via `queue.write_buffer`. Model matrix: `Mat4::from_rotation_y(t) * Mat4::from_rotation_x(t * 0.7)`.

### Depth texture

```rust
struct DepthTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

impl DepthTarget {
    fn ensure(&mut self, device: &wgpu::Device, size: [u32; 2]) { /* recreate if size differs */ }
}
```

### Integration

This is where I-1/I-2's "paint inside egui's render pass" model breaks — egui's pass has a color target but no depth attachment. Two options:

**Option A (chosen):** Move from the `paint`-style callback to a render-to-texture flow. The cube renders into a standalone color + depth target each frame; the resulting color texture is registered with `egui_wgpu::Renderer::register_native_texture` and shown in the viewport rect via `ui.painter().image(...)`. This matches Phase 2 §2.1 of the design series.

**Option B:** Stay inside the egui pass; skip depth; render cube faces in back-to-front order. Not scalable. Rejected.

### Consequences of Option A

- `ViewportCallback` becomes `ViewportRenderer::render(&mut self, device, queue, size, time) -> &wgpu::TextureView`.
- `viewport_3d.rs` exposes `fn render_to_image(ctx: &egui::Context, ui: &mut egui::Ui, rect: egui::Rect, state: &mut SharedRendererState)` which:
  1. Drives a render to `state.color_target`.
  2. Ensures the egui-registered `TextureId` is up to date for that target.
  3. Paints `ui.painter().image(id, rect, UV, WHITE)`.
- `SharedRendererState` lives on `RustForgeEditorApp` (not in egui CallbackResources) because it's stateful across frames and involves an egui_wgpu Renderer handle.
- Need `eframe::CreationContext::wgpu_render_state` at startup to grab the `Arc<Mutex<egui_wgpu::Renderer>>`.

### Verification

- `cargo check --workspace` clean.
- `cargo run -p rustforge-editor` — rotating colored cube, at least two faces visible at any time.
- Viewport resize: cube stays proportional; fast resizes don't flicker.
- Profiler panel FPS > 100 at 1280×720 on an integrated GPU. (Current FPS counter is fed from `RuntimeStats` — I-3 updates it from real frame time.)

### Risks

- Depth texture must be recreated on size change. Off-by-one errors produce either a frozen cube (size stale) or a per-frame alloc (leak).
- egui's `register_native_texture` returns a `TextureId` that must be freed on texture drop. Use `free_texture`.

### Done when

Commenting out the rotation update pins the cube in place but it still renders (proves uniform update is independent from pipeline correctness).

---

## I-4 — Adopt `hecs` ECS

**Status:** not yet implemented.

**Prereq:** I-3.

**Goal:** the cube is driven by an entity in a `hecs::World`. Removing the single spawn line makes the cube disappear. Adding a second spawn line makes a second cube appear.

### Files

- `Cargo.toml` workspace — add `hecs = "0.10"`.
- `crates/rustforge-core/Cargo.toml` — add `hecs.workspace = true`.
- New: `crates/rustforge-core/src/world.rs` — thin wrapper exposing `World` and scene-facing helpers.
- Edit: `crates/rustforge-core/src/lib.rs` — `pub mod world;`, add prelude re-exports.
- Edit: `crates/rustforge-core/src/engine.rs` — `MockEngine` embeds a `World`. Rename is deferred to I-5.
- Edit: `crates/rustforge-editor-ui/src/components/viewport_3d.rs` — query the world for `(Transform, MeshHandle)` tuples and issue one draw per entity.

### Components

In `rustforge-core/src/world.rs`:

```rust
use glam::{Quat, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MeshHandle(pub u32);

#[derive(Debug, Clone, Copy)]
pub struct MaterialHandle(pub u32);
```

Note: `glam` needs to be a `rustforge-core` dep too, not just editor-ui. Add there as well.

### World wrapper

```rust
pub struct World {
    pub inner: hecs::World,
}

impl World {
    pub fn new() -> Self { Self { inner: hecs::World::new() } }

    pub fn spawn_cube(&mut self, at: Vec3) -> hecs::Entity {
        self.inner.spawn((
            Transform { translation: at, ..Transform::IDENTITY },
            MeshHandle(0),
            MaterialHandle(0),
        ))
    }
}
```

### Render query

In `viewport_3d.rs`, replace the single hardcoded model matrix with:

```rust
for (_id, (transform, _mesh)) in world.inner.query::<(&Transform, &MeshHandle)>().iter() {
    let model = transform.matrix();
    let uniform = CameraUniform { view_proj: vp.to_cols_array_2d(), model: model.to_cols_array_2d() };
    queue.write_buffer(&renderer.uniform_buf, 0, bytemuck::bytes_of(&uniform));
    render_pass.set_pipeline(&renderer.pipeline);
    render_pass.set_vertex_buffer(0, renderer.vbo.slice(..));
    render_pass.set_index_buffer(renderer.ibo.slice(..), wgpu::IndexFormat::Uint16);
    render_pass.set_bind_group(0, &renderer.bind_group, &[]);
    render_pass.draw_indexed(0..36, 0, 0..1);
}
```

One draw per entity. Crude but correct. Instancing is a later perf phase, not in I-4 scope.

### Mock elimination

`MockEngine::tick_headless`, `render_to_texture`, `pick_entity` still exist but start delegating to the real `World`:

- `tick_headless(dt)`: stores dt, no longer a pure stub — future phase I-7 wires real camera motion in.
- `render_to_texture`: still returns an `attachment_id` counter for now, but now the counter tracks *the wgpu texture generation* (matches render-to-texture from I-3). Mock label stays until the engine owns the renderer (I-5 or later).
- `pick_entity`: still uses the slot trick. I-8 replaces it with GPU picking.

### Verification

- `cargo check --workspace` clean.
- `cargo test --workspace` — add `world::tests::spawn_and_query_cube_transform`.
- `cargo run -p rustforge-editor` — visually identical to I-3.
- Comment out the `spawn_cube` call → the cube disappears. (This is the acceptance test.)

### Risks

- Borrow-checker pain: rendering takes `&World` but the world might need to tick (mutate) on the same frame. Split: tick first, render after.
- `hecs::Entity::to_bits` returns `u64` — relevant for I-8 picking but need not be used now.

### Done when

Adding `world.spawn_cube(Vec3::new(3.0, 0.0, 0.0))` in editor app bootstrap makes a second cube appear without any other code change.

---

## I-5 — Scene file drives entities

**Status:** not yet implemented.

**Prereq:** I-4.

**Goal:** `projects/sandbox/assets/scenes/sandbox.scene.ron` populates the ECS world at startup. Adding an entity to the RON file, reloading, makes a new cube appear at the RON-specified position.

### Files

- Edit: `crates/rustforge-core/src/scene/mod.rs` — add `into_world` / `from_world` helpers.
- Edit: `crates/rustforge-core/src/scene/document.rs` — ensure the `"Transform"` `ComponentData` carries `x,y,z` as `PrimitiveValue::F64` (already does per `commands/transform.rs`).
- Edit: `crates/rustforge-editor-ui/src/app.rs` — at bootstrap, after loading `ProjectWorkspace`, call `scene.into_world(&mut world)`.

### API

```rust
impl SceneDocument {
    pub fn into_world(&self, world: &mut rustforge_core::world::World) {
        for entity in &self.root_entities {
            spawn_recursive(entity, world, None);
        }
    }
}

fn spawn_recursive(
    scene_entity: &SceneEntity,
    world: &mut World,
    parent: Option<hecs::Entity>,
) {
    let transform = extract_transform(scene_entity).unwrap_or(Transform::IDENTITY);
    let entity = world.inner.spawn((transform, MeshHandle(0), MaterialHandle(0)));
    if let Some(parent) = parent {
        world.inner.insert_one(entity, Parent(parent)).expect("just-spawned entity is valid");
    }
    for child in &scene_entity.children {
        spawn_recursive(child, world, Some(entity));
    }
}

fn extract_transform(entity: &SceneEntity) -> Option<Transform> {
    let data = entity.components.iter().find(|c| c.type_name == "Transform")?;
    let x = field_to_f32(data.fields.get("x"));
    let y = field_to_f32(data.fields.get("y"));
    let z = field_to_f32(data.fields.get("z"));
    Some(Transform { translation: Vec3::new(x, y, z), ..Transform::IDENTITY })
}
```

`Parent` is a new component; `scene/mod.rs` or `world.rs` is the right home:

```rust
#[derive(Debug, Clone, Copy)]
pub struct Parent(pub hecs::Entity);
```

(Hierarchical transform propagation is a separate future I-phase — noted, not in scope.)

### Round-trip

A `World::to_scene_document` in reverse is NOT in I-5 — that lands in I-10. I-5 is load-only.

### Verification

- New test in `scene::tests`: build a small `SceneDocument` with two entities at different positions, call `into_world`, query with `hecs::World::query::<&Transform>()`, assert 2 entities with the right positions.
- `cargo run -p rustforge-editor` with the existing `sandbox.scene.ron` shows as many cubes as there are scene entities (currently 3 by default project template).
- Hand-edit `sandbox.scene.ron` to add a fourth entity, restart, confirm fourth cube appears.

### Risks

- Existing `sandbox.scene.ron` may not have `x/y/z` on every entity. `extract_transform` falls back to `IDENTITY` — verify the file after bootstrap has valid Transforms.
- Root entity list may be empty if the project loader is in a fallback path. Check `app.rs` fallback logic.

### Done when

Commenting out `scene.into_world(&mut world)` in `app.rs` causes all cubes to disappear from the viewport.

---

## I-6 — Reflection macro & generic component serialization

**Status:** not yet implemented.

**Prereq:** I-5.

**Goal:** `#[derive(Reflect)]` on `Transform` generates field descriptors. The inspector walks those descriptors and renders editable widgets — no component-specific UI code.

### Files

- New crate: `crates/rustforge-reflect/Cargo.toml` + `src/lib.rs` — proc-macro.
- `Cargo.toml` workspace — add the crate as a member.
- Edit `crates/rustforge-core/Cargo.toml` — depend on `rustforge-reflect` as a normal crate (not dev).
- Edit `crates/rustforge-core/src/reflection.rs` — define `Reflect`, `FieldDescriptor`, `ValueKind` with at least `F32`, `Vec3`, `Bool`, `String`.
- Edit `crates/rustforge-core/src/world.rs` — `impl Reflect for Transform` via `#[derive(Reflect)]`.
- Edit `crates/rustforge-editor-ui/src/components/inspector.rs` — walk fields via registry.

### Reflect trait (real signature)

Already sketched in `reflection.rs`. I-6 makes it work:

```rust
pub trait Reflect: 'static {
    fn type_name() -> &'static str where Self: Sized;
    fn fields() -> &'static [FieldDescriptor] where Self: Sized;
    fn get_field(&self, index: usize) -> Option<FieldRef<'_>>;
    fn set_field(&mut self, index: usize, value: FieldRef<'_>) -> Result<(), ReflectError>;
}

#[derive(Debug, Clone, Copy)]
pub struct FieldDescriptor {
    pub name: &'static str,
    pub kind: ValueKind,
    pub offset: usize,
}

pub enum FieldRef<'a> {
    F32(&'a mut f32),
    Bool(&'a mut bool),
    Vec3(&'a mut glam::Vec3),
    // ...
}
```

### Macro expansion

`#[derive(Reflect)]` on a plain struct generates:

```rust
impl Reflect for Transform {
    fn type_name() -> &'static str { "Transform" }
    fn fields() -> &'static [FieldDescriptor] {
        &[
            FieldDescriptor { name: "translation", kind: ValueKind::Vec3, offset: 0 },
            FieldDescriptor { name: "rotation",    kind: ValueKind::Quat, offset: 16 },
            FieldDescriptor { name: "scale",       kind: ValueKind::Vec3, offset: 32 },
        ]
    }
    // get_field / set_field by pointer arithmetic with offset
}
```

(Offsets produced by `memoffset::offset_of!` or `std::mem::offset_of!` — the latter is stable since Rust 1.77.)

### Inspector rewrite

`inspector::render` currently special-cases Transform via string keys. Replace with:

```rust
for component_data in &entity.components {
    if let Some(desc) = registry.get(&component_data.type_name) {
        ui.collapsing(desc.label, |ui| {
            for field in desc.fields {
                render_field(ui, field, component_data);
            }
        });
    }
}
```

`render_field` dispatches by `ValueKind` to the right egui widget (`ui.add(egui::DragValue::new(&mut f))` for floats, etc.). Edits produce `WorkspaceAction::SetComponentField` commands, which already route through the command stack.

### Verification

- `cargo test -p rustforge-core` — new test: derive Reflect on a test struct, assert fields returns expected names.
- `cargo run -p rustforge-editor` — click an entity in hierarchy; inspector shows `translation`, `rotation`, `scale` with live-editable drag values. Editing `x` moves the cube in the viewport in real time.
- Ctrl+Z reverts the edit.

### Risks

- Proc-macro crate requires `proc-macro = true` in Cargo.toml. Easy to miss.
- `std::mem::offset_of!` requires the field type to be known at compile time and the struct to be `#[repr(C)]` or layout-stable. For `Transform { Vec3, Quat, Vec3 }` this is fine but watch generics.
- `Reflect::fields()` must return `&'static`. Use `const fn` construction if possible, else a `OnceLock`.

### Done when

Adding a new `#[derive(Reflect)] struct Spin { angular_velocity: Vec3 }` and registering it makes the field appear in the inspector with zero additional UI code.

---

## I-7 — Orbit camera with viewport input

**Status:** not yet implemented.

**Prereq:** I-3 (camera uniform already exists). Cleaner if I-4 has landed so the camera is ECS-compatible, but strictly I-7 depends only on I-3.

**Goal:** left-drag orbits the editor camera around the focus point; middle-drag pans; scroll zooms; framerate unchanged from I-3.

### Files

- New: `crates/rustforge-core/src/camera.rs`.
- Edit: `crates/rustforge-editor-ui/src/shell.rs` — replace the existing 2D `ViewportCameraState` (pan_x/pan_y/zoom/orbit_yaw/orbit_pitch) with a 3D `EditorCamera`. The existing struct is already a near-match — upgrade types.
- Edit: `crates/rustforge-editor-ui/src/components/viewport.rs` — feed mouse input to the camera, pass the computed view-proj matrix to the renderer.
- Edit: `crates/rustforge-editor-ui/src/components/viewport_3d.rs` — uniform gets its `view_proj` from `EditorCamera::view_proj(aspect)` instead of a hardcoded perspective.

### Camera

```rust
use glam::{Mat4, Quat, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct EditorCamera {
    pub target: Vec3,
    pub yaw_rad: f32,
    pub pitch_rad: f32,
    pub distance: f32,
    pub fov_y_rad: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw_rad: 0.8,
            pitch_rad: -0.5,
            distance: 8.0,
            fov_y_rad: 45f32.to_radians(),
            near: 0.1,
            far: 1000.0,
        }
    }
}

impl EditorCamera {
    pub fn position(&self) -> Vec3 {
        let (sy, cy) = self.yaw_rad.sin_cos();
        let (sp, cp) = self.pitch_rad.sin_cos();
        self.target + Vec3::new(cy * cp, sp, sy * cp) * self.distance
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.position(), self.target, Vec3::Y)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y_rad, aspect, self.near, self.far)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw_rad += dx * 0.005;
        self.pitch_rad = (self.pitch_rad + dy * 0.005).clamp(-1.55, 1.55);
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        let right = self.view().transpose().x_axis.truncate();
        let up = self.view().transpose().y_axis.truncate();
        self.target += right * -dx * 0.01 * self.distance + up * dy * 0.01 * self.distance;
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance * (1.0 - delta * 0.001)).clamp(0.5, 500.0);
    }
}
```

### Input wiring

In `components/viewport.rs`, after the rect is allocated:

```rust
if response.hovered() {
    let scroll = ui.input(|i| i.raw_scroll_delta.y);
    if scroll.abs() > f32::EPSILON {
        camera.zoom(scroll);
    }
}
if response.dragged_by(egui::PointerButton::Primary) {
    let d = response.drag_delta();
    camera.orbit(d.x, d.y);
}
if response.dragged_by(egui::PointerButton::Middle) {
    let d = response.drag_delta();
    camera.pan(d.x, d.y);
}
```

Existing input code that mutated the old `ViewportCameraState` is deleted.

### Uniform

`viewport_3d.rs`'s `CameraUniform` gets `view_proj` from `camera.view_proj(aspect)` where `aspect = rect.width() / rect.height()`. This replaces the dummy identity / I-3 placeholder.

### Verification

- `cargo test -p rustforge-core` — new camera test: yaw 90° CCW moves position along X for a zero-target default camera.
- `cargo run -p rustforge-editor` — drag-orbit, middle-drag-pan, scroll-zoom. Camera state persists across viewport panel focus changes.

### Risks

- Sign conventions: egui `drag_delta.y` is screen-down; camera pitch is world-up. Negate or not — decide and stick.
- Aspect ratio plumbing: viewport panel size vs. renderer target size must agree. Pass explicit.

### Done when

Dragging the camera to look at the back face of the cube reveals the darker "back" color from I-3's face-color scheme (proves the projection is 3D, not 2D).

---

## I-8 — GPU entity-ID picking

**Status:** not yet implemented.

**Prereq:** I-4 (entities exist) + I-5 (scene-driven population).

**Goal:** clicking a cube in the viewport selects it in hierarchy + inspector. Clicking background deselects. Shift-click adds to selection; Ctrl-click toggles.

### Files

- New: `crates/rustforge-editor-ui/src/components/viewport_pick.wgsl`.
- Edit: `viewport_3d.rs` — add an auxiliary `R32Uint` color target + second render pipeline writing entity ID per fragment.
- New: `crates/rustforge-editor-ui/src/components/viewport_pick.rs` — pixel readback coordination.
- Edit: `crates/rustforge-editor-ui/src/components/viewport.rs` — on click, request pick at `(local_x, local_y)`.
- Edit: `crates/rustforge-core/src/engine.rs` — `MockEngine::pick_entity` becomes `Engine::pick_entity` using the real readback.

### Pick pipeline

Same vertex layout as color pipeline, but a different fragment shader:

```wgsl
struct EntityUniform {
    model:       mat4x4<f32>,
    view_proj:   mat4x4<f32>,
    entity_id:   u32,
};

@group(0) @binding(0) var<uniform> u: EntityUniform;

struct VsIn { @location(0) pos: vec3<f32> };
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) id: u32 };

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.pos = u.view_proj * u.model * vec4<f32>(in.pos, 1.0);
    out.id = u.entity_id;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) u32 {
    return in.id;
}
```

Color target format `R32Uint`. Same depth buffer as the color pass (shared).

### Render flow per frame

1. Clear color target to (clear color), depth to 1.0, id target to 0.
2. For each entity: write uniform with `id = hecs_entity.to_bits() as u32` (low 32 bits — adequate for editor-scale worlds, document the upper-bound). Draw into color target using I-3/I-4 pipeline AND into id target using pick pipeline.
3. (Optimization: single render pass with two color attachments. Start with two passes — correctness over perf.)

### Readback

On click:

```rust
pub struct PickRequest { x: u32, y: u32 }

fn pick(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, req: PickRequest) -> Option<hecs::Entity> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustforge.pick.readback"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pick") });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture { texture: &self.id_target.texture, mip_level: 0, origin: wgpu::Origin3d { x: req.x, y: req.y, z: 0 }, aspect: wgpu::TextureAspect::All },
        wgpu::ImageCopyBuffer { buffer: &buffer, layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) } },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().ok()?.ok()?;
    let data = slice.get_mapped_range();
    let id = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
    if id == 0 { None } else { Some(hecs::Entity::from_bits(id as u64).ok()?) }
}
```

Synchronous stall is fine for click-to-pick at <100 ms. If perf matters later, switch to fire-and-forget with frame-latency.

### Selection integration

`SelectionSet` becomes real (or keep the existing `shell::selected_entity` single-entity path and upgrade to `Vec<hecs::Entity>` in I-8). The minimum I-8 deliverable uses single-entity selection.

### Verification

- New test: render a single entity, read back the pixel at its screen-center, assert the decoded `Entity` equals the spawned one.
- `cargo run -p rustforge-editor` — click a cube, hierarchy highlights the entity, inspector shows its Transform. Click background, selection clears.

### Risks

- `hecs::Entity::to_bits()` is `u64`; we truncate to `u32`. With > 4 billion entity generations the pick breaks. Document and plan for later widening to `RG32Uint` (two channels for `u64`).
- Entity `0` is a valid hecs bit pattern, so we cannot use 0 as "no entity". Shift all IDs by 1 or reserve. (Actually hecs never returns an `Entity` with bits 0 in practice — `DanglingId` is `u64::MAX`. Document the chosen convention.)
- `wgpu::Maintain::Wait` blocks the main thread until submission completes. Acceptable for click. For per-frame usage, use `Poll`.

### Done when

Click picking resolves the correct entity for each of three spawned cubes at different positions, verified visually + via hierarchy.

---

## I-9 — Translation gizmo

**Status:** not yet implemented.

**Prereq:** I-7 (camera), I-8 (selection).

**Goal:** drag a gizmo handle in the viewport to translate the selected entity. One undo entry per drag regardless of drag duration.

### Files

- New: `crates/rustforge-editor-ui/src/components/gizmo.rs`.
- New: `crates/rustforge-editor-ui/src/components/gizmo.wgsl`.
- Edit: `components/viewport_3d.rs` — render the gizmo as a third pass after main scene.
- Edit: `components/viewport.rs` — handle drag events on gizmo.
- Edit: `crates/rustforge-core/src/commands/transform.rs` — `NudgeTransformCommand` already exists; make sure it captures `before`/`after` and coalesces on identical entity within a transaction.

### Gizmo geometry

Three axis-aligned arrows (X red, Y green, Z blue), length scaled by distance from camera (so gizmo stays a readable size on screen). Arrows are per-axis line segments + cone tips.

### Handle picking

Reuse I-8's `R32Uint` target. Gizmo handles take IDs `0xFFFF_FFF0`, `0xFFFF_FFF1`, `0xFFFF_FFF2` — reserved range outside the entity-ID range. Add a decoder:

```rust
enum PickTarget { None, Entity(hecs::Entity), GizmoAxis(Axis) }

fn decode_pick(id: u32) -> PickTarget {
    match id {
        0 => PickTarget::None,
        0xFFFF_FFF0 => PickTarget::GizmoAxis(Axis::X),
        0xFFFF_FFF1 => PickTarget::GizmoAxis(Axis::Y),
        0xFFFF_FFF2 => PickTarget::GizmoAxis(Axis::Z),
        bits => hecs::Entity::from_bits(bits as u64).map(PickTarget::Entity).unwrap_or(PickTarget::None),
    }
}
```

### Drag

On drag-start over a gizmo axis:

1. `command_stack.begin_transaction("Move")`.
2. Snapshot entity's current `Transform.translation` as `before`.
3. Remember the screen-space drag start and the axis' world vector.
4. On drag-delta: project the screen delta onto the axis in world space, mutate the entity's `Transform.translation` directly.
5. On drag-end: read current translation as `after`; push `NudgeTransformCommand { entity, before, after }`; `command_stack.end_transaction()`.

`NudgeTransformCommand` already exists in `crates/rustforge-core/src/commands/transform.rs`. Make sure its `execute` sets to `after` and its `undo` restores `before`.

### Screen-to-axis projection

Classic approach:

```rust
fn delta_along_axis(
    axis_world: Vec3,
    entity_pos_world: Vec3,
    camera: &EditorCamera,
    viewport_size: Vec2,
    mouse_delta_screen: Vec2,
) -> f32 {
    let vp = camera.view_proj(viewport_size.x / viewport_size.y);
    let p0 = project(vp, entity_pos_world, viewport_size);
    let p1 = project(vp, entity_pos_world + axis_world, viewport_size);
    let axis_screen = (p1 - p0).normalize_or_zero();
    let world_per_screen = 1.0 / (p1 - p0).length();
    mouse_delta_screen.dot(axis_screen) * world_per_screen
}
```

Not the cleanest projection math but good enough for I-9. Future phases can add "screen-space distance to axis ray" for hit detection refinement.

### Verification

- Drag X-axis handle → cube moves only in X.
- Release drag → Ctrl+Z reverts to pre-drag position.
- 60-frame drag produces exactly one command entry in the stack (check `CommandStack::undo_stack.len()` via a debug hotkey or test harness).

### Risks

- Integer vs float precision in screen projection — small for viewports up to 4K.
- Gizmo Z-fighting vs cubes — render gizmo with `depth_write_enabled: false` and a later pass, or with a distinct depth range.
- Clicking an axis should NOT deselect the entity. Handle order: gizmo first, entity pick only if no handle hit.

### Done when

Dragging the gizmo for 1 second produces exactly 1 undo entry, not 60.

---

## I-10 — Scene save/load round-trip

**Status:** not yet implemented.

**Prereq:** I-6 (reflection), I-5 (load).

**Goal:** `Ctrl+S` serializes the live hecs world to the project's `scene.ron`. Close the editor, relaunch — the world restores identically. Round-trip is byte-clean.

### Files

- Edit: `crates/rustforge-core/src/world.rs` — `World::to_scene_document()`.
- Edit: `crates/rustforge-core/src/scene/mod.rs` — ensure `SceneDocument::write_ron(&self, path)` exists; already partially in `crates/rustforge-editor-ui/src/project.rs::save_scene`.
- Edit: `crates/rustforge-editor-ui/src/components/menu_bar.rs` — `File → Save` wiring to the action handler.
- New property test: `crates/rustforge-core/tests/scene_roundtrip.rs`.

### World → SceneDocument

Walks all entities in the world, extracts reflected component data for each registered type, builds `SceneEntity` trees according to `Parent` relationships.

```rust
impl World {
    pub fn to_scene_document(&self, registry: &ComponentRegistry, name: &str) -> SceneDocument {
        let mut roots = Vec::new();
        let mut children_by_parent: HashMap<hecs::Entity, Vec<hecs::Entity>> = HashMap::new();
        for (entity, parent) in self.inner.query::<&Parent>().iter() {
            children_by_parent.entry(parent.0).or_default().push(entity);
        }

        for (entity, _) in self.inner.iter().filter(|(e, _)| self.inner.get::<&Parent>(*e).is_err()) {
            roots.push(entity_to_scene(entity, &self.inner, registry, &children_by_parent));
        }

        SceneDocument { name: name.into(), root_entities: roots, metadata: Default::default() }
    }
}
```

Entity-to-SceneEntity serializes each registered component via `Reflect::get_field` → `PrimitiveValue`. Only reflected types round-trip cleanly; non-reflected components are dropped with a logged warning (documented limitation — matches Phase 7 §12 of the design series).

### Property test

```rust
#[test]
fn world_to_scene_to_world_roundtrip() {
    let mut registry = ComponentRegistry::default();
    registry.register::<Transform>();

    let mut original = World::new();
    let e1 = original.inner.spawn((Transform { translation: Vec3::new(1.0, 2.0, 3.0), ..Transform::IDENTITY },));
    let e2 = original.inner.spawn((Transform { translation: Vec3::new(-5.0, 0.5, 7.25), ..Transform::IDENTITY },));

    let doc = original.to_scene_document(&registry, "test");

    let mut restored = World::new();
    doc.into_world(&mut restored);

    let original_positions: Vec<Vec3> = original.inner.query::<&Transform>().iter().map(|(_, t)| t.translation).collect();
    let restored_positions: Vec<Vec3> = restored.inner.query::<&Transform>().iter().map(|(_, t)| t.translation).collect();
    assert_eq!(original_positions.len(), restored_positions.len());
    for pos in original_positions { assert!(restored_positions.contains(&pos)); }
}
```

Randomized version: 50 entities with random Transforms, save, clear, load, assert multi-set equality.

### Save menu wiring

Ctrl+S already exists in the design but not in the current `menu_bar.rs`. Add:

```rust
if ui.button("Save Scene").clicked() || ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S)) {
    actions.push(WorkspaceAction::SaveScene);
}
```

`WorkspaceAction::SaveScene` handler calls `world.to_scene_document(...)` then `project.save_scene(&doc)`.

### Verification

- Property test passes.
- `cargo run -p rustforge-editor`, move a cube via I-9 gizmo, Ctrl+S, close, relaunch — cube is at the moved position.
- `git diff projects/sandbox/assets/scenes/sandbox.scene.ron` shows only the translation delta, no other noise.

### Risks

- Round-trip must be stable on entity *order*. `hecs` iteration order is not guaranteed. Sort by `SceneId` in the doc if order matters — already the case since `SceneDocument` stores a tree.
- Non-reflected components lost silently; surface a toast on save "N components not persisted" if any.
- `SceneId` collision: I-5 populates the world from the scene doc, spawning new hecs entities. On save we allocate fresh `SceneId` values. Persistent ID round-trip requires a `SceneIdComponent` component that travels with each entity — add in I-10 if not present.

### Done when

The round-trip property test is green AND a manual move + save + relaunch shows the moved cube at the new position, with the RON diff showing only that change.

---

## After I-10

The editor has:
- Real wgpu 3D viewport driven by a real ECS
- Real reflection-driven inspector
- Real selection, translation gizmo, undo/redo
- Real scene save/load round-trip

This is the **minimum viable editor**. Every design phase from Phase 4 onward (prefabs, content browser, PIE, specialized editors, etc.) becomes implementable as incremental I-11, I-12, … phases on this foundation.

The next arc (I-11+) should be scoped after I-10 lands, based on what the real code reveals. The current I-11+ candidates from the design series:

| I-phase candidate | Design source | Depends on |
|---|---|---|
| Multi-select + marquee | phase3 §1, phase8 §3 | I-8 |
| Prefab instance (spawn + override) | phase4 §3 | I-5, I-10 |
| Content browser with thumbnails | phase5 | I-5 |
| Play/Pause/Stop state machine | phase7 §3 | I-10 (needs snapshot) |
| Profiler frame graph | phase8 §6 | I-3 (GPU timestamps) |

These are not specced here — they're named so the reader knows the runway is real, not a cliff.

## Relationship to `IMPL_PHASES.md`

`IMPL_PHASES.md` is the top-level roadmap with I-0..I-10 one-pagers. This file is the detailed work-order for I-2..I-10 only. I-1 is already landed; its record is in `IMPL_PHASES.md` and in the commit that introduced `viewport_3d.rs`.

When a phase here lands:
1. Move its section from "not yet implemented" to "landed" in the header status.
2. Update `IMPL_PHASES.md`'s "current state" table.
3. Commit with a message referencing the phase number.

## Anti-goals (reaffirmed)

- Writing phase docs that can't be cited back to real source files.
- Jumping ahead to fancy features (path tracing, PCG, ML) before I-10 is solid.
- Adding Mock paths to unblock a phase. If a phase needs a thing, build the thing.
- Committing code that doesn't `cargo check --workspace` clean on Windows, Linux, macOS.
