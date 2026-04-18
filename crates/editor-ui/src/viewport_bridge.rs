use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use glam::{Mat4, Quat, Vec3};
use engine::hooks::{EngineHooks, MockEngine, PickRequest, RenderRequest};
use engine::mesh::{gltf as gltf_import, MeshData, MeshImportError};
use engine::picking::{self, GizmoAxis, GizmoLayout, GizmoMode, Ray};
use engine::scene::{SceneDocument, SceneId};
use engine::scripting::{ScriptError, ScriptHost};
use engine::world::{
    Entity, MeshHandle, MeshSource, RenderEntity, SceneInstantiation, TextureHandle, TextureSource,
    Transform, World,
};
use render::camera::Camera as RenderCamera;
use render::mesh::MeshAssetId;
use render::texture::TextureAssetId;

use crate::components::viewport_3d::{MeshDraw, PendingMeshUpload, PendingTextureUpload};

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportSnapshot {
    pub attachment_id: u64,
    pub width: u32,
    pub height: u32,
    pub entity_slots: Vec<SceneId>,
}

/// Simple per-entity angular velocity in radians/second. Exists only
/// so I-4 can demo an ECS-driven rotation without a full scheduler —
/// real systems live in `engine::world` starting I-5+.
#[derive(Debug, Clone, Copy)]
pub struct Spin {
    pub yaw_speed:   f32,
    pub pitch_speed: f32,
}

pub struct ViewportBridge {
    engine: MockEngine,
    last_snapshot: Option<ViewportSnapshot>,
    /// Runtime ECS world. Populated at construction with a small
    /// tableau of cubes and rebuilt on every `sync_scene` call once
    /// a scene document is attached (I-5).
    world: World,
    world_entities: Vec<Entity>,
    /// Mapping from scene ids → runtime entity handles, populated
    /// whenever the world is rebuilt from a `SceneDocument`.
    scene_mapping: SceneInstantiation,
    elapsed_secs: f32,
    /// I-27: imports scheduled for GPU upload on the next paint. The
    /// egui paint callback takes this list, feeds it to `MeshRegistry`,
    /// and the bridge clears it — uploads are one-shot per mesh.
    pending_mesh_uploads: Vec<PendingMeshUpload>,
    /// I-27: handles already uploaded to the registry this session.
    /// Prevents the bridge from re-reading .gltf files from disk on
    /// every rebuild when the same path is still referenced. Cleared
    /// on `reset_gpu_caches` (called from exit_play_mode today).
    uploaded_handles: HashSet<u64>,
    /// I-32: parallel queue for texture uploads. Same one-shot
    /// lifecycle as `pending_mesh_uploads` — the callback consumes and
    /// the bridge clears between frames.
    pending_texture_uploads: Vec<PendingTextureUpload>,
    /// I-32: texture handles already resident in `TextureRegistry`.
    /// Skips re-decoding PNGs on every scene rebuild.
    uploaded_texture_handles: HashSet<u64>,
    /// I-35: Rhai scripting host. Owned by the bridge (not the core
    /// `World`) because it performs filesystem I/O and carries
    /// editor-only state — error log, AST cache — that the runtime
    /// side has no use for. The bridge ticks it from the same
    /// `tick_gameplay` call path the editor uses for `Mover`.
    script_host: ScriptHost,
}

impl Default for ViewportBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewportBridge {
    pub fn new() -> Self {
        let mut world = World::new();
        // Three starter cubes so the viewport proves the renderer is
        // walking the ECS and not still hardcoding a single draw.
        //   - center: spins on both axes (replaces the I-3 cube).
        //   - left:   static, slightly smaller.
        //   - right:  spins fast on yaw only.
        let center = world.spawn_cube(Transform::IDENTITY);
        let left = world.spawn_cube(
            Transform::from_translation(Vec3::new(-2.0, 0.0, 0.0))
                .with_scale(Vec3::splat(0.6)),
        );
        let right = world.spawn_cube(Transform::from_translation(Vec3::new(2.0, 0.0, 0.0)));

        // Attach Spin components via the raw ECS handle.
        let _ = world.ecs_mut().insert_one(
            center,
            Spin {
                yaw_speed:   0.7,
                pitch_speed: 0.4,
            },
        );
        let _ = world.ecs_mut().insert_one(
            right,
            Spin {
                yaw_speed:   1.8,
                pitch_speed: 0.0,
            },
        );
        // Left has no Spin — stays put to prove Spin is opt-in.

        Self {
            engine: MockEngine::default(),
            last_snapshot: None,
            world,
            world_entities: vec![center, left, right],
            scene_mapping: SceneInstantiation::default(),
            elapsed_secs: 0.0,
            pending_mesh_uploads: Vec::new(),
            uploaded_handles: HashSet::new(),
            pending_texture_uploads: Vec::new(),
            uploaded_texture_handles: HashSet::new(),
            script_host: ScriptHost::new(),
        }
    }

    /// Rebuild the runtime world from a scene document (I-5). The old
    /// world is cleared, every `SceneEntity` is translated into its
    /// runtime counterpart, and a `Spin` is attached to any entity
    /// whose name starts with `Spin` so the scene can demonstrate the
    /// I-4 animation path through authoring data.
    pub fn rebuild_world_from_scene(&mut self, doc: &SceneDocument) {
        self.world.clear();
        self.scene_mapping = self.world.instantiate_scene(doc);
        // Rebuild the authoring-index list so selection helpers stay
        // valid — order matches the flattened scene traversal.
        self.world_entities = self
            .scene_mapping
            .entity_to_scene
            .keys()
            .copied()
            .collect();

        // Demo-only: entities whose scene name starts with "Spin" get
        // a default Spin component. Real authored motion arrives with
        // the reflection derive in I-6.
        let ecs = self.world.ecs_mut();
        let to_spin: Vec<Entity> = doc
            .root_entities
            .iter()
            .flat_map(collect_spin_names)
            .filter_map(|scene_id| self.scene_mapping.entity(scene_id))
            .collect();
        for entity in to_spin {
            let _ = ecs.insert_one(
                entity,
                Spin {
                    yaw_speed:   0.9,
                    pitch_speed: 0.3,
                },
            );
        }
    }

    pub fn scene_mapping(&self) -> &SceneInstantiation {
        &self.scene_mapping
    }

    /// I-27: walk the ECS for every `MeshSource`, read the referenced
    /// glTF off disk, and queue the resulting geometry for upload on
    /// the next paint. Returns a list of `(path, result)` tuples so
    /// the caller can surface successes + failures to the logs.
    ///
    /// Resolution rules:
    ///   - Paths are joined against `asset_root` (typically
    ///     `<project>/assets`). Absolute source paths are used as-is.
    ///   - The resolver closure handed to `gltf::import_from_slice`
    ///     reads sidecar `.bin` files from the directory that holds
    ///     the top-level `.gltf`. Data URIs embedded in the file work
    ///     out of the box without touching disk again.
    ///   - Handles already in `uploaded_handles` are skipped — the
    ///     renderer's `MeshRegistry` is persistent, so re-uploading
    ///     on every scene rebuild would be wasted GPU traffic.
    pub fn import_mesh_sources_from_disk(
        &mut self,
        asset_root: &Path,
    ) -> Vec<(String, Result<usize, MeshImportError>)> {
        // Collect a de-duplicated map: path → MeshHandle. Multiple
        // entities sharing the same source only trigger one import.
        let mut wanted: HashMap<String, u64> = HashMap::new();
        for (_entity, (handle, source)) in self.world.ecs().query::<(&MeshHandle, &MeshSource)>().iter() {
            if self.uploaded_handles.contains(&handle.0) {
                continue;
            }
            wanted.entry(source.path.clone()).or_insert(handle.0);
        }

        let mut results = Vec::new();
        for (path_str, handle_id) in wanted {
            let full_path = resolve_asset_path(asset_root, &path_str);
            match load_gltf_from_disk(&full_path) {
                Ok(meshes) => {
                    let primitive_count = meshes.len();
                    if primitive_count == 0 {
                        results.push((path_str, Err(MeshImportError::NoMeshes)));
                        continue;
                    }
                    // A glTF file can hold multiple primitives; we
                    // only keep the first for now because `MeshHandle`
                    // is a single u64 with no sub-primitive index.
                    // Multi-primitive assets land when material slots
                    // do — for now, warn loud and continue.
                    let mesh = meshes.into_iter().next().unwrap();
                    self.pending_mesh_uploads.push(PendingMeshUpload {
                        id:        MeshAssetId(handle_id),
                        name:      mesh.name.clone(),
                        positions: mesh.positions,
                        normals:   mesh.normals,
                        indices:   mesh.indices,
                    });
                    self.uploaded_handles.insert(handle_id);
                    results.push((path_str, Ok(primitive_count)));
                }
                Err(error) => results.push((path_str, Err(error))),
            }
        }
        results
    }

    /// Take the queued mesh uploads. The egui paint callback receives
    /// this Vec and hands it to `MeshRegistry::upload`; after the call
    /// the bridge's queue is empty until another import fires.
    pub fn take_pending_mesh_uploads(&mut self) -> Vec<PendingMeshUpload> {
        std::mem::take(&mut self.pending_mesh_uploads)
    }

    /// I-32: mirror of `import_mesh_sources_from_disk` for albedo
    /// textures. Walks the ECS for every `(TextureHandle, TextureSource)`
    /// pair, decodes the PNG/JPEG off disk, and enqueues a
    /// `PendingTextureUpload` the next paint pushes into
    /// `TextureRegistry`.
    ///
    /// Handles already tracked in `uploaded_texture_handles` are
    /// skipped — re-decoding a 2 MP PNG on every scene rebuild is
    /// wasted CPU + GPU traffic. `reset_mesh_import_cache` clears
    /// both caches together so a project switch re-imports cleanly.
    pub fn import_texture_sources_from_disk(
        &mut self,
        asset_root: &Path,
    ) -> Vec<(String, Result<(u32, u32), TextureImportError>)> {
        // De-dupe by path: two entities pointing at the same PNG only
        // trigger one decode.
        let mut wanted: HashMap<String, u64> = HashMap::new();
        for (_entity, (material, source)) in self
            .world
            .ecs()
            .query::<(&engine::world::Material, &TextureSource)>()
            .iter()
        {
            let Some(handle) = material.albedo_texture else {
                continue;
            };
            if self.uploaded_texture_handles.contains(&handle.0) {
                continue;
            }
            wanted.entry(source.path.clone()).or_insert(handle.0);
        }

        let mut results = Vec::new();
        for (path_str, handle_id) in wanted {
            let full_path = resolve_asset_path(asset_root, &path_str);
            match load_texture_from_disk(&full_path) {
                Ok((width, height, rgba8)) => {
                    self.pending_texture_uploads.push(PendingTextureUpload {
                        id: TextureAssetId(handle_id),
                        name: path_str.clone(),
                        width,
                        height,
                        rgba8,
                    });
                    self.uploaded_texture_handles.insert(handle_id);
                    results.push((path_str, Ok((width, height))));
                }
                Err(error) => results.push((path_str, Err(error))),
            }
        }
        results
    }

    /// Take the queued texture uploads. Paint callback sinks these
    /// into `TextureRegistry` and the bridge's queue resets to empty.
    pub fn take_pending_texture_uploads(&mut self) -> Vec<PendingTextureUpload> {
        std::mem::take(&mut self.pending_texture_uploads)
    }

    /// Partition the current render snapshot into the subset that
    /// goes through the baked cube pipeline (handle == UNIT_CUBE) and
    /// the subset routed through `MeshInstanceRenderer` (imported
    /// glTF assets keyed by non-zero handles).
    ///
    /// Returning two separate Vecs instead of one + a filter closure
    /// matches the shape of the paint callback's API (`entities` +
    /// `mesh_draws`) so call sites don't have to split manually.
    pub fn split_render_snapshot(
        &self,
        selected: Option<SceneId>,
    ) -> (Vec<RenderEntity>, Vec<MeshDraw>) {
        self.split_render_snapshot_for_mode(selected, GizmoMode::Translate)
    }

    /// I-34: mode-aware variant. Paints the gizmo geometry that
    /// matches the current editor mode — arrows for translate, rings
    /// for rotate, axis-cubes-plus-center for scale.
    pub fn split_render_snapshot_for_mode(
        &self,
        selected: Option<SceneId>,
        mode: GizmoMode,
    ) -> (Vec<RenderEntity>, Vec<MeshDraw>) {
        let snapshot = self.render_snapshot_with_gizmo_mode(selected, mode);
        let mut cubes = Vec::with_capacity(snapshot.len());
        let mut draws = Vec::new();
        for entity in snapshot {
            if entity.mesh == MeshHandle::UNIT_CUBE {
                cubes.push(entity);
            } else {
                draws.push(MeshDraw {
                    mesh:           MeshAssetId(entity.mesh.0),
                    model:          entity.model,
                    albedo:         entity.albedo,
                    // I-32: carry the per-entity texture handle
                    // through — `TextureHandle` and `TextureAssetId`
                    // share their u64 payload, so a bit-cast is
                    // enough (tested in core's `texture_handle_unit_
                    // white_is_zero`).
                    albedo_texture: TextureAssetId(entity.albedo_texture.0),
                });
            }
        }
        (cubes, draws)
    }

    /// Drop the import bookkeeping so the next scene rebuild re-reads
    /// .gltf files from disk. Called on project switch so stale handles
    /// from a different project don't block a fresh import; the GPU
    /// registry itself is cleared by the paint callback via its own
    /// reset pathway (handled out of band with `MeshRegistry::clear`).
    pub fn reset_mesh_import_cache(&mut self) {
        self.pending_mesh_uploads.clear();
        self.uploaded_handles.clear();
        // I-32: clear the parallel texture state so a project switch
        // forces a re-decode against the new asset root. The GPU
        // registry itself is reset elsewhere (paint-side) alongside
        // `MeshRegistry::clear`.
        self.pending_texture_uploads.clear();
        self.uploaded_texture_handles.clear();
    }

    /// Push authoring-side edits (rename, field change, nudge) into the
    /// already-instantiated runtime world without dropping transient
    /// components like `Spin`. I-10's save/load story is symmetric:
    /// editing the scene document is what gets saved, so the viewport
    /// must render the exact same state we'd round-trip through RON.
    ///
    /// If the scene has been mutated topologically (entities added or
    /// removed after the initial instantiation — not yet a supported
    /// command) callers should use [`Self::rebuild_world_from_scene`]
    /// instead.
    pub fn resync_world_from_scene(&mut self, doc: &SceneDocument) {
        self.world
            .resync_transforms_from_scene(doc, &self.scene_mapping);
    }

    /// Advance the ECS by `dt` seconds. Currently this only drives the
    /// Spin component; later phases will schedule real systems here.
    pub fn tick_world(&mut self, dt: f32) {
        self.elapsed_secs += dt;
        for (_entity, (transform, spin)) in self
            .world
            .ecs_mut()
            .query_mut::<(&mut Transform, &Spin)>()
        {
            let delta = Quat::from_rotation_y(spin.yaw_speed * dt)
                * Quat::from_rotation_x(spin.pitch_speed * dt);
            transform.rotation = delta * transform.rotation;
        }
    }

    /// I-26: advance gameplay systems (currently just `Mover`) by one
    /// frame. Editor calls this only while Play mode is active and
    /// leaves `tick_world` alone so Play mode doesn't double-advance
    /// the demo Spin animation.
    ///
    /// The caller owns the `Input` snapshot (built in `app.rs` from
    /// egui's keyboard state). Keeping the bridge ignorant of egui is
    /// deliberate — headless tests in `engine` construct
    /// `Input` by hand and exercise `World::tick_gameplay` directly.
    pub fn tick_gameplay(&mut self, input: &engine::input::Input, dt: f32) {
        self.world.tick_gameplay(input, dt);
    }

    /// I-35: tick every `Script` entity in the world using `project_root`
    /// to resolve `.rhai` source paths. Called immediately after
    /// [`Self::tick_gameplay`] so scripts observe post-physics
    /// transforms and their writes become the authoritative pose for
    /// rendering this frame.
    ///
    /// Split from `tick_gameplay` because scripting is purely
    /// editor-side (the runtime `World` has no `ScriptHost`) and the
    /// host needs the project root — a concept the core crate doesn't
    /// know about.
    pub fn tick_scripts(&mut self, input: &engine::input::Input, dt: f32, project_root: &Path) {
        self.script_host
            .tick_world(&mut self.world, input, dt, project_root);
    }

    /// I-35: drain the script error log. Editor calls this each frame
    /// to surface compile/runtime failures to the Console panel without
    /// letting them pile up indefinitely in the host.
    pub fn drain_script_errors(&mut self) -> Vec<ScriptError> {
        self.script_host.drain_errors()
    }

    /// I-35: reset the script clock + purge cached ASTs. Editor calls
    /// this on Play-mode entry (zero `TIME`) and on project switch
    /// (stale ASTs from the previous project must not execute).
    pub fn reset_script_host(&mut self) {
        self.script_host.clear_cache();
        self.script_host.reset_play_time();
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Snapshot the current ECS state for the renderer. Main-thread
    /// call before paint; the resulting Vec is shipped into the
    /// egui_wgpu callback.
    pub fn render_snapshot(&self) -> Vec<RenderEntity> {
        self.world.collect_render_snapshot()
    }

    /// Render snapshot extended with gizmo handles for the current
    /// selection (I-9). Defaults to translate mode — callers wanting
    /// rotate/scale should use [`Self::render_snapshot_with_gizmo_mode`].
    pub fn render_snapshot_with_gizmo(
        &self,
        selected: Option<SceneId>,
    ) -> Vec<RenderEntity> {
        self.render_snapshot_with_gizmo_mode(selected, GizmoMode::Translate)
    }

    /// I-34: mode-aware gizmo painter.
    ///
    /// Every handle is rendered via the same unit-cube mesh (the
    /// only mesh the shadow pipeline + cube pipeline know about
    /// today), so different shapes get approximated through
    /// scale / placement:
    ///
    /// * **Translate** — three tip cubes at `pivot + axis *
    ///   arm_length`. Small, cube-shaped, sized `handle_size`. Matches
    ///   legacy I-9 behaviour exactly.
    /// * **Rotate** — three rings drawn as a chain of `RING_SEGMENTS`
    ///   small cubes around each axis plane. A real ring mesh would
    ///   be cheaper but hauls in a second pipeline; the cube chain is
    ///   visually indistinguishable at editor scale and reuses every
    ///   existing draw path (shadow, lighting, material).
    /// * **Scale** — same three tip cubes as translate but slightly
    ///   larger so the user can tell the modes apart at a glance,
    ///   plus a fourth center cube (uniform scale handle) at the
    ///   pivot.
    ///
    /// Colors always follow the R/G/B = X/Y/Z convention. The uniform
    /// scale handle is painted white-grey so it reads as "all axes".
    pub fn render_snapshot_with_gizmo_mode(
        &self,
        selected: Option<SceneId>,
        mode: GizmoMode,
    ) -> Vec<RenderEntity> {
        let mut snapshot = self.world.collect_render_snapshot();
        let Some(scene_id) = selected else {
            return snapshot;
        };
        let Some(entity) = self.scene_mapping.entity(scene_id) else {
            return snapshot;
        };
        let Ok(transform) = self.world.ecs().get::<&Transform>(entity) else {
            return snapshot;
        };
        let pivot = transform.translation;
        let layout = GizmoLayout::centered(pivot);

        match mode {
            GizmoMode::Translate => push_translate_handles(&mut snapshot, entity, pivot, &layout),
            GizmoMode::Rotate => push_rotate_handles(&mut snapshot, entity, pivot, &layout),
            GizmoMode::Scale => push_scale_handles(&mut snapshot, entity, pivot, &layout),
        }

        snapshot
    }

    /// I-25: resolve the primary gameplay camera into a render-ready
    /// `Camera`. `None` when the scene has no `Camera` component —
    /// the viewport falls back to the editor orbit camera in that case.
    ///
    /// World-to-render mapping:
    ///   - **Position** = translation column of the camera entity's
    ///     world transform (parent chain composed).
    ///   - **Forward**  = `world_rotation * Vec3::NEG_Z` (right-hand
    ///     coord system, "down the negative Z axis" is the standard
    ///     OpenGL/glTF camera convention).
    ///   - **Up**       = `world_rotation * Vec3::Y`. Using the
    ///     rotated up (not world Y) lets authors roll the camera by
    ///     rotating the entity around its local Z.
    ///   - **Target**   = position + forward (unit distance — the
    ///     renderer only needs a look-at direction; magnitude is
    ///     irrelevant past normalization).
    ///
    /// Degenerate inputs (zero-scale transforms) fall through to a
    /// world-identity camera via `look_at`'s normalize-or-zero semantics
    /// rather than panicking.
    pub fn primary_gameplay_camera(&self) -> Option<RenderCamera> {
        let (camera, world_transform) = self.world.primary_camera()?;

        // Decompose: translation column + extract orientation basis.
        let position = world_transform.col(3).truncate();
        // `Mat4::to_scale_rotation_translation` would work, but we
        // only need forward/up — pulling columns straight out of the
        // matrix dodges the square root in scale extraction.
        let right = world_transform.col(0).truncate();
        let up_raw = world_transform.col(1).truncate();
        let forward_neg_z = world_transform.col(2).truncate();

        // Normalize — scale on the camera entity would otherwise
        // creep into the view matrix and warp the projection.
        let forward = (-forward_neg_z).normalize_or_zero();
        let up = up_raw.normalize_or_zero();
        // `right` kept alive in case future phases need camera-local
        // axis for frustum viz. Silence the unused-variable lint.
        let _ = right;

        // Fall back to world Y if the rotation somehow produced a
        // near-zero up vector (scale = 0 on the camera entity).
        let up = if up.length_squared() < 1e-6 {
            Vec3::Y
        } else {
            up
        };
        let forward = if forward.length_squared() < 1e-6 {
            Vec3::NEG_Z
        } else {
            forward
        };

        Some(RenderCamera {
            position,
            target: position + forward,
            up,
            fov_y_rad: camera.fov_y_rad,
            near: camera.near,
            far: camera.far,
        })
    }

    /// Transform of the runtime entity backing the given scene id, if
    /// one exists. Used by the I-9 gizmo to resolve the drag pivot.
    pub fn selected_world_transform(&self, scene_id: SceneId) -> Option<Transform> {
        let entity = self.scene_mapping.entity(scene_id)?;
        self.world
            .ecs()
            .get::<&Transform>(entity)
            .ok()
            .map(|t| *t)
    }

    pub fn world_entity_count(&self) -> usize {
        self.world_entities.len()
    }

    pub fn render_scene(
        &mut self,
        scene: &SceneDocument,
        width: u32,
        height: u32,
        delta_seconds: f32,
    ) -> ViewportSnapshot {
        self.engine.sync_scene(scene);
        self.engine.tick_headless(delta_seconds);
        let output = self.engine.render_to_texture(RenderRequest { width, height });
        let snapshot = ViewportSnapshot {
            attachment_id: output.color_attachment_id,
            width,
            height,
            entity_slots: collect_scene_ids(scene),
        };
        self.last_snapshot = Some(snapshot.clone());
        snapshot
    }

    /// Legacy pick entry point — still used by tests that exercise the
    /// MockEngine. I-8 prefers [`Self::pick_in_viewport`] which goes
    /// through the real ECS ray-AABB test.
    pub fn pick(&self, x: f32, y: f32) -> Option<SceneId> {
        self.engine.pick_entity(PickRequest { x, y })
    }

    /// Pick the entity under the supplied pixel. The caller provides
    /// the inverse view-projection (same matrix the renderer used
    /// this frame) + viewport size in pixels. Returns the authoring
    /// `SceneId` so the existing selection plumbing works unchanged.
    pub fn pick_in_viewport(
        &self,
        pixel_xy: [f32; 2],
        viewport_wh: [f32; 2],
        inv_view_proj: Mat4,
    ) -> Option<SceneId> {
        let ray = Ray::from_viewport_pixel(pixel_xy, viewport_wh, inv_view_proj);
        let entity = picking::pick_entity(&self.world, &ray)?;
        self.scene_mapping.scene_id(entity)
    }

    pub fn last_snapshot(&self) -> Option<&ViewportSnapshot> {
        self.last_snapshot.as_ref()
    }
}

/// Walk a scene subtree collecting ids of entities whose name starts
/// with `Spin` — demo hook that becomes obsolete once I-6 introduces
/// authored components with real derived fields.
/// Per-axis color used by every mode of the gizmo. R/G/B = X/Y/Z is
/// near-universal in DCC tools so users don't have to learn a new
/// convention. The uniform-scale handle gets its own neutral tint so
/// it doesn't compete visually with an axis handle.
const GIZMO_X_COLOR: [f32; 4] = [1.00, 0.25, 0.25, 1.0];
const GIZMO_Y_COLOR: [f32; 4] = [0.25, 0.95, 0.35, 1.0];
const GIZMO_Z_COLOR: [f32; 4] = [0.30, 0.50, 1.00, 1.0];
const GIZMO_UNIFORM_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];

/// Number of cube segments per rotate ring. 24 is the point where a
/// ring visually reads as a smooth circle at editor-typical camera
/// distances (1–10m) without inflating the draw-call count past
/// negligible (72 extra cubes when a rotate-enabled entity is
/// selected; still one-draw-per-cube-instance under the current
/// CubeRenderer).
const ROTATE_RING_SEGMENTS: usize = 24;

fn axis_color(axis: GizmoAxis) -> [f32; 4] {
    match axis {
        GizmoAxis::X => GIZMO_X_COLOR,
        GizmoAxis::Y => GIZMO_Y_COLOR,
        GizmoAxis::Z => GIZMO_Z_COLOR,
    }
}

/// I-34 translate: the legacy I-9 layout — three axis-color cubes at
/// the tips of the gizmo arms.
fn push_translate_handles(
    out: &mut Vec<RenderEntity>,
    entity: Entity,
    pivot: Vec3,
    layout: &GizmoLayout,
) {
    for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
        let center = pivot + axis.direction() * layout.arm_length;
        let model = Mat4::from_scale_rotation_translation(
            Vec3::splat(layout.handle_size),
            Quat::IDENTITY,
            center,
        );
        out.push(RenderEntity {
            entity,
            model,
            mesh: MeshHandle::UNIT_CUBE,
            albedo: axis_color(axis),
            albedo_texture: TextureHandle::UNIT_WHITE,
        });
    }
}

/// I-34 rotate: three circular rings around the pivot, one per axis,
/// each approximated by `ROTATE_RING_SEGMENTS` small cubes. Every
/// segment shares the axis color so the ring reads as a single
/// colored arc even when it partially disappears behind geometry.
fn push_rotate_handles(
    out: &mut Vec<RenderEntity>,
    entity: Entity,
    pivot: Vec3,
    layout: &GizmoLayout,
) {
    // Segment cubes are smaller than the tip cubes so the ring doesn't
    // read as a string of beads — just a thin discontinuous line that
    // the eye fills in.
    let seg_size = layout.handle_size * 0.5;
    for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
        let (u, v) = axis.plane_basis();
        let color = axis_color(axis);
        for i in 0..ROTATE_RING_SEGMENTS {
            let theta =
                i as f32 * std::f32::consts::TAU / ROTATE_RING_SEGMENTS as f32;
            let pos = pivot
                + u * (theta.cos() * layout.arm_length)
                + v * (theta.sin() * layout.arm_length);
            let model = Mat4::from_scale_rotation_translation(
                Vec3::splat(seg_size),
                Quat::IDENTITY,
                pos,
            );
            out.push(RenderEntity {
                entity,
                model,
                mesh: MeshHandle::UNIT_CUBE,
                albedo: color,
                albedo_texture: TextureHandle::UNIT_WHITE,
            });
        }
    }
}

/// I-34 scale: three axis-color tip cubes (slightly oversized vs
/// translate so the user can tell the modes apart at a glance) plus
/// a neutral uniform-scale cube at the pivot.
fn push_scale_handles(
    out: &mut Vec<RenderEntity>,
    entity: Entity,
    pivot: Vec3,
    layout: &GizmoLayout,
) {
    // Axis tips — a touch larger so "scale" looks chunkier than
    // "translate arrows".
    let tip_size = layout.handle_size * 1.2;
    for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
        let center = pivot + axis.direction() * layout.arm_length;
        let model = Mat4::from_scale_rotation_translation(
            Vec3::splat(tip_size),
            Quat::IDENTITY,
            center,
        );
        out.push(RenderEntity {
            entity,
            model,
            mesh: MeshHandle::UNIT_CUBE,
            albedo: axis_color(axis),
            albedo_texture: TextureHandle::UNIT_WHITE,
        });
    }
    // Uniform handle at the pivot. Its hit-test half-extent is
    // `handle_size * 0.4` (see `pick_scale_handle`); the rendered
    // cube uses the full `handle_size * 0.8` visual diameter so it's
    // bigger than its hit-box — feels clickable, harder to miss.
    let uniform = Mat4::from_scale_rotation_translation(
        Vec3::splat(layout.handle_size * 0.8),
        Quat::IDENTITY,
        pivot,
    );
    out.push(RenderEntity {
        entity,
        model: uniform,
        mesh: MeshHandle::UNIT_CUBE,
        albedo: GIZMO_UNIFORM_COLOR,
        albedo_texture: TextureHandle::UNIT_WHITE,
    });
}

fn collect_spin_names(entity: &engine::scene::SceneEntity) -> Vec<SceneId> {
    let mut out = Vec::new();
    if entity.name.starts_with("Spin") {
        out.push(entity.id);
    }
    for child in &entity.children {
        out.extend(collect_spin_names(child));
    }
    out
}

/// Resolve an authored `source` string (e.g. `"meshes/ship.gltf"`)
/// into an absolute filesystem path. Relative paths join against
/// `asset_root`; absolute paths are returned untouched so tests can
/// point at fixtures in `std::env::temp_dir()`.
fn resolve_asset_path(asset_root: &Path, source: &str) -> PathBuf {
    let candidate = PathBuf::from(source);
    if candidate.is_absolute() {
        candidate
    } else {
        asset_root.join(candidate)
    }
}

/// Read a `.gltf` / `.glb` file from disk and hand it to the core
/// importer. The resolver closure reads sidecar `.bin` buffers from
/// the same directory as the top-level file — the usual glTF
/// separate-files layout.
/// I-32: reasons an albedo-texture import can fail. Shaped like
/// `MeshImportError` — one variant per failure mode so the UI can
/// surface actionable errors (missing file vs. corrupt PNG vs. an
/// unsupported format) rather than a single opaque string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureImportError {
    /// `std::fs::read` failed — file doesn't exist, permission denied,
    /// etc. Carries the OS error message for log surfaces.
    ReadFailed(String),
    /// The `image` crate couldn't decode the file. Covers "unknown
    /// format", "corrupted PNG chunk", "unsupported color depth",
    /// etc. The inner string is the decoder's own message.
    DecodeFailed(String),
    /// The decoded image had zero pixels — treated as a hard failure
    /// so the renderer never registers a degenerate texture that
    /// would trip wgpu's `Extent3d::ZERO` assertion.
    Empty,
}

impl std::fmt::Display for TextureImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadFailed(msg) => write!(f, "read failed: {msg}"),
            Self::DecodeFailed(msg) => write!(f, "decode failed: {msg}"),
            Self::Empty => write!(f, "image had zero pixels"),
        }
    }
}

impl std::error::Error for TextureImportError {}

/// Read a PNG/JPEG/TGA/etc. off disk and decode it into row-major
/// RGBA8. Uses the `image` crate (already in our dependency tree via
/// `eframe`) so we don't pull in a second decoder. The sRGB transfer
/// function lives on the GPU texture format (`Rgba8UnormSrgb`), so no
/// gamma conversion is performed here — the decoded bytes are the
/// same linear/sRGB layout the author wrote.
fn load_texture_from_disk(path: &Path) -> Result<(u32, u32, Vec<u8>), TextureImportError> {
    let bytes = std::fs::read(path)
        .map_err(|e| TextureImportError::ReadFailed(format!("{}: {}", path.display(), e)))?;
    let dynamic = image::load_from_memory(&bytes)
        .map_err(|e| TextureImportError::DecodeFailed(e.to_string()))?;
    let rgba = dynamic.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err(TextureImportError::Empty);
    }
    Ok((w, h, rgba.into_raw()))
}

fn load_gltf_from_disk(path: &Path) -> Result<Vec<MeshData>, MeshImportError> {
    let bytes = std::fs::read(path).map_err(|e| {
        MeshImportError::GltfParse(format!(
            "reading {}: {}",
            path.display(),
            e
        ))
    })?;
    let parent = path.parent().map(Path::to_path_buf);
    gltf_import::import_from_slice(&bytes, move |uri| {
        let sidecar = match parent.as_ref() {
            Some(dir) => dir.join(uri),
            None => PathBuf::from(uri),
        };
        std::fs::read(sidecar).ok()
    })
}

fn collect_scene_ids(scene: &SceneDocument) -> Vec<SceneId> {
    let mut slots = Vec::new();
    for entity in &scene.root_entities {
        collect_entity_ids(entity, &mut slots);
    }
    slots
}

fn collect_entity_ids(entity: &engine::scene::SceneEntity, slots: &mut Vec<SceneId>) {
    slots.push(entity.id);
    for child in &entity.children {
        collect_entity_ids(child, slots);
    }
}

#[cfg(test)]
mod tests {
    use engine::scene::{ComponentData, PrimitiveValue, SceneDocument, SceneEntity, SceneId};

    use super::ViewportBridge;

    #[test]
    fn bridge_spawns_three_cubes_in_runtime_world() {
        let bridge = super::ViewportBridge::new();
        // I-4 starter tableau: center, left, right.
        assert_eq!(bridge.world_entity_count(), 3);
        assert_eq!(bridge.render_snapshot().len(), 3);
    }

    #[test]
    fn tick_world_rotates_spin_entities() {
        use engine::world::Transform;

        let mut bridge = super::ViewportBridge::new();
        // Capture pre-tick rotations.
        let pre: Vec<_> = bridge
            .world()
            .ecs()
            .query::<&Transform>()
            .iter()
            .map(|(e, t)| (e, t.rotation))
            .collect();

        bridge.tick_world(1.0 / 60.0);

        let post: Vec<_> = bridge
            .world()
            .ecs()
            .query::<&Transform>()
            .iter()
            .map(|(e, t)| (e, t.rotation))
            .collect();

        // At least one entity's rotation moved (Spin components are
        // attached to the spinning cubes only).
        let mut any_changed = false;
        for (entity, post_rot) in &post {
            let pre_rot = pre.iter().find(|(pe, _)| pe == entity).unwrap().1;
            if (post_rot.x - pre_rot.x).abs() > 1e-6
                || (post_rot.y - pre_rot.y).abs() > 1e-6
                || (post_rot.z - pre_rot.z).abs() > 1e-6
                || (post_rot.w - pre_rot.w).abs() > 1e-6
            {
                any_changed = true;
            }
        }
        assert!(any_changed, "tick_world should rotate Spin entities");
    }

    #[test]
    fn primary_gameplay_camera_reflects_scene_camera_entity() {
        // I-25: a scene with a Camera component must surface through
        // the bridge's `primary_gameplay_camera()` helper, with the
        // authored transform and intrinsics composed into a
        // render-ready `Camera`.
        let doc = SceneDocument::new("PlayTest").with_root(
            SceneEntity::new(SceneId::new(1), "Play Camera")
                .with_component(
                    ComponentData::new("Transform")
                        .with_field("x", PrimitiveValue::F64(0.0))
                        .with_field("y", PrimitiveValue::F64(1.5))
                        .with_field("z", PrimitiveValue::F64(-7.0)),
                )
                .with_component(
                    ComponentData::new("Camera")
                        .with_field("fov", PrimitiveValue::F64(72.0))
                        .with_field("is_primary", PrimitiveValue::Bool(true)),
                ),
        );
        let mut bridge = super::ViewportBridge::new();
        bridge.rebuild_world_from_scene(&doc);

        let camera = bridge
            .primary_gameplay_camera()
            .expect("scene with a Camera component should produce an override");
        // Authored y=1.5, z=-7 — position column lands here.
        assert!((camera.position.y - 1.5).abs() < 1e-5);
        assert!((camera.position.z + 7.0).abs() < 1e-5);
        // FOV 72° → radians.
        assert!((camera.fov_y_rad - 72f32.to_radians()).abs() < 1e-4);
    }

    #[test]
    fn primary_gameplay_camera_is_none_when_scene_has_no_camera_component() {
        let doc = SceneDocument::new("NoCam").with_root(
            SceneEntity::new(SceneId::new(1), "Cube").with_component(
                ComponentData::new("Mesh")
                    .with_field("primitive", PrimitiveValue::String("cube".into())),
            ),
        );
        let mut bridge = super::ViewportBridge::new();
        bridge.rebuild_world_from_scene(&doc);
        // Edit mode always falls back to the orbit camera; the bridge
        // simply reports `None` so the caller knows no override exists.
        assert!(bridge.primary_gameplay_camera().is_none());
    }

    #[test]
    fn split_render_snapshot_routes_cube_and_imported_mesh_entries() {
        // I-27: entities spawned from `Mesh { primitive: "cube" }`
        // must end up in the cube list; entities spawned from
        // `Mesh { source: "x.gltf" }` must end up in the mesh draws
        // list with a non-zero asset id. Gizmo (None selected) skipped
        // so the test focuses on the raw split.
        let doc = SceneDocument::new("Mixed")
            .with_root(
                SceneEntity::new(SceneId::new(1), "PrimitiveCube")
                    .with_component(ComponentData::new("Transform"))
                    .with_component(
                        ComponentData::new("Mesh")
                            .with_field("primitive", PrimitiveValue::String("cube".into())),
                    ),
            )
            .with_root(
                SceneEntity::new(SceneId::new(2), "ImportedMesh")
                    .with_component(ComponentData::new("Transform"))
                    .with_component(
                        ComponentData::new("Mesh")
                            .with_field("source", PrimitiveValue::String("x.gltf".into())),
                    ),
            );
        let mut bridge = super::ViewportBridge::new();
        bridge.rebuild_world_from_scene(&doc);

        let (cubes, draws) = bridge.split_render_snapshot(None);
        assert_eq!(cubes.len(), 1, "one cube entity");
        assert_eq!(draws.len(), 1, "one imported mesh entity");
        assert_ne!(
            draws[0].mesh.0, 0,
            "imported mesh must not share the reserved UNIT_CUBE id",
        );
    }

    #[test]
    fn gizmo_snapshot_per_mode_yields_distinct_handle_counts() {
        // I-34: the per-mode gizmo painter emits
        //   * Translate → 3 tip cubes
        //   * Rotate    → 3 × ROTATE_RING_SEGMENTS (= 72) ring cubes
        //   * Scale     → 3 tip cubes + 1 uniform cube = 4
        // Compare against the base (no-selection) snapshot so the test
        // survives future additions to the starter tableau.
        use engine::picking::GizmoMode;
        let bridge = super::ViewportBridge::new();
        let baseline = bridge.render_snapshot_with_gizmo_mode(None, GizmoMode::Translate);
        let base_n = baseline.len();

        // Grab a scene id of an entity the bridge actually spawned in
        // its starter tableau. `render_snapshot` returns one
        // `RenderEntity` per runtime cube, but we need a `SceneId` to
        // look up — the starter has no SceneDocument, so fall back to
        // spawning via a tiny doc.
        let doc = SceneDocument::new("Gizmo")
            .with_root(
                SceneEntity::new(SceneId::new(1), "Cube")
                    .with_component(ComponentData::new("Transform"))
                    .with_component(
                        ComponentData::new("Mesh")
                            .with_field("primitive", PrimitiveValue::String("cube".into())),
                    ),
            );
        let mut bridge = super::ViewportBridge::new();
        bridge.rebuild_world_from_scene(&doc);
        let sel = Some(SceneId::new(1));

        let translate = bridge.render_snapshot_with_gizmo_mode(sel, GizmoMode::Translate);
        let rotate = bridge.render_snapshot_with_gizmo_mode(sel, GizmoMode::Rotate);
        let scale = bridge.render_snapshot_with_gizmo_mode(sel, GizmoMode::Scale);
        let base_after_rebuild = bridge.render_snapshot_with_gizmo_mode(None, GizmoMode::Translate).len();
        // Sanity: the baseline is the scene snapshot without gizmo.
        assert!(
            base_after_rebuild >= base_n.saturating_sub(3),
            "baseline shouldn't shrink unexpectedly",
        );

        assert_eq!(
            translate.len() - base_after_rebuild,
            3,
            "translate mode adds 3 tip handles",
        );
        assert_eq!(
            rotate.len() - base_after_rebuild,
            3 * 24,
            "rotate mode adds 3 × 24 ring segments",
        );
        assert_eq!(
            scale.len() - base_after_rebuild,
            4,
            "scale mode adds 3 tip handles + 1 uniform handle",
        );
    }

    #[test]
    fn split_render_snapshot_for_mode_defaults_match_translate() {
        use engine::picking::GizmoMode;
        let bridge = super::ViewportBridge::new();
        let (cubes_default, _) = bridge.split_render_snapshot(None);
        let (cubes_explicit, _) =
            bridge.split_render_snapshot_for_mode(None, GizmoMode::Translate);
        assert_eq!(cubes_default.len(), cubes_explicit.len());
    }

    #[test]
    fn import_mesh_sources_queues_uploads_and_skips_second_pass() {
        // I-27: pointing the bridge at a directory with a glTF file
        // must yield one PendingMeshUpload. Re-running the import a
        // second time should be a no-op (handle already tracked) —
        // steady-state frames need to avoid re-reading disk.
        use std::time::{SystemTime, UNIX_EPOCH};

        // Stand up a throwaway project on disk so we can aim the
        // bridge at a real asset_root.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rustforge_import_test_{nanos}"));
        let project = crate::project::ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let doc = SceneDocument::new("WithImport").with_root(
            SceneEntity::new(SceneId::new(1), "Tetra")
                .with_component(ComponentData::new("Transform"))
                .with_component(
                    ComponentData::new("Mesh").with_field(
                        "source",
                        PrimitiveValue::String("meshes/tetrahedron.gltf".into()),
                    ),
                ),
        );
        let mut bridge = super::ViewportBridge::new();
        bridge.rebuild_world_from_scene(&doc);

        let asset_root = project.root.join("assets");
        let first = bridge.import_mesh_sources_from_disk(&asset_root);
        assert_eq!(first.len(), 1, "one asset path processed");
        assert!(matches!(first[0].1, Ok(_)), "import must succeed");
        let uploads = bridge.take_pending_mesh_uploads();
        assert_eq!(uploads.len(), 1);
        assert!(uploads[0].positions.len() >= 3, "real geometry");

        // Second pass: same scene, already-uploaded handle — must
        // report zero work.
        let second = bridge.import_mesh_sources_from_disk(&asset_root);
        assert!(
            second.is_empty(),
            "uploaded handle should be cached; got {second:?}",
        );
        assert!(bridge.take_pending_mesh_uploads().is_empty());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn bridge_renders_and_picks_from_mock_engine() {
        let mut bridge = ViewportBridge::new();
        let scene = SceneDocument::new("Sandbox")
            .with_root(
                SceneEntity::new(SceneId::new(1), "Camera").with_component(
                    ComponentData::new("Transform")
                        .with_field("x", PrimitiveValue::F64(0.0)),
                ),
            )
            .with_root(SceneEntity::new(SceneId::new(2), "Player"));

        let snapshot = bridge.render_scene(&scene, 1200, 720, 1.0 / 60.0);

        assert_eq!(snapshot.attachment_id, 1);
        assert_eq!(bridge.pick(100.0, 10.0), Some(SceneId::new(1)));
        assert_eq!(bridge.pick(900.0, 10.0), Some(SceneId::new(2)));
    }
}
