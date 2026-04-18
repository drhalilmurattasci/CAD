//! Pipeline builders.
//!
//! `wgpu::RenderPipelineDescriptor` is verbose — 15+ fields of mostly
//! defaults. This module offers fluent builders that cover common cases
//! (position+color vertex, no depth, single color target) while still
//! letting callers drop down to raw `wgpu` for advanced needs.
//!
//! Current citizens:
//! - [`StandardPipeline`] — single-VBO, no depth, one color target.
//!
//! Future additions will include:
//! - `DepthPipeline` for I-3 (adds depth attachment + depth-stencil state).
//! - `InstancedPipeline` once instancing lands.

pub mod builder;
pub mod mesh;

pub use builder::StandardPipeline;
pub use mesh::{MeshPipeline, MeshPipelineOptions, ShadowPipeline};
