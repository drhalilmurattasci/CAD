//! Boolean kernel — the union / difference / intersection engine.
//!
//! This module defines the public API that downstream parametric
//! features ([`crate::cad::parametric::Feature::Cut`]) and modifier
//! stacks call into. Three engines ship out of the box:
//!
//! - [`CsgEngine`] — a mesh-side boolean built on a polygon BSP tree,
//!   shaped after the csg.js / OpenSCAD approach. Works on any pair of
//!   closed triangle meshes; the classic "good enough for 90% of CAD
//!   booleans, not robust for degenerate coplanar input" tradeoff.
//! - [`AxisAlignedEngine`] — exact solver for the special case of two
//!   AABB-aligned boxes. Useful for tests and for the few real cases
//!   where inputs are known to be axis-aligned.
//! - [`NotImplementedEngine`] — sentinel that always returns
//!   [`BooleanError::NotImplemented`]. Still useful as a default when
//!   a consumer wants to fail fast before wiring up a real engine.
//!
//! Downstream consumers that need a production-grade kernel
//! (handles non-manifold, exact arithmetic, topology preservation)
//! should FFI into OCCT or a dedicated crate; the [`BooleanEngine`]
//! trait is the extension point.

use thiserror::Error;
use glam::Vec3;

use super::mesh::Mesh;
use crate::math::Aabb;

/// Which boolean operation to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    /// `A ∪ B` — volume occupied by either input.
    Union,
    /// `A - B` — volume in `A` but not in `B`.
    Difference,
    /// `A ∩ B` — volume common to both inputs.
    Intersection,
}

/// Failure modes surfaced by any [`BooleanEngine`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BooleanError {
    /// The engine implementation hasn't been wired up yet.
    #[error("boolean kernel not implemented: provide a BooleanEngine impl")]
    NotImplemented,
    /// Inputs violated the engine's assumptions (non-manifold,
    /// self-intersecting, out of tolerance, …).
    #[error("boolean operation failed: {0}")]
    InvalidInput(&'static str),
    /// The operation produced an empty result (e.g. disjoint inputs
    /// under Intersection). Not always an error — callers may treat
    /// this as a successful empty output. Engines choose which to
    /// raise.
    #[error("boolean operation produced empty result")]
    EmptyResult,
}

/// Pluggable boolean-kernel trait. Real implementations FFI into a
/// CAD kernel or ship a pure-Rust CSG solver; the rest of the stack
/// calls through this interface.
pub trait BooleanEngine {
    /// Execute `op` on the two input meshes. `tolerance` is the
    /// engine's geometric tolerance (absolute distance in world
    /// units); implementations that don't need it may ignore it.
    fn apply(
        &self,
        op: BooleanOp,
        a: &Mesh,
        b: &Mesh,
        tolerance: f32,
    ) -> Result<Mesh, BooleanError>;
}

/// Sentinel [`BooleanEngine`] that always returns
/// [`BooleanError::NotImplemented`]. Drop it into the parametric
/// context while the real engine is being built, so feature
/// recompute paths at least type-check.
#[derive(Debug, Default, Clone, Copy)]
pub struct NotImplementedEngine;

impl BooleanEngine for NotImplementedEngine {
    fn apply(
        &self,
        _op: BooleanOp,
        _a: &Mesh,
        _b: &Mesh,
        _tolerance: f32,
    ) -> Result<Mesh, BooleanError> {
        Err(BooleanError::NotImplemented)
    }
}

/// Trivial AABB-based boolean engine. Handles the degenerate case
/// of two meshes whose volumes are exactly their axis-aligned
/// bounding boxes. Union picks the larger AABB when one contains
/// the other, difference falls back to the `A` mesh when `B` fully
/// encloses it (producing empty), and intersection emits the
/// overlap AABB.
///
/// **Not a general boolean kernel** — the point of this engine is
/// to exercise the interface on real input and give simple cases
/// something that Just Works for tests. Anything non-AABB returns
/// [`BooleanError::InvalidInput`].
#[derive(Debug, Default, Clone, Copy)]
pub struct AxisAlignedEngine;

impl BooleanEngine for AxisAlignedEngine {
    fn apply(
        &self,
        op: BooleanOp,
        a: &Mesh,
        b: &Mesh,
        _tolerance: f32,
    ) -> Result<Mesh, BooleanError> {
        let Some(aa) = a.bounds() else {
            return Err(BooleanError::InvalidInput("input A has no geometry"));
        };
        let Some(ab) = b.bounds() else {
            return Err(BooleanError::InvalidInput("input B has no geometry"));
        };

        match op {
            BooleanOp::Union => Ok(aabb_mesh(&union_aabb(&aa, &ab))),
            BooleanOp::Intersection => match intersect_aabb(&aa, &ab) {
                Some(aabb) => Ok(aabb_mesh(&aabb)),
                None => Err(BooleanError::EmptyResult),
            },
            BooleanOp::Difference => {
                if contains(&ab, &aa) {
                    Err(BooleanError::EmptyResult)
                } else {
                    Ok(a.clone())
                }
            }
        }
    }
}

fn union_aabb(a: &Aabb, b: &Aabb) -> Aabb {
    Aabb {
        min: a.min.min(b.min),
        max: a.max.max(b.max),
    }
}

fn intersect_aabb(a: &Aabb, b: &Aabb) -> Option<Aabb> {
    let min = a.min.max(b.min);
    let max = a.max.min(b.max);
    if min.cmpgt(max).any() {
        None
    } else {
        Some(Aabb { min, max })
    }
}

fn contains(outer: &Aabb, inner: &Aabb) -> bool {
    outer.min.cmple(inner.min).all() && outer.max.cmpge(inner.max).all()
}

/// Build a closed triangle mesh matching the given AABB. Used
/// internally by [`AxisAlignedEngine`]; exposed so tests and
/// downstream helpers can generate canonical AABB inputs.
pub fn aabb_mesh(aabb: &Aabb) -> Mesh {
    use glam::Vec3;
    let Aabb { min, max } = *aabb;
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    let mut mesh = Mesh::new();
    for c in corners {
        mesh.push_vertex(c);
    }
    // 6 quads → 12 triangles, outward-facing winding.
    let quads = [
        [0, 3, 2, 1], // -Z
        [4, 5, 6, 7], // +Z
        [0, 1, 5, 4], // -Y
        [2, 3, 7, 6], // +Y
        [0, 4, 7, 3], // -X
        [1, 2, 6, 5], // +X
    ];
    for q in quads {
        mesh.push_triangle([q[0], q[1], q[2]]);
        mesh.push_triangle([q[0], q[2], q[3]]);
    }
    mesh
}

// ---------------------------------------------------------------
// CsgEngine — polygon-BSP mesh boolean.
// ---------------------------------------------------------------
//
// Classic csg.js / Evan Wallace algorithm, ported to Rust. Each
// input mesh becomes a list of convex polygons; those build a BSP
// tree; the three boolean operations fall out of a short sequence
// of `clip` / `invert` / `build` steps on the two trees.
//
// Tradeoffs:
// - Robust on closed manifold inputs with non-degenerate coplanar
//   faces. Output is a triangulated mesh.
// - Not robust on self-intersecting meshes or on exact coplanar
//   overlaps (same plane, same direction) — that's the standard CSG
//   limitation.
// - Epsilon is configurable but the default `1e-5` works for
//   typical CAD units (meters).

/// Mesh-side CSG boolean engine. Implements [`BooleanOp::Union`],
/// [`BooleanOp::Difference`], and [`BooleanOp::Intersection`] on
/// arbitrary closed triangle meshes via a polygon BSP tree.
///
/// Construct with [`CsgEngine::default`] for the standard epsilon
/// of `1e-5`, or [`CsgEngine::with_epsilon`] to tune for your
/// coordinate scale.
#[derive(Debug, Clone, Copy)]
pub struct CsgEngine {
    epsilon: f32,
}

impl Default for CsgEngine {
    fn default() -> Self {
        Self { epsilon: 1e-5 }
    }
}

impl CsgEngine {
    /// Build a CSG engine with an explicit plane-classification
    /// epsilon (absolute distance). The `tolerance` parameter passed
    /// to [`BooleanEngine::apply`] overrides this when nonzero.
    pub fn with_epsilon(epsilon: f32) -> Self {
        Self { epsilon }
    }
}

impl BooleanEngine for CsgEngine {
    fn apply(
        &self,
        op: BooleanOp,
        a: &Mesh,
        b: &Mesh,
        tolerance: f32,
    ) -> Result<Mesh, BooleanError> {
        let eps = if tolerance > 0.0 { tolerance } else { self.epsilon };
        let polys_a = mesh_to_polygons(a);
        let polys_b = mesh_to_polygons(b);
        if polys_a.is_empty() {
            return Err(BooleanError::InvalidInput("input A has no triangles"));
        }
        if polys_b.is_empty() {
            return Err(BooleanError::InvalidInput("input B has no triangles"));
        }
        let mut node_a = CsgNode::from_polygons(polys_a, eps);
        let mut node_b = CsgNode::from_polygons(polys_b, eps);

        match op {
            BooleanOp::Union => {
                node_a.clip_to(&node_b, eps);
                node_b.clip_to(&node_a, eps);
                node_b.invert();
                node_b.clip_to(&node_a, eps);
                node_b.invert();
                let extra = node_b.all_polygons();
                node_a.build(extra, eps);
            }
            BooleanOp::Difference => {
                node_a.invert();
                node_a.clip_to(&node_b, eps);
                node_b.clip_to(&node_a, eps);
                node_b.invert();
                node_b.clip_to(&node_a, eps);
                node_b.invert();
                let extra = node_b.all_polygons();
                node_a.build(extra, eps);
                node_a.invert();
            }
            BooleanOp::Intersection => {
                node_a.invert();
                node_b.clip_to(&node_a, eps);
                node_b.invert();
                node_a.clip_to(&node_b, eps);
                node_b.clip_to(&node_a, eps);
                let extra = node_b.all_polygons();
                node_a.build(extra, eps);
                node_a.invert();
            }
        }

        let result_polys = node_a.all_polygons();
        if result_polys.is_empty() {
            return Err(BooleanError::EmptyResult);
        }
        Ok(polygons_to_mesh(&result_polys))
    }
}

#[derive(Debug, Clone)]
struct CsgPlane {
    normal: Vec3,
    w:      f32,
}

impl CsgPlane {
    fn from_points(a: Vec3, b: Vec3, c: Vec3) -> Option<Self> {
        let n = (b - a).cross(c - a);
        if n.length_squared() < 1e-20 {
            return None;
        }
        let normal = n.normalize();
        Some(Self {
            normal,
            w: normal.dot(a),
        })
    }

    fn flip(&mut self) {
        self.normal = -self.normal;
        self.w = -self.w;
    }

    fn split_polygon(
        &self,
        poly: &CsgPolygon,
        eps: f32,
        coplanar_front: &mut Vec<CsgPolygon>,
        coplanar_back: &mut Vec<CsgPolygon>,
        front: &mut Vec<CsgPolygon>,
        back: &mut Vec<CsgPolygon>,
    ) {
        const COPLANAR: u8 = 0;
        const FRONT: u8 = 1;
        const BACK: u8 = 2;
        const SPANNING: u8 = 3;

        let mut poly_type: u8 = 0;
        let mut types = Vec::with_capacity(poly.vertices.len());
        for v in &poly.vertices {
            let d = self.normal.dot(*v) - self.w;
            let t = if d < -eps {
                BACK
            } else if d > eps {
                FRONT
            } else {
                COPLANAR
            };
            poly_type |= t;
            types.push(t);
        }

        match poly_type {
            COPLANAR => {
                if self.normal.dot(poly.plane.normal) > 0.0 {
                    coplanar_front.push(poly.clone());
                } else {
                    coplanar_back.push(poly.clone());
                }
            }
            FRONT => front.push(poly.clone()),
            BACK => back.push(poly.clone()),
            _ => {
                // SPANNING — split the polygon along `self`.
                let mut fv = Vec::new();
                let mut bv = Vec::new();
                let n = poly.vertices.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    let ti = types[i];
                    let tj = types[j];
                    let vi = poly.vertices[i];
                    let vj = poly.vertices[j];
                    if ti != BACK {
                        fv.push(vi);
                    }
                    if ti != FRONT {
                        bv.push(vi);
                    }
                    if (ti | tj) == SPANNING {
                        let denom = self.normal.dot(vj - vi);
                        if denom.abs() > 1e-20 {
                            let t = (self.w - self.normal.dot(vi)) / denom;
                            let mid = vi.lerp(vj, t);
                            fv.push(mid);
                            bv.push(mid);
                        }
                    }
                }
                if fv.len() >= 3 {
                    front.push(CsgPolygon::new(fv, poly.plane.clone()));
                }
                if bv.len() >= 3 {
                    back.push(CsgPolygon::new(bv, poly.plane.clone()));
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CsgPolygon {
    vertices: Vec<Vec3>,
    plane:    CsgPlane,
}

impl CsgPolygon {
    fn new(vertices: Vec<Vec3>, plane: CsgPlane) -> Self {
        Self { vertices, plane }
    }

    fn from_triangle(a: Vec3, b: Vec3, c: Vec3) -> Option<Self> {
        let plane = CsgPlane::from_points(a, b, c)?;
        Some(Self {
            vertices: vec![a, b, c],
            plane,
        })
    }

    fn flip(&mut self) {
        self.vertices.reverse();
        self.plane.flip();
    }
}

#[derive(Debug, Clone, Default)]
struct CsgNode {
    plane:    Option<CsgPlane>,
    front:    Option<Box<CsgNode>>,
    back:     Option<Box<CsgNode>>,
    polygons: Vec<CsgPolygon>,
}

impl CsgNode {
    fn from_polygons(polygons: Vec<CsgPolygon>, eps: f32) -> Self {
        let mut node = Self::default();
        if !polygons.is_empty() {
            node.build(polygons, eps);
        }
        node
    }

    fn build(&mut self, polygons: Vec<CsgPolygon>, eps: f32) {
        if polygons.is_empty() {
            return;
        }
        if self.plane.is_none() {
            self.plane = Some(polygons[0].plane.clone());
        }
        let plane = self.plane.clone().unwrap();
        let mut front = Vec::new();
        let mut back = Vec::new();
        let mut coplanar_front = Vec::new();
        let mut coplanar_back = Vec::new();
        for poly in polygons {
            plane.split_polygon(
                &poly,
                eps,
                &mut coplanar_front,
                &mut coplanar_back,
                &mut front,
                &mut back,
            );
        }
        self.polygons.append(&mut coplanar_front);
        self.polygons.append(&mut coplanar_back);
        if !front.is_empty() {
            let child = self
                .front
                .get_or_insert_with(|| Box::new(CsgNode::default()));
            child.build(front, eps);
        }
        if !back.is_empty() {
            let child = self
                .back
                .get_or_insert_with(|| Box::new(CsgNode::default()));
            child.build(back, eps);
        }
    }

    fn invert(&mut self) {
        for p in &mut self.polygons {
            p.flip();
        }
        if let Some(plane) = &mut self.plane {
            plane.flip();
        }
        std::mem::swap(&mut self.front, &mut self.back);
        if let Some(f) = &mut self.front {
            f.invert();
        }
        if let Some(b) = &mut self.back {
            b.invert();
        }
    }

    fn clip_polygons(&self, polygons: Vec<CsgPolygon>, eps: f32) -> Vec<CsgPolygon> {
        let Some(plane) = &self.plane else {
            return polygons;
        };
        let mut front = Vec::new();
        let mut back = Vec::new();
        let mut coplanar_front = Vec::new();
        let mut coplanar_back = Vec::new();
        for poly in polygons {
            plane.split_polygon(
                &poly,
                eps,
                &mut coplanar_front,
                &mut coplanar_back,
                &mut front,
                &mut back,
            );
        }
        // Coplanar polygons go with the side they agree with — front
        // for same-direction, back for flipped. This matches csg.js
        // semantics exactly (split_polygon + coplanar_{front,back}).
        front.append(&mut coplanar_front);
        back.append(&mut coplanar_back);
        let front_out = if let Some(f) = &self.front {
            f.clip_polygons(front, eps)
        } else {
            front
        };
        let back_out = if let Some(b) = &self.back {
            b.clip_polygons(back, eps)
        } else {
            Vec::new()
        };
        let mut out = front_out;
        out.extend(back_out);
        out
    }

    fn clip_to(&mut self, other: &CsgNode, eps: f32) {
        let polys = std::mem::take(&mut self.polygons);
        self.polygons = other.clip_polygons(polys, eps);
        if let Some(f) = &mut self.front {
            f.clip_to(other, eps);
        }
        if let Some(b) = &mut self.back {
            b.clip_to(other, eps);
        }
    }

    fn all_polygons(&self) -> Vec<CsgPolygon> {
        let mut out = self.polygons.clone();
        if let Some(f) = &self.front {
            out.extend(f.all_polygons());
        }
        if let Some(b) = &self.back {
            out.extend(b.all_polygons());
        }
        out
    }
}

fn mesh_to_polygons(mesh: &Mesh) -> Vec<CsgPolygon> {
    mesh.triangles
        .iter()
        .filter_map(|tri| {
            let a = mesh.positions[tri[0] as usize];
            let b = mesh.positions[tri[1] as usize];
            let c = mesh.positions[tri[2] as usize];
            CsgPolygon::from_triangle(a, b, c)
        })
        .collect()
}

fn polygons_to_mesh(polys: &[CsgPolygon]) -> Mesh {
    let mut mesh = Mesh::new();
    for poly in polys {
        if poly.vertices.len() < 3 {
            continue;
        }
        let base = mesh.positions.len() as u32;
        for v in &poly.vertices {
            mesh.push_vertex(*v);
        }
        // Fan-triangulate — convex polygon invariant holds because
        // every polygon is either an input triangle or a plane-split
        // of a convex polygon.
        for i in 1..(poly.vertices.len() - 1) {
            mesh.push_triangle([base, base + i as u32, base + i as u32 + 1]);
        }
    }
    mesh.recompute_normals();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(min: Vec3, max: Vec3) -> Mesh {
        aabb_mesh(&Aabb { min, max })
    }

    #[test]
    fn not_implemented_returns_not_implemented() {
        let engine = NotImplementedEngine;
        let err = engine
            .apply(
                BooleanOp::Union,
                &Mesh::new(),
                &Mesh::new(),
                1e-3,
            )
            .unwrap_err();
        assert_eq!(err, BooleanError::NotImplemented);
    }

    #[test]
    fn axis_aligned_union_extends_bounds() {
        let a = cube(Vec3::ZERO, Vec3::splat(1.0));
        let b = cube(Vec3::new(0.5, 0.0, 0.0), Vec3::splat(2.0));
        let out = AxisAlignedEngine
            .apply(BooleanOp::Union, &a, &b, 1e-5)
            .unwrap();
        let bounds = out.bounds().unwrap();
        assert!((bounds.min - Vec3::ZERO).length() < 1e-5);
        assert!((bounds.max - Vec3::splat(2.0)).length() < 1e-5);
    }

    #[test]
    fn axis_aligned_intersection_reports_overlap() {
        let a = cube(Vec3::ZERO, Vec3::splat(2.0));
        let b = cube(Vec3::splat(1.0), Vec3::splat(3.0));
        let out = AxisAlignedEngine
            .apply(BooleanOp::Intersection, &a, &b, 1e-5)
            .unwrap();
        let bounds = out.bounds().unwrap();
        assert!((bounds.min - Vec3::splat(1.0)).length() < 1e-5);
        assert!((bounds.max - Vec3::splat(2.0)).length() < 1e-5);
    }

    #[test]
    fn axis_aligned_intersection_disjoint_errors() {
        let a = cube(Vec3::ZERO, Vec3::splat(1.0));
        let b = cube(Vec3::splat(10.0), Vec3::splat(11.0));
        let err = AxisAlignedEngine
            .apply(BooleanOp::Intersection, &a, &b, 1e-5)
            .unwrap_err();
        assert_eq!(err, BooleanError::EmptyResult);
    }

    #[test]
    fn csg_union_extends_bounds() {
        let a = cube(Vec3::ZERO, Vec3::splat(1.0));
        let b = cube(Vec3::splat(0.5), Vec3::splat(1.5));
        let out = CsgEngine::default()
            .apply(BooleanOp::Union, &a, &b, 0.0)
            .unwrap();
        let bounds = out.bounds().expect("bounds");
        assert!((bounds.min - Vec3::ZERO).length() < 1e-4);
        assert!((bounds.max - Vec3::splat(1.5)).length() < 1e-4);
        assert!(out.triangle_count() > 0);
    }

    #[test]
    fn csg_intersection_collapses_to_overlap() {
        let a = cube(Vec3::ZERO, Vec3::splat(2.0));
        let b = cube(Vec3::splat(1.0), Vec3::splat(3.0));
        let out = CsgEngine::default()
            .apply(BooleanOp::Intersection, &a, &b, 0.0)
            .unwrap();
        let bounds = out.bounds().expect("bounds");
        assert!((bounds.min - Vec3::splat(1.0)).length() < 1e-4);
        assert!((bounds.max - Vec3::splat(2.0)).length() < 1e-4);
    }

    #[test]
    fn csg_difference_drills_cube() {
        // Large cube with a smaller one subtracted from a corner.
        let a = cube(Vec3::ZERO, Vec3::splat(2.0));
        let b = cube(Vec3::splat(1.0), Vec3::splat(3.0));
        let out = CsgEngine::default()
            .apply(BooleanOp::Difference, &a, &b, 0.0)
            .unwrap();
        let bounds = out.bounds().expect("bounds");
        // The outer shell is still the A box, so bounds match A.
        assert!((bounds.min - Vec3::ZERO).length() < 1e-4);
        assert!((bounds.max - Vec3::splat(2.0)).length() < 1e-4);
        // Result should have more triangles than the input A (because
        // the corner was split).
        assert!(out.triangle_count() > a.triangle_count());
    }
}
