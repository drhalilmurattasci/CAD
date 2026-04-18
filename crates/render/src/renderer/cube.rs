//! `CubeRenderer` — rotating 3D cube(s) with a per-instance MVP uniform.
//!
//! History:
//!  - I-3: single cube, one uniform buffer.
//!  - I-4: many cubes, one dynamic-offset uniform buffer holding up to
//!    `MAX_INSTANCES` packed `TransformUniform` slots. The caller
//!    (`editor-ui::viewport_3d`) snapshots world entities and hands us
//!    a `&[TransformUniform]`; we upload and dispatch one indexed draw
//!    per instance by rebinding group 0 with a different byte offset.
//!
//! Why dynamic offsets rather than storage buffers or push constants:
//!   - portable to all wgpu backends (including web/wasm).
//!   - zero shader changes from I-3.
//!   - no feature flags, no alignment surprises past the fixed 256-byte
//!     offset alignment limit (which we respect by padding each slot
//!     out to `UNIFORM_STRIDE`).
//!
//! Consumers call:
//!   1. `CubeRenderer::new(device, color_target_format)` once.
//!   2. `CubeRenderer::upload_instances(queue, &[TransformUniform])`
//!      each frame with one uniform per entity.
//!   3. `CubeRenderer::draw(render_pass)` inside a pass.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use wgpu::{
    util::DeviceExt, BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer,
    BufferBinding, BufferUsages, Device, IndexFormat, Queue, RenderPass, RenderPipeline,
    TextureFormat,
};

use crate::camera::TransformUniform;
use crate::mesh::{PositionNormalColor3D, CUBE_INDICES, CUBE_LIT_VERTICES};
use crate::pipeline::{MeshPipeline, MeshPipelineOptions, ShadowPipeline};
use crate::renderer::shadow::SHADOW_MAP_FORMAT;
use crate::shader::{compile_wgsl, CUBE_WGSL, SHADOW_WGSL};
use crate::texture::{TextureAssetId, TextureRegistry};

/// Max renderable cubes per frame until we add a proper asset/renderer
/// scheduler. 256 is enough for everything I-4..I-9 need.
pub const MAX_INSTANCES: usize = 256;

/// Byte stride between `TransformUniform` slots. Every uniform binding
/// must align to `min_uniform_buffer_offset_alignment`, which is 256 on
/// every backend wgpu defaults target. `TransformUniform` is 128 bytes,
/// so we pad to 256.
pub const UNIFORM_STRIDE: u64 = 256;

const _: () = assert!(
    std::mem::size_of::<TransformUniform>() as u64 <= UNIFORM_STRIDE,
    "TransformUniform no longer fits in one 256-byte uniform slot"
);

pub struct CubeRenderer {
    pipeline:    RenderPipeline,
    vertex_buf:  Buffer,
    index_buf:   Buffer,
    index_count: u32,
    /// One big uniform buffer sized for `MAX_INSTANCES * UNIFORM_STRIDE`
    /// bytes. Writes are sparse — we only touch the first
    /// `active_instances` slots each frame.
    uniform_buf: Buffer,
    /// Bind group over the first `UNIFORM_STRIDE` bytes of
    /// `uniform_buf`; dynamic offset picks which slot at draw time.
    bind_group:        BindGroup,
    /// Count stashed by `upload_instances` and consumed by `draw`.
    /// Atomic so both methods stay `&self`, matching egui_wgpu's
    /// `CallbackResources` ergonomics (paint sees `&CallbackResources`)
    /// and satisfying the `Send + Sync` bound on resource insertion.
    active_instances:  AtomicU32,
    /// I-32: per-instance albedo texture handle, captured at
    /// `upload_instances` time and consumed by `draw` so the render
    /// pass can resolve the right bind group from a borrowed
    /// `TextureRegistry`. Mutex so we can mutate from `&self` while
    /// `paint` takes `&CallbackResources`.
    draw_textures:    Mutex<Vec<TextureAssetId>>,
    /// I-33: depth-only shadow pipeline + its dedicated bind group
    /// over the same `uniform_buf`. The shadow pipeline has its own
    /// bind-group-layout (no material textures, no shadow sampler —
    /// just the uniform), so we need a distinct bind group even
    /// though it targets the same underlying buffer.
    shadow_pipeline:   Option<RenderPipeline>,
    shadow_bind_group: Option<BindGroup>,
}

impl CubeRenderer {
    pub fn new(device: &Device, color_target_format: TextureFormat) -> Self {
        Self::new_with_depth(device, color_target_format, None, None, None)
    }

    /// I-30/I-32/I-33: construct a CubeRenderer.
    ///
    /// * `depth_format: None` keeps the historical no-depth path;
    ///   `Some(format)` enables Less-compare + depth write.
    /// * `material_texture_layout: None` keeps the single-bind-group
    ///   layout (the shader samples the DEFAULT_WHITE texture through
    ///   the layout, so `Some` is effectively required for real
    ///   rendering now — the optional form stays for tests).
    /// * `shadow_map_layout: None` disables the shadow pipeline —
    ///   the main draw still runs, but the shader samples the
    ///   identity-default shadow binding. `Some` enables:
    ///     * a second render pipeline (depth-only, into the shadow
    ///       map) exposed by [`Self::draw_shadow`];
    ///     * a fragment-shader shadow lookup in the main pipeline at
    ///       bind group 2.
    pub fn new_with_depth(
        device: &Device,
        color_target_format: TextureFormat,
        depth_format: Option<TextureFormat>,
        material_texture_layout: Option<&wgpu::BindGroupLayout>,
        shadow_map_layout: Option<&wgpu::BindGroupLayout>,
    ) -> Self {
        let shader = compile_wgsl(device, "rustforge.render.cube.shader", CUBE_WGSL);

        let MeshPipeline {
            pipeline,
            bind_group_layout,
        } = MeshPipeline::build_with_options(
            device,
            "rustforge.render.cube",
            &shader,
            PositionNormalColor3D::layout(),
            color_target_format,
            MeshPipelineOptions {
                depth_format,
                depth_write_enabled: true,
                material_texture_layout,
                shadow_map_layout,
                ..Default::default()
            },
        );

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustforge.render.cube.vbo"),
            contents: bytemuck::cast_slice(&CUBE_LIT_VERTICES),
            usage: BufferUsages::VERTEX,
        });

        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustforge.render.cube.ibo"),
            contents: bytemuck::cast_slice(&CUBE_INDICES),
            usage: BufferUsages::INDEX,
        });

        let total_size = UNIFORM_STRIDE * MAX_INSTANCES as u64;
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustforge.render.cube.ubo"),
            size: total_size,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group covers *one* slot — the dynamic offset at draw time
        // slides that binding across the full buffer.
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("rustforge.render.cube.bind_group"),
            layout: &bind_group_layout,
            entries: &[BindGroupEntry {
                binding:  0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &uniform_buf,
                    offset: 0,
                    size:   std::num::NonZeroU64::new(
                        std::mem::size_of::<TransformUniform>() as u64,
                    ),
                }),
            }],
        });

        // I-33: build the shadow pipeline when the caller supplied a
        // shadow layout. The shadow pipeline has its own uniform
        // bind-group layout (no textures), but binds the same
        // underlying `uniform_buf` — so we create a second bind group
        // over that buffer.
        let (shadow_pipeline, shadow_bind_group) = if shadow_map_layout.is_some() {
            let shadow_shader =
                compile_wgsl(device, "rustforge.render.cube.shadow.shader", SHADOW_WGSL);
            let ShadowPipeline {
                pipeline:         sp_pipeline,
                bind_group_layout: sp_bgl,
            } = ShadowPipeline::build(
                device,
                "rustforge.render.cube.shadow",
                &shadow_shader,
                PositionNormalColor3D::layout(),
                SHADOW_MAP_FORMAT,
            );
            let sp_bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("rustforge.render.cube.shadow.bind_group"),
                layout: &sp_bgl,
                entries: &[BindGroupEntry {
                    binding:  0,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer: &uniform_buf,
                        offset: 0,
                        size:   std::num::NonZeroU64::new(
                            std::mem::size_of::<TransformUniform>() as u64,
                        ),
                    }),
                }],
            });
            (Some(sp_pipeline), Some(sp_bg))
        } else {
            (None, None)
        };

        let _ = total_size;
        Self {
            pipeline,
            vertex_buf,
            index_buf,
            index_count: CUBE_INDICES.len() as u32,
            uniform_buf,
            bind_group,
            active_instances: AtomicU32::new(0),
            draw_textures: Mutex::new(Vec::new()),
            shadow_pipeline,
            shadow_bind_group,
        }
    }

    /// Upload one `TransformUniform` per entity to draw this frame.
    /// Returns the number of entities accepted (capped at
    /// `MAX_INSTANCES`). I-32: every instance also carries a
    /// `TextureAssetId` so the draw pass can bind an albedo texture
    /// per-entity. Callers that don't care about textures can pass
    /// `TextureAssetId::DEFAULT_WHITE` and get pre-I-32 output.
    pub fn upload_instances(
        &self,
        queue: &Queue,
        instances: &[(TransformUniform, TextureAssetId)],
    ) -> u32 {
        let count = instances.len().min(MAX_INSTANCES);
        self.active_instances.store(count as u32, Ordering::Relaxed);

        let mut textures = self
            .draw_textures
            .lock()
            .expect("cube draw texture list mutex poisoned");
        textures.clear();

        if count == 0 {
            return 0;
        }

        // Pack instances into a stride-aligned byte buffer. Bytes
        // between `size_of::<TransformUniform>()` and `UNIFORM_STRIDE`
        // are padding and never read by the shader. Allocated per
        // frame — 256 entities × 256 bytes = 64 KiB, cheap.
        let stride = UNIFORM_STRIDE as usize;
        let item_size = std::mem::size_of::<TransformUniform>();
        let mut staging = vec![0u8; stride * count];
        for (i, (u, tex_id)) in instances.iter().take(count).enumerate() {
            let start = i * stride;
            staging[start..start + item_size].copy_from_slice(bytemuck::bytes_of(u));
            textures.push(*tex_id);
        }

        queue.write_buffer(&self.uniform_buf, 0, &staging);
        count as u32
    }

    /// Record one indexed cube draw per active instance. Each draw
    /// rebinds group 0 with a different byte offset into the shared
    /// uniform buffer and group 1 to the instance's albedo texture
    /// resolved through the borrowed `TextureRegistry`. Missing
    /// textures fall back to `DEFAULT_WHITE` silently (see
    /// `TextureRegistry::bind_group_or_default`).
    ///
    /// I-33: `shadow_bind_group` is bound at group 2 when the
    /// pipeline was built with a shadow-map layout. Callers always
    /// pass the current shadow bind group — if the pipeline was built
    /// without shadows, the bind group is silently ignored (wgpu
    /// permits extra bind groups as long as the pipeline layout
    /// doesn't reference them).
    pub fn draw(
        &self,
        pass: &mut RenderPass<'_>,
        textures: &TextureRegistry,
        shadow_bind_group: Option<&wgpu::BindGroup>,
    ) {
        let count = self.active_instances.load(Ordering::Relaxed);
        if count == 0 {
            return;
        }
        let tex_ids = self
            .draw_textures
            .lock()
            .expect("cube draw texture list mutex poisoned");

        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.set_index_buffer(self.index_buf.slice(..), IndexFormat::Uint16);
        // Bind the shadow map once per pass — same for every instance.
        // When the pipeline was built without shadow support, skip.
        if let Some(shadow_bg) = shadow_bind_group {
            if self.shadow_pipeline.is_some() {
                pass.set_bind_group(2, shadow_bg, &[]);
            }
        }
        for i in 0..count {
            let offset = (i as u64 * UNIFORM_STRIDE) as u32;
            pass.set_bind_group(0, &self.bind_group, &[offset]);
            let tex_id = tex_ids
                .get(i as usize)
                .copied()
                .unwrap_or(TextureAssetId::DEFAULT_WHITE);
            pass.set_bind_group(1, textures.bind_group_or_default(tex_id), &[]);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
    }

    /// I-33: depth-only shadow pass. Runs the shadow pipeline over
    /// the same instance list `upload_instances` already uploaded —
    /// binds group 0 once per instance with the per-slot dynamic
    /// offset, no textures, no fragment work. No-op when the renderer
    /// was built without a shadow pipeline.
    pub fn draw_shadow(&self, pass: &mut RenderPass<'_>) {
        let count = self.active_instances.load(Ordering::Relaxed);
        if count == 0 {
            return;
        }
        let (Some(shadow_pipeline), Some(shadow_bg)) =
            (self.shadow_pipeline.as_ref(), self.shadow_bind_group.as_ref())
        else {
            return;
        };
        pass.set_pipeline(shadow_pipeline);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.set_index_buffer(self.index_buf.slice(..), IndexFormat::Uint16);
        for i in 0..count {
            let offset = (i as u64 * UNIFORM_STRIDE) as u32;
            pass.set_bind_group(0, shadow_bg, &[offset]);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
    }

    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    pub fn active_instances(&self) -> u32 {
        self.active_instances.load(Ordering::Relaxed)
    }
}
