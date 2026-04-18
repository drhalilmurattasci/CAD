// Allow `#[derive(Reflect)]`-generated code inside this crate to refer
// to paths as `::engine::reflection::...`. External consumers
// get that path naturally; `extern crate self as engine` makes
// it resolve for our own tests too.
extern crate self as engine;

pub mod assets;
pub mod audio;
pub mod capabilities;
pub mod commands;
// Editor ↔ engine hook / adapter layer. Renamed from `engine` to
// `hooks` during the rustforge-prefix removal so this inner module
// stops shadowing the crate's own `engine` self-alias (`extern crate
// self as engine`) — without the rename, `::engine::reflection` and
// friends couldn't resolve because `mod engine` would occupy the slot.
pub mod hooks;
pub mod events;
pub mod input;
pub mod mesh;
pub mod picking;
pub mod play;
pub mod reflection;
pub mod scene;
pub mod scripting;
pub mod world;

pub mod prelude {
    pub use crate::assets::{AssetKind, AssetMeta};
    pub use crate::audio::{
        audio_handle_for_source, AudioClipHandle, AudioCommand, AudioSource,
    };
    pub use crate::capabilities::{Capability, ModManifest, PluginManifest, ServiceAdapterManifest};
    pub use crate::commands::{
        Command, CommandError, CommandStack, NudgeTransformCommand, RenameEntityCommand,
        RotateTransformCommand, ScaleTransformCommand, SetComponentFieldCommand,
        SpawnEntityCommand, SpawnPrefabCommand,
    };
    pub use crate::hooks::{EngineHooks, MockEngine, PickRequest, RenderRequest};
    pub use crate::mesh::{MeshData, MeshImportError};
    pub use crate::play::PlayModeSession;
    pub use crate::picking::{pick_entity, pick_gizmo, Aabb, GizmoAxis, GizmoLayout, Ray};
    pub use crate::events::{EditorEvent, EventBus, PlayModeState};
    pub use crate::reflection::{
        ComponentRegistry, FieldDescriptor, Reflect, ReflectDerive, RegisteredComponent, ValueKind,
    };
    pub use crate::scene::{
        ComponentData, IdAllocator, PrimitiveValue, PrefabDocument, SceneDocument, SceneEntity, SceneId,
    };
    pub use crate::input::{Input, Key};
    pub use crate::scripting::{ScriptError, ScriptHost};
    pub use crate::world::{
        mesh_handle_for_source, texture_handle_for_source, Camera, Collider, DirectionalLight,
        Entity, Material, MaterialHandle, MeshHandle, MeshSource, Mover, Parent, RenderEntity,
        RigidBody, SceneInstantiation, Script, TextureHandle, TextureSource, Transform, World,
    };
}
