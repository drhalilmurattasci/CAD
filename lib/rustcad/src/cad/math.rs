//! CAD-specific math primitives.
//!
//! Thin layer on top of [`crate::math`] (which owns [`Ray`], [`Aabb`],
//! and `ray_plane_hit`) plus a handful of infinite-analytic types —
//! [`Plane`] and [`Line2`] / [`Line3`] — that CAD-specific code keeps
//! asking for. No duplication: ray / AABB still come from
//! `rustcad::math`.

use glam::{Vec2, Vec3};

pub use crate::math::{Aabb, Ray, ray_plane_hit};

/// Infinite plane, point + unit normal form.
///
/// Used as a building block by [`crate::cad::kernel::Surface::Plane`]
/// and by [`crate::cad::modifier::MirrorModifier`]. No implicit
/// orientation: the half-space with `(p - origin) · normal > 0` is
/// considered "above".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    /// A point that lies on the plane.
    pub origin: Vec3,
    /// Unit-length surface normal. Non-unit inputs still work with
    /// the signed-distance queries but metric results will be scaled.
    pub normal: Vec3,
}

impl Plane {
    /// Construct a plane from a point + normal. No normalization is
    /// performed.
    pub fn new(origin: Vec3, normal: Vec3) -> Self {
        Self { origin, normal }
    }

    /// Signed distance from `point` to the plane, positive on the
    /// same side as `normal`.
    pub fn signed_distance(&self, point: Vec3) -> f32 {
        (point - self.origin).dot(self.normal)
    }

    /// Orthogonally project `point` onto the plane.
    pub fn project(&self, point: Vec3) -> Vec3 {
        point - self.normal * self.signed_distance(point)
    }

    /// Reflect `point` across the plane. Core building block of
    /// `MirrorModifier`.
    pub fn reflect(&self, point: Vec3) -> Vec3 {
        point - self.normal * (2.0 * self.signed_distance(point))
    }
}

/// Infinite 2D line in point + direction form. Used throughout the
/// 2D sketch layer for projection, intersection, and fillet setup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line2 {
    /// A point on the line.
    pub origin:    Vec2,
    /// Direction vector. Non-unit is allowed; projection scales
    /// accordingly.
    pub direction: Vec2,
}

impl Line2 {
    /// Construct from origin + direction.
    pub fn new(origin: Vec2, direction: Vec2) -> Self {
        Self { origin, direction }
    }

    /// Build a line from two distinct points. Returns `None` when the
    /// points coincide (within `f32::EPSILON`) since no direction is
    /// defined there.
    pub fn from_two_points(a: Vec2, b: Vec2) -> Option<Self> {
        let dir = b - a;
        if dir.length_squared() < f32::EPSILON {
            None
        } else {
            Some(Self::new(a, dir))
        }
    }
}

/// Infinite 3D line. The 3D counterpart of [`Line2`]; used by the
/// B-Rep kernel as the carrier curve of straight edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line3 {
    /// A point on the line.
    pub origin:    Vec3,
    /// Direction vector.
    pub direction: Vec3,
}

impl Line3 {
    /// Construct from origin + direction.
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self { origin, direction }
    }

    /// Evaluate the line at parameter `t`.
    pub fn point_at(&self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

/// Numerical-stability helpers that show up across the CAD stack.
pub mod num {
    /// Default geometric tolerance for "are these points / lengths the
    /// same?" comparisons. Chosen to match CAD-typical millimeter
    /// scales at `f32` precision.
    pub const EPS: f32 = 1e-5;

    /// `true` when `a` and `b` are within [`EPS`].
    pub fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    /// `true` when `a` and `b` are within the caller-supplied tolerance.
    pub fn approx_eq_tol(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_signed_distance_matches_offset() {
        let p = Plane::new(Vec3::ZERO, Vec3::Y);
        assert!((p.signed_distance(Vec3::new(0.0, 3.0, 0.0)) - 3.0).abs() < 1e-5);
        assert!((p.signed_distance(Vec3::new(0.0, -2.0, 0.0)) + 2.0).abs() < 1e-5);
    }

    #[test]
    fn plane_reflect_mirrors_across_plane() {
        let p = Plane::new(Vec3::ZERO, Vec3::Y);
        let mirrored = p.reflect(Vec3::new(1.0, 2.0, 3.0));
        assert!((mirrored - Vec3::new(1.0, -2.0, 3.0)).length() < 1e-5);
    }

    #[test]
    fn line2_from_two_points_rejects_coincident() {
        assert!(Line2::from_two_points(Vec2::ZERO, Vec2::ZERO).is_none());
        assert!(Line2::from_two_points(Vec2::ZERO, Vec2::X).is_some());
    }
}
