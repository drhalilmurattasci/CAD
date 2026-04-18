//! I-22: draw meshes out of a `MeshRegistry` with per-instance uniforms.
//!
//! Shape mirrors `CubeRenderer`, but instead of a single baked VBO/IBO
//! every instance carries a `MeshAssetId` that the draw pass resolves
//! against a borrowed `MeshRegistry`. Each instance still uses its own
//! `TransformUniform` slot in a dynamic-offset uniform buffer — the
//! same light + model + view_proj layout as the cube pipeline so both
//! renderers share one WGSL module and one pipeline layout.
//!
//! Why a second renderer instead of rolling into `CubeRenderer`:
//! * The cube renderer holds one VBO/IBO pair; registry meshes each
//!   own their own. Draw bodies diverge at the first GPU call.
//! * Mixing u16 (cube) and u32 (registry) index formats in one
//!   renderer means conditional `set_index_buffer` per instance —
//!   easier to keep them separate and cull the cube path once every
//!   entity carries an explicit mesh asset.
//!
//! Integration contract:
//!   1. caller builds a `MeshRegistry` and uploads every referenced
//!      mesh via `registry.upload(device, id, &upload)`;
//!   2. each frame, caller gathers `(MeshAssetId, TransformUniform)`
//!      for every entity with a non-cube mesh and hands them to
//!      `upload_instances`;
//!   3. `draw(pass, &registry)` dispatches one indexed draw per
//!      instance, silently skipping any id that isn't resident (so a
//!      stale handle from a pending import is an empty frame, not a
//!      panic).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer, BufferBinding,
    BufferUsages, Device, Queue, RenderPass, RenderPipeline, TextureFormat,
};

use crate::camera::TransformUniform;
use crate::mesh::{MeshAssetId, MeshRegistry, PositionNormalColor3D};
use crate::pipeline::{MeshPipeline, MeshPipelineOptions, ShadowPipeline};
use crate::renderer::shadow::SHADOW_MAP_FORMAT;
use crate::shader::{compile_wgsl, CUBE_WGSL, SHADOW_WGSL};
use crate::texture::{TextureAssetId, TextureRegistry};

/// Maximum mesh draws per frame. Matches `CubeRenderer::MAX_INSTANCES`
/// so the two renderers together can cover 512 entities before any
/// batching kicks in — plenty for the small scenes I-18..I-22 target.
pub const MAX_INSTANCES: usize = 256;

/// Same 256-byte padding rule as the cube renderer — see the comment
/// on `CubeRenderer::UNIFORM_STRIDE` for why we go through dynamic
/// offsets rather than storage buffers.
pub const UNIFORM_STRIDE: u64 = 256;

const _: () = assert!(
    std::mem::size_of::<TransformUniform>() as u64 <= UNIFORM_STRIDE,
    "TransformUniform no longer fits in one 256-byte uniform slot"
);

pub struct MeshInstanceRenderer {
    pipeline:    RenderPipeline,
    uniform_buf: Buffer,
    bind_group:  BindGroup,

    /// `(MeshAssetId, slot_offset_bytes, TextureAssetId)` per active
    /// instance. The draw pass walks this list in order, binding the
    /// right uniform slot + asset VBO/IBO + albedo texture for each
    /// entry. Mutex so `upload_*` can stash it from `&self` (paint
    /// callbacks see `&CallbackResources`).
    draw_plan:        Mutex<Vec<(MeshAssetId, u32, TextureAssetId)>>,
    active_instances: AtomicU32,
    /// I-33: depth-only shadow pipeline + bind group over the same
    /// `uniform_buf`. `None` when the renderer was built without
    /// shadow support.
    shadow_pipeline:   Option<RenderPipeline>,
    shadow_bind_group: Option<BindGroup>,
}

impl MeshInstanceRenderer {
    pub fn new(device: &Device, color_target_format: TextureFormat) -> Self {
        Self::new_with_depth(device, color_target_format, None, None, None)
    }

    /// I-30/I-32/I-33: full constructor. See
    /// [`CubeRenderer::new_with_depth`] for the parameter contract —
    /// this renderer matches it one-for-one so the editor can build
    /// both with the same options.
    pub fn new_with_depth(
        device: &Device,
        color_target_format: TextureFormat,
        depth_format: Option<TextureFormat>,
        material_texture_layout: Option<&wgpu::BindGroupLayout>,
        shadow_map_layout: Option<&wgpu::BindGroupLayout>,
    ) -> Self {
        let shader = compile_wgsl(device, "rustforge.render.mesh.shader", CUBE_WGSL);

        let MeshPipeline {
            pipeline,
            bind_group_layout,
        } = MeshPipeline::build_with_options(
            device,
            "rustforge.render.mesh",
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

        let total_size = UNIFORM_STRIDE * MAX_INSTANCES as u64;
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustforge.render.mesh.ubo"),
            size: total_size,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("rustforge.render.mesh.bind_group"),
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

        // I-33: optional shadow pipeline sharing the uniform buffer.
        let (shadow_pipeline, shadow_bind_group) = if shadow_map_layout.is_some() {
            let shadow_shader =
                compile_wgsl(device, "rustforge.render.mesh.shadow.shader", SHADOW_WGSL);
            let ShadowPipeline {
                pipeline:         sp_pipeline,
                bind_group_layout: sp_bgl,
            } = ShadowPipeline::build(
                device,
                "rustforge.render.mesh.shadow",
                &shadow_shader,
                PositionNormalColor3D::layout(),
                SHADOW_MAP_FORMAT,
            );
            let sp_bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("rustforge.render.mesh.shadow.bind_group"),
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

        Self {
            pipeline,
            uniform_buf,
            bind_group,
            draw_plan: Mutex::new(Vec::new()),
            active_instances: AtomicU32::new(0),
            shadow_pipeline,
            shadow_bind_group,
        }
    }

    /// Pack per-instance transform uniforms into the GPU buffer and
    /// record the asset id for each slot.
    ///
    /// Instances beyond `MAX_INSTANCES` are dropped silently — tight
    /// budgets make that the least-bad failure mode (spam a log, skip
    /// the tail). Returns the number of instances actually uploaded.
    pub fn upload_instances(
        &self,
        queue: &Queue,
        instances: &[(MeshAssetId, TransformUniform, TextureAssetId)],
    ) -> u32 {
        let count = instances.len().min(MAX_INSTANCES);
        self.active_instances.store(count as u32, Ordering::Relaxed);

        let mut plan = self
            .draw_plan
            .lock()
            .expect("mesh draw plan mutex poisoned");
        plan.clear();

        if count == 0 {
            return 0;
        }

        let stride = UNIFORM_STRIDE as usize;
        let item_size = std::mem::size_of::<TransformUniform>();
        let mut staging = vec![0u8; stride * count];
        for (i, (id, transform, tex_id)) in instances.iter().take(count).enumerate() {
            let start = i * stride;
            staging[start..start + item_size].copy_from_slice(bytemuck::bytes_of(transform));
            plan.push((*id, (i * stride) as u32, *tex_id));
        }

        queue.write_buffer(&self.uniform_buf, 0, &staging);
        count as u32
    }

    /// Record one indexed draw per instance against the registry.
    /// Instances whose mesh isn't resident in the registry are skipped
    /// silently — their slot is already paid for in the uniform
    /// buffer, but emitting zero triangles is a cleaner failure mode
    /// than panicking halfway through a paint callback.
    ///
    /// I-33: `shadow_bind_group` is bound once at group 2 when
    /// shadows are enabled. See [`CubeRenderer::draw`] for the
    /// contract.
    pub fn draw(
        &self,
        pass: &mut RenderPass<'_>,
        registry: &MeshRegistry,
        textures: &TextureRegistry,
        shadow_bind_group: Option<&wgpu::BindGroup>,
    ) {
        let count = self.active_instances.load(Ordering::Relaxed);
        if count == 0 {
            return;
        }
        let plan = self
            .draw_plan
            .lock()
            .expect("mesh draw plan mutex poisoned");

        pass.set_pipeline(&self.pipeline);
        if let Some(shadow_bg) = shadow_bind_group {
            if self.shadow_pipeline.is_some() {
                pass.set_bind_group(2, shadow_bg, &[]);
            }
        }
        for (asset_id, slot_offset, tex_id) in plan.iter().take(count as usize) {
            let Some(asset) = registry.get(*asset_id) else {
                // Handle not resident (import still pending, say). Skip.
                continue;
            };
            pass.set_bind_group(0, &self.bind_group, &[*slot_offset]);
            pass.set_bind_group(1, textures.bind_group_or_default(*tex_id), &[]);
            pass.set_vertex_buffer(0, asset.vertex_buf.slice(..));
            pass.set_index_buffer(asset.index_buf.slice(..), asset.index_format);
            pass.draw_indexed(0..asset.index_count, 0, 0..1);
        }
    }

    /// I-33: depth-only shadow pass. Mirrors `CubeRenderer::draw_shadow`
    /// but against the `MeshRegistry`. Instances whose asset isn't
    /// resident are skipped — same failure mode as the main pass.
    pub fn draw_shadow(&self, pass: &mut RenderPass<'_>, registry: &MeshRegistry) {
        let count = self.active_instances.load(Ordering::Relaxed);
        if count == 0 {
            return;
        }
        let (Some(shadow_pipeline), Some(shadow_bg)) =
            (self.shadow_pipeline.as_ref(), self.shadow_bind_group.as_ref())
        else {
            return;
        };
        let plan = self
            .draw_plan
            .lock()
            .expect("mesh draw plan mutex poisoned");

        pass.set_pipeline(shadow_pipeline);
        for (asset_id, slot_offset, _tex_id) in plan.iter().take(count as usize) {
            let Some(asset) = registry.get(*asset_id) else {
                continue;
            };
            pass.set_bind_group(0, shadow_bg, &[*slot_offset]);
            pass.set_vertex_buffer(0, asset.vertex_buf.slice(..));
            pass.set_index_buffer(asset.index_buf.slice(..), asset.index_format);
            pass.draw_indexed(0..asset.index_count, 0, 0..1);
        }
    }

    pub fn active_instances(&self) -> u32 {
        self.active_instances.load(Ordering::Relaxed)
    }

    /// For tests / diagnostics: the current draw plan. Clone so we
    /// don't hold the mutex past the call.
    pub fn draw_plan_snapshot(&self) -> Vec<(MeshAssetId, u32, TextureAssetId)> {
        self.draw_plan
            .lock()
            .expect("mesh draw plan mutex poisoned")
            .clone()
    }
}
