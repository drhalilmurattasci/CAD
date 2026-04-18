//! WGSL shader helpers — embedded source strings + a tiny loader
//! helper. Shaders live alongside this file so they ship with the crate
//! (no runtime file I/O for built-in shaders).

use wgpu::{Device, ShaderModule, ShaderModuleDescriptor, ShaderSource};

/// Source for the standard position-color 2D triangle pipeline.
pub const TRIANGLE_2D_WGSL: &str = include_str!("triangle_2d.wgsl");

/// Source for the 3D MVP cube pipeline.
pub const CUBE_WGSL: &str = include_str!("cube.wgsl");

/// Source for the editor ground grid / axis marker line pipeline.
/// Introduced in I-11 alongside the `GridRenderer`.
pub const GRID_WGSL: &str = include_str!("grid.wgsl");

/// Source for the I-30 fullscreen blit pipeline. `BlitRenderer` uses
/// it to copy the offscreen color texture into the egui-supplied
/// render pass after the depth-aware scene pass has finished.
pub const BLIT_WGSL: &str = include_str!("blit.wgsl");

/// Source for the I-33 depth-only shadow map pipeline. Shared by the
/// cube + mesh instance renderers — both bundle the same vertex
/// layout (`PositionNormalColor3D`) and the same `TransformUniform`
/// at group 0, so one vertex shader suffices.
pub const SHADOW_WGSL: &str = include_str!("shadow.wgsl");

/// Compile a WGSL string into a `ShaderModule` with a labelled
/// descriptor. Thin wrapper, but saves boilerplate at every call site.
pub fn compile_wgsl(device: &Device, label: &str, source: &str) -> ShaderModule {
    device.create_shader_module(ShaderModuleDescriptor {
        label: Some(label),
        source: ShaderSource::Wgsl(source.into()),
    })
}
