//! B-Rep geometry kernel — the FreeCAD / OCCT layer.
//!
//! A CAD solid is modeled topologically: a [`Solid`] is one or more
//! [`Shell`]s, each made up of [`Face`]s, each face bounded by
//! [`Wire`]s of [`Edge`]s, each edge carried by a [`Curve`] and
//! terminated by [`Vertex`] endpoints. The geometric side (curves +
//! surfaces) is parameterized so tessellation and intersection
//! operate on continuous math rather than discrete polygons.
//!
//! ## Implementation status
//!
//! - Straight-line, circular, and elliptical curves: full evaluation.
//! - NURBS curves and surfaces: Cox-de Boor basis-function evaluation
//!   + rational tensor-product surface eval. Clamped knot vectors are
//!   expected (standard CAD convention).
//! - Planar + cylindrical + NURBS surfaces: all evaluate.
//! - B-Rep container ([`Brep`]): typed id allocation, insertion, and
//!   lookup — all working. Higher-level ops (face splitting, shell
//!   sewing, solid validation) are left as per-method TODOs.
//!
//! ## Design notes
//!
//! Every B-Rep object is stored by id in a flat map inside [`Brep`],
//! and references between objects are by id rather than by pointer.
//! That keeps the whole structure `Clone`able, `Serialize`able in a
//! future pass, and undo-friendly — a command can snapshot / restore
//! the graph without worrying about aliasing.

use std::collections::HashMap;

use glam::Vec3;

use super::math::Plane;
use crate::id::IdAllocator;

macro_rules! typed_id {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub u64);

        impl From<u64> for $name {
            fn from(n: u64) -> Self {
                Self(n)
            }
        }
    };
}

typed_id!(VertexId, "Typed id for a B-Rep [`Vertex`].");
typed_id!(EdgeId, "Typed id for a B-Rep [`Edge`].");
typed_id!(WireId, "Typed id for a B-Rep [`Wire`].");
typed_id!(FaceId, "Typed id for a B-Rep [`Face`].");
typed_id!(ShellId, "Typed id for a B-Rep [`Shell`].");
typed_id!(SolidId, "Typed id for a B-Rep [`Solid`].");

/// Continuous 3D curve — the geometric carrier of an [`Edge`].
///
/// Each variant knows its own parameter range (convention: `[0, 1]`
/// for bounded shapes, `[0, 2π]` for circles).
#[derive(Debug, Clone)]
pub enum Curve {
    /// Straight line segment between two points.
    Line {
        /// Start point.
        start: Vec3,
        /// End point.
        end:   Vec3,
    },
    /// Planar circle, traversed counter-clockwise around `axis`.
    Circle {
        /// Circle center.
        center: Vec3,
        /// Unit-length axis of the plane the circle lies in.
        axis:   Vec3,
        /// Reference direction on the plane (0-radian direction).
        u_dir:  Vec3,
        /// Circle radius.
        radius: f32,
    },
    /// Planar elliptical arc, traversed counter-clockwise around
    /// `axis`. Parameter `t ∈ [0, 1]` sweeps a full revolution; use
    /// `t_min` / `t_max` on the containing [`Edge`] to trim.
    Ellipse {
        /// Ellipse center.
        center:     Vec3,
        /// Axis-normal of the containing plane.
        axis:       Vec3,
        /// Major-axis reference direction.
        u_dir:      Vec3,
        /// Major-axis radius.
        radius_maj: f32,
        /// Minor-axis radius.
        radius_min: f32,
    },
    /// Rational B-spline (NURBS) curve. Evaluated via Cox-de Boor
    /// basis-function recursion; clamped knot vectors are required
    /// (standard CAD convention — first and last knot repeat
    /// `degree + 1` times, so the curve passes through the first and
    /// last control points).
    Nurbs {
        /// Control-point polyline.
        control: Vec<Vec3>,
        /// Per-control-point weights (same length as `control`).
        weights: Vec<f32>,
        /// Clamped knot vector, length `control.len() + degree + 1`.
        knots:   Vec<f32>,
        /// Polynomial degree.
        degree:  u32,
    },
}

impl Curve {
    /// Evaluate the curve at parameter `t` (`[0, 1]` for bounded
    /// shapes, full revolution for circle / ellipse).
    ///
    /// NURBS curves map `t ∈ [0, 1]` onto the knot domain
    /// `[knots[degree], knots[n]]` and evaluate via rational de Boor.
    /// Degenerate cases — empty controls, zero-sum weights — fall back
    /// to the first control point.
    pub fn evaluate(&self, t: f32) -> Vec3 {
        match self {
            Curve::Line { start, end } => start.lerp(*end, t),
            Curve::Circle {
                center,
                axis,
                u_dir,
                radius,
            } => {
                let v_dir = axis.cross(*u_dir).normalize_or_zero();
                let angle = t * std::f32::consts::TAU;
                *center + (*u_dir * angle.cos() + v_dir * angle.sin()) * *radius
            }
            Curve::Ellipse {
                center,
                axis,
                u_dir,
                radius_maj,
                radius_min,
            } => {
                let v_dir = axis.cross(*u_dir).normalize_or_zero();
                let angle = t * std::f32::consts::TAU;
                *center + *u_dir * (angle.cos() * *radius_maj) + v_dir * (angle.sin() * *radius_min)
            }
            Curve::Nurbs {
                control,
                weights,
                knots,
                degree,
            } => nurbs_curve_point(control, weights, knots, *degree as usize, t),
        }
    }
}

/// Rational de-Boor point query for a clamped NURBS curve.
fn nurbs_curve_point(
    control: &[Vec3],
    weights: &[f32],
    knots: &[f32],
    degree: usize,
    t: f32,
) -> Vec3 {
    if control.is_empty() {
        return Vec3::ZERO;
    }
    let n = control.len() - 1;
    // Required knot-vector length: n + degree + 2.
    if knots.len() < n + degree + 2 {
        return control[0];
    }
    let u_min = knots[degree];
    let u_max = knots[n + 1];
    let u = u_min + t.clamp(0.0, 1.0) * (u_max - u_min);
    let span = find_span(n, degree, u, knots);
    let basis = basis_funcs(span, u, degree, knots);
    let mut num = Vec3::ZERO;
    let mut den = 0.0f32;
    for i in 0..=degree {
        let idx = span - degree + i;
        let cp = control[idx];
        let w = *weights.get(idx).unwrap_or(&1.0);
        num += cp * (basis[i] * w);
        den += basis[i] * w;
    }
    if den.abs() < 1e-12 {
        control[0]
    } else {
        num / den
    }
}

/// Locate the knot span containing `u` (the standard NURBS
/// "find span" from Piegl & Tiller). `n` is `control.len() - 1`,
/// `p` the degree. Assumes a clamped knot vector.
fn find_span(n: usize, p: usize, u: f32, knots: &[f32]) -> usize {
    if u >= knots[n + 1] {
        return n;
    }
    if u <= knots[p] {
        return p;
    }
    let mut low = p;
    let mut high = n + 1;
    let mut mid = (low + high) / 2;
    while u < knots[mid] || u >= knots[mid + 1] {
        if u < knots[mid] {
            high = mid;
        } else {
            low = mid;
        }
        mid = (low + high) / 2;
    }
    mid
}

/// Cox-de Boor basis-function evaluation. Returns the `p + 1`
/// non-zero basis values `N_{span-p, p}(u) .. N_{span, p}(u)`.
fn basis_funcs(span: usize, u: f32, p: usize, knots: &[f32]) -> Vec<f32> {
    let mut n = vec![0.0_f32; p + 1];
    let mut left = vec![0.0_f32; p + 1];
    let mut right = vec![0.0_f32; p + 1];
    n[0] = 1.0;
    for j in 1..=p {
        left[j] = u - knots[span + 1 - j];
        right[j] = knots[span + j] - u;
        let mut saved = 0.0f32;
        for r in 0..j {
            let denom = right[r + 1] + left[j - r];
            let temp = if denom.abs() < 1e-12 {
                0.0
            } else {
                n[r] / denom
            };
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }
    n
}

/// Continuous 3D surface — the geometric carrier of a [`Face`]. Each
/// variant parameterizes by `(u, v)` on a canonical domain.
#[derive(Debug, Clone)]
pub enum Surface {
    /// Infinite plane.
    Plane(Plane),
    /// Right circular cylinder. `u` sweeps around the axis, `v`
    /// advances along it.
    Cylinder {
        /// Axis origin.
        axis_origin: Vec3,
        /// Unit-length axis direction.
        axis:        Vec3,
        /// Reference direction perpendicular to `axis` for `u = 0`.
        u_dir:       Vec3,
        /// Cylinder radius.
        radius:      f32,
    },
    /// Rational tensor-product NURBS surface. Evaluated by running
    /// Cox-de Boor in `u` and `v` independently and combining the
    /// results (rational because weights multiply into the basis).
    Nurbs {
        /// 2D array of control points, `u` varying slowest.
        control: Vec<Vec<Vec3>>,
        /// Per-control-point weights, same shape as `control`.
        weights: Vec<Vec<f32>>,
        /// Clamped knot vector in `u`.
        knots_u: Vec<f32>,
        /// Clamped knot vector in `v`.
        knots_v: Vec<f32>,
        /// Degree in `u`.
        degree_u: u32,
        /// Degree in `v`.
        degree_v: u32,
    },
}

impl Surface {
    /// Evaluate the surface at `(u, v)`. Parameter domain for
    /// [`Surface::Plane`] is world units on the plane's basis; for
    /// [`Surface::Cylinder`] `u ∈ [0, 1]` sweeps around the axis and
    /// `v` is the axial offset; NURBS surfaces map `(u, v) ∈ [0, 1]²`
    /// onto the clamped knot domains.
    pub fn evaluate(&self, u: f32, v: f32) -> Vec3 {
        match self {
            Surface::Plane(plane) => {
                // u runs along an arbitrary basis vector of the plane.
                let any_perp = if plane.normal.x.abs() < 0.9 {
                    Vec3::X
                } else {
                    Vec3::Y
                };
                let u_dir = any_perp.cross(plane.normal).normalize_or_zero();
                let v_dir = plane.normal.cross(u_dir).normalize_or_zero();
                plane.origin + u_dir * u + v_dir * v
            }
            Surface::Cylinder {
                axis_origin,
                axis,
                u_dir,
                radius,
            } => {
                let v_dir = axis.cross(*u_dir).normalize_or_zero();
                let angle = u * std::f32::consts::TAU;
                *axis_origin
                    + (*u_dir * angle.cos() + v_dir * angle.sin()) * *radius
                    + *axis * v
            }
            Surface::Nurbs {
                control,
                weights,
                knots_u,
                knots_v,
                degree_u,
                degree_v,
            } => nurbs_surface_point(
                control,
                weights,
                knots_u,
                knots_v,
                *degree_u as usize,
                *degree_v as usize,
                u,
                v,
            ),
        }
    }
}

fn nurbs_surface_point(
    control: &[Vec<Vec3>],
    weights: &[Vec<f32>],
    knots_u: &[f32],
    knots_v: &[f32],
    degree_u: usize,
    degree_v: usize,
    u: f32,
    v: f32,
) -> Vec3 {
    if control.is_empty() || control[0].is_empty() {
        return Vec3::ZERO;
    }
    let nu = control.len() - 1;
    let nv = control[0].len() - 1;
    if knots_u.len() < nu + degree_u + 2 || knots_v.len() < nv + degree_v + 2 {
        return control[0][0];
    }
    let u_min = knots_u[degree_u];
    let u_max = knots_u[nu + 1];
    let v_min = knots_v[degree_v];
    let v_max = knots_v[nv + 1];
    let uu = u_min + u.clamp(0.0, 1.0) * (u_max - u_min);
    let vv = v_min + v.clamp(0.0, 1.0) * (v_max - v_min);
    let span_u = find_span(nu, degree_u, uu, knots_u);
    let span_v = find_span(nv, degree_v, vv, knots_v);
    let basis_u = basis_funcs(span_u, uu, degree_u, knots_u);
    let basis_v = basis_funcs(span_v, vv, degree_v, knots_v);
    let mut num = Vec3::ZERO;
    let mut den = 0.0f32;
    for i in 0..=degree_u {
        for j in 0..=degree_v {
            let iu = span_u - degree_u + i;
            let iv = span_v - degree_v + j;
            let cp = control[iu][iv];
            let w = weights
                .get(iu)
                .and_then(|row| row.get(iv))
                .copied()
                .unwrap_or(1.0);
            let b = basis_u[i] * basis_v[j];
            num += cp * (b * w);
            den += b * w;
        }
    }
    if den.abs() < 1e-12 {
        control[0][0]
    } else {
        num / den
    }
}

/// A zero-dimensional B-Rep entity.
#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    /// Identity within the parent [`Brep`].
    pub id:    VertexId,
    /// World-space location.
    pub point: Vec3,
}

/// One-dimensional B-Rep entity — a trimmed piece of a [`Curve`]
/// between two [`Vertex`] endpoints.
#[derive(Debug, Clone)]
pub struct Edge {
    /// Identity within the parent [`Brep`].
    pub id:    EdgeId,
    /// Carrier curve.
    pub curve: Curve,
    /// Parameter at the start of the trim range.
    pub t_min: f32,
    /// Parameter at the end of the trim range.
    pub t_max: f32,
    /// Vertex at `t_min`.
    pub start: VertexId,
    /// Vertex at `t_max`.
    pub end:   VertexId,
}

/// An ordered loop of edges. Typically closes around a single
/// [`Face`] (outer boundary) or punches a hole in one (inner loop).
#[derive(Debug, Clone)]
pub struct Wire {
    /// Identity within the parent [`Brep`].
    pub id:     WireId,
    /// Ordered edge ids. Orientation implied by order — the wire is
    /// traversed `edges[0].start → edges[0].end → edges[1].end → …`.
    pub edges:  Vec<EdgeId>,
    /// `true` when the wire's last edge ends at the first edge's
    /// start.
    pub closed: bool,
}

/// Two-dimensional B-Rep entity — a trimmed region of a [`Surface`].
#[derive(Debug, Clone)]
pub struct Face {
    /// Identity within the parent [`Brep`].
    pub id:          FaceId,
    /// Underlying surface.
    pub surface:     Surface,
    /// Outer bounding wire.
    pub outer_wire:  WireId,
    /// Inner wires (holes). May be empty.
    pub inner_wires: Vec<WireId>,
}

/// Two-dimensional manifold — a connected set of oriented faces.
#[derive(Debug, Clone)]
pub struct Shell {
    /// Identity within the parent [`Brep`].
    pub id:    ShellId,
    /// Member face ids.
    pub faces: Vec<FaceId>,
}

/// Three-dimensional B-Rep entity — the final output of a feature
/// recompute. Usually a single-shell closed manifold.
#[derive(Debug, Clone)]
pub struct Solid {
    /// Identity within the parent [`Brep`].
    pub id:     SolidId,
    /// Shells that bound the solid. Multiple shells describe a solid
    /// with internal voids (outer shell + hollow shells).
    pub shells: Vec<ShellId>,
}

/// B-Rep container: id-addressed storage for every topology object.
#[derive(Debug, Default, Clone)]
pub struct Brep {
    /// Every vertex keyed by id.
    pub vertices: HashMap<VertexId, Vertex>,
    /// Every edge keyed by id.
    pub edges:    HashMap<EdgeId, Edge>,
    /// Every wire keyed by id.
    pub wires:    HashMap<WireId, Wire>,
    /// Every face keyed by id.
    pub faces:    HashMap<FaceId, Face>,
    /// Every shell keyed by id.
    pub shells:   HashMap<ShellId, Shell>,
    /// Every solid keyed by id.
    pub solids:   HashMap<SolidId, Solid>,

    vertex_ids: IdAllocator<VertexId>,
    edge_ids:   IdAllocator<EdgeId>,
    wire_ids:   IdAllocator<WireId>,
    face_ids:   IdAllocator<FaceId>,
    shell_ids:  IdAllocator<ShellId>,
    solid_ids:  IdAllocator<SolidId>,
}

impl Brep {
    /// Empty B-Rep.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a fresh vertex at `point`, returning its new id.
    pub fn add_vertex(&mut self, point: Vec3) -> VertexId {
        let id = self.vertex_ids.next();
        self.vertices.insert(id, Vertex { id, point });
        id
    }

    /// Insert an edge. The start / end vertices are assumed to
    /// already exist in this B-Rep; no cross-check is performed here.
    pub fn add_edge(&mut self, curve: Curve, start: VertexId, end: VertexId) -> EdgeId {
        let id = self.edge_ids.next();
        self.edges.insert(
            id,
            Edge {
                id,
                curve,
                t_min: 0.0,
                t_max: 1.0,
                start,
                end,
            },
        );
        id
    }

    /// Insert a wire built from `edges`. Caller decides whether the
    /// wire is closed.
    pub fn add_wire(&mut self, edges: Vec<EdgeId>, closed: bool) -> WireId {
        let id = self.wire_ids.next();
        self.wires.insert(id, Wire { id, edges, closed });
        id
    }

    /// Insert a trimmed face. Inner wires default to empty.
    pub fn add_face(&mut self, surface: Surface, outer_wire: WireId) -> FaceId {
        let id = self.face_ids.next();
        self.faces.insert(
            id,
            Face {
                id,
                surface,
                outer_wire,
                inner_wires: Vec::new(),
            },
        );
        id
    }

    /// Insert a shell over the given face set.
    pub fn add_shell(&mut self, faces: Vec<FaceId>) -> ShellId {
        let id = self.shell_ids.next();
        self.shells.insert(id, Shell { id, faces });
        id
    }

    /// Insert a solid bounded by the given shells.
    pub fn add_solid(&mut self, shells: Vec<ShellId>) -> SolidId {
        let id = self.solid_ids.next();
        self.solids.insert(id, Solid { id, shells });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_curve_evaluates_linearly() {
        let curve = Curve::Line {
            start: Vec3::ZERO,
            end:   Vec3::new(10.0, 0.0, 0.0),
        };
        assert!((curve.evaluate(0.5) - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn circle_curve_loops() {
        let curve = Curve::Circle {
            center: Vec3::ZERO,
            axis:   Vec3::Z,
            u_dir:  Vec3::X,
            radius: 1.0,
        };
        let start = curve.evaluate(0.0);
        let full = curve.evaluate(1.0);
        assert!((start - full).length() < 1e-4);
    }

    #[test]
    fn nurbs_curve_as_degree_one_matches_control_poly() {
        // Degree-1 NURBS with clamped knots = open control polyline.
        let curve = Curve::Nurbs {
            control: vec![Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0)],
            weights: vec![1.0, 1.0, 1.0],
            knots:   vec![0.0, 0.0, 0.5, 1.0, 1.0],
            degree:  1,
        };
        assert!((curve.evaluate(0.0) - Vec3::ZERO).length() < 1e-4);
        assert!((curve.evaluate(0.25) - Vec3::new(0.5, 0.0, 0.0)).length() < 1e-4);
        assert!((curve.evaluate(1.0) - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn nurbs_quarter_circle_matches_true_arc() {
        // Degree-2 rational quarter circle (classic conic NURBS).
        let r = 1.0f32;
        let curve = Curve::Nurbs {
            control: vec![
                Vec3::new(r, 0.0, 0.0),
                Vec3::new(r, r, 0.0),
                Vec3::new(0.0, r, 0.0),
            ],
            weights: vec![1.0, std::f32::consts::FRAC_1_SQRT_2, 1.0],
            knots:   vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            degree:  2,
        };
        // Midpoint (t=0.5) must land on the unit circle.
        let mid = curve.evaluate(0.5);
        assert!((mid.length() - r).abs() < 1e-4, "mid = {mid:?}");
    }

    #[test]
    fn nurbs_surface_as_bilinear_matches_corners() {
        let surf = Surface::Nurbs {
            control: vec![
                vec![Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)],
                vec![Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0)],
            ],
            weights: vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            knots_u: vec![0.0, 0.0, 1.0, 1.0],
            knots_v: vec![0.0, 0.0, 1.0, 1.0],
            degree_u: 1,
            degree_v: 1,
        };
        assert!((surf.evaluate(0.0, 0.0) - Vec3::ZERO).length() < 1e-4);
        assert!((surf.evaluate(1.0, 1.0) - Vec3::new(1.0, 1.0, 1.0)).length() < 1e-4);
        assert!((surf.evaluate(1.0, 0.0) - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn ellipse_evaluates_to_axis_radii() {
        let e = Curve::Ellipse {
            center:     Vec3::ZERO,
            axis:       Vec3::Z,
            u_dir:      Vec3::X,
            radius_maj: 3.0,
            radius_min: 1.0,
        };
        let p0 = e.evaluate(0.0);
        let p_quarter = e.evaluate(0.25);
        assert!((p0 - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4);
        assert!((p_quarter - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn brep_assembles_unit_square_face() {
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
        let shell = brep.add_shell(vec![face]);
        let solid = brep.add_solid(vec![shell]);

        assert_eq!(brep.vertices.len(), 4);
        assert_eq!(brep.edges.len(), 4);
        assert!(brep.faces.contains_key(&face));
        assert!(brep.solids.contains_key(&solid));
    }
}
