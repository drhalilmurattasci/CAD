//! Pure math primitives for tooling — rays, axis-aligned bounding
//! boxes, and the handful of intersection tests that keep popping up
//! in every editor and picker.
//!
//! Lifted out of `engine::picking` so downstream tools that just need
//! a ray / AABB / plane intersection don't have to pull the whole
//! engine (ECS, render hooks, scene document) along for the ride. The
//! types here are pure [`glam`] — no serde, no threading, no globals.
//!
//! The intended consumption pattern is small: build a [`Ray`] from a
//! camera's inverse view-projection and a pointer pixel, intersect it
//! against one or more [`Aabb`]s, and act on the closest positive hit.
//! For anything fancier (BVH acceleration, GPU picking, swept
//! collision) consumers are expected to build on top.

use glam::{Mat4, Vec3, Vec4};

/// World-space ray. `origin + t * direction` for `t >= 0` sweeps
/// outward from the ray's starting point.
///
/// Typical construction in an editor is [`Ray::from_viewport_pixel`],
/// which unprojects a clicked pixel through the camera's inverse
/// view-projection matrix.
#[derive(Debug, Clone, Copy)]
pub struct Ray {
    /// Start of the ray in world space.
    pub origin:    Vec3,
    /// Unit-length direction vector. Callers that want to pass a
    /// non-unit vector should normalize first; intersection tests
    /// treat this as direction-only.
    pub direction: Vec3,
}

impl Ray {
    /// Construct a new ray. No normalization is performed — if you need
    /// `direction` to be unit length, normalize before calling.
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self { origin, direction }
    }

    /// Construct a world-space ray from a viewport pixel + camera
    /// matrices.
    ///
    /// - `pixel_xy` is integer-rounded (x: right, y: down, 0-indexed).
    /// - `viewport_wh` is the render target size in pixels.
    /// - `inv_view_proj` is `camera.view_proj(aspect).inverse()`.
    ///
    /// Two points in NDC (near + far) at the same xy are unprojected
    /// and the normalized difference becomes the direction. This stays
    /// numerically stable even when the near plane is tiny.
    ///
    /// Assumes the NDC z convention used by `wgpu`/`glam` (near = 0,
    /// far = 1 with reverse-Z disabled). For OpenGL-style NDC
    /// (near = -1, far = 1) the caller can either flip `inv_view_proj`
    /// to match or construct the ray manually.
    pub fn from_viewport_pixel(
        pixel_xy: [f32; 2],
        viewport_wh: [f32; 2],
        inv_view_proj: Mat4,
    ) -> Self {
        let [px, py] = pixel_xy;
        let [w, h] = viewport_wh;
        // Flip Y — pixel space is top-down, NDC is bottom-up.
        let ndc_x = (px / w.max(1.0)) * 2.0 - 1.0;
        let ndc_y = 1.0 - (py / h.max(1.0)) * 2.0;
        let near_world = unproject(inv_view_proj, Vec3::new(ndc_x, ndc_y, 0.0));
        let far_world = unproject(inv_view_proj, Vec3::new(ndc_x, ndc_y, 1.0));
        let direction = (far_world - near_world).normalize_or_zero();
        Self {
            origin: near_world,
            direction,
        }
    }
}

fn unproject(inv_view_proj: Mat4, ndc: Vec3) -> Vec3 {
    let clip = Vec4::new(ndc.x, ndc.y, ndc.z, 1.0);
    let world = inv_view_proj * clip;
    if world.w.abs() < f32::EPSILON {
        world.truncate()
    } else {
        world.truncate() / world.w
    }
}

/// Axis-aligned bounding box in world space. Two opposite corners fully
/// describe the volume; no invariants are enforced on construction, so
/// pathological boxes (min > max on any axis) produce empty
/// intersections rather than panicking.
#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    /// Corner closest to `(-∞, -∞, -∞)`.
    pub min: Vec3,
    /// Corner closest to `(+∞, +∞, +∞)`.
    pub max: Vec3,
}

impl Aabb {
    /// Build an AABB from a center point + positive half-extents. The
    /// resulting box is always well-formed (`min <= max` componentwise)
    /// as long as `half_extents` is non-negative.
    pub fn new(center: Vec3, half_extents: Vec3) -> Self {
        Self {
            min: center - half_extents,
            max: center + half_extents,
        }
    }

    /// Ray-AABB slab test. Returns the entry `t`-value on hit (always
    /// `>= 0` — rays starting inside the box return `0`). Returns
    /// `None` when the ray misses or is parallel-to-and-outside a slab.
    pub fn ray_hit(&self, ray: &Ray) -> Option<f32> {
        let mut t_min = 0.0_f32;
        let mut t_max = f32::INFINITY;
        for axis in 0..3 {
            let origin = ray.origin[axis];
            let dir = ray.direction[axis];
            let min = self.min[axis];
            let max = self.max[axis];
            if dir.abs() < 1e-8 {
                // Ray parallel to slab — miss unless origin is inside.
                if origin < min || origin > max {
                    return None;
                }
                continue;
            }
            let inv = 1.0 / dir;
            let mut t1 = (min - origin) * inv;
            let mut t2 = (max - origin) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_min = t_min.max(t1);
            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        }
        Some(t_min)
    }
}

/// Ray-plane intersection. `plane_normal` need not be unit length —
/// it's only used as a half-space reference. Returns `(t, hit_point)`
/// for `t >= 0`, or `None` when the ray is parallel (within FP slop)
/// or pointing away from the plane.
pub fn ray_plane_hit(ray: &Ray, plane_point: Vec3, plane_normal: Vec3) -> Option<(f32, Vec3)> {
    let denom = plane_normal.dot(ray.direction);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_point - ray.origin).dot(plane_normal) / denom;
    if t < 0.0 {
        return None;
    }
    Some((t, ray.origin + ray.direction * t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_new_stores_components() {
        let ray = Ray::new(Vec3::ZERO, Vec3::Z);
        assert_eq!(ray.origin, Vec3::ZERO);
        assert_eq!(ray.direction, Vec3::Z);
    }

    #[test]
    fn aabb_ray_hit_detects_front_face() {
        let aabb = Aabb::new(Vec3::new(0.0, 0.0, 5.0), Vec3::splat(0.5));
        let ray = Ray::new(Vec3::ZERO, Vec3::Z);
        let t = aabb.ray_hit(&ray).expect("hit");
        assert!((t - 4.5).abs() < 1e-5, "expected 4.5, got {t}");
    }

    #[test]
    fn aabb_ray_hit_misses_when_perpendicular() {
        let aabb = Aabb::new(Vec3::new(0.0, 0.0, 5.0), Vec3::splat(0.5));
        let ray = Ray::new(Vec3::new(10.0, 0.0, 0.0), Vec3::Z);
        assert!(aabb.ray_hit(&ray).is_none());
    }

    #[test]
    fn aabb_ray_hit_ray_inside_returns_zero() {
        let aabb = Aabb::new(Vec3::ZERO, Vec3::splat(1.0));
        let ray = Ray::new(Vec3::ZERO, Vec3::X);
        let t = aabb.ray_hit(&ray).expect("hit");
        assert!(t < 1e-5);
    }

    #[test]
    fn ray_plane_hit_returns_point_at_plane() {
        let ray = Ray::new(Vec3::ZERO, Vec3::Z);
        let (t, point) = ray_plane_hit(&ray, Vec3::new(0.0, 0.0, 3.0), Vec3::Z).expect("hit");
        assert!((t - 3.0).abs() < 1e-5);
        assert!((point - Vec3::new(0.0, 0.0, 3.0)).length() < 1e-5);
    }

    #[test]
    fn ray_plane_hit_miss_when_parallel() {
        let ray = Ray::new(Vec3::ZERO, Vec3::X);
        assert!(ray_plane_hit(&ray, Vec3::new(0.0, 0.0, 3.0), Vec3::Z).is_none());
    }

    #[test]
    fn ray_plane_hit_miss_when_behind() {
        let ray = Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::Z);
        assert!(ray_plane_hit(&ray, Vec3::new(0.0, 0.0, 3.0), Vec3::Z).is_none());
    }
}
