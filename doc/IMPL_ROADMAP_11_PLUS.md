# RustForge — Implementation Roadmap I-11 through I-30

Continuation of the implementation arc. Picks up where `IMPL_NEXT.md` leaves off (post-I-10 minimum viable editor) and runs to I-30, the point where the engine can ship a simple standalone game.

## Depth policy

Phases are specced at three tiers of rigor based on distance from present:

- **Arc 2 (I-11 → I-20) — Complete Editor.** Near-term. Each phase gets concrete code paths, real file locations, dependency versions. ~100-150 lines per phase. Work-order quality.
- **Arc 3 (I-21 → I-30) — Runtime Foundations.** Farther out. Each phase gets architecture sketch + critical decisions + named crates. ~50-80 lines per phase. Direction, not implementation.
- **Beyond I-30.** Not planned here. Re-assess after I-30 reality check.

The deeper the phase, the less useful precise code is — real I-3 discoveries (wgpu 27 backend features panic, gpu-allocator transient build issue) would have been fake if pre-specified. Write detail where it holds value, not everywhere.

## Ground rules (unchanged from IMPL_PHASES.md)

1. No fake code. Every snippet must compile given stated prereqs.
2. Each phase ends at a runnable milestone.
3. Observable deliverables only.
4. Mock debt shrinks monotonically.
5. One-commit scope per phase.

## Entry state (post-I-10)

| Subsystem | State after I-10 |
|---|---|
| Viewport | wgpu 3D, perspective camera, depth buffer |
| ECS | `hecs::World`, Transform/MeshHandle/MaterialHandle components |
| Scene | RON load AND save, round-trip tested |
| Inspector | Reflection-driven, edits undo/redo |
| Selection | Click-to-pick via GPU entity-ID target |
| Gizmo | Translate only, one undo per drag |
| Assets | Hardcoded cube mesh; no importer pipeline |
| Play mode | None |
| Scripts | None |
| Physics | None |
| Audio | None |

---

# Arc 2 — Complete Editor (I-11 → I-20)

## I-11 — Multi-select & marquee

**Goal:** Shift-click adds to selection; Ctrl-click toggles; middle-drag in empty viewport space draws a rectangle that selects all entities whose screen-space bounds intersect it.

**Files**
- Edit `crates/rustforge-editor-ui/src/shell.rs`: replace `selected_entity: Option<SceneId>` with `selected: SmallVec<[SceneId; 4]>` + `primary: Option<SceneId>`.
- New `crates/rustforge-editor-ui/src/components/marquee.rs` — screen-space rect drag state + hit test.
- Edit `components/viewport.rs` — route Shift/Ctrl modifiers at pick time; draw marquee rect via `ui.painter().rect_stroke`.
- Edit `components/hierarchy.rs` — reflect multi-selection (multi-highlight + Shift-click range select).

**Code sketch**
```rust
pub struct Selection {
    pub entities: SmallVec<[hecs::Entity; 4]>,
    pub primary: Option<hecs::Entity>,
}

impl Selection {
    pub fn click(&mut self, e: hecs::Entity, modifiers: egui::Modifiers) {
        if modifiers.ctrl {
            if let Some(pos) = self.entities.iter().position(|x| *x == e) {
                self.entities.remove(pos);
            } else {
                self.entities.push(e);
            }
        } else if modifiers.shift {
            if !self.entities.contains(&e) { self.entities.push(e); }
        } else {
            self.entities.clear();
            self.entities.push(e);
        }
        self.primary = Some(e);
    }
}
```

**Deps:** `smallvec = "1"` (workspace).

**Verification**
- Shift-click 3 entities → all 3 selected, hierarchy shows 3 highlighted rows.
- Middle-drag a rectangle over 2 entities → those 2 selected.
- Ctrl+A selects all; Esc clears.
- Inspector shows primary entity; "3 entities selected" hint visible.

**Risks**
- Gizmo now needs to handle multi-entity translation (drag moves all, each keeps relative offset). Either land in I-11 or explicitly defer to I-12.
- Marquee hit test needs screen-space AABB per entity — use the existing `view_proj` × bounding sphere center.

**Done when:** command stack's "Move" transaction after a multi-entity drag contains one composite undo entry that reverts all moves.

---

## I-12 — Rotate & Scale gizmos

**Goal:** press E to enable rotate gizmo, R for scale, W returns to translate (I-9). Gizmo pass uses reserved ID range from I-8.

**Files**
- Edit `components/gizmo.rs` (introduced in I-9): add `GizmoMode { Translate, Rotate, Scale }`.
- New `components/gizmo_rotate.wgsl` — three arcs (one per axis).
- New `components/gizmo_scale.wgsl` — three axis-cubes.
- Edit key handling in `components/viewport.rs` and `components/menu_bar.rs`.

**Code sketch**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoMode { Translate, Rotate, Scale }

pub struct GizmoState {
    pub mode: GizmoMode,
    pub space: GizmoSpace, // Local | World toggle (Ctrl+Q)
}
```

**Rotate math:** drag delta → angle around axis; apply `Quat::from_axis_angle(axis, delta)` to entity's rotation.

**Scale math:** drag along axis → scale multiplier; apply `Vec3` component-wise.

**Handle IDs reserved:**
```
0xFFFF_FFF0..0xFFFF_FFF2  Translate X/Y/Z  (I-9)
0xFFFF_FFF3..0xFFFF_FFF5  Rotate    X/Y/Z  (I-12)
0xFFFF_FFF6..0xFFFF_FFF8  Scale     X/Y/Z  (I-12)
```

**Verification**
- E → rotate handles visible; drag rotates cube around selected axis; Ctrl+Z reverts.
- R → scale handles; drag scales.
- Toggle local/world: rotate cube via E, then E again with Ctrl+Q — handles align with cube's local axes vs world.

**Done when:** 3 separate undo entries for translate→rotate→scale on the same entity, each undoable independently in reverse order.

---

## I-13 — Grid & axis widget

**Goal:** infinite ground grid in the viewport; small X/Y/Z axis widget in bottom-right corner showing camera orientation.

**Files**
- New `components/grid.wgsl` — distance-faded grid lines using screen-space derivatives.
- New `components/axis_gizmo.rs` — renders three colored arrows at fixed screen position.
- Edit `components/viewport_3d.rs` — three extra passes (grid before entities, axis after all).

**Grid shader approach**
Standard "infinite plane grid" technique — fullscreen quad, ray-plane intersect in fragment shader, grid line intensity from distance to nearest multiple, fade by distance from camera.

**Axis gizmo**
A small 3D scene rendered to a tiny viewport (e.g., 80×80 pixels) in the corner. Reuses the main camera's rotation but fixed position + orthographic.

**Verification**
- Grid visible, lines crisp near camera, fade at 100 m+ range.
- Axis widget rotates with orbit drag; X=red, Y=green, Z=blue.
- Grid draws beneath entities (depth test + write).

**Risks**
- Grid shader has well-known flickering issues at grazing angles. Mitigate with anti-aliasing in the fragment shader.
- Axis widget viewport clamping — must not bleed outside its corner box.

**Done when:** rotating camera through 360° yaw keeps axis widget legible the entire time.

---

## I-14 — Hierarchical transform propagation

**Goal:** entities with a `Parent` component have their world matrix computed as `parent_world * local`. Moving a parent moves all children.

**Files**
- Edit `crates/rustforge-core/src/world.rs` — add `WorldTransform` component + propagation system.
- New `crates/rustforge-core/src/scene/hierarchy.rs` — traversal helpers.
- Edit `components/viewport_3d.rs` — render pass uses `WorldTransform`, not `Transform`.

**Algorithm**
Topological traversal starting from roots (entities with no `Parent`). For each root, DFS, accumulating matrices. Write `WorldTransform` component.

```rust
pub fn propagate_transforms(world: &mut World) {
    let roots: Vec<hecs::Entity> = world.inner.query::<(&Transform, Without<Parent>)>().iter().map(|(e, _)| e).collect();
    for root in roots {
        propagate_recursive(world, root, Mat4::IDENTITY);
    }
}

fn propagate_recursive(world: &mut World, entity: hecs::Entity, parent_world: Mat4) {
    let local = world.inner.get::<&Transform>(entity).map(|t| t.matrix()).unwrap_or(Mat4::IDENTITY);
    let world_mat = parent_world * local;
    let _ = world.inner.insert_one(entity, WorldTransform(world_mat));

    let children: Vec<hecs::Entity> = world.inner.query::<&Parent>().iter()
        .filter_map(|(e, p)| (p.0 == entity).then_some(e)).collect();
    for child in children {
        propagate_recursive(world, child, world_mat);
    }
}
```

**Verification**
- Spawn parent + child; move parent via gizmo; child moves with it maintaining local offset.
- Round-trip test: save scene with hierarchy, load, hierarchy preserved, world transforms recomputed identically.

**Done when:** reparenting a cube under another cube via a hierarchy drag-drop (add if not present) makes the cube inherit parent transforms live.

---

## I-15 — glTF mesh import

**Goal:** drop a `.gltf`/`.glb` file into the Content Browser; it appears as an asset; dragging it into the viewport spawns an entity with that mesh.

**Files**
- New `crates/rustforge-core/src/assets/mesh.rs` — `CpuMesh { vertices, indices }`, `GpuMesh` (wgpu-side upload).
- New `crates/rustforge-core/src/assets/gltf_importer.rs` — uses `gltf` crate.
- Edit `crates/rustforge-core/src/assets.rs` — extend `AssetKind` with `Mesh`.
- Edit `rustforge-editor-ui/src/components/content_browser.rs` — drag-drop handling.
- Edit `components/viewport_3d.rs` — `MeshHandle(u32)` now indexes a real mesh table, not just `0 = builtin cube`.

**Deps:** `gltf = "1.4"` (workspace).

**Vertex layout unification**
Decide once and never change:
```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct StandardVertex {
    position: [f32; 3],
    normal:   [f32; 3],
    uv:       [f32; 2],
    color:    [f32; 4],
}
```
Cube (I-3) and all glTF meshes use this layout. Breaking change to I-3; bundled in I-15.

**Verification**
- Drop `Duck.glb` (Khronos sample) into content browser; `.meta` sidecar appears.
- Drag mesh asset icon into viewport → cube-sized duck spawns, lit by existing flat shader (I-16 adds proper lighting).
- Scene save/load preserves the mesh reference via asset GUID.

**Risks**
- glTF animation / skinning explicitly excluded — I-15 imports static meshes only.
- Large meshes (> 1 M verts) may need chunked upload; keep a simple all-at-once upload and flag as future optimization.

**Done when:** content browser shows 3 different meshes imported; each can be dragged into the scene and rendered correctly.

---

## I-16 — Basic PBR material + texture loading

**Goal:** materials have albedo color + albedo texture + metallic + roughness. Directional light (from I-17 prereq — split or bundle per session call) lights the scene. Textures load from glTF.

**Files**
- New `crates/rustforge-core/src/assets/texture.rs` — `CpuTexture`, `GpuTexture` (wgpu).
- New `crates/rustforge-core/src/assets/material.rs` — `Material { albedo_color, albedo_texture, metallic, roughness }`.
- New `components/pbr.wgsl` — replaces the placeholder `viewport_cube.wgsl`.
- Edit `gltf_importer.rs` — extract materials and textures alongside meshes.

**Deps:** `image = "0.25"` (workspace) — already transitively available, pin explicitly.

**WGSL shader**
Standard Cook-Torrance BRDF:
```wgsl
struct MaterialUniform {
    albedo: vec4<f32>,
    metallic_roughness: vec4<f32>, // .x=metallic, .y=roughness
};

@group(1) @binding(0) var<uniform> mat: MaterialUniform;
@group(1) @binding(1) var albedo_tex: texture_2d<f32>;
@group(1) @binding(2) var albedo_sampler: sampler;
```

Fragment: one directional light hardcoded for I-16; I-17 upgrades to real light entities.

**Verification**
- Duck renders with its albedo texture (not white or vertex-colored).
- Metallic sphere (glTF `MetalRoughSpheres.glb` sample) shows expected gradient.
- Material inspector (built via reflection) edits color/metallic/roughness live.

**Risks**
- sRGB vs linear is the #1 bug source. Textures loaded as sRGB, output to sRGB surface format, shading in linear. Pipeline format must match.
- Bind group management gets non-trivial — 0=camera, 1=material. Keep it explicit.

**Done when:** changing `metallic` from 0→1 in the inspector visibly transforms a sphere from diffuse-matte to metallic-specular in the viewport.

---

## I-17 — Lights (directional + point)

**Goal:** scene has real `DirectionalLight` and `PointLight` entities. Inspector edits their color/intensity live. Up to 4 point lights supported in the shader.

**Files**
- New `crates/rustforge-core/src/world.rs` additions: `DirectionalLight`, `PointLight` components.
- Edit `components/pbr.wgsl` — light loop in fragment.
- Edit `components/viewport_3d.rs` — gather lights into a uniform buffer per frame.

**Components**
```rust
#[derive(Debug, Clone, Copy, Reflect)]
pub struct DirectionalLight {
    pub direction: Vec3,
    pub color: Vec3,
    pub intensity: f32,
}

#[derive(Debug, Clone, Copy, Reflect)]
pub struct PointLight {
    pub color: Vec3,
    pub intensity: f32,
    pub range: f32,
}
```

Point light position comes from the entity's `WorldTransform` (I-14).

**Lights uniform**
```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LightsUniform {
    directional_direction: [f32; 4],
    directional_color:     [f32; 4], // .w = intensity
    point_count:           u32,
    _pad:                  [u32; 3],
    points: [PointLightGpu; 4],
}
```

Fixed-size array of 4 point lights — keep it simple for I-17; dynamic lights come later.

**Verification**
- Spawn a directional light, watch cube shading respond to direction edits.
- Spawn 4 point lights at cube corners; cube lights up from 4 sides.
- Light gizmo (sun icon for directional, bulb icon for point) visible in viewport.

**Done when:** rotating directional light 180° around X visibly flips which side of the cube is lit.

---

## I-18 — Prefab system

**Goal:** select a subtree, right-click → "Create Prefab". Prefab asset saved to disk. Drag prefab from content browser into scene → subtree instantiated with fresh entity IDs.

**Files**
- New `crates/rustforge-core/src/scene/prefab.rs` — `PrefabDocument` (currently a stub in existing scene code — extend).
- Edit content browser for prefab asset type.
- Edit hierarchy context menu.
- Edit `components/viewport_3d.rs` for drop-to-spawn from prefab.

**Prefab format**
Same RON structure as scene subtree, but rooted at a single entity. Stored as `.rprefab` in `projects/<name>/assets/prefabs/`.

**Instance semantics (v1)**
Plain duplicate — drop a prefab, a copy of its subtree is spawned with new hecs entities. No instance-to-prefab link yet (overrides/propagation is I-19's problem, or later).

**Verification**
- Build a prefab with a parent cube + 2 children at specific local offsets.
- Instantiate twice; each instance has distinct entity IDs but identical structure.
- Edit the original prefab source; re-instantiate; new instances reflect the edit (old instances do not — they're independent copies in v1).

**Done when:** round-tripping a prefab through the content browser creates a new `.rprefab` file that a separate `cargo run` can load.

---

## I-19 — Content Browser with real file watcher

**Goal:** the Content Browser panel lists actual files in `projects/<name>/assets/`, live-updates when files are added/removed externally, and shows thumbnails for meshes and textures.

**Files**
- New `crates/rustforge-editor-ui/src/assets/watcher.rs` — wraps `notify` crate.
- New `crates/rustforge-editor-ui/src/assets/thumbnails.rs` — async thumbnail generation.
- Edit `components/content_browser.rs` — tree view + grid view.

**Deps:** `notify = "6"`, `parking_lot = "0.12"` (workspace).

**Thumbnail generation**
- Texture thumbnails: downscale via `image` crate on a background thread.
- Mesh thumbnails: render the mesh in a hidden 128×128 wgpu offscreen target, save as PNG in `.rustforge/cache/thumbnails/<guid>.png`.
- Cache invalidated by source file's mtime (tracked in `.meta`).

**Verification**
- Drop a `.png` into `projects/sandbox/assets/textures/` via Explorer → content browser updates within 1 s.
- Texture shows as thumbnail in the browser grid.
- Delete file externally → asset removed from browser with a toast.

**Risks**
- `notify` has platform edge cases (Windows rename fires separate Create/Delete). Document and handle.
- Thumbnail for a mesh requires a GPU context — the editor's wgpu device must be shared with the thumbnail renderer. Use existing `egui_wgpu::RenderState`.

**Done when:** navigating into a subfolder of assets/ shows only that folder's contents; external file changes appear without editor restart.

---

## I-20 — Play / Pause / Stop (PIE)

**Goal:** toolbar has Play, Pause, Stop buttons. Pressing Play snapshots the scene, ticks the engine; Stop restores snapshot exactly.

**Files**
- New `crates/rustforge-core/src/play_mode.rs` — state machine.
- Edit toolbar component for the buttons.
- Edit viewport border to color orange during Playing, blue during Paused.
- Edit `app.rs` tick loop to dispatch on play state.

**State machine**
Matches design Phase 7 §3:
```rust
pub enum PlayState { Edit, Playing, Paused }

pub struct PlayMode {
    pub state: PlayState,
    snapshot: Option<SceneDocument>, // full RON snapshot
    session_time: Duration,
    step_pending: bool,
}
```

**Snapshot strategy (v1)**
Strategy A from Phase 7: serialize via `SceneDocument`, restore by clearing world + `into_world`. Reuses I-10 round-trip.

**Tick**
- Edit: `engine.tick_edit(dt)` — light systems (transform propagation, render).
- Playing: `engine.tick_play(dt)` — transform, physics (I-23 prereq), scripts (I-21 prereq).

For I-20 specifically, without scripts or physics yet, Play mode just freezes the command stack and runs a timer — it's mostly the *invariant* work. Real tick content comes with I-21 onward.

**Command stack freeze**
```rust
command_stack.set_enabled(play_mode.state == PlayState::Edit);
```

**Verification**
- Press Play → viewport border turns orange, Stop button enabled, command stack shows "disabled".
- Move a cube via gizmo during play (edit allowed, not recorded).
- Press Stop → cube snaps back to pre-play position; viewport border returns to normal.
- Ctrl+Z after Stop undoes the pre-play edit (stack untouched across play).

**Done when:** the byte-identical round-trip invariant is verified by a test: snapshot → play 100 frames → stop → `SceneDocument` equals pre-play.

---

# Arc 3 — Runtime Foundations (I-21 → I-30)

Less detailed. Each phase states direction + key decisions + named crates. Actual specs to be written closer to landing.

## I-21 — WASM scripting runtime

Wire `wasmtime` as the script host. Scripts compile separately; load `.wasm` modules at editor startup; host functions expose `print`, `transform_set`, `log`, `input_get`. Reuses the already-sketched Phase 7 contract.

**Crates:** `wasmtime = "25"`, `wasmtime-wasi = "25"`.

**Deliverable:** sample script `println!("hello from WASM")` runs once on Play; log visible in Console panel.

**Critical decision:** component model vs classic `--target wasm32-unknown-unknown`. Recommend classic for I-21; component model post-I-30.

## I-22 — Script components & tick

Attach a `Script(AssetGuid)` component to an entity; during `tick_play` each entity with a Script runs its `on_tick(dt)` WASM export. Host functions let scripts read/write Transform of the entity they're attached to.

**Deliverable:** spin-a-cube script (`rot.y += dt`) makes a cube rotate during Play, stops on Stop (snapshot restore reverts rotation).

**Gotcha:** per-entity script state lives inside the WASM module instance; one instance per entity-script pair. Instance creation cost measured and optimized if > 1 ms.

## I-23 — Physics (Rapier)

Integrate `rapier3d` for rigid bodies + colliders. Each entity with `RigidBody` + `Collider` components participates in stepping during Play.

**Crate:** `rapier3d = "0.22"`.

**Deliverable:** spawn a box with `RigidBody::Dynamic` above a static plane; press Play, box falls under gravity, rests on plane, stops on Stop (snapshot restores initial pose).

**Decision:** bake Rapier's world from ECS components each frame, or maintain persistent Rapier handles? Persistent wins on perf; manage lifecycle carefully when entities despawn.

## I-24 — Physics debug draw

Wireframe colliders visible in viewport during Edit and Play. Contact points overlaid during Play. Toggle via View menu.

**Deliverable:** collider shape matches visual mesh; bounce on contact shows a white dot at the impact.

## I-25 — Audio engine (cpal mixer)

Wire `cpal` for platform audio output; `rodio` or custom mixer for source → bus routing. `AudioSource` component plays a clip at entity position (spatial attenuation only, no HRTF).

**Crates:** `cpal = "0.15"`, `rodio = "0.20"` (or custom mixer).

**Deliverable:** drop a `.wav` into assets, attach to a cube via inspector, press Play — audio plays at cube position, stops at Stop (mixer flush).

## I-26 — Input action system

`ActionMap` asset with `Action("jump") -> KeyCode::Space`. Scripts call `input::is_pressed("jump")`. Hot-reload bindings without restart.

**Deliverable:** edit action map RON; script reacts to renamed action immediately after save.

## I-27 — Asset GUID + hot reload pipeline

Every asset gets a stable `AssetGuid` (uuid) in its `.meta` sidecar. Reimport events fire when source changes; meshes/textures reload without restart.

**Deliverable:** edit a texture externally in Photoshop → save → the texture on the cube updates within 1 s without restart.

## I-28 — Frame profiler

GPU timestamp queries (wgpu `timestamp_writes`) for each render pass. CPU `tracing`-style scoped timers for ECS systems. Profiler panel shows a frame graph.

**Deliverable:** Profiler panel shows per-system CPU time and per-pass GPU time with a 240-frame rolling graph.

## I-29 — Build pipeline

`cargo xtask cook` bakes all project assets into a platform-specific `.pak`. `cargo xtask build --release` produces a standalone game binary that loads the pak.

**Decision:** xtask vs separate `rustforge-cli` crate. Recommend xtask for I-29; extract to dedicated CLI in a later phase.

**Deliverable:** `cargo xtask build --release --out dist/` produces `dist/game.exe` + `dist/game.pak` that runs the sandbox scene standalone, no editor dependency.

## I-30 — Standalone game runtime

`rustforge-game` binary crate (separate from editor) that loads a pak and ticks the engine with gameplay systems (scripts, physics, audio, rendering, input) — no editor UI. The shipped artifact of I-29.

**Deliverable:** `dist/game.exe` opens in fullscreen, shows the sandbox scene rendered, responds to input via scripts, closes cleanly.

**Done when:** the game binary runs on a machine that has never had Rust installed.

---

# Beyond I-30

Not specced in this roadmap. At I-30 the engine can:
- Build and edit 3D scenes with real assets, lights, materials
- Run play mode with physics and scripts
- Ship a standalone game

What's still missing vs the 50-phase design series:
- Networking (Phase 14)
- Game UI framework (Phase 15)
- Particles / VFX (Phase 18)
- Sky / weather (Phase 27)
- World partition (Phase 31)
- Advanced rendering: Lumen/Nanite equivalents (Phase 47/48)
- Visual scripting (Phase 49)
- Modding runtime (Phase 38)
- Most of the "advanced" features

These become I-31+ phases, specced after I-30's reality informs the roadmap. Do not pre-plan past I-30 — the shape of real I-11..I-30 will reveal things that make pre-planned I-31+ stale.

## Summary table

| Arc | Phases | Theme | Detail level |
|---|---|---|---|
| Arc 1 | I-0 → I-10 | Minimum viable editor | Full work-orders (IMPL_NEXT.md) |
| **Arc 2** | **I-11 → I-20** | **Complete editor** | **Full work-orders (this doc, part 1)** |
| **Arc 3** | **I-21 → I-30** | **Runtime foundations** | **Direction + decisions (this doc, part 2)** |
| Arc 4+ | I-31+ | Advanced / specialized | Not planned yet |

## Verification discipline (unchanged)

Every phase landing:
1. Code diff visible in Git.
2. `cargo check --workspace` clean.
3. `cargo test --workspace` clean with at least one new test.
4. Screenshot / CLI output evidence in the commit message.
5. Mock call-site inventory shrinks.

## Anti-goals (unchanged)

- No design docs replacing running code.
- No Mock paths added to unblock a phase.
- No jumping ahead past I-30 in speculative-spec form.
- No Rust snippets here that wouldn't compile when the phase lands.

## Status as of this file's creation

- I-0: landed (audit).
- I-1: landed (`viewport_3d.rs`, real wgpu draw in viewport).
- I-2 through I-30: not yet implemented. This file is the work-order queue.
