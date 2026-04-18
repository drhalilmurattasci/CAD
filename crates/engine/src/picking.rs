//! CPU ray-picking against the runtime world (I-8).
//!
//! A "real" pick — we unproject the pointer through the camera's
//! inverse view-projection into a world-space ray, then intersect it
//! against each entity's axis-aligned bounding box. The closest hit
//! wins.
//!
//! Why CPU first: with < 10⁴ entities (every single-scene test we'll
//! run in I-8..I-10) ray/AABB brute force is sub-millisecond on
//! commodity hardware, deterministic, and doesn't require a second
//! render target + readback stall. When hierarchy + instancing push
//! entity counts into the tens of thousands, this function is the
//! drop-in point where we'd hand off to a BVH or a GPU R32Uint pass.
//!
//! Bounding volumes: the world currently only renders the unit cube
//! mesh (`MeshHandle::UNIT_CUBE`), which spans `[-0.5, 0.5]³` in its
//! local frame. The transform's translation + uniform scale define
//! the world-space AABB. When concave meshes arrive they bring their
//! own bounds helpers.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::world::{Entity, MeshHandle, Transform, World};

// `Ray`, `Aabb`, and the intersection primitives now live in the
// standalone `rustcad::math` module — pulled out so downstream tools
// can reuse them without dragging the engine along. The re-exports
// here preserve the legacy `engine::picking::{Ray, Aabb}` import path
// so existing call sites keep compiling unchanged.
pub use rustcad::math::{ray_plane_hit, Aabb, Ray};

/// The three cardinal gizmo handle axes. Used by translate, rotate,
/// and scale modes alike — each mode interprets an axis hit
/// differently (drag-along, rotate-around, scale-along).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GizmoAxis {
    X,
    Y,
    Z,
}

impl GizmoAxis {
    /// Unit vector the axis represents in world space.
    pub fn direction(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Y => Vec3::Y,
            Self::Z => Vec3::Z,
        }
    }

    /// Two unit vectors spanning the plane perpendicular to this axis.
    /// Used by the rotate drag math to measure angle on the ring.
    /// The returned basis is right-handed with `direction()` as the
    /// third axis — `cross(u, v) == direction()` within FP slop.
    pub fn plane_basis(self) -> (Vec3, Vec3) {
        match self {
            // Ring lies in YZ plane. u=Y, v=Z gives cross = X. ✓
            Self::X => (Vec3::Y, Vec3::Z),
            // Ring lies in ZX plane. u=Z, v=X gives cross = Y. ✓
            Self::Y => (Vec3::Z, Vec3::X),
            // Ring lies in XY plane. u=X, v=Y gives cross = Z. ✓
            Self::Z => (Vec3::X, Vec3::Y),
        }
    }
}

/// Which interaction a gizmo drag performs. I-34 introduced the three-
/// mode switch; the editor shell holds the current mode and hands it
/// to the picker + drag handlers.
///
/// Stored on the editor side (viewport state) rather than the runtime
/// world, since it's an authoring-time concern — no runtime system
/// reacts to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GizmoMode {
    /// W key — axis-locked translation (I-9 legacy behaviour).
    #[default]
    Translate,
    /// E key — axis rotation rings drawn around the pivot.
    Rotate,
    /// R key — axis scale boxes at the tips + a uniform handle at the
    /// pivot center.
    Scale,
}

impl GizmoMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Translate => "Translate",
            Self::Rotate => "Rotate",
            Self::Scale => "Scale",
        }
    }
}

/// A specific gizmo hit target. Rotate + translate can only grab an
/// axis handle, so they collapse to `Axis(..)`. Scale additionally
/// offers a center `Uniform` box that scales all three dimensions
/// together — the classic three-axis-plus-center gizmo layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GizmoHandle {
    Axis(GizmoAxis),
    /// Scale only — uniform multiplier across XYZ. Never returned by
    /// translate or rotate pickers.
    Uniform,
}

/// Layout parameters for the translation gizmo. One pivot, three
/// arrow tips. The tips live `arm_length` units out from the pivot
/// along each axis; each is a cube AABB sized `handle_size`.
#[derive(Debug, Clone, Copy)]
pub struct GizmoLayout {
    pub pivot:       Vec3,
    pub arm_length:  f32,
    pub handle_size: f32,
}

impl GizmoLayout {
    /// Default layout — handles just offscreen of a unit cube so
    /// they're always clickable.
    pub fn centered(pivot: Vec3) -> Self {
        Self {
            pivot,
            arm_length:  1.2,
            handle_size: 0.25,
        }
    }

    /// AABB of the tip for the given axis.
    pub fn handle_aabb(&self, axis: GizmoAxis) -> Aabb {
        let center = self.pivot + axis.direction() * self.arm_length;
        Aabb::new(center, Vec3::splat(self.handle_size * 0.5))
    }

    /// Handles in draw/pick order. Picks test each one against the
    /// ray and return the closest hit.
    pub fn handles(&self) -> [(GizmoAxis, Aabb); 3] {
        [
            (GizmoAxis::X, self.handle_aabb(GizmoAxis::X)),
            (GizmoAxis::Y, self.handle_aabb(GizmoAxis::Y)),
            (GizmoAxis::Z, self.handle_aabb(GizmoAxis::Z)),
        ]
    }
}

/// Pick the gizmo handle (if any) the ray intersects. Preferred over
/// `pick_entity` when a selection exists so click-and-drag on an
/// arrow translates instead of reselecting whatever is behind it.
pub fn pick_gizmo(layout: &GizmoLayout, ray: &Ray) -> Option<GizmoAxis> {
    let mut best: Option<(GizmoAxis, f32)> = None;
    for (axis, aabb) in layout.handles() {
        if let Some(t) = aabb.ray_hit(ray) {
            let closer = best.map(|(_, bt)| t < bt).unwrap_or(true);
            if closer {
                best = Some((axis, t));
            }
        }
    }
    best.map(|(axis, _)| axis)
}

/// Pick a rotate-mode axis ring. Each ring lies in the plane normal
/// to its axis at radius `arm_length`. We ray-plane intersect, then
/// require the hit point's distance from the pivot to sit inside the
/// ring thickness band `[arm_length - handle_size, arm_length + handle_size]`.
///
/// Why a thickness band rather than a tube test: projects cleanly to
/// the same visual rendered by `render_snapshot_with_gizmo` (a line-
/// loop ring), avoids the quartic-root gymnastics of ray-torus, and
/// the band is generous enough that typical pointer jitter still
/// selects the handle the user intended.
pub fn pick_rotate_handle(layout: &GizmoLayout, ray: &Ray) -> Option<GizmoAxis> {
    let ring_radius = layout.arm_length;
    let thickness = layout.handle_size;
    let mut best: Option<(GizmoAxis, f32)> = None;
    for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
        let normal = axis.direction();
        let Some((t, hit)) = ray_plane_hit(ray, layout.pivot, normal) else {
            continue;
        };
        let d = (hit - layout.pivot).length();
        if (d - ring_radius).abs() > thickness {
            continue;
        }
        let closer = best.map(|(_, bt)| t < bt).unwrap_or(true);
        if closer {
            best = Some((axis, t));
        }
    }
    best.map(|(axis, _)| axis)
}

/// Pick a scale-mode handle. The per-axis tip boxes are the same
/// geometry as translate handles — easy to recognize and already
/// rendered — plus a center box at the pivot for uniform scale.
pub fn pick_scale_handle(layout: &GizmoLayout, ray: &Ray) -> Option<GizmoHandle> {
    let mut best: Option<(GizmoHandle, f32)> = None;
    // Center uniform handle. Slightly smaller than the axis tips so
    // it doesn't occlude them from angles where the camera looks
    // straight down an axis.
    let center = Aabb::new(
        layout.pivot,
        Vec3::splat(layout.handle_size * 0.4),
    );
    if let Some(t) = center.ray_hit(ray) {
        best = Some((GizmoHandle::Uniform, t));
    }
    for (axis, aabb) in layout.handles() {
        if let Some(t) = aabb.ray_hit(ray) {
            let closer = best.map(|(_, bt)| t < bt).unwrap_or(true);
            if closer {
                best = Some((GizmoHandle::Axis(axis), t));
            }
        }
    }
    best.map(|(h, _)| h)
}

/// Compute the signed angle (radians) of a world-space point around
/// a gizmo axis, measured in that axis' plane basis. Used by the
/// rotate drag: call at drag-start to snapshot the initial angle,
/// then each frame on the current hit point to compute the delta.
///
/// Returned range is `(-PI, PI]` via `atan2`.
pub fn angle_on_ring(layout: &GizmoLayout, axis: GizmoAxis, point: Vec3) -> f32 {
    let (u, v) = axis.plane_basis();
    let rel = point - layout.pivot;
    rel.dot(v).atan2(rel.dot(u))
}

/// Pick the closest entity whose AABB the given ray hits. Returns
/// `None` if no entity was hit.
///
/// Only entities with a `MeshHandle::UNIT_CUBE` component contribute —
/// pure-transform entities (cameras, lights, empties) are not
/// pickable in I-8.
pub fn pick_entity(world: &World, ray: &Ray) -> Option<Entity> {
    // I-14: use world-space transforms so parented entities pick from
    // their final composed position, not their parent-local one.
    let worlds = world.compute_world_transforms();
    let mut best: Option<(Entity, f32)> = None;
    for (entity, (_transform, mesh)) in world
        .ecs()
        .query::<(&Transform, &MeshHandle)>()
        .iter()
    {
        if *mesh != MeshHandle::UNIT_CUBE {
            continue;
        }
        let Some(model) = worlds.get(&entity) else {
            continue;
        };
        // Decompose world matrix to recover translation + scale. The
        // cube AABB is axis-aligned in world space only when rotation
        // is axis-aligned — a true oriented-bounding-box test lands
        // alongside rotate-gizmo work.
        let translation = model.col(3).truncate();
        let scale = Vec3::new(
            model.col(0).truncate().length(),
            model.col(1).truncate().length(),
            model.col(2).truncate().length(),
        );
        let aabb = Aabb::new(translation, scale * 0.5);
        if let Some(t) = aabb.ray_hit(ray) {
            let closer = match best {
                Some((_, best_t)) => t < best_t,
                None => true,
            };
            if closer {
                best = Some((entity, t));
            }
        }
    }
    best.map(|(entity, _)| entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Mat4;

    fn identity_inv_view_proj() -> Mat4 {
        Mat4::IDENTITY
    }

    #[test]
    fn aabb_hits_ray_through_center() {
        let aabb = Aabb::new(Vec3::new(0.0, 0.0, 5.0), Vec3::splat(0.5));
        let ray = Ray {
            origin:    Vec3::ZERO,
            direction: Vec3::new(0.0, 0.0, 1.0),
        };
        let t = aabb.ray_hit(&ray).expect("should hit");
        assert!((t - 4.5).abs() < 1e-4);
    }

    #[test]
    fn aabb_misses_ray_offset_sideways() {
        let aabb = Aabb::new(Vec3::new(0.0, 0.0, 5.0), Vec3::splat(0.5));
        let ray = Ray {
            origin:    Vec3::new(2.0, 0.0, 0.0),
            direction: Vec3::new(0.0, 0.0, 1.0),
        };
        assert!(aabb.ray_hit(&ray).is_none());
    }

    #[test]
    fn aabb_treats_origin_inside_as_zero() {
        let aabb = Aabb::new(Vec3::ZERO, Vec3::splat(1.0));
        let ray = Ray {
            origin:    Vec3::ZERO,
            direction: Vec3::X,
        };
        assert_eq!(aabb.ray_hit(&ray), Some(0.0));
    }

    #[test]
    fn ray_unprojects_center_pixel_at_identity() {
        // Identity inv_view_proj: NDC == world. Center pixel → NDC (0,0).
        let ray = Ray::from_viewport_pixel(
            [500.0, 300.0],
            [1000.0, 600.0],
            identity_inv_view_proj(),
        );
        assert!((ray.origin.x).abs() < 1e-4);
        assert!((ray.origin.y).abs() < 1e-4);
    }

    #[test]
    fn pick_entity_finds_closest_cube_along_ray() {
        let mut world = World::new();
        world.spawn_cube(Transform::from_translation(Vec3::new(0.0, 0.0, 10.0)));
        let near = world.spawn_cube(Transform::from_translation(Vec3::new(0.0, 0.0, 3.0)));
        world.spawn_cube(Transform::from_translation(Vec3::new(0.0, 0.0, 20.0)));

        let ray = Ray {
            origin:    Vec3::ZERO,
            direction: Vec3::new(0.0, 0.0, 1.0),
        };
        assert_eq!(pick_entity(&world, &ray), Some(near));
    }

    #[test]
    fn gizmo_layout_places_handles_along_axes() {
        let layout = GizmoLayout::centered(Vec3::new(1.0, 2.0, 3.0));
        let x_box = layout.handle_aabb(GizmoAxis::X);
        let expected_center = Vec3::new(1.0 + layout.arm_length, 2.0, 3.0);
        let actual_center = (x_box.min + x_box.max) * 0.5;
        assert!((actual_center - expected_center).length() < 1e-4);
    }

    #[test]
    fn pick_gizmo_returns_axis_for_direct_hit() {
        let layout = GizmoLayout::centered(Vec3::ZERO);
        // Ray aimed straight at the +X handle.
        let ray = Ray {
            origin:    Vec3::new(-5.0, 0.0, 0.0),
            direction: Vec3::X,
        };
        assert_eq!(pick_gizmo(&layout, &ray), Some(GizmoAxis::X));
    }

    #[test]
    fn pick_gizmo_returns_none_when_missed() {
        let layout = GizmoLayout::centered(Vec3::ZERO);
        let ray = Ray {
            origin:    Vec3::new(-5.0, 5.0, 0.0),
            direction: Vec3::X,
        };
        assert!(pick_gizmo(&layout, &ray).is_none());
    }

    #[test]
    fn pick_rotate_handle_hits_ring_through_axis_plane() {
        // Pivot at origin, default arm_length = 1.2. Fire a ray from
        // +Z straight down at the Y-axis ring (which lies in ZX plane).
        // The ring passes through (+1.2, 0, 0) — cast from (1.2, 5, 0)
        // down -Y to hit the Y ring cleanly.
        let layout = GizmoLayout::centered(Vec3::ZERO);
        let ray = Ray {
            origin:    Vec3::new(1.2, 5.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        assert_eq!(pick_rotate_handle(&layout, &ray), Some(GizmoAxis::Y));
    }

    #[test]
    fn pick_rotate_handle_misses_when_outside_thickness_band() {
        let layout = GizmoLayout::centered(Vec3::ZERO);
        // Far outside arm_length — ray hits the Y-plane at the origin,
        // distance = 0 which is well outside the ring band.
        let ray = Ray {
            origin:    Vec3::new(0.0, 5.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        assert!(pick_rotate_handle(&layout, &ray).is_none());
    }

    #[test]
    fn pick_scale_handle_returns_uniform_on_center_hit() {
        let layout = GizmoLayout::centered(Vec3::ZERO);
        // Ray aimed at the pivot from -Z; the center box (0.4 *
        // handle_size half-extent) is within the first 0.1 units of
        // the pivot along the ray.
        let ray = Ray {
            origin:    Vec3::new(0.0, 0.0, -5.0),
            direction: Vec3::Z,
        };
        assert_eq!(pick_scale_handle(&layout, &ray), Some(GizmoHandle::Uniform));
    }

    #[test]
    fn pick_scale_handle_returns_axis_on_tip_hit() {
        let layout = GizmoLayout::centered(Vec3::ZERO);
        // Aim from +Y straight down at the X tip. The ray x stays at
        // arm_length (1.2) so it misses the uniform center box (whose
        // half-extent is 0.1) and only the X tip AABB registers.
        let ray = Ray {
            origin:    Vec3::new(1.2, 5.0, 0.0),
            direction: Vec3::NEG_Y,
        };
        assert_eq!(
            pick_scale_handle(&layout, &ray),
            Some(GizmoHandle::Axis(GizmoAxis::X)),
        );
    }

    #[test]
    fn angle_on_ring_is_zero_on_plane_basis_u() {
        // For Y axis, u = Z. A point at +Z on the ring should read 0
        // radians. +X (= v) should read PI/2.
        let layout = GizmoLayout::centered(Vec3::ZERO);
        let a_u = angle_on_ring(&layout, GizmoAxis::Y, Vec3::new(0.0, 0.0, 1.2));
        let a_v = angle_on_ring(&layout, GizmoAxis::Y, Vec3::new(1.2, 0.0, 0.0));
        assert!(a_u.abs() < 1e-4, "angle at +u should be 0, got {a_u}");
        assert!(
            (a_v - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "angle at +v should be PI/2, got {a_v}",
        );
    }

    #[test]
    fn gizmo_axis_plane_basis_is_right_handed() {
        // cross(u, v) must equal direction() for each axis so the
        // rotate-drag sign convention matches the right-hand rule
        // (positive angles rotate counter-clockwise looking down the
        // axis from the positive side).
        for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
            let (u, v) = axis.plane_basis();
            let n = u.cross(v);
            let d = axis.direction();
            assert!(
                (n - d).length() < 1e-4,
                "axis {axis:?}: cross(u,v)={n} != direction={d}",
            );
        }
    }

    #[test]
    fn ray_plane_hit_returns_none_for_parallel_ray() {
        let ray = Ray {
            origin:    Vec3::new(0.0, 5.0, 0.0),
            direction: Vec3::X,
        };
        assert!(ray_plane_hit(&ray, Vec3::ZERO, Vec3::Y).is_none());
    }

    #[test]
    fn ray_plane_hit_returns_none_for_ray_behind_plane() {
        // Ray starts on +Y side, points further +Y — plane at origin
        // with +Y normal is behind it.
        let ray = Ray {
            origin:    Vec3::new(0.0, 5.0, 0.0),
            direction: Vec3::Y,
        };
        assert!(ray_plane_hit(&ray, Vec3::ZERO, Vec3::Y).is_none());
    }

    #[test]
    fn gizmo_mode_default_is_translate() {
        assert_eq!(GizmoMode::default(), GizmoMode::Translate);
    }

    #[test]
    fn pick_entity_skips_non_renderable_entities() {
        let mut world = World::new();
        // Spawn a pure-transform entity (no mesh).
        world
            .ecs_mut()
            .spawn((Transform::from_translation(Vec3::new(0.0, 0.0, 3.0)),));
        let ray = Ray {
            origin:    Vec3::ZERO,
            direction: Vec3::Z,
        };
        assert_eq!(pick_entity(&world, &ray), None);
    }
}
