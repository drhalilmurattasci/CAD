//! B-Rep → mesh bridge.
//!
//! Tessellation turns the continuous parametric kernel
//! ([`crate::cad::kernel`]) into discrete triangles
//! ([`crate::cad::mesh::Mesh`]) that a renderer can consume. This
//! module owns every conversion that crosses the two layers, which
//! keeps the kernel and mesh modules ignorant of each other.
//!
//! Implementation status:
//!
//! - Planar faces, any mix of Line / Circle / Ellipse / NURBS edge
//!   boundaries: discretized per [`TessellationOptions`] and
//!   triangulated via ear clipping.
//! - Cylindrical faces: boundary sampled in 3D, projected into the
//!   cylinder's `(u, v)` parameter space, ear-clipped there. Faces
//!   that cross the cylinder's `u = 0` seam are not yet supported
//!   (the projection wraps around).
//! - NURBS-surface faces: TODO — needs a general UV-domain walker.

use glam::{Vec2, Vec3};

use super::kernel::{Brep, Curve, Face, FaceId, Surface, Wire};
use super::math::Plane;
use super::mesh::Mesh;

/// Parameters controlling tessellation fidelity. Defaults are tuned
/// for CAD viewports — fine enough to look smooth, coarse enough
/// that a complex model doesn't spike vertex count.
#[derive(Debug, Clone, Copy)]
pub struct TessellationOptions {
    /// Target maximum edge length when discretizing curved edges.
    /// Smaller → smoother + more triangles.
    pub max_edge_length:   f32,
    /// Maximum chord deviation (distance between a tessellation
    /// segment and the true curve). Used alongside
    /// `max_edge_length` to trigger refinement on high-curvature
    /// regions.
    pub chord_tolerance:   f32,
    /// Maximum angular deviation between adjacent segments, in
    /// radians. Mostly matters on circles and NURBS.
    pub angular_tolerance: f32,
}

impl Default for TessellationOptions {
    fn default() -> Self {
        Self {
            max_edge_length:   0.5,
            chord_tolerance:   0.01,
            angular_tolerance: 0.1,
        }
    }
}

/// Failure modes surfaced by the tessellation bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TessellationError {
    /// Referenced an id that isn't in the parent [`Brep`].
    MissingEntity,
    /// Tried to tessellate a surface / curve whose evaluator hasn't
    /// been implemented yet (see [`crate::cad::kernel`]).
    UnsupportedGeometry,
    /// The wire didn't form a simple closed loop — e.g. was open,
    /// self-intersecting, or had a degenerate segment.
    InvalidWire,
}

/// Tessellate a planar face bounded by a single outer wire of
/// straight edges. Inner wires (holes) and curved boundaries are
/// not supported by this fast path — callers needing them should
/// drop down to [`tessellate_face`].
///
/// The wire is projected into the plane's local 2D frame,
/// triangulated via ear clipping, then lifted back into world space.
pub fn tessellate_planar_face(
    brep: &Brep,
    face: &Face,
    plane: &Plane,
    opts: &TessellationOptions,
) -> Result<Mesh, TessellationError> {
    let wire = brep
        .wires
        .get(&face.outer_wire)
        .ok_or(TessellationError::MissingEntity)?;
    let boundary_3d = walk_wire(brep, wire, opts)?;
    let (u_dir, v_dir) = plane_basis(plane);
    let boundary_2d: Vec<Vec2> = boundary_3d
        .iter()
        .map(|p| {
            let local = *p - plane.origin;
            Vec2::new(local.dot(u_dir), local.dot(v_dir))
        })
        .collect();
    let tris = ear_clip(&boundary_2d).ok_or(TessellationError::InvalidWire)?;
    let mut mesh = Mesh::new();
    for p in &boundary_3d {
        mesh.push_vertex(*p);
    }
    for tri in tris {
        mesh.push_triangle([tri[0] as u32, tri[1] as u32, tri[2] as u32]);
    }
    mesh.recompute_normals();
    Ok(mesh)
}

/// General face tessellator. Dispatches on the surface variant.
///
/// - [`Surface::Plane`] → delegates to [`tessellate_planar_face`].
/// - [`Surface::Cylinder`] → [`tessellate_cylindrical_face`].
/// - [`Surface::Nurbs`] → [`TessellationError::UnsupportedGeometry`]
///   (no general UV-domain walker yet).
pub fn tessellate_face(
    brep: &Brep,
    face_id: FaceId,
    opts: &TessellationOptions,
) -> Result<Mesh, TessellationError> {
    let face = brep
        .faces
        .get(&face_id)
        .ok_or(TessellationError::MissingEntity)?;
    match &face.surface {
        Surface::Plane(plane) => tessellate_planar_face(brep, face, plane, opts),
        Surface::Cylinder {
            axis_origin,
            axis,
            u_dir,
            radius,
        } => tessellate_cylindrical_face(
            brep,
            face,
            *axis_origin,
            *axis,
            *u_dir,
            *radius,
            opts,
        ),
        Surface::Nurbs { .. } => Err(TessellationError::UnsupportedGeometry),
    }
}

/// Tessellate a face carried by a right-circular cylinder. The
/// boundary is sampled in world space, then projected into the
/// cylinder's `(u, v)` parameter plane (u wraps [0, 1), v advances
/// along the axis) and ear-clipped there. The triangulated indices
/// lift back to the world-space boundary, so no cylinder-interior
/// sampling is needed for the common case where the face is just a
/// trimmed window on the cylinder.
///
/// Faces whose boundary crosses the `u = 0` seam will produce
/// self-intersecting 2D projections and fail here — surface a
/// dedicated seam-splitter later if that case shows up.
pub fn tessellate_cylindrical_face(
    brep: &Brep,
    face: &Face,
    axis_origin: Vec3,
    axis: Vec3,
    u_dir: Vec3,
    radius: f32,
    opts: &TessellationOptions,
) -> Result<Mesh, TessellationError> {
    let wire = brep
        .wires
        .get(&face.outer_wire)
        .ok_or(TessellationError::MissingEntity)?;
    let boundary_3d = walk_wire(brep, wire, opts)?;
    if radius.abs() < 1e-8 {
        return Err(TessellationError::InvalidWire);
    }
    let v_dir = axis.cross(u_dir).normalize_or_zero();
    let boundary_2d: Vec<Vec2> = boundary_3d
        .iter()
        .map(|p| {
            let local = *p - axis_origin;
            let u_coord = local.dot(u_dir);
            let v_coord = local.dot(v_dir);
            // atan2 → [-π, π], normalize to [0, 1).
            let angle = v_coord.atan2(u_coord);
            let u = angle / std::f32::consts::TAU;
            let v = local.dot(axis);
            Vec2::new(u * radius, v) // scale u by radius so edges match arc length
        })
        .collect();
    let tris = ear_clip(&boundary_2d).ok_or(TessellationError::InvalidWire)?;
    let mut mesh = Mesh::new();
    for p in &boundary_3d {
        mesh.push_vertex(*p);
    }
    for tri in tris {
        mesh.push_triangle([tri[0] as u32, tri[1] as u32, tri[2] as u32]);
    }
    mesh.recompute_normals();
    Ok(mesh)
}

/// Tessellate every face in the B-Rep, merging the results into one
/// mesh. Faces that can't be tessellated (unsupported surface
/// variants, missing wires) are skipped; inspect the returned
/// `Vec<TessellationError>` to diagnose what was dropped.
pub fn tessellate_brep(brep: &Brep, opts: &TessellationOptions) -> (Mesh, Vec<TessellationError>) {
    let mut mesh = Mesh::new();
    let mut errors = Vec::new();
    for face_id in brep.faces.keys() {
        match tessellate_face(brep, *face_id, opts) {
            Ok(face_mesh) => mesh.merge(&face_mesh),
            Err(err) => errors.push(err),
        }
    }
    (mesh, errors)
}

/// Sample a [`Wire`] into a polyline, discretizing each curved edge
/// per [`TessellationOptions`].
///
/// The last sample of each edge is dropped so the next edge's first
/// sample picks up where we left off (they're the shared vertex).
/// For a closed wire this yields one point per unique vertex
/// around the loop.
fn walk_wire(
    brep: &Brep,
    wire: &Wire,
    opts: &TessellationOptions,
) -> Result<Vec<Vec3>, TessellationError> {
    let mut points = Vec::with_capacity(wire.edges.len());
    let edges_len = wire.edges.len();
    for (i, edge_id) in wire.edges.iter().enumerate() {
        let edge = brep
            .edges
            .get(edge_id)
            .ok_or(TessellationError::MissingEntity)?;
        let samples = discretize_edge(&edge.curve, edge.t_min, edge.t_max, opts);
        let is_last = i + 1 == edges_len;
        // On a closed wire every edge ends at the next edge's start
        // (and the last edge ends at the first edge's start), so
        // always drop the final sample to avoid duplicates.
        let end = if wire.closed || !is_last {
            samples.len().saturating_sub(1)
        } else {
            samples.len()
        };
        points.extend_from_slice(&samples[..end]);
    }
    Ok(points)
}

/// Discretize a single parametric [`Curve`] into a polyline of world
/// points. Sample count scales with
/// [`TessellationOptions::max_edge_length`] and
/// [`TessellationOptions::angular_tolerance`] so both length-based
/// and curvature-based refinement policies take effect.
fn discretize_edge(
    curve: &Curve,
    t_min: f32,
    t_max: f32,
    opts: &TessellationOptions,
) -> Vec<Vec3> {
    let sweep = (t_max - t_min).max(0.0);
    match curve {
        Curve::Line { .. } => vec![curve.evaluate(t_min), curve.evaluate(t_max)],
        Curve::Circle { radius, .. } => {
            let arc = radius.abs() * sweep * std::f32::consts::TAU;
            let n_len = (arc / opts.max_edge_length.max(1e-4)).ceil() as usize;
            let n_ang = (sweep * std::f32::consts::TAU / opts.angular_tolerance.max(1e-4))
                .ceil() as usize;
            sample_curve(curve, t_min, t_max, n_len.max(n_ang).max(8))
        }
        Curve::Ellipse {
            radius_maj,
            radius_min,
            ..
        } => {
            let r = radius_maj.abs().max(radius_min.abs());
            let arc = r * sweep * std::f32::consts::TAU;
            let n_len = (arc / opts.max_edge_length.max(1e-4)).ceil() as usize;
            let n_ang = (sweep * std::f32::consts::TAU / opts.angular_tolerance.max(1e-4))
                .ceil() as usize;
            sample_curve(curve, t_min, t_max, n_len.max(n_ang).max(8))
        }
        Curve::Nurbs { control, .. } => {
            // Use the control polygon as a conservative arc-length
            // proxy; a real adaptive sampler can replace this later.
            let mut arc = 0.0f32;
            for w in control.windows(2) {
                arc += (w[1] - w[0]).length();
            }
            arc *= sweep;
            let n_len = (arc / opts.max_edge_length.max(1e-4)).ceil() as usize;
            sample_curve(curve, t_min, t_max, n_len.max(control.len()).max(8))
        }
    }
}

fn sample_curve(curve: &Curve, t_min: f32, t_max: f32, segments: usize) -> Vec<Vec3> {
    let segments = segments.max(1);
    let mut pts = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let alpha = i as f32 / segments as f32;
        let t = t_min + (t_max - t_min) * alpha;
        pts.push(curve.evaluate(t));
    }
    pts
}

/// Build an orthonormal basis `(u_dir, v_dir)` on the plane.
fn plane_basis(plane: &Plane) -> (Vec3, Vec3) {
    let fallback = if plane.normal.x.abs() < 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let u_dir = fallback.cross(plane.normal).normalize_or_zero();
    let v_dir = plane.normal.cross(u_dir).normalize_or_zero();
    (u_dir, v_dir)
}

/// Ear-clipping triangulation of a simple 2D polygon (no holes, no
/// self-intersections). Returns `None` when the polygon is
/// degenerate (< 3 vertices or no ear found).
///
/// Vertices are indexed into the input polyline so the caller can
/// splice the triangulation back onto its own vertex buffer.
fn ear_clip(polygon: &[Vec2]) -> Option<Vec<[usize; 3]>> {
    if polygon.len() < 3 {
        return None;
    }
    let ccw = signed_area(polygon) > 0.0;
    let mut indices: Vec<usize> = (0..polygon.len()).collect();
    let mut tris = Vec::with_capacity(polygon.len() - 2);
    let mut guard = polygon.len() * polygon.len();

    while indices.len() > 3 {
        if guard == 0 {
            return None;
        }
        guard -= 1;
        let n = indices.len();
        let mut ear_found = false;
        for i in 0..n {
            let prev = indices[(i + n - 1) % n];
            let cur = indices[i];
            let next = indices[(i + 1) % n];
            let a = polygon[prev];
            let b = polygon[cur];
            let c = polygon[next];
            let cross = (b - a).perp_dot(c - b);
            let convex = if ccw { cross > 0.0 } else { cross < 0.0 };
            if !convex {
                continue;
            }
            let mut contains_other = false;
            for j in 0..n {
                let j_idx = indices[j];
                if j_idx == prev || j_idx == cur || j_idx == next {
                    continue;
                }
                if point_in_triangle(polygon[j_idx], a, b, c) {
                    contains_other = true;
                    break;
                }
            }
            if contains_other {
                continue;
            }
            tris.push([prev, cur, next]);
            indices.remove(i);
            ear_found = true;
            break;
        }
        if !ear_found {
            return None;
        }
    }
    tris.push([indices[0], indices[1], indices[2]]);
    Some(tris)
}

fn signed_area(polygon: &[Vec2]) -> f32 {
    let mut area = 0.0;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area * 0.5
}

fn point_in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let s1 = (b - a).perp_dot(p - a);
    let s2 = (c - b).perp_dot(p - b);
    let s3 = (a - c).perp_dot(p - c);
    (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ear_clip_square_emits_two_triangles() {
        let square = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let tris = ear_clip(&square).unwrap();
        assert_eq!(tris.len(), 2);
    }

    #[test]
    fn ear_clip_rejects_degenerate() {
        let two_points = vec![Vec2::ZERO, Vec2::X];
        assert!(ear_clip(&two_points).is_none());
    }

    #[test]
    fn tessellate_planar_disk_via_circular_edge() {
        use super::super::kernel::{Brep, Curve, Surface};
        use glam::Vec3;

        let mut brep = Brep::new();
        // Single vertex at the circle's t=0 location; the wire
        // carries a single closed circular edge.
        let v = brep.add_vertex(Vec3::new(1.0, 0.0, 0.0));
        let e = brep.add_edge(
            Curve::Circle {
                center: Vec3::ZERO,
                axis:   Vec3::Z,
                u_dir:  Vec3::X,
                radius: 1.0,
            },
            v,
            v,
        );
        let wire = brep.add_wire(vec![e], true);
        let face = brep.add_face(Surface::Plane(Plane::new(Vec3::ZERO, Vec3::Z)), wire);
        let opts = TessellationOptions {
            max_edge_length:   0.3,
            ..TessellationOptions::default()
        };
        let mesh = tessellate_face(&brep, face, &opts).unwrap();
        assert!(mesh.vertex_count() >= 8, "got {}", mesh.vertex_count());
        assert!(mesh.triangle_count() >= 6);
    }

    #[test]
    fn tessellate_planar_square_face() {
        use super::super::kernel::{Brep, Curve, Surface};
        use glam::Vec3;

        let mut brep = Brep::new();
        let v0 = brep.add_vertex(Vec3::ZERO);
        let v1 = brep.add_vertex(Vec3::new(1.0, 0.0, 0.0));
        let v2 = brep.add_vertex(Vec3::new(1.0, 1.0, 0.0));
        let v3 = brep.add_vertex(Vec3::new(0.0, 1.0, 0.0));

        let e0 = brep.add_edge(
            Curve::Line {
                start: Vec3::ZERO,
                end:   Vec3::new(1.0, 0.0, 0.0),
            },
            v0,
            v1,
        );
        let e1 = brep.add_edge(
            Curve::Line {
                start: Vec3::new(1.0, 0.0, 0.0),
                end:   Vec3::new(1.0, 1.0, 0.0),
            },
            v1,
            v2,
        );
        let e2 = brep.add_edge(
            Curve::Line {
                start: Vec3::new(1.0, 1.0, 0.0),
                end:   Vec3::new(0.0, 1.0, 0.0),
            },
            v2,
            v3,
        );
        let e3 = brep.add_edge(
            Curve::Line {
                start: Vec3::new(0.0, 1.0, 0.0),
                end:   Vec3::ZERO,
            },
            v3,
            v0,
        );
        let wire = brep.add_wire(vec![e0, e1, e2, e3], true);
        let face = brep.add_face(Surface::Plane(Plane::new(Vec3::ZERO, Vec3::Z)), wire);
        let opts = TessellationOptions::default();
        let mesh = tessellate_face(&brep, face, &opts).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.vertex_count(), 4);
    }
}
