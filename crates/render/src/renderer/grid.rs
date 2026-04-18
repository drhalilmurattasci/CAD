//! `GridRenderer` — editor ground plane + world-origin axis markers.
//!
//! Introduced in I-11. The goal is to give the viewport a sense of
//! scale and orientation without pulling in a full infinite-grid
//! shader: a finite line-list mesh is cheap, procedurally generated at
//! construction time, and compiles down to a single draw call per
//! frame.
//!
//! Why line-list + procedural geometry (rather than the common fragment-
//! shader "infinite grid" trick):
//!   - Portable to every wgpu backend with zero fragment derivatives.
//!   - Deterministic — the tests can count vertices and verify colors.
//!   - No shimmering at glancing angles: real geometry, real
//!     rasterizer, no screen-space aliasing.
//!
//! What we draw:
//!   1. A square grid on the XZ plane centered at the origin. Every
//!      `MAJOR_EVERY` line gets a slightly brighter color so ten-unit
//!      divisions read at a glance.
//!   2. Three cardinal axis lines through the origin colored R/G/B for
//!      X/Y/Z — a free world-gizmo that also confirms handedness.
//!
//! The mesh is built once, uploaded, and reused every frame. Only the
//! 64-byte camera `view_proj` uniform needs to change per frame.

use wgpu::{
    util::DeviceExt, BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer,
    BufferBinding, BufferUsages, Device, PrimitiveTopology, Queue, RenderPass, RenderPipeline,
    TextureFormat,
};

use crate::mesh::PositionColor3D;
use crate::pipeline::{MeshPipeline, MeshPipelineOptions};
use crate::shader::{compile_wgsl, GRID_WGSL};

/// GPU-side view-projection uniform used by the grid shader. Padded
/// out to the 256-byte alignment so reusing [`CubeRenderer`]'s dynamic-
/// offset machinery later is painless if we ever share the buffer.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable, Debug)]
pub struct GridUniform {
    pub view_proj: [[f32; 4]; 4],
}

/// Number of lines on each side of the origin along X and Z. The grid
/// covers `[-HALF_EXTENT, HALF_EXTENT]` in world units.
pub const HALF_EXTENT: i32 = 20;
/// World-unit spacing between grid lines. Matches the unit-cube size so
/// a default cube sits neatly inside a cell.
pub const CELL_SIZE: f32 = 1.0;
/// Every Nth line gets the major color. N=10 gives Blender/Unity
/// ten-unit subdivisions.
pub const MAJOR_EVERY: i32 = 10;

const MINOR_COLOR: [f32; 3] = [0.22, 0.22, 0.24];
const MAJOR_COLOR: [f32; 3] = [0.38, 0.38, 0.42];
/// World-axis colors — standard RGB = XYZ. Used by the three cardinal
/// lines drawn through the origin so +X / +Y / +Z read at a glance.
const AXIS_X_COLOR: [f32; 3] = [0.85, 0.20, 0.20];
const AXIS_Y_COLOR: [f32; 3] = [0.20, 0.75, 0.20];
const AXIS_Z_COLOR: [f32; 3] = [0.20, 0.40, 0.90];
/// Length of each cardinal axis line — long enough to escape the grid
/// so the world gizmo is visible when orbited.
const AXIS_LENGTH: f32 = HALF_EXTENT as f32 + 2.0;

pub struct GridRenderer {
    pipeline:    RenderPipeline,
    vertex_buf:  Buffer,
    vertex_count: u32,
    uniform_buf: Buffer,
    bind_group:  BindGroup,
}

impl GridRenderer {
    pub fn new(device: &Device, color_target_format: TextureFormat) -> Self {
        Self::new_with_depth(device, color_target_format, None)
    }

    /// I-30: depth-aware variant. The grid *tests* depth (so it hides
    /// behind opaque geometry) but does **not** write depth — that way
    /// lines can't silently occlude solid meshes that happen to draw
    /// later, and transparent visuals composited on top still have a
    /// clean Z buffer to sort against.
    pub fn new_with_depth(
        device: &Device,
        color_target_format: TextureFormat,
        depth_format: Option<TextureFormat>,
    ) -> Self {
        let shader = compile_wgsl(device, "rustforge.render.grid.shader", GRID_WGSL);

        let MeshPipeline {
            pipeline,
            bind_group_layout,
        } = MeshPipeline::build_with_options(
            device,
            "rustforge.render.grid",
            &shader,
            PositionColor3D::layout(),
            color_target_format,
            MeshPipelineOptions {
                topology:            PrimitiveTopology::LineList,
                has_dynamic_offset:  false,
                // Lines have no winding; culling would silently drop
                // half the segments depending on vertex order.
                cull_mode:           None,
                depth_format,
                depth_write_enabled: false,
                // Grid is a wireframe pass with no texturing — keep
                // the single-group pipeline layout.
                material_texture_layout: None,
                // And no shadow sampling — the editor doesn't shadow
                // its world gizmo.
                shadow_map_layout: None,
            },
        );

        let vertices = build_grid_vertices();
        let vertex_count = vertices.len() as u32;

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rustforge.render.grid.vbo"),
            contents: bytemuck::cast_slice(&vertices),
            usage: BufferUsages::VERTEX,
        });

        // Single uniform slot holding just view_proj — the grid mesh is
        // already in world space, no per-line model matrix needed.
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustforge.render.grid.ubo"),
            size: std::mem::size_of::<GridUniform>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("rustforge.render.grid.bind_group"),
            layout: &bind_group_layout,
            entries: &[BindGroupEntry {
                binding:  0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &uniform_buf,
                    offset: 0,
                    size:   std::num::NonZeroU64::new(
                        std::mem::size_of::<GridUniform>() as u64,
                    ),
                }),
            }],
        });

        Self {
            pipeline,
            vertex_buf,
            vertex_count,
            uniform_buf,
            bind_group,
        }
    }

    /// Upload the current frame's camera view-projection. Cheap — one
    /// 64-byte copy — so callers should just call it every frame rather
    /// than dirty-tracking the camera.
    pub fn upload_view_proj(&self, queue: &Queue, view_proj: glam::Mat4) {
        let uniform = GridUniform {
            view_proj: view_proj.to_cols_array_2d(),
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Record the grid + axis lines as a single draw call.
    pub fn draw(&self, pass: &mut RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }

    pub fn vertex_count(&self) -> u32 {
        self.vertex_count
    }
}

/// Build the full vertex buffer: grid lines on XZ plane + three axis
/// cardinals. Kept as a free fn so tests can sample it without a GPU.
pub fn build_grid_vertices() -> Vec<PositionColor3D> {
    let mut out =
        Vec::with_capacity(((HALF_EXTENT * 2 + 1) as usize) * 4 + 6 /* axis endpoints */);
    let extent = HALF_EXTENT as f32 * CELL_SIZE;

    for i in -HALF_EXTENT..=HALF_EXTENT {
        // Skip lines at exactly x=0 and z=0 — the cardinal axes drawn
        // below replace those with brighter colors. Avoids z-fighting
        // from two overlapping line segments.
        let color = if i % MAJOR_EVERY == 0 {
            MAJOR_COLOR
        } else {
            MINOR_COLOR
        };
        let p = i as f32 * CELL_SIZE;

        // Lines parallel to Z (constant x).
        if i != 0 {
            out.push(PositionColor3D {
                position: [p, 0.0, -extent],
                color,
            });
            out.push(PositionColor3D {
                position: [p, 0.0, extent],
                color,
            });
        }
        // Lines parallel to X (constant z).
        if i != 0 {
            out.push(PositionColor3D {
                position: [-extent, 0.0, p],
                color,
            });
            out.push(PositionColor3D {
                position: [extent, 0.0, p],
                color,
            });
        }
    }

    // Cardinal axes through the origin. Each is drawn as the full
    // length (negative side → positive side) so users can see both the
    // +axis and its mirror.
    out.push(PositionColor3D {
        position: [-AXIS_LENGTH, 0.0, 0.0],
        color:    AXIS_X_COLOR,
    });
    out.push(PositionColor3D {
        position: [AXIS_LENGTH, 0.0, 0.0],
        color:    AXIS_X_COLOR,
    });
    out.push(PositionColor3D {
        position: [0.0, -AXIS_LENGTH, 0.0],
        color:    AXIS_Y_COLOR,
    });
    out.push(PositionColor3D {
        position: [0.0, AXIS_LENGTH, 0.0],
        color:    AXIS_Y_COLOR,
    });
    out.push(PositionColor3D {
        position: [0.0, 0.0, -AXIS_LENGTH],
        color:    AXIS_Z_COLOR,
    });
    out.push(PositionColor3D {
        position: [0.0, 0.0, AXIS_LENGTH],
        color:    AXIS_Z_COLOR,
    });

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_vertex_count_is_deterministic() {
        let verts = build_grid_vertices();
        // For each i in [-HALF_EXTENT..=HALF_EXTENT] except 0 we emit 4
        // vertices (2 for the Z-parallel line, 2 for the X-parallel).
        // Plus 6 vertices for the three cardinal axis pairs.
        let lines_per_side = HALF_EXTENT as usize * 2;
        let expected = lines_per_side * 4 + 6;
        assert_eq!(verts.len(), expected);
    }

    #[test]
    fn grid_does_not_overlap_cardinal_axis_lines() {
        let verts = build_grid_vertices();
        // No non-axis vertex should sit exactly at (0,0,*) or (*,0,0)
        // — those positions are reserved for the R/G/B cardinals.
        let axis_colors = [AXIS_X_COLOR, AXIS_Y_COLOR, AXIS_Z_COLOR];
        for v in &verts {
            let on_x_axis = v.position[0] == 0.0 && v.position[1] == 0.0;
            let on_z_axis = v.position[1] == 0.0 && v.position[2] == 0.0;
            if on_x_axis || on_z_axis {
                assert!(
                    axis_colors.contains(&v.color),
                    "vertex {:?} sits on a cardinal axis but is not an axis color",
                    v.position
                );
            }
        }
    }

    #[test]
    fn grid_uniform_is_64_bytes() {
        // One mat4x4<f32> = 64 bytes — matches the grid shader's uniform.
        assert_eq!(std::mem::size_of::<GridUniform>(), 64);
    }
}
