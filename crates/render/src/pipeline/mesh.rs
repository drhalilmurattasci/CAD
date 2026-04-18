//! Mesh pipeline — vertex buffer + uniform bind group + back-face cull.
//!
//! For I-3 this was the single-cube pipeline: one uniform, no depth.
//! I-4 switched the uniform binding to **dynamic offset** so a single
//! buffer can hold N per-instance `TransformUniform` slots and the
//! renderer dispatches many entities by rebinding group 0 with a
//! different byte offset between draws. Still no depth attachment —
//! back-face culling + convex meshes (just the unit cube so far) keep
//! the frame correct without one. Depth arrives alongside concave
//! meshes in a later phase.

use wgpu::{
    BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingType, BufferBindingType,
    ColorTargetState, ColorWrites, CompareFunction, DepthStencilState, Device, Face, FragmentState,
    FrontFace, MultisampleState, PipelineCompilationOptions, PipelineLayoutDescriptor, PolygonMode,
    PrimitiveState, PrimitiveTopology, RenderPipeline, RenderPipelineDescriptor, ShaderModule,
    ShaderStages, StencilState, TextureFormat, VertexBufferLayout, VertexState,
};

/// Construct a mesh pipeline that consumes one vertex buffer + one
/// uniform bind group (group 0, binding 0) and writes to one color
/// target with back-face culling enabled. No depth attachment.
pub struct MeshPipeline {
    pub pipeline:         RenderPipeline,
    pub bind_group_layout: BindGroupLayout,
}

/// Tuning knobs that differ between renderers sharing this pipeline
/// shape. Introduced when the I-11 grid needed LineList topology + a
/// non-dynamic uniform; keeping the knobs explicit avoids a silent
/// divergence between renderers that look alike.
#[derive(Debug, Clone, Copy)]
pub struct MeshPipelineOptions<'a> {
    pub topology:           PrimitiveTopology,
    /// `true` for the cube renderer (many entities packed in one
    /// uniform buffer); `false` for renderers that bind a single-slot
    /// uniform for the whole draw (e.g. the grid).
    pub has_dynamic_offset: bool,
    /// `None` for wireframe passes like the grid; `Some(Face::Back)`
    /// for solid geometry to skip back-facing triangles.
    pub cull_mode:          Option<Face>,
    /// I-30: optional depth-stencil attachment format. `None` keeps
    /// the historical depth-less path (single-cube demos, screenshot
    /// tests). `Some(Depth32Float)` switches the pipeline into
    /// "writes + tests Z" mode, matching an offscreen render target
    /// that carries a depth texture of the same format.
    ///
    /// Grid pipelines want `depth_write: false` so line geometry
    /// occludes correctly without punching into the Z buffer; solid
    /// meshes want it `true`. See [`Self::depth_write_enabled`].
    pub depth_format:       Option<TextureFormat>,
    /// Should the pipeline *write* to the depth buffer, not just
    /// test against it? Solid triangles write; wireframe grids test
    /// so they hide behind opaque geometry without permanently
    /// occluding anything. Ignored when `depth_format` is `None`.
    pub depth_write_enabled: bool,
    /// I-32: optional material texture bind group layout bound at
    /// group 1. When `Some`, the pipeline layout declares a second
    /// bind group so the shader can sample an albedo texture; every
    /// draw must supply a layout-compatible bind group (which
    /// `TextureRegistry` guarantees for all its assets). `None`
    /// keeps the single-group layout used by the grid.
    pub material_texture_layout: Option<&'a BindGroupLayout>,
    /// I-33: optional shadow map bind group layout bound at group 2.
    /// Present when the pipeline wants to sample the directional-light
    /// shadow map in the fragment shader; None keeps a 2-group layout
    /// (main pipelines pre-I-33, or the grid renderer forever).
    pub shadow_map_layout: Option<&'a BindGroupLayout>,
}

impl Default for MeshPipelineOptions<'_> {
    fn default() -> Self {
        Self {
            topology:            PrimitiveTopology::TriangleList,
            has_dynamic_offset:  true,
            cull_mode:           Some(Face::Back),
            depth_format:        None,
            depth_write_enabled: true,
            material_texture_layout: None,
            shadow_map_layout:       None,
        }
    }
}

/// I-33: depth-only shadow pipeline — same vertex layout + uniform
/// bind-group-0 as the main mesh pipeline, but no color targets and
/// no fragment entry point. The result writes only the depth
/// attachment (the shadow map) so the main pass can later sample it
/// for an occlusion test.
///
/// Depth bias is applied at pipeline build time to combat "shadow
/// acne" — surfaces nearly parallel to the light rasterize into the
/// same texel as their shadow plane and self-shadow without an
/// offset. The constants here are conservative defaults that work for
/// the editor's ~1 m scene scale; once the engine grows a user-facing
/// lighting panel they'll move into `DirectionalLight`.
pub struct ShadowPipeline {
    pub pipeline:         RenderPipeline,
    pub bind_group_layout: BindGroupLayout,
}

impl ShadowPipeline {
    pub fn build(
        device: &Device,
        label: &str,
        shader: &ShaderModule,
        vertex_layout: VertexBufferLayout<'_>,
        shadow_depth_format: TextureFormat,
    ) -> Self {
        // Same layout as the main pipeline's bind group 0 — dynamic
        // offset uniform. The shadow pipeline reuses the main
        // renderer's uniform buffer so we don't duplicate uploads.
        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some(&format!("{label}.bgl")),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some(&format!("{label}.layout")),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some(&format!("{label}.pipeline")),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                buffers: &[vertex_layout],
                compilation_options: PipelineCompilationOptions::default(),
            },
            // No fragment stage — the shadow pass writes depth only.
            fragment: None,
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                // No culling. Front-face culling is a common
                // "peter-panning" mitigation, but it breaks open
                // geometry (flat ground quads) — skipping culling
                // plus the depth bias below keeps the whole scene
                // self-consistent.
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(DepthStencilState {
                format: shadow_depth_format,
                depth_write_enabled: true,
                depth_compare:       CompareFunction::Less,
                stencil:             StencilState::default(),
                // Constants tuned against the editor's 1-unit scale +
                // 2048² shadow map. `constant = 2` pushes every
                // fragment two shadow-map texels deeper; `slope = 2.0`
                // scales that push with the surface's tilt so steep
                // faces don't over-darken.
                bias: wgpu::DepthBiasState {
                    constant:   2,
                    slope_scale: 2.0,
                    clamp:      0.0,
                },
            }),
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
        }
    }
}

impl MeshPipeline {
    pub fn build(
        device: &Device,
        label: &str,
        shader: &ShaderModule,
        vertex_layout: VertexBufferLayout<'_>,
        color_target_format: TextureFormat,
    ) -> Self {
        Self::build_with_options(
            device,
            label,
            shader,
            vertex_layout,
            color_target_format,
            MeshPipelineOptions::default(),
        )
    }

    pub fn build_with_options(
        device: &Device,
        label: &str,
        shader: &ShaderModule,
        vertex_layout: VertexBufferLayout<'_>,
        color_target_format: TextureFormat,
        options: MeshPipelineOptions<'_>,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some(&format!("{label}.bgl")),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                // The cube/mesh fragment shader reads `light_dir`,
                // `light_color`, `albedo`, and (I-33) derives shadow
                // UV math from `light_view_proj` that was piped
                // through the vertex stage. Making the uniform
                // visible to both stages is the minimum needed to
                // pass wgpu's shader-vs-layout validation; it doesn't
                // cost anything on the GPU side since unused
                // visibility flags are free. The grid shader doesn't
                // read the uniform from FS but widening the layout
                // to VERTEX_FRAGMENT is still valid there — wgpu
                // only complains when the shader needs a stage the
                // layout *doesn't* declare.
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    // I-4: enable dynamic offsets so CubeRenderer can
                    // pack many per-entity uniforms into one buffer and
                    // walk them by rebinding with a different offset
                    // per `pass.set_bind_group` call. Grid renderer
                    // (I-11) opts out — single uniform per frame.
                    has_dynamic_offset: options.has_dynamic_offset,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // I-32: stitch in the optional material texture layout as
        // bind group 1. Gather into a local Vec so `&[_]` sees a
        // stable slice — `create_pipeline_layout` takes a slice of
        // references, not a slice of owned layouts.
        //
        // I-33: also optionally stitch in the shadow map layout at
        // group 2. Binding slots are positional — we push the
        // material layout first so the shadow layout lands at 2 even
        // when the material layout is `None`, falling back to a
        // material-layout-less pipeline in that shape is future work
        // (no caller wants it today: every renderer that wants
        // shadows also wants material textures).
        let mut group_layouts: Vec<&BindGroupLayout> = vec![&bind_group_layout];
        if let Some(material_layout) = options.material_texture_layout {
            group_layouts.push(material_layout);
        }
        if let Some(shadow_layout) = options.shadow_map_layout {
            assert!(
                options.material_texture_layout.is_some(),
                "shadow_map_layout without material_texture_layout would bind shadows at group 1, \
                 mismatching the shader which expects shadows at group 2",
            );
            group_layouts.push(shadow_layout);
        }
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some(&format!("{label}.layout")),
            bind_group_layouts: &group_layouts,
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some(&format!("{label}.pipeline")),
            layout: Some(&pipeline_layout),
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
            primitive: PrimitiveState {
                topology: options.topology,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                cull_mode: options.cull_mode,
                unclipped_depth: false,
                polygon_mode: PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: options.depth_format.map(|format| DepthStencilState {
                format,
                depth_write_enabled: options.depth_write_enabled,
                depth_compare:       CompareFunction::Less,
                stencil:             StencilState::default(),
                bias:                wgpu::DepthBiasState::default(),
            }),
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
        }
    }
}
