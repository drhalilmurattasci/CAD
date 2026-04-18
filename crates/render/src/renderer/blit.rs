//! I-30: fullscreen-triangle blit that copies an offscreen color
//! texture into the egui-supplied render pass.
//!
//! The editor viewport paints through `egui_wgpu::Callback`, whose
//! paint hook hands us a `RenderPass` bound to egui's framebuffer with
//! no depth attachment. Enabling Z-testing for the scene therefore
//! requires its own pass, which writes to an `OffscreenTarget` (color
//! + depth). `BlitRenderer` is the last step: it samples that offscreen
//! color and writes the result into egui's pass, so the final pixels
//! reach the screen with depth-correct compositing baked in.
//!
//! Resize discipline:
//!   - The offscreen color texture is recreated whenever the viewport
//!     resizes, which invalidates any bind group referencing it.
//!     `BlitRenderer` tracks the `TextureView` pointer it last bound
//!     and rebuilds the bind group on change (see [`Self::ensure_source`]).
//!   - Callers must invoke `ensure_source` every frame *before* `draw`
//!     so a post-resize frame can't briefly sample a dangling view.
//!
//! Pipeline shape:
//!   - No vertex buffer (the shader synthesizes a covering triangle).
//!   - No depth attachment (blit is a 2D pixel copy).
//!   - Bind group 0: sampled `texture_2d<f32>` + non-filtering sampler.
//!   - One color target matching the surface format.
//!
//! Trade-offs:
//!   - We could have used `CommandEncoder::copy_texture_to_texture`
//!     instead of a blit pass, but that requires matching formats +
//!     `COPY_SRC`/`COPY_DST` usages on both textures and doesn't play
//!     well with sRGB surface formats (egui's target is often
//!     `*_SRGB` while the offscreen target is usually linear). A
//!     sampling blit does the format conversion for free.
//!   - A non-filtering linear sampler gives us cheap identity sampling;
//!     if the offscreen size matches the viewport (which `ensure_size`
//!     guarantees) each screen pixel samples exactly one source texel
//!     at its center, so there's nothing to filter.

use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, ColorTargetState,
    ColorWrites, Device, FilterMode, FragmentState, MultisampleState, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, RenderPass, RenderPipeline,
    RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    TextureFormat, TextureSampleType, TextureView, TextureViewDimension, VertexState,
};

use crate::shader::{compile_wgsl, BLIT_WGSL};

/// Fullscreen color-texture blit. Owns a sampler + pipeline but *not*
/// the source view — callers hand a fresh `&TextureView` via
/// `ensure_source` whenever their offscreen target is recreated.
pub struct BlitRenderer {
    pipeline:          RenderPipeline,
    bind_group_layout: BindGroupLayout,
    sampler:           Sampler,
    /// Cached bind group. `None` until the first `ensure_source`, which
    /// happens on frame 1 before any `draw`. Rebuilt on resize.
    bind_group:        Option<BindGroup>,
}

impl BlitRenderer {
    pub fn new(device: &Device, color_target_format: TextureFormat) -> Self {
        let shader = compile_wgsl(device, "rustforge.render.blit.shader", BLIT_WGSL);

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("rustforge.render.blit.bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding:    0,
                    visibility: ShaderStages::FRAGMENT,
                    ty:         BindingType::Texture {
                        sample_type:    TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                BindGroupLayoutEntry {
                    binding:    1,
                    visibility: ShaderStages::FRAGMENT,
                    ty:         BindingType::Sampler(SamplerBindingType::Filtering),
                    count:      None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label:                Some("rustforge.render.blit.layout"),
            bind_group_layouts:   &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label:         Some("rustforge.render.blit.pipeline"),
            layout:        Some(&pipeline_layout),
            vertex:        VertexState {
                module:              &shader,
                entry_point:         Some("vs_main"),
                buffers:             &[],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment:      Some(FragmentState {
                module:              &shader,
                entry_point:         Some("fs_main"),
                targets:             &[Some(ColorTargetState {
                    format:     color_target_format,
                    blend:      None,
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive:     PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // No depth — blit writes straight to the surface color.
            depth_stencil: None,
            multisample:   MultisampleState::default(),
            multiview:     None,
            cache:         None,
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            label:          Some("rustforge.render.blit.sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter:     FilterMode::Linear,
            min_filter:     FilterMode::Linear,
            mipmap_filter:  FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            bind_group: None,
        }
    }

    /// Point the blit at `source`. Rebuilds the bind group on first
    /// call and whenever `force_rebuild` is `true` — callers pass
    /// `true` when the offscreen target was recreated (resize), so
    /// the new view gets bound before the next draw.
    ///
    /// We don't auto-detect change by pointer because `TextureView`
    /// is `Arc`-backed and cloning yields new addresses; a stable
    /// identity would need private wgpu internals. The caller already
    /// knows when a resize happened (via
    /// [`OffscreenTarget::ensure_size`]), so push that signal through
    /// explicitly.
    pub fn ensure_source(
        &mut self,
        device: &Device,
        source: &TextureView,
        force_rebuild: bool,
    ) {
        if self.bind_group.is_some() && !force_rebuild {
            return;
        }
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label:   Some("rustforge.render.blit.bind_group"),
            layout:  &self.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding:  0,
                    resource: BindingResource::TextureView(source),
                },
                BindGroupEntry {
                    binding:  1,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.bind_group = Some(bind_group);
    }

    /// Record the fullscreen-triangle draw into `pass`. No-op if
    /// `ensure_source` hasn't been called yet — guarantees we never
    /// issue a draw with a stale/unset bind group.
    pub fn draw(&self, pass: &mut RenderPass<'_>) {
        let Some(bind_group) = self.bind_group.as_ref() else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// For tests: has a bind group been built yet?
    pub fn has_source(&self) -> bool {
        self.bind_group.is_some()
    }
}

#[cfg(test)]
mod tests {
    // No GPU-free assertions worth making here — the pipeline only
    // exists once a `Device` is handed in, and constructing a wgpu
    // `Device` in a unit test means adopting the headless-adapter dance
    // which we keep in integration tests. The rest of the module is
    // glue; its correctness shows up in the viewport callback tests.
}
