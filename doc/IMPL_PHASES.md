# RustForge — Implementation Phase Series

A separate, grounded track from the 50-phase design series (`phase1.md`–`phase50.md`). The design series answers *what should exist*; this series answers *what actually ships, in what order, verifiable by `cargo run` and by looking at the screen*.

## Ground rules

1. **No fake code.** Every Rust snippet in an implementation phase doc corresponds to a commit that compiles and runs. No illustrative sketches.
2. **Each phase ends with a runnable milestone.** Phase N-1 must run; phase N replaces or augments something and still runs. A broken mid-phase state is not a phase.
3. **Deliverables are observable.** Either a visible change in the editor, a passing test, or a `cargo run` exit code. "The design is clearer" is not a deliverable.
4. **MockEngine is a debt marker.** Every phase that eliminates a Mock call is progress; phases that add new Mock calls are a regression unless explicitly labeled as scaffolding.
5. **Scope is one commit, not one quarter.** If a phase is larger than a reasonable single-person workday, split it.

## Current state (I-0 — what actually works today)

- Workspace: `rustforge-core` + `rustforge-editor` + `rustforge-editor-ui`, compiles clean with `cargo check --workspace`.
- Editor opens: `cargo run -p rustforge-editor` shows a dockable `eframe`/`glow` window with menu, toolbar, hierarchy, inspector, viewport, content browser, console, profiler panels.
- Scene format: `.ron` via `SceneDocument`; load/save works.
- Commands: `CommandStack` with `NudgeTransform`, `RenameEntity`, `SetComponentField`, `SpawnEntity` and tests.
- Reflection: `ComponentRegistry` + `Reflect` trait stub; no derive macro, no dynamic field walking.
- **Rendering: none.** `MockEngine::render_to_texture` returns an incrementing `u64` counter. Viewport panel is pure egui widgets.
- **ECS: none.** `hecs` is not a workspace dep; entities live only in the scene document tree.

Everything from I-1 onward is work. Milestones below are ordered; each depends on the previous.

---

## I-1 — Real wgpu in the viewport

**Goal:** a solid clear color inside the viewport rect, produced by an actual wgpu render pass, not an egui `ui.painter().rect_filled()`.

**Changes:**
- `Cargo.toml`: swap eframe features from `glow` to `wgpu`.
- Add workspace deps: `wgpu = "22"`, `bytemuck = "1"`, `glam = "0.29"`.
- `rustforge-editor-ui/src/components/viewport.rs`: add an `egui::PaintCallback` wrapping a `CallbackResource` that owns a wgpu render pipeline; clear color inside the callback.
- `rustforge-core/src/engine.rs`: rename `MockEngine` → `Engine` (the real one, even if small). Move render-to-texture concept aside; for I-1 we draw directly into the egui paint callback, not offscreen.

**Verification:** `cargo run -p rustforge-editor` → the viewport rectangle is the wgpu clear color (pick something distinctive like `(0.1, 0.15, 0.2, 1.0)`), provably not egui, by toggling the callback on/off.

**Done when:** you can drag the dock splitter and the clear-color rect resizes with it.

---

## I-2 — First triangle

**Goal:** replace clear color with a single colored triangle rendered by a WGSL shader.

**Changes:**
- `assets/shaders/triangle.wgsl` (new): minimal `@vertex` + `@fragment` with hardcoded three-vertex `vertex_index` switch.
- Render pipeline created once on callback prepare.
- No vertex buffer yet — use `@builtin(vertex_index)`.

**Verification:** triangle appears in the viewport, colors smoothly gradient-interpolated at vertices.

**Done when:** resizing the viewport doesn't stretch the triangle (viewport uniform updated per frame).

---

## I-3 — Spinning cube with depth

**Goal:** a rotating 3D cube with a depth buffer and a perspective camera.

**Changes:**
- `assets/shaders/cube.wgsl`: vertex + fragment with MVP uniform, per-vertex color.
- Vertex buffer (24 verts, 36 indices for 6 faces).
- Depth texture attached to callback; recreated on resize.
- `glam::Mat4::perspective_rh` for projection; rotating model matrix from `Instant::elapsed`.
- Uniform buffer for camera + model.

**Verification:** rotating shaded cube in the viewport. Resizing preserves aspect. No z-fighting.

**Done when:** FPS shown in profiler panel is > 100 at 1280×720 on an integrated GPU.

---

## I-4 — Adopt `hecs` ECS; cube becomes an entity

**Goal:** the rendered cube is driven by an entity in a `hecs::World`, not hardcoded.

**Changes:**
- `Cargo.toml`: `hecs = "0.10"`.
- `rustforge-core/src/world.rs` (new): `struct World { hecs: hecs::World }`; wrappers.
- Components (real): `Transform { translation: Vec3, rotation: Quat, scale: Vec3 }`, `MeshHandle(u32)`, `MaterialHandle(u32)`.
- At startup, spawn one entity with `Transform::IDENTITY`, `MeshHandle(0)` = built-in cube, `MaterialHandle(0)` = default.
- Render function queries `(Transform, MeshHandle, MaterialHandle)` and draws each.
- Scene-document → world conversion is still stubbed; we will wire it in I-5.

**Verification:** visually identical to I-3, but `Engine::render` iterates a hecs query. Unit test: spawn 3 entities, render-query returns 3.

**Done when:** removing the entity spawn line makes the cube disappear (proving ECS is the source of truth).

---

## I-5 — Scene file drives entities

**Goal:** the existing `projects/sandbox` scene RON file populates the ECS world at startup; adding an entity to the RON file makes it appear on next launch.

**Changes:**
- `rustforge-core/src/scene/mod.rs`: add `SceneDocument::into_world(&self, &mut World)` and `World::to_scene_document(&self) -> SceneDocument`.
- Handle `Transform` component round-trip via serde. Other components preserved as opaque `ComponentData` for now (documented as Mock until reflection lands in I-6).
- Editor startup loads `projects/sandbox/scene.ron` (already exists) and spawns entities.

**Verification:** 3 entities defined in RON → 3 cubes in the viewport at the RON-specified positions. Close editor, edit RON, reopen — scene reflects edit.

**Done when:** Git-diffing the RON between sessions after moving an entity in memory (via command stack, still goes through scene doc) shows a clean positional delta.

---

## I-6 — Reflection macro & generic component serialization

**Goal:** the reflection registry actually enumerates fields; `Transform` is `#[derive(Reflect)]`, inspector reads it dynamically.

**Changes:**
- New `rustforge-reflect` proc-macro crate.
- `#[derive(Reflect)]` generates `fields() -> &[FieldDescriptor]`.
- `Transform` opts in.
- `Inspector` panel walks the registered component list and renders widgets per `ValueKind`.
- Inspector edits route through `SetComponentFieldCommand` (already exists).

**Verification:** editing `Transform.translation.x` in the inspector moves the cube in real time; Ctrl+Z reverts.

**Done when:** adding a new `#[derive(Reflect)] struct Spin { angular_velocity: Vec3 }` and registering it makes it appear in the inspector with zero additional UI code.

---

## I-7 — Orbit camera, viewport input

**Goal:** mouse drag orbits the editor camera; scroll zooms; WASD flies.

**Changes:**
- `rustforge-core/src/camera.rs`: `EditorCamera { yaw, pitch, distance, target }`.
- Viewport callback reads `ui.input()` for drag deltas inside the viewport rect.
- Camera state lives on editor app, pushed to uniform each frame.

**Verification:** drag-orbit feels snappy; Shift-drag pans; scroll zooms. Framerate unchanged.

**Done when:** camera state persists to prefs and restores on relaunch.

---

## I-8 — GPU entity-ID picking

**Goal:** click an entity in the viewport → selected in hierarchy + inspector.

**Changes:**
- Second render target `R32Uint` drawn alongside color during viewport render.
- Each entity's `hecs::Entity.to_bits()` written to fragments.
- On click, wgpu `CommandEncoder::copy_texture_to_buffer` the single pixel under the cursor; map, read, resolve to `Entity`.
- `SelectionSet` (real type, not mock) receives picked ID.

**Verification:** click a cube → it's highlighted in hierarchy, inspector shows its Transform.

**Done when:** clicking background deselects; Shift-click adds to selection; Ctrl-click toggles.

---

## I-9 — Translation gizmo

**Goal:** drag a manipulator handle in the viewport to translate the selected entity; one undo entry per drag.

**Changes:**
- Immediate-mode gizmo lines + handle hitboxes (GPU picking reused for handle IDs in a distinct ID range).
- Drag start → `CommandStack::begin_transaction("Move")`.
- Drag updates world directly (transactional).
- Drag end → `NudgeTransformCommand { entity, before, after }` pushed; `end_transaction()`.

**Verification:** drag gizmo → cube moves → release → Ctrl+Z reverts cleanly; dragging a 1-second arc produces exactly one undo entry (not 60).

**Done when:** visually identical to the design in `doc/phase3.md` §3 at manipulation feel.

---

## I-10 — Scene save/load round-trip

**Goal:** `Ctrl+S` serializes the live hecs world to RON; reload produces identical world.

**Changes:**
- `File → Save` wiring.
- `World::to_scene_document` implemented for all registered components (via reflection).
- Property test: spawn N random entities with random Transforms, save, clear world, load, assert equality.

**Verification:** the property test passes; manually moving a cube + save + restart shows the cube at the new position.

**Done when:** the sandbox scene RON, round-tripped through save/load, is byte-identical (or diff is only whitespace).

---

## After I-10 — where we are

At I-10 the editor has:
- Real wgpu 3D viewport
- Real hecs ECS
- Real reflection-driven inspector
- Real selection + gizmo + undo/redo
- Real scene save/load

That's enough to call it a *minimum viable editor*. Everything in the design series from Phase 4 onward (prefabs, content browser, undo polish, PIE, specialized editors, etc.) becomes implementable as incremental I-11, I-12, … phases on top of this foundation.

## Relationship to the 50-phase design series

The design series is the **specification**; this series is the **build log**.

| Design phase | Implementation phases |
|---|---|
| P1 workspace plan | I-0 current state |
| P2 foundation | I-1 (wgpu), I-2 (shader), I-3 (cube), I-4 (ECS), I-6 (reflection), I-8 (picking) |
| P3 hierarchy/inspector/gizmos | I-6 (inspector), I-7 (camera), I-8 (selection), I-9 (gizmo) |
| P4 scene I/O | I-5, I-10 |
| P5 content browser | future — I-11+ |
| P6 undo/redo | partially real today; completed at I-9 |
| P7 PIE | future — I-12+ requires tick loop separation |
| P8–P50 | each becomes its own I-series arc |

The honest gap: 50 design phases vs 10 implementation phases to reach *minimum viable editor*. Everything past I-10 is its own multi-phase effort that should be broken down the same way — small, runnable, verifiable increments.

## Anti-goals (things this series will not do)

- Write more design docs before running code.
- Add Mock paths to "unblock" a phase. If a phase needs a thing, build the thing.
- Jump ahead to fancy features (path tracing, PCG, ML) before I-10 is solid.
- Commit code that doesn't `cargo build --workspace` clean on Windows, Linux, macOS.
- Write Rust snippets in this doc that aren't cited from real source files.

## Verification discipline per phase

Every phase landing should include:

1. **Code diff** visible in Git.
2. **`cargo check --workspace`** clean.
3. **`cargo test --workspace`** clean (with at least one new test per phase where applicable).
4. **Screenshot or CLI-output evidence** of the observable change pasted into the PR / commit message.
5. **Updated MockEngine inventory**: the list of remaining Mock call sites shrinks or the phase is scaffolding-labeled.

## Why this series is capped at 10 phases (for now)

Beyond I-10 is planning without hands-on feedback. The real engine at I-10 will tell us which design decisions survive contact with reality and which don't. At that point the design series (phases 1–50) gets revisited against the real code, and the I-11+ roadmap is written *after* the evidence is in.

Design predicts. Implementation discovers. Don't pretend the map is the territory.
