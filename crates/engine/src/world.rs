//! Runtime ECS world — introduced in I-4.
//!
//! The authoring format (`scene::SceneDocument`) is a human-readable,
//! stable-id, RON-backed description of a scene. The *runtime* world is
//! a live `hecs` ECS: transient entity handles, tightly packed
//! archetypes, fast queries for render + simulation.
//!
//! Both representations coexist:
//!
//!  - Editor authoring ↔ `SceneDocument`  (save/load, undo stack, UI).
//!  - Per-frame systems ↔ `World`          (render, physics, scripts).
//!
//! I-5 will introduce the bridge `SceneDocument → World`. For I-4 we
//! focus on the runtime side: give every renderable entity a
//! `Transform` + `MeshHandle` so the viewport stops hardcoding a single
//! cube and starts iterating the ECS.
//!
//! We deliberately keep handles opaque `u64`s rather than typed
//! asset ids for now — asset pipeline lands in a later phase and the
//! renderer only needs stable comparable ids to dispatch pipelines.
//! Once the asset server arrives (Arc 2) these handles become thin
//! wrappers over `AssetId`.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};

pub use hecs::{Entity, World as EcsWorld};

use crate::scene::{ComponentData, PrimitiveValue, SceneDocument, SceneEntity, SceneId};

/// Opaque mesh asset reference. `0` = the built-in unit cube used by
/// the I-3 `CubeRenderer`. Real asset ids arrive with the asset
/// pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshHandle(pub u64);

impl MeshHandle {
    /// The built-in unit cube baked into `render`. Every
    /// entity spawned before the asset pipeline exists uses this.
    pub const UNIT_CUBE: Self = Self(0);
}

/// Opaque material reference. `0` = the default per-vertex color
/// material used by the I-3 cube shader. Parity with `MeshHandle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaterialHandle(pub u64);

impl MaterialHandle {
    /// The default vertex-color material baked into the cube shader.
    pub const DEFAULT: Self = Self(0);
}

/// I-32: authoring-side handle for a GPU-resident albedo texture.
///
/// `0` is reserved for the 1×1 opaque-white texture every
/// `TextureRegistry` seeds on startup — entities that don't reference
/// a texture round-trip through this handle and sample white at draw
/// time, which multiplies by 1.0 and yields the pre-I-32 shading.
///
/// The payload is a deterministic hash of the authoring source path,
/// matching how `MeshHandle` is derived. Two scene authors writing
/// `"textures/wood.png"` produce the same handle in both runs, so
/// registry uploads reuse rather than duplicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureHandle(pub u64);

impl TextureHandle {
    /// The default 1×1 opaque-white texture — matches
    /// `render::TextureAssetId::DEFAULT_WHITE`.
    pub const UNIT_WHITE: Self = Self(0);
}

/// Deterministic path → `TextureHandle` hash. Mirrors the shape of
/// `mesh_handle_for_source`: SipHash for cross-run stability, and
/// `raw | 1` so we never collide with the reserved `UNIT_WHITE` slot.
pub fn texture_handle_for_source(source: &str) -> TextureHandle {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    let raw = hasher.finish();
    TextureHandle(raw | 1)
}

/// I-32: path reference to an albedo texture file, same shape as
/// [`MeshSource`]. The editor bridge reads `path` off disk, decodes
/// it, and pushes the pixel data into the render-side
/// `TextureRegistry` at `texture_handle_for_source(path).0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureSource {
    pub path: String,
}

/// Directional light component (I-13). Attached to ECS entities that
/// should contribute world lighting. The runtime renderer picks the
/// first `DirectionalLight` it finds per frame — multi-light support
/// arrives alongside shadow maps in a later phase.
///
/// Authoring-side, this comes from a `Light` scene component with
/// numeric fields:
///   - `direction_x/y/z`: direction from surface toward the light
///     (renderer normalizes). Default = (0.5, 0.8, 0.3).
///   - `color_r/g/b`: 0..1 RGB. Default = warm white.
///   - `intensity`: multiplier on N·L. Default = 1.0.
///   - `ambient`: flat additive term in [0, 1]. Default = 0.18.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DirectionalLight {
    pub direction: Vec3,
    pub color:     [f32; 3],
    pub intensity: f32,
    pub ambient:   f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            direction: Vec3::new(0.5, 0.8, 0.3),
            color:     [1.0, 0.97, 0.92],
            intensity: 1.0,
            ambient:   0.18,
        }
    }
}

/// I-26: gameplay mover. Attached to entities the player is expected
/// to drive during Play mode. The mover system reads an `Input` and
/// translates the entity's `Transform` by `speed * dt` along the
/// input's WASD axes (X from A/D, Z from W/S, in world space).
///
/// Authoring-side, comes from a scene component with type_name
/// `"Mover"` and an optional `speed` field (default 3.0 world units
/// per second).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mover {
    pub speed: f32,
}

impl Default for Mover {
    fn default() -> Self {
        Self { speed: 3.0 }
    }
}

/// I-35: Rhai script component.
///
/// Attached to any entity that wants to run gameplay logic via the
/// `ScriptHost`. `source` is a project-relative path to a `.rhai`
/// file — the host resolves it against the current project root each
/// tick, reads the file if its mtime has changed (giving hot-reload
/// on save for free), and evaluates it in a per-entity scope that
/// exposes the entity's transform fields as mutable variables plus
/// a handful of read-only constants (`TIME`, `DT`, `KEY_*`, ...).
///
/// Kept deliberately thin: just the source path. The compiled AST
/// and mtime live in the host's cache so scene-doc serialization
/// stays RON-friendly (strings only) and the component stays
/// trivially `Clone`/`PartialEq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub source: String,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            source: String::new(),
        }
    }
}

/// I-28: AABB collider. The box is centered on the entity's world-space
/// `Transform::translation` and extends `±half_extents` along each axis.
///
/// We intentionally stay AABB-only (no rotated boxes, no capsules, no
/// meshes) for the first physics landing. Axis-aligned overlap tests
/// reduce to three interval overlaps — easy to get right, easy to test
/// — and the MTV resolution has a closed form that falls out cleanly.
/// Rotated colliders and convex hulls can replace this later without
/// changing the integrator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Collider {
    pub half_extents: Vec3,
}

impl Default for Collider {
    fn default() -> Self {
        // Unit cube — matches the baked-in `MeshHandle::UNIT_CUBE`
        // geometry so a default-collider on a default-cube entity
        // behaves the way designers intuitively expect.
        Self { half_extents: Vec3::splat(0.5) }
    }
}

/// I-28: dynamic rigid body.
///
/// Entities with both `RigidBody` and `Collider` are integrated by
/// `World::tick_physics`: gravity accumulates into `velocity`, velocity
/// is applied to `Transform::translation`, and overlaps against static
/// colliders (entities with `Collider` but **no** `RigidBody`) resolve
/// by pushing the body out along the axis of least penetration.
///
/// Deliberate simplifications:
///   - No mass, no impulses — `velocity` is authoritative and direct.
///     Collision response zeros the velocity component along the
///     contact axis rather than bouncing (restitution = 0).
///   - No dynamic-vs-dynamic resolution yet; two moving bodies can
///     currently tunnel into each other. Static-vs-dynamic covers the
///     95% of platformer / top-down cases at this stage.
///   - `gravity_scale` is a multiplier on the world's gravity vector
///     (see [`World::gravity`]); 0.0 makes an entity float.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigidBody {
    pub velocity:      Vec3,
    pub gravity_scale: f32,
    /// If true, the Mover system writes to `velocity.x/z` instead of
    /// translating `Transform` directly. Lets WASD input compose with
    /// gravity / collision response instead of teleporting through
    /// floors. Default `true` so attaching a RigidBody to an existing
    /// Mover entity "just works".
    pub mover_drives_velocity: bool,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            velocity:              Vec3::ZERO,
            gravity_scale:         1.0,
            mover_drives_velocity: true,
        }
    }
}

/// I-31: per-entity material parameters.
///
/// At this stage the material system is deliberately minimal: one
/// RGBA `albedo` color, multiplied into the fragment shader's base
/// color. It gives designers a way to tint a mesh without authoring
/// new vertex colors, and it gives the renderer a single uniform-slot
/// extension (no new bind group, no new buffer) that can be widened
/// later when textures + PBR channels arrive in I-32.
///
/// Authoring-side, this comes from a scene component with type_name
/// `"Material"`. Recognized fields (all optional, defaults = white):
///   - `color_r`, `color_g`, `color_b`, `color_a` — floats in [0, 1].
///   - Shorthand `color` field that accepts a 4-element array if the
///     RON author prefers one-line syntax (handled at extract time).
///
/// Entities without a `Material` component render with identity
/// albedo (`DEFAULT_ALBEDO`), matching the pre-I-31 behavior byte-
/// for-byte. The scene loader never inserts a default-valued
/// `Material`; a missing component and a white one are intentionally
/// distinguishable so later material-asset references can replace
/// "no material override" with "explicit default".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// Linear-space RGBA tint. Stored as `[f32; 4]` to match the
    /// renderer's uniform layout directly — no conversion on the
    /// hot path.
    pub albedo: [f32; 4],
    /// I-32: optional albedo texture handle. `None` means "no texture
    /// authored" — at draw time the renderer substitutes
    /// `TextureHandle::UNIT_WHITE`, which samples to 1.0 and leaves
    /// the tint + vertex colors unchanged. `Some(h)` resolves through
    /// the render-side `TextureRegistry` to a `BindGroup`; missing
    /// entries fall back to UNIT_WHITE rather than panicking (so a
    /// pending import shows up as untextured instead of crashing).
    pub albedo_texture: Option<TextureHandle>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            // Identity multiplier — same as `DEFAULT_ALBEDO` in the
            // render crate. Kept numerically identical so a round-trip
            // through the default path leaves uniforms bit-exact.
            albedo:         [1.0, 1.0, 1.0, 1.0],
            albedo_texture: None,
        }
    }
}

/// Gameplay camera component (I-25). Attached to ECS entities that
/// represent in-game cameras — distinct from the editor's authoring
/// OrbitCamera which lives in the UI shell, not the ECS.
///
/// During Play mode the viewport picks the primary gameplay camera
/// (highest priority `is_primary` flag; falls back to the first
/// camera found) and renders from its world-space Transform. During
/// Edit mode the viewport keeps using the OrbitCamera so designers
/// can fly around freely regardless of where gameplay cameras live.
///
/// Authoring-side, this comes from a scene component with type_name
/// `"Camera"`. Recognized fields (all optional):
///   - `fov`        — vertical FOV in **degrees**. Default 60.
///   - `near`       — near clip plane. Default 0.1.
///   - `far`        — far clip plane. Default 500.
///   - `is_primary` — bool, default `true`. Only the first primary
///                    camera per scene renders; subsequent primaries
///                    demote to non-primary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub fov_y_rad: f32,
    pub near:      f32,
    pub far:       f32,
    pub is_primary: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y_rad:  60f32.to_radians(),
            near:       0.1,
            far:        500.0,
            is_primary: true,
        }
    }
}

/// Parent pointer for scene hierarchy (I-14). Child transforms are
/// authored in parent-local space; `compute_world_transforms` walks
/// the parent chain and composes the final model matrix the renderer
/// uses.
///
/// Stored as a component (not a `hecs::Relation`) because hecs doesn't
/// have native relations yet — a plain component keeps the query cost
/// predictable and lets us evict/change parents without archetype
/// churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// SRT transform in local (or world, if no parent) space.
///
/// We store the decomposed form so systems that animate rotation/scale
/// independently don't have to re-orthonormalize a matrix every frame.
/// `model_matrix()` composes them in the standard T * R * S order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation:    Quat,
    pub scale:       Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation:    Quat::IDENTITY,
        scale:       Vec3::ONE,
    };

    pub fn from_translation(translation: Vec3) -> Self {
        Self { translation, ..Self::IDENTITY }
    }

    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// Column-major 4x4 model matrix consumable by the cube shader.
    pub fn model_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Owning wrapper around a `hecs::World`.
///
/// Wrapping — rather than re-exporting `hecs::World` directly — gives
/// us a stable surface for the rest of the engine: future phases can
/// add per-frame dirty tracking, change detection, or system scheduling
/// without churning every call site.
pub struct World {
    ecs: EcsWorld,
    /// I-28: world-space gravity acceleration applied to every
    /// `RigidBody` each tick, scaled by the body's `gravity_scale`.
    /// Default is Earth-ish Y-down (-9.81 m/s²). The editor doesn't
    /// expose this yet — gameplay code tweaks it at spawn for moon /
    /// zero-g scenes if needed.
    gravity: Vec3,
}

impl Default for World {
    fn default() -> Self {
        Self {
            ecs:     EcsWorld::default(),
            gravity: Vec3::new(0.0, -9.81, 0.0),
        }
    }
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// I-28: configure the world-space gravity vector. Applied every
    /// `tick_physics` to each `RigidBody` scaled by `gravity_scale`.
    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.gravity = gravity;
    }

    pub fn gravity(&self) -> Vec3 {
        self.gravity
    }

    /// Direct access to the inner `hecs::World`. Kept `pub` so crates
    /// that need to layer additional behaviour (editor, renderer) can
    /// borrow queries/spawns without waiting for new thin wrappers.
    pub fn ecs(&self) -> &EcsWorld {
        &self.ecs
    }

    pub fn ecs_mut(&mut self) -> &mut EcsWorld {
        &mut self.ecs
    }

    /// Spawn a renderable cube entity. Convenience for pre-asset-server
    /// wiring — used by the editor to make I-4 visible.
    pub fn spawn_cube(&mut self, transform: Transform) -> Entity {
        self.ecs
            .spawn((transform, MeshHandle::UNIT_CUBE, MaterialHandle::DEFAULT))
    }

    /// Number of live entities. Thin wrapper for tests + diagnostics.
    pub fn len(&self) -> usize {
        self.ecs.len() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot the `(Transform, MeshHandle)` pairs the renderer needs
    /// this frame. Called on the main thread; the returned `Vec` is
    /// handed to the egui paint callback which runs later in the same
    /// frame with no access back to the ECS.
    ///
    /// I-14: model matrices are now *world-space* — each entity's
    /// local transform is multiplied by every `Parent` in the chain up
    /// to the root before shipping to the GPU. Depth-limited at 64 so
    /// a cycle in the parent graph can't hang the renderer (logged
    /// silently — a real cycle-detection pass arrives with the
    /// hierarchy inspector).
    pub fn collect_render_snapshot(&self) -> Vec<RenderEntity> {
        let worlds = self.compute_world_transforms();
        self.ecs
            .query::<(&MeshHandle,)>()
            .iter()
            .filter_map(|(entity, (mesh,))| {
                worlds.get(&entity).map(|model| {
                    // I-31/I-32: pick up the optional `Material`
                    // override once, pulling both the tint and the
                    // albedo texture handle out of the same lookup.
                    // Missing component → white tint + UNIT_WHITE
                    // texture (bit-exact match to pre-I-31 output).
                    let (albedo, albedo_texture) = self
                        .ecs
                        .get::<&Material>(entity)
                        .map(|m| {
                            (
                                m.albedo,
                                m.albedo_texture.unwrap_or(TextureHandle::UNIT_WHITE),
                            )
                        })
                        .unwrap_or((
                            Material::default().albedo,
                            TextureHandle::UNIT_WHITE,
                        ));
                    RenderEntity {
                        entity,
                        model:          *model,
                        mesh:           *mesh,
                        albedo,
                        albedo_texture,
                    }
                })
            })
            .collect()
    }

    /// Resolve every renderable entity's world-space model matrix.
    /// Public so picking + gizmo code can share the same resolution
    /// (world-space AABBs keep translation consistent across the
    /// pipeline).
    pub fn compute_world_transforms(&self) -> HashMap<Entity, Mat4> {
        let mut cache: HashMap<Entity, Mat4> = HashMap::new();
        for (entity, _) in self.ecs.query::<&Transform>().iter() {
            let _ = resolve_world_transform(&self.ecs, entity, &mut cache);
        }
        cache
    }
}

/// Per-frame render snapshot of one entity. Flat `Mat4` is what the
/// shader consumes, so we pre-compose here instead of on the render
/// thread.
#[derive(Debug, Clone, Copy)]
pub struct RenderEntity {
    pub entity: Entity,
    pub model:  Mat4,
    pub mesh:   MeshHandle,
    /// I-31: per-entity albedo tint, defaulting to white (identity
    /// multiplier) for entities without an explicit `Material`.
    /// Stored inline here rather than as `Option<Material>` so the
    /// render path has nothing to unwrap per draw — every entity
    /// contributes a value, even if that value is no-op.
    pub albedo: [f32; 4],
    /// I-32: per-entity albedo texture. `UNIT_WHITE` for entities
    /// with no texture, which the `TextureRegistry` resolves to the
    /// default 1×1 opaque-white bind group — no branching at draw
    /// time, consistent with how `albedo` is already flattened from
    /// `Option<Material>` into an inline value.
    pub albedo_texture: TextureHandle,
}

/// Bidirectional mapping between the authoring ids that live in a
/// `SceneDocument` and the transient `hecs::Entity` handles created
/// when the document is instantiated into a runtime `World`.
///
/// The editor keeps this around so "click in the 3D viewport" (which
/// returns a runtime entity) can surface the matching `SceneId` in the
/// hierarchy/inspector, and vice versa.
#[derive(Debug, Default, Clone)]
pub struct SceneInstantiation {
    pub scene_to_entity: HashMap<SceneId, Entity>,
    pub entity_to_scene: HashMap<Entity, SceneId>,
}

impl SceneInstantiation {
    pub fn scene_id(&self, entity: Entity) -> Option<SceneId> {
        self.entity_to_scene.get(&entity).copied()
    }

    pub fn entity(&self, scene_id: SceneId) -> Option<Entity> {
        self.scene_to_entity.get(&scene_id).copied()
    }

    pub fn len(&self) -> usize {
        self.scene_to_entity.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl World {
    /// Instantiate every entity in `doc` into this world, translating
    /// the scene's authoring components into runtime ECS components.
    ///
    /// Component mapping rules (I-5):
    ///   - `Transform { x, y, z }` → `Transform::from_translation`.
    ///     Optional `rot_x`, `rot_y`, `rot_z` (Euler radians) and
    ///     `scale` (uniform) extend the identity.
    ///   - `Mesh { primitive: "cube" }` → `MeshHandle::UNIT_CUBE` +
    ///     `MaterialHandle::DEFAULT`. Primitives other than `"cube"`
    ///     are currently skipped and logged as `None`; real mesh asset
    ///     resolution arrives with the asset pipeline.
    ///   - Everything else (Light, Camera, custom) is carried as
    ///     authoring-only metadata — no runtime component yet. This
    ///     keeps I-5 focused on the render path; later phases plug in
    ///     Light + Camera + scripts.
    ///
    /// Children are flattened: each `SceneEntity` becomes one ECS
    /// entity. Hierarchy propagation is a separate system added later.
    pub fn instantiate_scene(&mut self, doc: &SceneDocument) -> SceneInstantiation {
        let mut mapping = SceneInstantiation::default();
        for root in &doc.root_entities {
            self.spawn_scene_subtree(root, &mut mapping);
        }
        mapping
    }

    fn spawn_scene_subtree(
        &mut self,
        entity: &SceneEntity,
        mapping: &mut SceneInstantiation,
    ) {
        self.spawn_scene_subtree_with_parent(entity, None, mapping);
    }

    fn spawn_scene_subtree_with_parent(
        &mut self,
        entity: &SceneEntity,
        parent: Option<Entity>,
        mapping: &mut SceneInstantiation,
    ) {
        let transform = extract_transform(entity).unwrap_or(Transform::IDENTITY);
        let runtime_entity = match extract_mesh(entity) {
            Some(mesh) => self.ecs.spawn((transform, mesh, MaterialHandle::DEFAULT)),
            // Non-renderable entities (cameras, lights, empty groups)
            // still live in the ECS so systems can find them later.
            None => self.ecs.spawn((transform,)),
        };
        // I-27: if the Mesh component references an external asset by
        // `source`, attach a `MeshSource` so the editor bridge can find
        // every entity that still needs a GPU upload. `MeshHandle` alone
        // isn't enough — the hash-derived id lets the renderer look up a
        // resident asset, but the *path* is what the loader needs to
        // actually read bytes off disk.
        if let Some(source) = extract_mesh_source(entity) {
            let _ = self.ecs.insert_one(runtime_entity, source);
        }
        // I-13: attach a DirectionalLight if the scene entity carries
        // one. Layered on top of whatever transform/mesh was spawned so
        // a Light can share a transform with a mesh (useful for debug
        // "show light as cube" workflows).
        if let Some(light) = extract_directional_light(entity) {
            let _ = self.ecs.insert_one(runtime_entity, light);
        }
        // I-25: attach a Camera component for entities that author
        // a `Camera` scene component. The gameplay runtime reads this
        // to pick the play-mode POV; edit mode ignores it.
        if let Some(camera) = extract_camera(entity) {
            let _ = self.ecs.insert_one(runtime_entity, camera);
        }
        // I-26: attach Mover for entities that author one. The
        // gameplay tick will translate this entity's Transform along
        // WASD axes while Play mode is active.
        if let Some(mover) = extract_mover(entity) {
            let _ = self.ecs.insert_one(runtime_entity, mover);
        }
        // I-28: attach Collider + RigidBody when authored. A Collider
        // without RigidBody is a static obstacle (the ground, walls,
        // decorative geometry); the pair together makes a dynamic
        // body subject to gravity + collision response.
        if let Some(collider) = extract_collider(entity) {
            let _ = self.ecs.insert_one(runtime_entity, collider);
        }
        if let Some(rigid_body) = extract_rigid_body(entity) {
            let _ = self.ecs.insert_one(runtime_entity, rigid_body);
        }
        // I-35: attach Script if the scene component carries a source
        // path. The actual compile + eval lives in the `scripting`
        // module's `ScriptHost`; here we just record which entities
        // opt in.
        if let Some(script) = extract_script(entity) {
            let _ = self.ecs.insert_one(runtime_entity, script);
        }
        // I-29: attach AudioSource when authored. The editor-side
        // audio engine queries for these each tick and consumes the
        // `autoplay` entries once when Play mode enters.
        if let Some(audio_source) = crate::audio::extract_audio_source(entity) {
            let _ = self.ecs.insert_one(runtime_entity, audio_source);
        }
        // I-31: attach Material when authored. Missing = identity
        // albedo; a default-valued Material scene component is still
        // attached so later systems can tell "explicit default" from
        // "none".
        if let Some(material) = extract_material(entity) {
            let _ = self.ecs.insert_one(runtime_entity, material);
        }
        // I-32: attach TextureSource if the Material referenced an
        // `albedo_texture` path. The editor bridge walks every
        // (Material, TextureSource) pair on scene load, decodes the
        // PNG, and uploads into the GPU registry.
        if let Some(texture_source) = extract_texture_source(entity) {
            let _ = self.ecs.insert_one(runtime_entity, texture_source);
        }
        // I-14: record parent pointer so `compute_world_transforms`
        // can walk the chain.
        if let Some(parent_entity) = parent {
            let _ = self.ecs.insert_one(runtime_entity, Parent(parent_entity));
        }
        mapping
            .scene_to_entity
            .insert(entity.id, runtime_entity);
        mapping
            .entity_to_scene
            .insert(runtime_entity, entity.id);
        for child in &entity.children {
            self.spawn_scene_subtree_with_parent(child, Some(runtime_entity), mapping);
        }
    }

    /// Find the first `DirectionalLight` in the world, if any. Cheap —
    /// one archetype query per frame. Multi-light lands alongside
    /// shadow maps.
    pub fn primary_directional_light(&self) -> Option<DirectionalLight> {
        self.ecs
            .query::<&DirectionalLight>()
            .iter()
            .next()
            .map(|(_, light)| *light)
    }

    /// I-29: enumerate `AudioCommand::Play` entries for every
    /// AudioSource with `autoplay = true`. Called once by the editor
    /// when Play mode enters — the commands feed the audio engine's
    /// sink spawner and the `AudioSource` components are left intact
    /// so manual "play again" triggers from scripts can reuse them.
    ///
    /// Non-autoplay sources are ignored here; scripts (future) will
    /// push explicit `AudioCommand::Play` entries into a queue that
    /// the engine also drains.
    pub fn collect_autoplay_audio(&self) -> Vec<crate::audio::AudioCommand> {
        use crate::audio::{AudioCommand, AudioSource};
        self.ecs
            .query::<&AudioSource>()
            .iter()
            .filter(|(_, s)| s.autoplay)
            .map(|(_, s)| AudioCommand::Play {
                handle:  s.handle,
                path:    s.path.clone(),
                volume:  s.volume,
                pitch:   s.pitch,
                looping: s.looping,
            })
            .collect()
    }

    /// I-26 + I-28: advance gameplay systems for one tick using the
    /// supplied input. Called by the editor only while Play mode is
    /// active — Edit mode must not apply gameplay motion or the
    /// designer's authored transforms would drift before Save.
    ///
    /// Order within a tick:
    ///   1. **Mover** — consume input into either `RigidBody.velocity`
    ///      (if present and `mover_drives_velocity` is set) or direct
    ///      `Transform` translation (legacy input-only path).
    ///   2. **Physics** — gravity → velocity → position, with static
    ///      collider resolution. See [`Self::tick_physics`].
    ///
    /// Splitting Mover from Physics means gameplay scripts can compose:
    /// a "sprint" system can scale `velocity` between steps 1 and 2
    /// without fighting over Transform write ordering.
    pub fn tick_gameplay(&mut self, input: &crate::input::Input, dt: f32) {
        self.apply_mover_input(input, dt);
        self.tick_physics(dt);
    }

    fn apply_mover_input(&mut self, input: &crate::input::Input, dt: f32) {
        use crate::input::Key;
        let dx = input.axis(Key::A, Key::D);
        let dz = input.axis(Key::S, Key::W);
        let direction = Vec3::new(dx, 0.0, dz);
        // Path A: Mover + RigidBody → feed velocity. Gravity handles
        // y, so we overwrite only the horizontal components.
        for (_entity, (body, mover)) in self.ecs.query_mut::<(&mut RigidBody, &Mover)>() {
            if !body.mover_drives_velocity {
                continue;
            }
            let target = direction * mover.speed;
            body.velocity.x = target.x;
            body.velocity.z = target.z;
        }
        // Path B: Mover but no RigidBody → legacy direct translation.
        // Skipped when the input is zero (cheapest possible idle tick).
        if direction == Vec3::ZERO {
            return;
        }
        let mut legacy_targets: Vec<Entity> = Vec::new();
        for (entity, _) in self.ecs.query::<(&Transform, &Mover)>().without::<&RigidBody>().iter() {
            legacy_targets.push(entity);
        }
        for entity in legacy_targets {
            let Ok(mut transform) = self.ecs.get::<&mut Transform>(entity) else {
                continue;
            };
            let Ok(mover) = self.ecs.get::<&Mover>(entity) else {
                continue;
            };
            transform.translation += direction * mover.speed * dt;
        }
    }

    /// I-28: integrate rigid bodies + resolve collisions against
    /// static colliders for one frame of duration `dt`.
    ///
    /// Algorithm (semi-implicit Euler + positional correction):
    ///   1. For each dynamic body: `velocity += gravity * scale * dt`.
    ///   2. Apply `velocity * dt` to `Transform::translation`.
    ///   3. For each static collider overlapping the body's AABB at
    ///      the new position, resolve via minimum translation vector:
    ///      push along the single axis with the smallest overlap,
    ///      then zero the body's velocity along that axis so it
    ///      doesn't keep driving into the wall.
    ///
    /// Chosen complexity is O(dynamic × static). Broad-phase (BVH,
    /// spatial hash) replaces this when worlds grow past hundreds of
    /// colliders; for now the constant factor is small and gameplay
    /// scenes are tiny.
    pub fn tick_physics(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        // Snapshot static colliders up front. Collecting into a Vec
        // dodges the hecs borrow-checker dance: we need `&mut Transform`
        // on dynamic bodies later, which conflicts with an open query
        // over all Transforms.
        let static_colliders: Vec<(Vec3, Vec3)> = self
            .ecs
            .query::<(&Transform, &Collider)>()
            .without::<&RigidBody>()
            .iter()
            .map(|(_, (t, c))| (t.translation, c.half_extents))
            .collect();

        // Integrate gravity + velocity, collect target positions so
        // the next pass can resolve collisions without borrowing the
        // ECS simultaneously.
        let gravity = self.gravity;
        for (_entity, (transform, body, collider)) in self
            .ecs
            .query_mut::<(&mut Transform, &mut RigidBody, &Collider)>()
        {
            body.velocity += gravity * body.gravity_scale * dt;
            transform.translation += body.velocity * dt;

            // Resolve against every static collider. Iterating to a
            // fixed-point (re-testing after each resolution) would
            // be more correct for piles of tightly-packed walls, but
            // a single pass covers the common platformer cases.
            for (center, half_extents) in &static_colliders {
                if let Some(mtv) = resolve_aabb_overlap(
                    transform.translation,
                    collider.half_extents,
                    *center,
                    *half_extents,
                ) {
                    transform.translation += mtv;
                    // Zero the velocity component along the contact
                    // axis so the body rests instead of juddering.
                    if mtv.x.abs() > f32::EPSILON {
                        body.velocity.x = 0.0;
                    }
                    if mtv.y.abs() > f32::EPSILON {
                        body.velocity.y = 0.0;
                    }
                    if mtv.z.abs() > f32::EPSILON {
                        body.velocity.z = 0.0;
                    }
                }
            }
        }
    }

    /// I-25: pick the primary gameplay camera for this frame.
    ///
    /// Returns the camera's intrinsics plus its fully-resolved world
    /// transform (parent chains composed) so callers can build a
    /// view-projection without peeking at the ECS internals.
    ///
    /// Selection rules:
    ///   1. If any entity has `Camera { is_primary: true, .. }`, the
    ///      first such entity wins.
    ///   2. Otherwise fall back to the first entity with any `Camera`
    ///      component. This keeps single-camera scenes working even
    ///      if the designer forgot to flip `is_primary`.
    ///   3. Otherwise return `None` — the viewport falls back to the
    ///      editor orbit camera.
    pub fn primary_camera(&self) -> Option<(Camera, Mat4)> {
        let worlds = self.compute_world_transforms();
        let mut primary: Option<(Entity, Camera)> = None;
        let mut fallback: Option<(Entity, Camera)> = None;
        for (entity, camera) in self.ecs.query::<&Camera>().iter() {
            if camera.is_primary && primary.is_none() {
                primary = Some((entity, *camera));
            }
            if fallback.is_none() {
                fallback = Some((entity, *camera));
            }
        }
        let (entity, camera) = primary.or(fallback)?;
        let transform = worlds.get(&entity).copied().unwrap_or(Mat4::IDENTITY);
        Some((camera, transform))
    }

    /// Remove every entity from the world. Handy when reloading a
    /// scene — paired with `instantiate_scene` to swap worlds without
    /// constructing a fresh `World` (which would churn allocations).
    pub fn clear(&mut self) {
        self.ecs.clear();
    }

    /// Reapply every `SceneEntity`'s Transform to its already-mapped
    /// runtime entity *in place*. Unlike `clear` + `instantiate_scene`,
    /// this preserves transient components (Spin, future scripts, GPU
    /// caches) because the underlying `hecs::Entity` handles don't
    /// change.
    ///
    /// Used by the editor after any scene-mutating command (rename,
    /// nudge, component edit) so the runtime world mirrors authoring
    /// state without losing frame-to-frame animation progress.
    ///
    /// Entities that exist in the scene but not in the mapping (e.g.
    /// added after initial instantiation — a future feature) are
    /// ignored here; callers that expect topology changes should go
    /// through `rebuild_from_scene` instead.
    pub fn resync_transforms_from_scene(
        &mut self,
        doc: &SceneDocument,
        mapping: &SceneInstantiation,
    ) {
        for root in &doc.root_entities {
            self.resync_transform_subtree(root, mapping);
        }
    }

    fn resync_transform_subtree(
        &mut self,
        entity: &SceneEntity,
        mapping: &SceneInstantiation,
    ) {
        if let Some(runtime) = mapping.entity(entity.id) {
            let transform = extract_transform(entity).unwrap_or(Transform::IDENTITY);
            if let Ok(mut current) = self.ecs.get::<&mut Transform>(runtime) {
                *current = transform;
            }
        }
        for child in &entity.children {
            self.resync_transform_subtree(child, mapping);
        }
    }
}

fn extract_transform(entity: &SceneEntity) -> Option<Transform> {
    let component = entity.components.iter().find(|c| c.type_name == "Transform")?;
    let x = field_f32(&component.fields, "x");
    let y = field_f32(&component.fields, "y");
    let z = field_f32(&component.fields, "z");
    let rx = field_f32(&component.fields, "rot_x");
    let ry = field_f32(&component.fields, "rot_y");
    let rz = field_f32(&component.fields, "rot_z");
    // `scale` is optional uniform-only for now; vector scale arrives
    // with the reflection derive (I-6).
    let scale = match component.fields.get("scale") {
        Some(PrimitiveValue::F64(v)) => *v as f32,
        Some(PrimitiveValue::I64(v)) => *v as f32,
        _ => 1.0,
    };
    let rotation = Quat::from_rotation_x(rx) * Quat::from_rotation_y(ry) * Quat::from_rotation_z(rz);
    Some(Transform {
        translation: Vec3::new(x, y, z),
        rotation,
        scale: Vec3::splat(scale),
    })
}

/// Recursive, memoized world-transform resolver. Walks `Parent`
/// pointers up to the root, composing local model matrices on the way
/// back down. Depth-limited at [`MAX_HIERARCHY_DEPTH`] so a cycle in
/// the parent graph (should be unreachable given how we spawn, but…)
/// can't wedge the renderer in an infinite loop.
fn resolve_world_transform(
    ecs: &EcsWorld,
    entity: Entity,
    cache: &mut HashMap<Entity, Mat4>,
) -> Mat4 {
    if let Some(m) = cache.get(&entity) {
        return *m;
    }
    let local = ecs
        .get::<&Transform>(entity)
        .map(|t| t.model_matrix())
        .unwrap_or(Mat4::IDENTITY);
    let world = if let Ok(parent) = ecs.get::<&Parent>(entity) {
        if parent.0 == entity {
            // Self-parent — ignore to stay out of an infinite loop.
            local
        } else if cache.len() >= MAX_HIERARCHY_DEPTH {
            local
        } else {
            let parent_world = resolve_world_transform(ecs, parent.0, cache);
            parent_world * local
        }
    } else {
        local
    };
    cache.insert(entity, world);
    world
}

/// Safety valve against a parent-pointer cycle. Real scenes rarely go
/// deeper than 10; 64 gives us orders of magnitude of headroom.
const MAX_HIERARCHY_DEPTH: usize = 64;

fn extract_directional_light(entity: &SceneEntity) -> Option<DirectionalLight> {
    let component = entity.components.iter().find(|c| c.type_name == "Light")?;
    let mut light = DirectionalLight::default();
    // Each field is optional; absent fields fall back to the default
    // so minimally-authored lights still produce a sensible look.
    if let Some(v) = field_f32_opt(&component.fields, "direction_x") {
        light.direction.x = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "direction_y") {
        light.direction.y = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "direction_z") {
        light.direction.z = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "color_r") {
        light.color[0] = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "color_g") {
        light.color[1] = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "color_b") {
        light.color[2] = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "intensity") {
        // Scene convention (from I-5's `Key Light`) uses physical-ish
        // units in the thousands of lumens. Map into a shader-friendly
        // 0..~8 range so authored values look reasonable without
        // tonemapping.
        light.intensity = if v > 20.0 { v / 1000.0 } else { v };
    }
    if let Some(v) = field_f32_opt(&component.fields, "ambient") {
        light.ambient = v.clamp(0.0, 1.0);
    }
    Some(light)
}

fn field_f32_opt(
    fields: &std::collections::BTreeMap<String, PrimitiveValue>,
    key: &str,
) -> Option<f32> {
    match fields.get(key)? {
        PrimitiveValue::F64(v) => Some(*v as f32),
        PrimitiveValue::I64(v) => Some(*v as f32),
        _ => None,
    }
}

fn field_bool_opt(
    fields: &std::collections::BTreeMap<String, PrimitiveValue>,
    key: &str,
) -> Option<bool> {
    match fields.get(key)? {
        PrimitiveValue::Bool(v) => Some(*v),
        _ => None,
    }
}

/// I-26: parse a `Mover` scene component into a runtime `Mover`.
/// `speed` defaults to 3.0 world units / second when the field is
/// absent or non-numeric — matches `Mover::default`.
fn extract_mover(entity: &SceneEntity) -> Option<Mover> {
    let component = entity.components.iter().find(|c| c.type_name == "Mover")?;
    let mut mover = Mover::default();
    if let Some(speed) = field_f32_opt(&component.fields, "speed") {
        mover.speed = speed.max(0.0);
    }
    Some(mover)
}

/// I-28: parse a `Collider` scene component into a runtime `Collider`.
///
/// Three optional per-axis fields (`x`, `y`, `z`) override individual
/// half-extents. A single `half_extents` field (uniform) is accepted as
/// a shorthand — designers who want a 2×2×2 box just write
/// `half_extents: 1.0` instead of triplicating it. Missing fields fall
/// back to `Collider::default` (0.5 each → unit cube).
fn extract_collider(entity: &SceneEntity) -> Option<Collider> {
    let component = entity.components.iter().find(|c| c.type_name == "Collider")?;
    let mut collider = Collider::default();
    // Uniform shorthand first so per-axis overrides can still win.
    if let Some(uniform) = field_f32_opt(&component.fields, "half_extents") {
        let clamped = uniform.max(0.0);
        collider.half_extents = Vec3::splat(clamped);
    }
    if let Some(x) = field_f32_opt(&component.fields, "x") {
        collider.half_extents.x = x.max(0.0);
    }
    if let Some(y) = field_f32_opt(&component.fields, "y") {
        collider.half_extents.y = y.max(0.0);
    }
    if let Some(z) = field_f32_opt(&component.fields, "z") {
        collider.half_extents.z = z.max(0.0);
    }
    Some(collider)
}

/// I-31: parse a `Material` scene component into a runtime `Material`.
///
/// Present-but-empty components yield a white material (the default),
/// missing components yield `None` (the caller treats that as "no
/// material override" so systems can still tell explicit white apart
/// from omitted). Accepted fields, all optional:
///   - `color_r`, `color_g`, `color_b`, `color_a` — per-channel f32
///     in `[0, 1]`. Out-of-range values are clamped so shader math
///     stays well-behaved.
///
/// The scene primitive-value schema only models scalar/bool/string,
/// so RGBA lives across four fields rather than a tuple. If the
/// scene schema ever grows a sequence type, we can add a `color`
/// shorthand without breaking the per-channel form.
fn extract_material(entity: &SceneEntity) -> Option<Material> {
    let component = entity.components.iter().find(|c| c.type_name == "Material")?;
    let mut material = Material::default();
    let [mut r, mut g, mut b, mut a] = material.albedo;

    if let Some(v) = field_f32_opt(&component.fields, "color_r") {
        r = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "color_g") {
        g = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "color_b") {
        b = v;
    }
    if let Some(v) = field_f32_opt(&component.fields, "color_a") {
        a = v;
    }

    material.albedo = [
        r.clamp(0.0, 1.0),
        g.clamp(0.0, 1.0),
        b.clamp(0.0, 1.0),
        a.clamp(0.0, 1.0),
    ];

    // I-32: pick up the optional `albedo_texture` path. Empty string
    // or missing field = no texture; the handle derives from the path
    // so two scenes referencing `"textures/wood.png"` hash to the
    // same GPU asset without an auxiliary mapping.
    if let Some(PrimitiveValue::String(path)) = component.fields.get("albedo_texture") {
        if !path.is_empty() {
            material.albedo_texture = Some(texture_handle_for_source(path));
        }
    }

    Some(material)
}

/// I-32: return the authored `albedo_texture` path off a `Material`
/// component, if any. Mirror of `extract_mesh_source` — the editor
/// bridge needs the raw path to load bytes off disk; the runtime
/// component only carries the hash-derived handle.
fn extract_texture_source(entity: &SceneEntity) -> Option<TextureSource> {
    let component = entity.components.iter().find(|c| c.type_name == "Material")?;
    match component.fields.get("albedo_texture") {
        Some(PrimitiveValue::String(path)) if !path.is_empty() => {
            Some(TextureSource { path: path.clone() })
        }
        _ => None,
    }
}

/// I-28: parse a `RigidBody` scene component into a runtime `RigidBody`.
///
/// Every field is optional; the defaults match `RigidBody::default`
/// (zero velocity, full gravity, Mover drives horizontal velocity).
/// Authoring a `RigidBody` with no fields is the most common case —
/// "make this thing fall" — so we accept it as a marker-style component.
fn extract_rigid_body(entity: &SceneEntity) -> Option<RigidBody> {
    let component = entity.components.iter().find(|c| c.type_name == "RigidBody")?;
    let mut body = RigidBody::default();
    if let Some(gs) = field_f32_opt(&component.fields, "gravity_scale") {
        body.gravity_scale = gs;
    }
    if let Some(mdv) = field_bool_opt(&component.fields, "mover_drives_velocity") {
        body.mover_drives_velocity = mdv;
    }
    // Initial velocity fields — rare but useful for "thrown" props.
    if let Some(vx) = field_f32_opt(&component.fields, "velocity_x") {
        body.velocity.x = vx;
    }
    if let Some(vy) = field_f32_opt(&component.fields, "velocity_y") {
        body.velocity.y = vy;
    }
    if let Some(vz) = field_f32_opt(&component.fields, "velocity_z") {
        body.velocity.z = vz;
    }
    Some(body)
}

/// I-35: parse a `Script` scene component into a runtime `Script`.
///
/// Requires a non-empty `source` field (project-relative path).
/// Components with an empty or missing `source` yield `None` — the
/// host has nothing to run in that case, and an empty string would
/// just spam compile errors every tick.
fn extract_script(entity: &SceneEntity) -> Option<Script> {
    let component = entity.components.iter().find(|c| c.type_name == "Script")?;
    let Some(PrimitiveValue::String(path)) = component.fields.get("source") else {
        return None;
    };
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(Script {
        source: trimmed.to_string(),
    })
}

/// I-28: AABB minimum translation vector.
///
/// Returns `Some(mtv)` where `a_center + mtv` separates box A from box
/// B along exactly one axis (the axis of minimum penetration), or
/// `None` if the boxes don't overlap at all. The sign of the returned
/// component pushes A *away* from B.
///
/// We pick "single-axis push" over "full-vector push" because it matches
/// how platformer collisions should feel: a body sliding along a floor
/// should stay in contact with the floor (no lift from a tiny x
/// overlap) and a body bumping a wall should keep its vertical
/// velocity (no drop from a tiny y overlap).
fn resolve_aabb_overlap(
    a_center: Vec3,
    a_half: Vec3,
    b_center: Vec3,
    b_half: Vec3,
) -> Option<Vec3> {
    // Overlap on each axis = sum of half extents - distance between
    // centers. Positive = penetrating, non-positive = separated on that
    // axis (which means the boxes don't overlap at all, since AABB
    // overlap requires all three axes to overlap).
    let delta = a_center - b_center;
    let overlap_x = a_half.x + b_half.x - delta.x.abs();
    let overlap_y = a_half.y + b_half.y - delta.y.abs();
    let overlap_z = a_half.z + b_half.z - delta.z.abs();
    if overlap_x <= 0.0 || overlap_y <= 0.0 || overlap_z <= 0.0 {
        return None;
    }
    // Pick the axis of smallest penetration — that's the shortest way
    // out. Ties go to Y so a body falling onto a floor separates
    // vertically (expected behaviour) rather than sliding off sideways.
    let mut mtv = Vec3::ZERO;
    if overlap_x < overlap_y && overlap_x < overlap_z {
        mtv.x = if delta.x >= 0.0 { overlap_x } else { -overlap_x };
    } else if overlap_z < overlap_y {
        mtv.z = if delta.z >= 0.0 { overlap_z } else { -overlap_z };
    } else {
        mtv.y = if delta.y >= 0.0 { overlap_y } else { -overlap_y };
    }
    Some(mtv)
}

/// I-25: parse a `Camera` scene component into a runtime `Camera`.
/// Missing fields fall back to sensible defaults so minimally-authored
/// cameras (just `(type_name: "Camera", fields: {})`) still work.
fn extract_camera(entity: &SceneEntity) -> Option<Camera> {
    let component = entity.components.iter().find(|c| c.type_name == "Camera")?;
    let mut camera = Camera::default();
    if let Some(fov_deg) = field_f32_opt(&component.fields, "fov") {
        // Clamp to a sane range — 1° is a needle, 170° is fisheye.
        camera.fov_y_rad = fov_deg.clamp(1.0, 170.0).to_radians();
    }
    if let Some(near) = field_f32_opt(&component.fields, "near") {
        camera.near = near.max(0.0001);
    }
    if let Some(far) = field_f32_opt(&component.fields, "far") {
        camera.far = far.max(camera.near + 0.001);
    }
    if let Some(primary) = field_bool_opt(&component.fields, "is_primary") {
        camera.is_primary = primary;
    }
    Some(camera)
}

fn extract_mesh(entity: &SceneEntity) -> Option<MeshHandle> {
    let component = entity.components.iter().find(|c| c.type_name == "Mesh")?;
    // Primitive first — designers who've authored `primitive: "cube"`
    // stay on the baked-in cube fast path even if they also filled in a
    // `source` field by accident.
    if let Some(PrimitiveValue::String(name)) = component.fields.get("primitive") {
        if name == "cube" {
            return Some(MeshHandle::UNIT_CUBE);
        }
    }
    // I-27: an external asset reference (e.g. `source: "meshes/player.gltf"`)
    // produces a non-cube handle hashed from the path. The renderer-side
    // MeshRegistry is keyed by the same u64 so uploads and draws round-trip
    // without a second mapping table.
    if let Some(PrimitiveValue::String(source)) = component.fields.get("source") {
        if !source.is_empty() {
            return Some(mesh_handle_for_source(source));
        }
    }
    // Other primitives (sphere, plane, …) will land alongside the
    // mesh asset expansion in Arc 2.
    None
}

/// Return the `MeshSource` component authored on `entity`, if any.
/// Carries the raw path string verbatim — resolving it to a filesystem
/// path (relative to the project root) is the editor bridge's job.
///
/// Suppressed when `primitive: "cube"` is also set: the cube primitive
/// wins the handle selection in [`extract_mesh`], so shipping a
/// `MeshSource` too would make the bridge try to load a file that no
/// entity is actually waiting on.
fn extract_mesh_source(entity: &SceneEntity) -> Option<MeshSource> {
    let component = entity.components.iter().find(|c| c.type_name == "Mesh")?;
    if let Some(PrimitiveValue::String(name)) = component.fields.get("primitive") {
        if name == "cube" {
            return None;
        }
    }
    match component.fields.get("source") {
        Some(PrimitiveValue::String(source)) if !source.is_empty() => {
            Some(MeshSource { path: source.clone() })
        }
        _ => None,
    }
}

/// Deterministic path → `MeshHandle` hash.
///
/// `MeshHandle::UNIT_CUBE` is the reserved `0` value and must never
/// collide with an imported asset, so we fold the FxHash result through
/// `| 1` to guarantee the low bit is set. Stability across process
/// restarts is what matters here — two runs of the editor with the same
/// scene file must produce the same handle so previously-uploaded
/// assets in the registry get reused instead of double-uploaded.
pub fn mesh_handle_for_source(source: &str) -> MeshHandle {
    use std::hash::{Hash, Hasher};
    // `DefaultHasher` is SipHash — deterministic across runs with the
    // same seed. We feed the raw bytes (no normalization) so "foo.gltf"
    // and "./foo.gltf" map to different handles; callers normalize
    // before hashing if they want those to collide.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    let raw = hasher.finish();
    // Dodge the reserved UNIT_CUBE slot. Flipping the low bit is a
    // one-cycle guard that still preserves the remaining 63 bits of
    // entropy.
    MeshHandle(raw | 1)
}

/// I-27: authoring-side handle for an external mesh asset. The `path`
/// is exactly what was written in the scene file (e.g.
/// `"meshes/player.gltf"`) — the editor bridge resolves it against the
/// project root when it's time to read the file off disk.
///
/// Lives alongside `MeshHandle` in the ECS so the bridge can
/// distinguish "this entity already has a GPU-resident asset" from
/// "this entity needs an import kicked off" without touching the scene
/// document again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshSource {
    pub path: String,
}

fn field_f32(
    fields: &std::collections::BTreeMap<String, PrimitiveValue>,
    key: &str,
) -> f32 {
    match fields.get(key) {
        Some(PrimitiveValue::F64(v)) => *v as f32,
        Some(PrimitiveValue::I64(v)) => *v as f32,
        _ => 0.0,
    }
}

// Silence "ComponentData unused import" — reserved for upcoming
// Light/Camera extraction helpers in I-7/I-8 without churning the use
// list again.
#[allow(dead_code)]
fn _component_data_type_check(_: &ComponentData) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_identity_is_identity_matrix() {
        let m = Transform::IDENTITY.model_matrix();
        assert_eq!(m, Mat4::IDENTITY);
    }

    #[test]
    fn transform_translation_only_moves_origin() {
        let t = Transform::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let m = t.model_matrix();
        // Translation column in a column-major matrix is column 3.
        assert_eq!(m.col(3).truncate(), Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn spawn_cube_adds_expected_components() {
        let mut world = World::new();
        let entity = world.spawn_cube(Transform::from_translation(Vec3::X));
        assert_eq!(world.len(), 1);

        let ecs = world.ecs();
        assert!(ecs.get::<&Transform>(entity).is_ok());
        assert!(ecs.get::<&MeshHandle>(entity).is_ok());
        assert!(ecs.get::<&MaterialHandle>(entity).is_ok());
    }

    #[test]
    fn render_snapshot_matches_spawned_entities() {
        let mut world = World::new();
        world.spawn_cube(Transform::from_translation(Vec3::ZERO));
        world.spawn_cube(Transform::from_translation(Vec3::new(2.0, 0.0, 0.0)));
        world.spawn_cube(Transform::from_translation(Vec3::new(-2.0, 0.0, 0.0)));

        let snapshot = world.collect_render_snapshot();
        assert_eq!(snapshot.len(), 3);
        for item in &snapshot {
            assert_eq!(item.mesh, MeshHandle::UNIT_CUBE);
        }
    }

    #[test]
    fn empty_world_collects_empty_snapshot() {
        let world = World::new();
        assert!(world.is_empty());
        assert!(world.collect_render_snapshot().is_empty());
    }

    #[test]
    fn instantiate_scene_maps_cubes_into_renderable_entities() {
        let cube_a = SceneEntity::new(SceneId::new(10), "Block A")
            .with_component(
                ComponentData::new("Transform")
                    .with_field("x", PrimitiveValue::F64(1.0))
                    .with_field("y", PrimitiveValue::F64(0.0))
                    .with_field("z", PrimitiveValue::F64(0.0)),
            )
            .with_component(
                ComponentData::new("Mesh").with_field(
                    "primitive",
                    PrimitiveValue::String("cube".into()),
                ),
            );
        let cube_b = SceneEntity::new(SceneId::new(11), "Block B")
            .with_component(
                ComponentData::new("Transform")
                    .with_field("x", PrimitiveValue::F64(-1.0)),
            )
            .with_component(
                ComponentData::new("Mesh").with_field(
                    "primitive",
                    PrimitiveValue::String("cube".into()),
                ),
            );
        let empty_group =
            SceneEntity::new(SceneId::new(12), "Empty").with_component(
                ComponentData::new("Transform").with_field("y", PrimitiveValue::F64(3.0)),
            );

        let doc = SceneDocument::new("Fixture")
            .with_root(cube_a)
            .with_root(cube_b)
            .with_root(empty_group);

        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);

        assert_eq!(mapping.len(), 3);
        assert_eq!(world.len(), 3);
        // Two renderables, one pure transform.
        assert_eq!(world.collect_render_snapshot().len(), 2);
        // Mapping is bijective.
        let entity_a = mapping.entity(SceneId::new(10)).unwrap();
        assert_eq!(mapping.scene_id(entity_a), Some(SceneId::new(10)));
    }

    #[test]
    fn instantiate_scene_attaches_directional_light_from_light_component() {
        let doc = SceneDocument::new("LitScene").with_root(
            SceneEntity::new(SceneId::new(1), "Key Light").with_component(
                ComponentData::new("Light")
                    .with_field("direction_x", PrimitiveValue::F64(1.0))
                    .with_field("direction_y", PrimitiveValue::F64(0.0))
                    .with_field("direction_z", PrimitiveValue::F64(0.0))
                    .with_field("color_r", PrimitiveValue::F64(0.5))
                    .with_field("color_g", PrimitiveValue::F64(0.5))
                    .with_field("color_b", PrimitiveValue::F64(1.0))
                    .with_field("intensity", PrimitiveValue::F64(2.0))
                    .with_field("ambient", PrimitiveValue::F64(0.25)),
            ),
        );

        let mut world = World::new();
        world.instantiate_scene(&doc);
        let light = world
            .primary_directional_light()
            .expect("Light component should populate DirectionalLight");
        assert_eq!(light.color, [0.5, 0.5, 1.0]);
        assert!((light.intensity - 2.0).abs() < 1e-6);
        assert!((light.ambient - 0.25).abs() < 1e-6);
        assert!((light.direction.x - 1.0).abs() < 1e-6);
    }

    #[test]
    fn instantiate_scene_attaches_camera_from_camera_component() {
        // I-25: A scene with a Camera component must surface through
        // `primary_camera()` with the authored intrinsics preserved
        // and the world transform composed from the entity's
        // position (no parent chain here, so it's a straight copy).
        let doc = SceneDocument::new("CameraScene").with_root(
            SceneEntity::new(SceneId::new(1), "PlayCam")
                .with_component(
                    ComponentData::new("Transform")
                        .with_field("x", PrimitiveValue::F64(0.0))
                        .with_field("y", PrimitiveValue::F64(3.0))
                        .with_field("z", PrimitiveValue::F64(-10.0)),
                )
                .with_component(
                    ComponentData::new("Camera")
                        .with_field("fov", PrimitiveValue::F64(75.0))
                        .with_field("near", PrimitiveValue::F64(0.2))
                        .with_field("far", PrimitiveValue::F64(250.0))
                        .with_field("is_primary", PrimitiveValue::Bool(true)),
                ),
        );

        let mut world = World::new();
        world.instantiate_scene(&doc);
        let (camera, world_transform) = world
            .primary_camera()
            .expect("Camera component should populate primary_camera");

        assert!((camera.fov_y_rad - 75f32.to_radians()).abs() < 1e-5);
        assert!((camera.near - 0.2).abs() < 1e-6);
        assert!((camera.far - 250.0).abs() < 1e-6);
        assert!(camera.is_primary);

        // Transform column 3 carries translation.
        let translation = world_transform.col(3).truncate();
        assert!((translation.x - 0.0).abs() < 1e-6);
        assert!((translation.y - 3.0).abs() < 1e-6);
        assert!((translation.z + 10.0).abs() < 1e-6);
    }

    #[test]
    fn primary_camera_prefers_primary_flagged_over_first_found() {
        // Two Camera entities; only the second has `is_primary: true`.
        // The primary flag must win even though it appears later in
        // the scene traversal.
        let doc = SceneDocument::new("MultiCam")
            .with_root(
                SceneEntity::new(SceneId::new(1), "AuthCam")
                    .with_component(
                        ComponentData::new("Transform")
                            .with_field("x", PrimitiveValue::F64(1.0)),
                    )
                    .with_component(
                        ComponentData::new("Camera")
                            .with_field("is_primary", PrimitiveValue::Bool(false)),
                    ),
            )
            .with_root(
                SceneEntity::new(SceneId::new(2), "PlayCam")
                    .with_component(
                        ComponentData::new("Transform")
                            .with_field("x", PrimitiveValue::F64(5.0)),
                    )
                    .with_component(
                        ComponentData::new("Camera")
                            .with_field("is_primary", PrimitiveValue::Bool(true)),
                    ),
            );

        let mut world = World::new();
        world.instantiate_scene(&doc);
        let (_, transform) = world.primary_camera().unwrap();
        // PlayCam's x=5, not AuthCam's x=1.
        let translation = transform.col(3).truncate();
        assert!((translation.x - 5.0).abs() < 1e-6);
    }

    #[test]
    fn primary_camera_is_none_when_scene_has_no_camera() {
        let doc = SceneDocument::new("Empty").with_root(
            SceneEntity::new(SceneId::new(1), "Just a cube").with_component(
                ComponentData::new("Mesh")
                    .with_field("primitive", PrimitiveValue::String("cube".into())),
            ),
        );
        let mut world = World::new();
        world.instantiate_scene(&doc);
        assert!(world.primary_camera().is_none());
    }

    #[test]
    fn mesh_handle_from_source_is_stable_and_avoids_unit_cube() {
        // Hash-derived handles must be deterministic across calls so
        // re-rebuilds of the same scene re-use existing GPU uploads,
        // and they must never collide with `UNIT_CUBE(0)` which is
        // reserved for the baked-in cube.
        let a1 = mesh_handle_for_source("meshes/player.gltf");
        let a2 = mesh_handle_for_source("meshes/player.gltf");
        let b = mesh_handle_for_source("meshes/enemy.gltf");
        assert_eq!(a1, a2, "same path → same handle");
        assert_ne!(a1, b, "different paths → different handles");
        assert_ne!(a1, MeshHandle::UNIT_CUBE);
        assert_ne!(b, MeshHandle::UNIT_CUBE);
    }

    #[test]
    fn mesh_source_component_attached_when_scene_authors_a_source() {
        // Authoring `Mesh { source: "foo.gltf" }` must spawn an ECS
        // entity carrying both a `MeshHandle` (hashed from the path)
        // and a `MeshSource` (the raw path) — the bridge uses the
        // handle for draw dispatch and the path to actually read bytes
        // off disk.
        let doc = SceneDocument::new("Imported").with_root(
            SceneEntity::new(SceneId::new(1), "Spaceship").with_component(
                ComponentData::new("Mesh")
                    .with_field("source", PrimitiveValue::String("meshes/ship.gltf".into())),
            ),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let ship = mapping.entity(SceneId::new(1)).unwrap();

        let handle = *world.ecs().get::<&MeshHandle>(ship).unwrap();
        assert_eq!(handle, mesh_handle_for_source("meshes/ship.gltf"));
        assert_ne!(handle, MeshHandle::UNIT_CUBE);

        let source = world.ecs().get::<&MeshSource>(ship).unwrap();
        assert_eq!(source.path, "meshes/ship.gltf");
    }

    #[test]
    fn mesh_primitive_cube_takes_precedence_over_source() {
        // A component with both `primitive: "cube"` and `source: "x.gltf"`
        // stays on the cube fast path — primitives are engine-intrinsic
        // and can't race against asset availability. The `source` field
        // is silently ignored in that case; a separate lint-level check
        // could flag it later.
        let doc = SceneDocument::new("Ambiguous").with_root(
            SceneEntity::new(SceneId::new(1), "Both").with_component(
                ComponentData::new("Mesh")
                    .with_field("primitive", PrimitiveValue::String("cube".into()))
                    .with_field("source", PrimitiveValue::String("shouldnt-load.gltf".into())),
            ),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let both = mapping.entity(SceneId::new(1)).unwrap();

        let handle = *world.ecs().get::<&MeshHandle>(both).unwrap();
        assert_eq!(handle, MeshHandle::UNIT_CUBE);
    }

    #[test]
    fn tick_gameplay_moves_entities_with_mover_component() {
        // Spawn a Player with Mover{speed=2.0} + Transform at origin,
        // press D for 0.5s, expect x to advance by speed*dt = 1.0.
        let doc = SceneDocument::new("Playable").with_root(
            SceneEntity::new(SceneId::new(1), "Player")
                .with_component(ComponentData::new("Transform"))
                .with_component(
                    ComponentData::new("Mover")
                        .with_field("speed", PrimitiveValue::F64(2.0)),
                ),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let player = mapping.entity(SceneId::new(1)).unwrap();

        let mut input = crate::input::Input::new();
        input.press(crate::input::Key::D);
        world.tick_gameplay(&input, 0.5);

        let transform = world.ecs().get::<&Transform>(player).unwrap();
        assert!((transform.translation.x - 1.0).abs() < 1e-5);
        // No W/S input → z untouched.
        assert!(transform.translation.z.abs() < 1e-5);
    }

    #[test]
    fn tick_gameplay_ignores_entities_without_mover() {
        // A Transform-only entity must not drift when input fires —
        // the Mover component is the opt-in for gameplay movement.
        let doc = SceneDocument::new("Static").with_root(
            SceneEntity::new(SceneId::new(1), "Prop")
                .with_component(ComponentData::new("Transform")),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let prop = mapping.entity(SceneId::new(1)).unwrap();

        let mut input = crate::input::Input::new();
        input.press(crate::input::Key::W);
        world.tick_gameplay(&input, 1.0);

        let transform = world.ecs().get::<&Transform>(prop).unwrap();
        assert_eq!(transform.translation, Vec3::ZERO);
    }

    #[test]
    fn tick_gameplay_w_key_moves_forward_along_negative_z() {
        // W = positive axis of (S, W) pair → +axis → +dz in Input::axis.
        // Our Mover maps that to transform.z += dz * speed * dt.
        // Note: the convention is "W moves in +z" at the ECS layer;
        // camera look direction is what turns that into "forward" at
        // the render layer. This test pins the ECS contract.
        let doc = SceneDocument::new("Forward").with_root(
            SceneEntity::new(SceneId::new(1), "Player")
                .with_component(ComponentData::new("Transform"))
                .with_component(
                    ComponentData::new("Mover")
                        .with_field("speed", PrimitiveValue::F64(4.0)),
                ),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let player = mapping.entity(SceneId::new(1)).unwrap();

        let mut input = crate::input::Input::new();
        input.press(crate::input::Key::W);
        world.tick_gameplay(&input, 0.25);

        let transform = world.ecs().get::<&Transform>(player).unwrap();
        assert!((transform.translation.z - 1.0).abs() < 1e-5);
    }

    #[test]
    fn primary_directional_light_is_none_when_no_light_entity() {
        let doc = SceneDocument::new("Dark").with_root(
            SceneEntity::new(SceneId::new(1), "Just a cube").with_component(
                ComponentData::new("Mesh")
                    .with_field("primitive", PrimitiveValue::String("cube".into())),
            ),
        );
        let mut world = World::new();
        world.instantiate_scene(&doc);
        assert!(world.primary_directional_light().is_none());
    }

    #[test]
    fn resync_updates_transform_without_recreating_entities() {
        let doc = SceneDocument::new("Fixture").with_root(
            SceneEntity::new(SceneId::new(1), "Block")
                .with_component(
                    ComponentData::new("Transform")
                        .with_field("x", PrimitiveValue::F64(0.0)),
                )
                .with_component(
                    ComponentData::new("Mesh").with_field(
                        "primitive",
                        PrimitiveValue::String("cube".into()),
                    ),
                ),
        );

        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let entity = mapping.entity(SceneId::new(1)).unwrap();
        let entity_before = entity;

        // Mutate the authoring doc — simulate a NudgeTransform command.
        let mut doc = doc;
        let comp = doc
            .find_entity_mut(SceneId::new(1))
            .unwrap()
            .components
            .iter_mut()
            .find(|c| c.type_name == "Transform")
            .unwrap();
        comp.fields
            .insert("x".into(), PrimitiveValue::F64(7.0));

        world.resync_transforms_from_scene(&doc, &mapping);

        // Same entity handle.
        assert_eq!(entity_before, mapping.entity(SceneId::new(1)).unwrap());
        // Transform picked up the new x.
        let t = *world.ecs().get::<&Transform>(entity).unwrap();
        assert!((t.translation.x - 7.0).abs() < 1e-6);
    }

    #[test]
    fn scene_ron_roundtrip_yields_identical_render_snapshot() {
        // Save → load round-trip produces byte-for-byte identical render
        // snapshot. Covers the I-10 contract end-to-end.
        let doc = SceneDocument::new("Fixture")
            .with_root(
                SceneEntity::new(SceneId::new(1), "A")
                    .with_component(
                        ComponentData::new("Transform")
                            .with_field("x", PrimitiveValue::F64(1.25))
                            .with_field("y", PrimitiveValue::F64(-0.5))
                            .with_field("z", PrimitiveValue::F64(3.0)),
                    )
                    .with_component(
                        ComponentData::new("Mesh").with_field(
                            "primitive",
                            PrimitiveValue::String("cube".into()),
                        ),
                    ),
            )
            .with_root(
                SceneEntity::new(SceneId::new(2), "B")
                    .with_component(
                        ComponentData::new("Transform")
                            .with_field("x", PrimitiveValue::F64(-2.0))
                            .with_field("scale", PrimitiveValue::F64(0.75)),
                    )
                    .with_component(
                        ComponentData::new("Mesh").with_field(
                            "primitive",
                            PrimitiveValue::String("cube".into()),
                        ),
                    ),
            );

        // Original snapshot.
        let mut world_a = World::new();
        world_a.instantiate_scene(&doc);
        let mut snap_a = world_a.collect_render_snapshot();

        // Round-tripped snapshot.
        let ron = doc.to_ron_string().unwrap();
        let loaded = SceneDocument::from_ron_string(&ron).unwrap();
        let mut world_b = World::new();
        world_b.instantiate_scene(&loaded);
        let mut snap_b = world_b.collect_render_snapshot();

        // Entity handles differ across worlds — compare by model matrix.
        fn sort_key(m: &Mat4) -> [u32; 16] {
            let arr = m.to_cols_array();
            let mut out = [0u32; 16];
            for (i, v) in arr.iter().enumerate() {
                out[i] = v.to_bits();
            }
            out
        }
        snap_a.sort_by_key(|r| sort_key(&r.model));
        snap_b.sort_by_key(|r| sort_key(&r.model));

        assert_eq!(snap_a.len(), snap_b.len());
        for (a, b) in snap_a.iter().zip(snap_b.iter()) {
            assert_eq!(a.model, b.model);
            assert_eq!(a.mesh, b.mesh);
        }
    }

    #[test]
    fn world_transform_composes_parent_chain() {
        // Parent at (5, 0, 0); child authored at local (1, 0, 0) →
        // world (6, 0, 0).
        let mut world = World::new();
        let parent = world.ecs_mut().spawn((
            Transform::from_translation(Vec3::new(5.0, 0.0, 0.0)),
        ));
        let child = world.ecs_mut().spawn((
            Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            Parent(parent),
        ));

        let worlds = world.compute_world_transforms();
        let child_world = worlds.get(&child).unwrap();
        let translation = child_world.col(3).truncate();
        assert!((translation - Vec3::new(6.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn instantiate_scene_records_parent_pointers_for_children() {
        let doc = SceneDocument::new("Tree").with_root(
            SceneEntity::new(SceneId::new(1), "Root")
                .with_component(
                    ComponentData::new("Transform")
                        .with_field("x", PrimitiveValue::F64(10.0)),
                )
                .with_child(
                    SceneEntity::new(SceneId::new(2), "Child").with_component(
                        ComponentData::new("Transform")
                            .with_field("x", PrimitiveValue::F64(2.0)),
                    ),
                ),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);

        let child_entity = mapping.entity(SceneId::new(2)).unwrap();
        let parent_component = world
            .ecs()
            .get::<&Parent>(child_entity)
            .expect("child should have Parent");
        let parent_entity = mapping.entity(SceneId::new(1)).unwrap();
        assert_eq!(parent_component.0, parent_entity);

        let worlds = world.compute_world_transforms();
        let child_world = worlds.get(&child_entity).unwrap();
        // Root.x + Child.x = 12.0
        assert!((child_world.col(3).x - 12.0).abs() < 1e-5);
    }

    #[test]
    fn tick_physics_applies_gravity_to_falling_body() {
        // A RigidBody with no supporting collider falls under gravity.
        // After 1 second with g = -9.81 m/s² we expect velocity.y = -9.81
        // and position.y = -9.81 * 1.0 (semi-implicit Euler: velocity
        // updates first, then position uses the new velocity).
        let doc = SceneDocument::new("Falling").with_root(
            SceneEntity::new(SceneId::new(1), "Ball")
                .with_component(
                    ComponentData::new("Transform")
                        .with_field("y", PrimitiveValue::F64(10.0)),
                )
                .with_component(ComponentData::new("Collider"))
                .with_component(ComponentData::new("RigidBody")),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let ball = mapping.entity(SceneId::new(1)).unwrap();

        world.tick_physics(1.0);

        let t = *world.ecs().get::<&Transform>(ball).unwrap();
        let b = *world.ecs().get::<&RigidBody>(ball).unwrap();
        assert!((b.velocity.y - (-9.81)).abs() < 1e-3);
        // Semi-implicit: position += new_velocity * dt = -9.81.
        assert!((t.translation.y - (10.0 - 9.81)).abs() < 1e-3);
    }

    #[test]
    fn tick_physics_rests_body_on_static_ground() {
        // Dynamic body above a static ground plane. Gravity pulls it
        // down; the MTV push-out snaps it to rest exactly on top with
        // velocity.y zeroed. No bouncing (restitution = 0).
        //
        // Body at y=2, half-extents 0.5; ground at y=0, half-extents
        // (5, 0.5, 5). Top of the ground is y=0.5; bottom of body at
        // rest should be y=0.5, so body center y=1.0.
        let doc = SceneDocument::new("Grounded")
            .with_root(
                SceneEntity::new(SceneId::new(1), "Ground")
                    .with_component(ComponentData::new("Transform"))
                    .with_component(
                        ComponentData::new("Collider")
                            .with_field("x", PrimitiveValue::F64(5.0))
                            .with_field("y", PrimitiveValue::F64(0.5))
                            .with_field("z", PrimitiveValue::F64(5.0)),
                    ),
            )
            .with_root(
                SceneEntity::new(SceneId::new(2), "Ball")
                    .with_component(
                        ComponentData::new("Transform")
                            .with_field("y", PrimitiveValue::F64(2.0)),
                    )
                    .with_component(ComponentData::new("Collider"))
                    .with_component(ComponentData::new("RigidBody")),
            );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let ball = mapping.entity(SceneId::new(2)).unwrap();

        // Run enough ticks for the ball to fall and settle.
        for _ in 0..120 {
            world.tick_physics(1.0 / 60.0);
        }

        let t = *world.ecs().get::<&Transform>(ball).unwrap();
        let b = *world.ecs().get::<&RigidBody>(ball).unwrap();
        // Ball's bottom sits on ground's top (y=0.5), so center y=1.0.
        assert!(
            (t.translation.y - 1.0).abs() < 1e-3,
            "ball should rest at y=1.0, got {}",
            t.translation.y,
        );
        // Velocity zeroed on contact axis.
        assert!(b.velocity.y.abs() < 1e-3);
    }

    #[test]
    fn tick_physics_horizontal_wall_stops_lateral_motion() {
        // Dynamic body with positive x-velocity walks into a static
        // wall. After the push-out, its x-velocity is zero and its
        // center is flush against the wall's left face.
        //
        // Wall at x=5, half-extents 0.5 → left face at x=4.5. Ball
        // half-extents 0.5, approaching from x=0 at 10 m/s. No gravity
        // in this test — we lift it onto the wall's slab so horizontal
        // collision is the only force.
        let doc = SceneDocument::new("Wall")
            .with_root(
                SceneEntity::new(SceneId::new(1), "Wall")
                    .with_component(
                        ComponentData::new("Transform")
                            .with_field("x", PrimitiveValue::F64(5.0)),
                    )
                    .with_component(ComponentData::new("Collider")),
            )
            .with_root(
                SceneEntity::new(SceneId::new(2), "Ball")
                    .with_component(ComponentData::new("Transform"))
                    .with_component(ComponentData::new("Collider"))
                    .with_component(
                        ComponentData::new("RigidBody")
                            .with_field("gravity_scale", PrimitiveValue::F64(0.0))
                            .with_field("velocity_x", PrimitiveValue::F64(10.0)),
                    ),
            );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let ball = mapping.entity(SceneId::new(2)).unwrap();

        // 1 second of 60Hz ticks — at 10 m/s the ball starts at x=0
        // and would reach x=10 without collision. With the wall at
        // x=5, it should stop at x=4.0 (ball half 0.5 + wall half 0.5
        // → gap 1.0, wall center 5.0, so ball center 4.0).
        for _ in 0..60 {
            world.tick_physics(1.0 / 60.0);
        }

        let t = *world.ecs().get::<&Transform>(ball).unwrap();
        let b = *world.ecs().get::<&RigidBody>(ball).unwrap();
        assert!(
            (t.translation.x - 4.0).abs() < 1e-2,
            "ball should rest at x=4.0 against wall, got {}",
            t.translation.x,
        );
        assert!(b.velocity.x.abs() < 1e-3);
    }

    #[test]
    fn tick_gameplay_mover_with_rigid_body_feeds_velocity_not_transform() {
        // Mover + RigidBody + mover_drives_velocity=true composition:
        // pressing D must set velocity.x rather than teleporting the
        // Transform. Gravity is off so y stays pinned and we're only
        // observing the horizontal channel.
        let doc = SceneDocument::new("Player").with_root(
            SceneEntity::new(SceneId::new(1), "Player")
                .with_component(ComponentData::new("Transform"))
                .with_component(
                    ComponentData::new("Mover")
                        .with_field("speed", PrimitiveValue::F64(5.0)),
                )
                .with_component(ComponentData::new("Collider"))
                .with_component(
                    ComponentData::new("RigidBody")
                        .with_field("gravity_scale", PrimitiveValue::F64(0.0)),
                ),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let player = mapping.entity(SceneId::new(1)).unwrap();

        let mut input = crate::input::Input::new();
        input.press(crate::input::Key::D);
        // One tick should have been enough to set the velocity and
        // advance position by speed * dt = 5 * (1/60) ≈ 0.0833.
        world.tick_gameplay(&input, 1.0 / 60.0);

        let b = *world.ecs().get::<&RigidBody>(player).unwrap();
        assert!((b.velocity.x - 5.0).abs() < 1e-5);
        let t = *world.ecs().get::<&Transform>(player).unwrap();
        assert!(t.translation.x > 0.0);
    }

    #[test]
    fn resolve_aabb_overlap_none_when_separated() {
        // Two unit cubes 3 units apart on x → no overlap.
        let mtv = resolve_aabb_overlap(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::splat(0.5),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::splat(0.5),
        );
        assert!(mtv.is_none());
    }

    #[test]
    fn resolve_aabb_overlap_picks_smallest_axis() {
        // A is centered at (0.2, 0.6, 0) with half 0.5; B at origin
        // with half 0.5. Penetration: x = 0.8, y = 0.4, z = 1.0. Y is
        // smallest → MTV should push A in +y by 0.4.
        let mtv = resolve_aabb_overlap(
            Vec3::new(0.2, 0.6, 0.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
            Vec3::splat(0.5),
        )
        .expect("boxes overlap");
        assert!(mtv.x.abs() < 1e-6);
        assert!((mtv.y - 0.4).abs() < 1e-6);
        assert!(mtv.z.abs() < 1e-6);
    }

    #[test]
    fn collect_autoplay_audio_yields_one_command_per_autoplay_source() {
        // Two AudioSource entities — one autoplay, one not. Only the
        // autoplay one must show up in the emitted command list, and
        // its volume/pitch/looping must snapshot through unchanged.
        let doc = SceneDocument::new("Audible")
            .with_root(
                SceneEntity::new(SceneId::new(1), "Ambient").with_component(
                    ComponentData::new("AudioSource")
                        .with_field("source", PrimitiveValue::String("audio/loop.ogg".into()))
                        .with_field("volume", PrimitiveValue::F64(0.5))
                        .with_field("looping", PrimitiveValue::Bool(true))
                        .with_field("autoplay", PrimitiveValue::Bool(true)),
                ),
            )
            .with_root(
                SceneEntity::new(SceneId::new(2), "Trigger").with_component(
                    ComponentData::new("AudioSource")
                        .with_field("source", PrimitiveValue::String("audio/pop.wav".into())),
                ),
            );
        let mut world = World::new();
        world.instantiate_scene(&doc);

        let commands = world.collect_autoplay_audio();
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            crate::audio::AudioCommand::Play { path, volume, looping, .. } => {
                assert_eq!(path, "audio/loop.ogg");
                assert!((*volume - 0.5).abs() < 1e-6);
                assert!(*looping);
            }
            _ => panic!("expected Play command"),
        }
    }

    #[test]
    fn render_snapshot_defaults_albedo_to_white_without_material() {
        let mut world = World::new();
        world.spawn_cube(Transform::from_translation(Vec3::ZERO));
        let snap = world.collect_render_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].albedo, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn render_snapshot_picks_up_material_albedo_from_scene() {
        // Entity with an explicit Material component — the extractor
        // should clamp values into [0, 1] and the render snapshot
        // should surface the tint for the renderer to consume.
        let entity = SceneEntity::new(SceneId::new(1), "Tinted Block")
            .with_component(ComponentData::new("Transform"))
            .with_component(
                ComponentData::new("Mesh")
                    .with_field("primitive", PrimitiveValue::String("cube".into())),
            )
            .with_component(
                ComponentData::new("Material")
                    .with_field("color_r", PrimitiveValue::F64(0.6))
                    .with_field("color_g", PrimitiveValue::F64(0.1))
                    // Authored out-of-range to exercise the clamp.
                    .with_field("color_b", PrimitiveValue::F64(2.0))
                    .with_field("color_a", PrimitiveValue::F64(0.5)),
            );
        let doc = SceneDocument::new("TintScene").with_root(entity);

        let mut world = World::new();
        let _ = world.instantiate_scene(&doc);
        let snap = world.collect_render_snapshot();
        assert_eq!(snap.len(), 1);
        let [r, g, b, a] = snap[0].albedo;
        assert!((r - 0.6).abs() < 1e-5);
        assert!((g - 0.1).abs() < 1e-5);
        // 2.0 clamped → 1.0.
        assert!((b - 1.0).abs() < 1e-5);
        assert!((a - 0.5).abs() < 1e-5);
    }

    #[test]
    fn extract_material_returns_none_when_component_absent() {
        // Missing `Material` component must still produce `None` from
        // the extractor so callers can distinguish "authored default"
        // from "no material override" when later systems need the
        // difference.
        let entity = SceneEntity::new(SceneId::new(1), "Plain").with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        );
        assert!(super::extract_material(&entity).is_none());
    }

    #[test]
    fn extract_material_with_no_fields_yields_default_white() {
        // Designer drops a bare `Material` with no fields — that's
        // intentional "explicit default" authoring, should materialize
        // to the white identity albedo rather than a zeroed color.
        let entity = SceneEntity::new(SceneId::new(1), "Default Material")
            .with_component(ComponentData::new("Material"));
        let material = super::extract_material(&entity).expect("material extracted");
        assert_eq!(material.albedo, [1.0, 1.0, 1.0, 1.0]);
        assert!(material.albedo_texture.is_none());
    }

    #[test]
    fn texture_handle_unit_white_is_zero() {
        // Must match `render::TextureAssetId::DEFAULT_WHITE`
        // so the editor bridge can bit-cast between the two.
        assert_eq!(TextureHandle::UNIT_WHITE.0, 0);
    }

    #[test]
    fn texture_handle_for_source_is_deterministic_and_nonzero() {
        // Two calls with the same input must hash to the same handle
        // so registry uploads reuse the resident asset across scene
        // reloads. Never collides with UNIT_WHITE thanks to `| 1`.
        let a1 = texture_handle_for_source("textures/checker.png");
        let a2 = texture_handle_for_source("textures/checker.png");
        let b = texture_handle_for_source("textures/wood.png");
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        assert_ne!(a1, TextureHandle::UNIT_WHITE);
        assert_ne!(b, TextureHandle::UNIT_WHITE);
    }

    #[test]
    fn extract_material_reads_albedo_texture_path() {
        // Material with an `albedo_texture` field should hash through
        // `texture_handle_for_source` and round-trip cleanly.
        let path = "textures/checker.png";
        let entity = SceneEntity::new(SceneId::new(1), "Tex Block").with_component(
            ComponentData::new("Material")
                .with_field("albedo_texture", PrimitiveValue::String(path.into())),
        );
        let material = super::extract_material(&entity).expect("material extracted");
        assert_eq!(
            material.albedo_texture,
            Some(texture_handle_for_source(path))
        );
    }

    #[test]
    fn extract_material_ignores_empty_albedo_texture_path() {
        // Empty-string `albedo_texture` is a common authoring sentinel
        // ("clear the texture"); must be treated as "no texture", not
        // a handle to the empty-path hash.
        let entity = SceneEntity::new(SceneId::new(1), "No Tex").with_component(
            ComponentData::new("Material")
                .with_field("albedo_texture", PrimitiveValue::String(String::new())),
        );
        let material = super::extract_material(&entity).expect("material extracted");
        assert!(material.albedo_texture.is_none());
    }

    #[test]
    fn render_snapshot_propagates_albedo_texture() {
        // Scene authoring a Material with albedo_texture — the render
        // snapshot's `albedo_texture` field must round-trip the hashed
        // handle so the renderer can look up the right bind group.
        let path = "textures/checker.png";
        let entity = SceneEntity::new(SceneId::new(1), "Tex Cube")
            .with_component(ComponentData::new("Transform"))
            .with_component(
                ComponentData::new("Mesh")
                    .with_field("primitive", PrimitiveValue::String("cube".into())),
            )
            .with_component(
                ComponentData::new("Material")
                    .with_field("albedo_texture", PrimitiveValue::String(path.into())),
            );
        let doc = SceneDocument::new("TexScene").with_root(entity);

        let mut world = World::new();
        let _ = world.instantiate_scene(&doc);
        let snap = world.collect_render_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap[0].albedo_texture,
            texture_handle_for_source(path)
        );
    }

    #[test]
    fn render_snapshot_defaults_albedo_texture_to_unit_white() {
        // No Material → UNIT_WHITE, so the renderer samples the 1×1
        // white texture and nothing changes from pre-I-32 output.
        let mut world = World::new();
        world.spawn_cube(Transform::from_translation(Vec3::ZERO));
        let snap = world.collect_render_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].albedo_texture, TextureHandle::UNIT_WHITE);
    }

    #[test]
    fn spawn_scene_attaches_texture_source_when_authored() {
        // A Material with `albedo_texture` should also produce a
        // TextureSource component on the spawned entity so the editor
        // bridge knows what file to load off disk.
        let path = "textures/checker.png";
        let entity = SceneEntity::new(SceneId::new(1), "Tex")
            .with_component(ComponentData::new("Transform"))
            .with_component(
                ComponentData::new("Mesh")
                    .with_field("primitive", PrimitiveValue::String("cube".into())),
            )
            .with_component(
                ComponentData::new("Material")
                    .with_field("albedo_texture", PrimitiveValue::String(path.into())),
            );
        let doc = SceneDocument::new("TexScene").with_root(entity);
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let runtime = mapping.entity(SceneId::new(1)).expect("entity instantiated");
        let source = world
            .ecs()
            .get::<&TextureSource>(runtime)
            .expect("TextureSource attached");
        assert_eq!(source.path, path);
    }

    #[test]
    fn instantiate_scene_flattens_children() {
        let child = SceneEntity::new(SceneId::new(2), "Child").with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        );
        let parent = SceneEntity::new(SceneId::new(1), "Parent")
            .with_component(ComponentData::new("Transform"))
            .with_child(child);
        let doc = SceneDocument::new("Tree").with_root(parent);

        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);

        assert_eq!(mapping.len(), 2);
        assert_eq!(world.len(), 2);
    }
}
