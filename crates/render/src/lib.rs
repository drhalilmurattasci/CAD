//! RustForge render library — reusable wgpu primitives.
//!
//! This crate is deliberately **free of editor dependencies** (no egui,
//! no eframe) so games, tools, and the editor can all depend on it
//! without pulling UI code. The editor wraps these primitives in
//! `egui_wgpu::CallbackTrait` impls; a standalone game loop wires them
//! directly into a window surface.
//!
//! ## Module layout
//!
//! - [`surface`] — global target format tracking so pipelines built
//!   lazily can match the wgpu surface negotiated at startup.
//! - [`shader`] — WGSL source helpers and embedded shader strings.
//! - [`mesh`] — vertex types + primitive mesh data.
//! - [`pipeline`] — pipeline builders that fluently wrap the verbose
//!   `wgpu::RenderPipelineDescriptor`.
//! - [`renderer`] — composable renderers. First citizen:
//!   [`renderer::TriangleRenderer`], used by the editor viewport in I-2.

pub mod camera;
pub mod mesh;
pub mod pipeline;
pub mod renderer;
pub mod shader;
pub mod surface;
pub mod texture;

/// Convenience prelude — re-exports that most consumers will want.
pub mod prelude {
    pub use crate::camera::{
        directional_light_view_proj, Camera, DirectionalLight, OrbitCamera, TransformUniform,
    };
    pub use crate::mesh::{PositionColor2D, PositionColor3D, PositionNormalColor3D, TRIANGLE_2D};
    pub use crate::pipeline::{MeshPipeline, MeshPipelineOptions, ShadowPipeline, StandardPipeline};
    pub use crate::renderer::{
        BlitRenderer, CubeRenderer, GridRenderer, GridUniform, MeshInstanceRenderer,
        OffscreenTarget, ShadowMapTarget, TriangleRenderer, DEFAULT_SHADOW_RESOLUTION,
        SHADOW_MAP_FORMAT, VIEWPORT_DEPTH_FORMAT,
    };
    pub use crate::surface::{install_target_format, target_format};
    pub use crate::texture::{TextureAsset, TextureAssetId, TextureRegistry, TextureUpload};
}
