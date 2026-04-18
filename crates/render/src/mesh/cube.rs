//! Unit cube primitive mesh.
//!
//! 24 vertices (4 per face × 6 faces) and 36 indices (2 triangles per
//! face × 6 faces). Each face has its own vertices so we can assign a
//! distinct color per face without blending across shared vertices.
//!
//! Winding order is counter-clockwise as viewed from outside the cube,
//! matching `wgpu::FrontFace::Ccw` (the wgpu default) so back-face
//! culling works without a custom `FrontFace` setting.

use super::vertex::{PositionColor3D, PositionNormalColor3D};

const RED:         [f32; 3] = [0.90, 0.30, 0.30];
const DARK_RED:    [f32; 3] = [0.55, 0.18, 0.18];
const GREEN:       [f32; 3] = [0.30, 0.85, 0.40];
const DARK_GREEN:  [f32; 3] = [0.20, 0.50, 0.25];
const BLUE:        [f32; 3] = [0.35, 0.50, 1.00];
const DARK_BLUE:   [f32; 3] = [0.20, 0.30, 0.60];

/// Unit cube spanning `[-0.5, 0.5]^3`. One entry per vertex; four
/// vertices per face in CCW order when viewed from outside.
pub const CUBE_VERTICES: [PositionColor3D; 24] = [
    // +X face (right) — red
    PositionColor3D { position: [ 0.5, -0.5, -0.5], color: RED },
    PositionColor3D { position: [ 0.5, -0.5,  0.5], color: RED },
    PositionColor3D { position: [ 0.5,  0.5,  0.5], color: RED },
    PositionColor3D { position: [ 0.5,  0.5, -0.5], color: RED },
    // -X face (left) — dark red
    PositionColor3D { position: [-0.5, -0.5,  0.5], color: DARK_RED },
    PositionColor3D { position: [-0.5, -0.5, -0.5], color: DARK_RED },
    PositionColor3D { position: [-0.5,  0.5, -0.5], color: DARK_RED },
    PositionColor3D { position: [-0.5,  0.5,  0.5], color: DARK_RED },
    // +Y face (top) — green
    PositionColor3D { position: [-0.5,  0.5, -0.5], color: GREEN },
    PositionColor3D { position: [ 0.5,  0.5, -0.5], color: GREEN },
    PositionColor3D { position: [ 0.5,  0.5,  0.5], color: GREEN },
    PositionColor3D { position: [-0.5,  0.5,  0.5], color: GREEN },
    // -Y face (bottom) — dark green
    PositionColor3D { position: [-0.5, -0.5,  0.5], color: DARK_GREEN },
    PositionColor3D { position: [ 0.5, -0.5,  0.5], color: DARK_GREEN },
    PositionColor3D { position: [ 0.5, -0.5, -0.5], color: DARK_GREEN },
    PositionColor3D { position: [-0.5, -0.5, -0.5], color: DARK_GREEN },
    // +Z face (front) — blue
    PositionColor3D { position: [-0.5, -0.5,  0.5], color: BLUE },
    PositionColor3D { position: [ 0.5, -0.5,  0.5], color: BLUE },
    PositionColor3D { position: [ 0.5,  0.5,  0.5], color: BLUE },
    PositionColor3D { position: [-0.5,  0.5,  0.5], color: BLUE },
    // -Z face (back) — dark blue
    PositionColor3D { position: [ 0.5, -0.5, -0.5], color: DARK_BLUE },
    PositionColor3D { position: [-0.5, -0.5, -0.5], color: DARK_BLUE },
    PositionColor3D { position: [-0.5,  0.5, -0.5], color: DARK_BLUE },
    PositionColor3D { position: [ 0.5,  0.5, -0.5], color: DARK_BLUE },
];

/// Same unit cube as [`CUBE_VERTICES`], but with per-face normals
/// baked in for the Lambert-diffuse shader (I-12). Position + color
/// carry over 1:1 so the two arrays index identically; the index
/// buffer [`CUBE_INDICES`] works for both.
///
/// Normals are object-space and constant across each face — the
/// 4-verts-per-face layout was chosen in I-3 exactly so we could
/// assign a distinct normal per face without averaging at shared
/// edges.
pub const CUBE_LIT_VERTICES: [PositionNormalColor3D; 24] = [
    // +X face
    PositionNormalColor3D { position: [ 0.5, -0.5, -0.5], normal: [ 1.0,  0.0,  0.0], color: RED },
    PositionNormalColor3D { position: [ 0.5, -0.5,  0.5], normal: [ 1.0,  0.0,  0.0], color: RED },
    PositionNormalColor3D { position: [ 0.5,  0.5,  0.5], normal: [ 1.0,  0.0,  0.0], color: RED },
    PositionNormalColor3D { position: [ 0.5,  0.5, -0.5], normal: [ 1.0,  0.0,  0.0], color: RED },
    // -X face
    PositionNormalColor3D { position: [-0.5, -0.5,  0.5], normal: [-1.0,  0.0,  0.0], color: DARK_RED },
    PositionNormalColor3D { position: [-0.5, -0.5, -0.5], normal: [-1.0,  0.0,  0.0], color: DARK_RED },
    PositionNormalColor3D { position: [-0.5,  0.5, -0.5], normal: [-1.0,  0.0,  0.0], color: DARK_RED },
    PositionNormalColor3D { position: [-0.5,  0.5,  0.5], normal: [-1.0,  0.0,  0.0], color: DARK_RED },
    // +Y face
    PositionNormalColor3D { position: [-0.5,  0.5, -0.5], normal: [ 0.0,  1.0,  0.0], color: GREEN },
    PositionNormalColor3D { position: [ 0.5,  0.5, -0.5], normal: [ 0.0,  1.0,  0.0], color: GREEN },
    PositionNormalColor3D { position: [ 0.5,  0.5,  0.5], normal: [ 0.0,  1.0,  0.0], color: GREEN },
    PositionNormalColor3D { position: [-0.5,  0.5,  0.5], normal: [ 0.0,  1.0,  0.0], color: GREEN },
    // -Y face
    PositionNormalColor3D { position: [-0.5, -0.5,  0.5], normal: [ 0.0, -1.0,  0.0], color: DARK_GREEN },
    PositionNormalColor3D { position: [ 0.5, -0.5,  0.5], normal: [ 0.0, -1.0,  0.0], color: DARK_GREEN },
    PositionNormalColor3D { position: [ 0.5, -0.5, -0.5], normal: [ 0.0, -1.0,  0.0], color: DARK_GREEN },
    PositionNormalColor3D { position: [-0.5, -0.5, -0.5], normal: [ 0.0, -1.0,  0.0], color: DARK_GREEN },
    // +Z face
    PositionNormalColor3D { position: [-0.5, -0.5,  0.5], normal: [ 0.0,  0.0,  1.0], color: BLUE },
    PositionNormalColor3D { position: [ 0.5, -0.5,  0.5], normal: [ 0.0,  0.0,  1.0], color: BLUE },
    PositionNormalColor3D { position: [ 0.5,  0.5,  0.5], normal: [ 0.0,  0.0,  1.0], color: BLUE },
    PositionNormalColor3D { position: [-0.5,  0.5,  0.5], normal: [ 0.0,  0.0,  1.0], color: BLUE },
    // -Z face
    PositionNormalColor3D { position: [ 0.5, -0.5, -0.5], normal: [ 0.0,  0.0, -1.0], color: DARK_BLUE },
    PositionNormalColor3D { position: [-0.5, -0.5, -0.5], normal: [ 0.0,  0.0, -1.0], color: DARK_BLUE },
    PositionNormalColor3D { position: [-0.5,  0.5, -0.5], normal: [ 0.0,  0.0, -1.0], color: DARK_BLUE },
    PositionNormalColor3D { position: [ 0.5,  0.5, -0.5], normal: [ 0.0,  0.0, -1.0], color: DARK_BLUE },
];

/// Two triangles per face, 6 indices per face, 36 total.
pub const CUBE_INDICES: [u16; 36] = [
     0,  1,  2,   0,  2,  3, // +X
     4,  5,  6,   4,  6,  7, // -X
     8,  9, 10,   8, 10, 11, // +Y
    12, 13, 14,  12, 14, 15, // -Y
    16, 17, 18,  16, 18, 19, // +Z
    20, 21, 22,  20, 22, 23, // -Z
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_has_24_vertices() {
        assert_eq!(CUBE_VERTICES.len(), 24);
    }

    #[test]
    fn cube_has_36_indices() {
        assert_eq!(CUBE_INDICES.len(), 36);
    }

    #[test]
    fn cube_indices_reference_valid_vertices() {
        for &i in &CUBE_INDICES {
            assert!((i as usize) < CUBE_VERTICES.len());
        }
    }

    #[test]
    fn cube_vertices_within_unit_box() {
        for v in CUBE_VERTICES {
            for c in v.position {
                assert!(c.abs() <= 0.5 + 1e-6);
            }
        }
    }

    #[test]
    fn lit_cube_matches_positions_and_colors_of_base_cube() {
        assert_eq!(CUBE_LIT_VERTICES.len(), CUBE_VERTICES.len());
        for (lit, base) in CUBE_LIT_VERTICES.iter().zip(CUBE_VERTICES.iter()) {
            assert_eq!(lit.position, base.position);
            assert_eq!(lit.color, base.color);
        }
    }

    #[test]
    fn lit_cube_normals_are_unit_length_and_axis_aligned() {
        for v in CUBE_LIT_VERTICES {
            let [nx, ny, nz] = v.normal;
            let len_sq = nx * nx + ny * ny + nz * nz;
            assert!((len_sq - 1.0).abs() < 1e-6);
            // Axis-aligned: exactly one component non-zero.
            let nonzero = [nx, ny, nz].iter().filter(|c| c.abs() > 1e-6).count();
            assert_eq!(nonzero, 1, "face normal should be axis-aligned");
        }
    }
}
