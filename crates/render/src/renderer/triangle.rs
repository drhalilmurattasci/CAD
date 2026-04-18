//! `TriangleRenderer` — a minimal VBO-backed triangle.
//!
//! This is the first real renderer in the engine (shipped in I-2). It
//! exists partly as a smoke test for the pipeline + shader + mesh path
//! and partly as a persistent debug-draw primitive.

use wgpu::{util::DeviceExt, Buffer, BufferUsages, Device, RenderPass, RenderPipeline, TextureFormat};

use crate::mesh::{PositionColor2D, TRIANGLE_2D};
use crate::pipeline::StandardPipeline;
use crate::shader::{compile_wgsl, TRIANGLE_2D_WGSL};

/// Owns a pipeline + VBO; `draw` records three vertices into any pass
/// whose color target matches the format the renderer was built with.
pub struct TriangleRenderer {
    pipeline:    RenderPipeline,
    vertex_buf:  Buffer,
    vertex_count: u32,
}

impl TriangleRenderer {
    /// Build GPU state. `color_target_format` must match the format of
    /// the color attachment the caller will bind when issuing
    /// [`Self::draw`].
    pub fn new(device: &Device, color_target_format: TextureFormat) -> Self {
        let shader = compile_wgsl(device, "rustforge.render.triangle.shader", TRIANGLE_2D_WGSL);

        let pipeline = StandardPipeline::build(
            device,
            "rustforge.render.triangle",
            &shader,
            PositionColor2D::layout(),
            color_target_format,
        );

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustforge.render.triangle.vbo"),
            contents: bytemuck::cast_slice(&TRIANGLE_2D),
            usage: BufferUsages::VERTEX,
        });

        Self {
            pipeline,
            vertex_buf,
            vertex_count: TRIANGLE_2D.len() as u32,
        }
    }

    /// Record a triangle draw into an existing render pass. Safe to
    /// call from any callback that holds a `RenderPass` with the
    /// matching color target format.
    ///
    /// Note: wgpu ≥ 0.22 decouples bound-resource lifetimes from the
    /// pass lifetime via internal reference counting, so `self` need
    /// not outlive `pass`.
    pub fn draw(&self, pass: &mut RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }

    pub fn vertex_count(&self) -> u32 {
        self.vertex_count
    }
}
