//! Composable renderers.
//!
//! A *renderer* in this crate is a small struct that owns GPU resources
//! (buffers, pipelines) and exposes a `draw(render_pass)` method.
//! Renderers are deliberately unaware of how the enclosing pass is
//! created — whether by an egui callback, a windowed game loop, or an
//! offscreen thumbnail generator.
//!
//! Each renderer here is focused on one job. Composition happens at
//! the call site.

pub mod blit;
pub mod cube;
pub mod grid;
pub mod mesh_instance;
pub mod offscreen;
pub mod shadow;
pub mod triangle;

pub use blit::BlitRenderer;
pub use cube::CubeRenderer;
pub use grid::{GridRenderer, GridUniform};
pub use mesh_instance::MeshInstanceRenderer;
pub use offscreen::{OffscreenTarget, VIEWPORT_DEPTH_FORMAT};
pub use shadow::{ShadowMapTarget, DEFAULT_SHADOW_RESOLUTION, SHADOW_MAP_FORMAT};
pub use triangle::TriangleRenderer;
