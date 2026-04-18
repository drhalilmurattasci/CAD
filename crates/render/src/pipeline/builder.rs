//! Fluent pipeline builders.

use wgpu::{
    ColorTargetState, ColorWrites, Device, FragmentState, MultisampleState, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PrimitiveState, RenderPipeline, RenderPipelineDescriptor, ShaderModule,
    TextureFormat, VertexBufferLayout, VertexState,
};

/// A minimal render pipeline configuration for single-VBO, no-depth,
/// one color target draws. The returned pipeline uses no bind groups,
/// no push constants, and triangle-list primitive topology.
///
/// This covers the I-2 triangle renderer and will stay useful as a
/// fallback / debug-draw pipeline even once I-3 introduces depth and
/// uniform bind groups.
pub struct StandardPipeline;

impl StandardPipeline {
    /// Build a pipeline.
    ///
    /// - `label` — debug label passed to all wgpu objects.
    /// - `shader` — compiled shader module with `vs_main` and `fs_main`.
    /// - `vertex_layout` — vertex buffer layout (one slot, slot 0).
    /// - `color_target_format` — must match the color attachment the
    ///   caller will bind when recording draws.
    pub fn build(
        device: &Device,
        label: &str,
        shader: &ShaderModule,
        vertex_layout: VertexBufferLayout<'_>,
        color_target_format: TextureFormat,
    ) -> RenderPipeline {
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some(&format!("{label}.layout")),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some(&format!("{label}.pipeline")),
            layout: Some(&layout),
            vertex: VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                buffers: &[vertex_layout],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format: color_target_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }
}
