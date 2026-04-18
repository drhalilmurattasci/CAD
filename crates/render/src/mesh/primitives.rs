//! Built-in primitive meshes.
//!
//! Kept as `const` arrays so consumers can upload them directly via
//! `bytemuck::cast_slice` without any allocation.

use super::vertex::PositionColor2D;

/// The canonical colored triangle used by the viewport clear pass and
/// the render library's unit tests. Three vertices, NDC positions,
/// obviously distinct colors.
pub const TRIANGLE_2D: [PositionColor2D; 3] = [
    PositionColor2D { position: [ 0.0,  0.6], color: [1.00, 0.30, 0.30] }, // top, red
    PositionColor2D { position: [-0.6, -0.5], color: [0.30, 1.00, 0.40] }, // bottom-left, green
    PositionColor2D { position: [ 0.6, -0.5], color: [0.40, 0.50, 1.00] }, // bottom-right, blue
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_has_three_vertices() {
        assert_eq!(TRIANGLE_2D.len(), 3);
    }

    #[test]
    fn triangle_positions_within_clip_space() {
        for v in TRIANGLE_2D {
            assert!(v.position[0].abs() <= 1.0);
            assert!(v.position[1].abs() <= 1.0);
        }
    }

    #[test]
    fn triangle_colors_normalized() {
        for v in TRIANGLE_2D {
            for c in v.color {
                assert!((0.0..=1.0).contains(&c));
            }
        }
    }
}
