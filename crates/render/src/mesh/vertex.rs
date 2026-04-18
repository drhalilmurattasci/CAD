//! Vertex layouts used by built-in pipelines.
//!
//! Each vertex struct is `#[repr(C)]` + `Pod` + `Zeroable` so
//! `bytemuck::cast_slice` can hand raw bytes to wgpu, and each pairs
//! with a `vertex_attr_array![...]` that the pipeline builder consumes.

use wgpu::{VertexBufferLayout, VertexStepMode};

/// 2D position + per-vertex RGB color. Used by
/// [`crate::shader::TRIANGLE_2D_WGSL`] — matches its `VsIn` layout.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable, Debug)]
pub struct PositionColor2D {
    pub position: [f32; 2],
    pub color:    [f32; 3],
}

impl PositionColor2D {
    /// Attribute locations must match the WGSL `@location(...)` bindings.
    /// Kept as a const so callers can reference it from pipeline setup
    /// without re-declaring.
    pub const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x2,  // position
        1 => Float32x3,  // color
    ];

    /// The `VertexBufferLayout` this vertex type maps to.
    pub const fn layout() -> VertexBufferLayout<'static> {
        VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: Self::ATTRIBUTES,
        }
    }
}

/// 3D position + per-vertex RGB color. Used by the I-3 cube mesh and
/// any other 3D primitive that doesn't yet need normals / UVs.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable, Debug)]
pub struct PositionColor3D {
    pub position: [f32; 3],
    pub color:    [f32; 3],
}

impl PositionColor3D {
    pub const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x3,  // position
        1 => Float32x3,  // color
    ];

    pub const fn layout() -> VertexBufferLayout<'static> {
        VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: Self::ATTRIBUTES,
        }
    }
}

/// 3D position + object-space normal + per-vertex RGB color.
///
/// Added in I-12 for the lit cube shader. Normals are per-vertex (not
/// derived in the fragment shader) so the Lambert shading respects the
/// authored per-face orientation — the cube mesh uses 24 vertices (4
/// per face) precisely so faces don't share normals across the
/// boundary and get soft-shaded in a way they shouldn't.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable, Debug)]
pub struct PositionNormalColor3D {
    pub position: [f32; 3],
    pub normal:   [f32; 3],
    pub color:    [f32; 3],
}

impl PositionNormalColor3D {
    pub const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x3,  // position
        1 => Float32x3,  // normal
        2 => Float32x3,  // color
    ];

    pub const fn layout() -> VertexBufferLayout<'static> {
        VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: Self::ATTRIBUTES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_color_2d_size_matches_layout_stride() {
        assert_eq!(
            std::mem::size_of::<PositionColor2D>() as u64,
            PositionColor2D::layout().array_stride
        );
    }

    #[test]
    fn position_color_2d_has_two_attributes() {
        assert_eq!(PositionColor2D::ATTRIBUTES.len(), 2);
    }

    #[test]
    fn position_color_3d_size_matches_layout_stride() {
        assert_eq!(
            std::mem::size_of::<PositionColor3D>() as u64,
            PositionColor3D::layout().array_stride
        );
    }

    #[test]
    fn position_color_3d_layout_is_24_bytes() {
        // 3×f32 position + 3×f32 color = 6×4 = 24 bytes, no padding.
        assert_eq!(std::mem::size_of::<PositionColor3D>(), 24);
    }
}
