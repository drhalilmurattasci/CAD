//! Triangle-mesh container + fundamental ops — the Blender /
//! game-tools layer.
//!
//! Unlike the B-Rep side of the kernel ([`crate::cad::kernel`]) this
//! module deals in discrete polygons: the output of tessellation,
//! the input to renderers, and the working representation for
//! subdivision / decimation / remeshing. Storage is deliberately
//! loose — positions in one `Vec`, triangle index triples in another,
//! optional parallel `Vec`s for normals and uvs. Same shape every
//! real-time engine uses, so handing a [`Mesh`] to a renderer is a
//! one-step copy.

use glam::{Mat3, Vec2, Vec3};

use crate::math::Aabb;

/// Triangle mesh with optional per-vertex attributes.
#[derive(Debug, Clone, Default)]
pub struct Mesh {
    /// Per-vertex world-space position.
    pub positions: Vec<Vec3>,
    /// Per-vertex normal. Either empty (no normals computed yet) or
    /// the same length as `positions`.
    pub normals:   Vec<Vec3>,
    /// Per-vertex texture coordinate. Either empty or the same
    /// length as `positions`.
    pub uvs:       Vec<Vec2>,
    /// Triangle list — each triple indexes into the position / normal
    /// / uv arrays.
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// Empty mesh.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored vertices.
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// Number of stored triangles.
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// `true` when the mesh has no geometry.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Push a vertex and return its index. Does not touch the
    /// normal / uv arrays — callers that maintain parallel arrays
    /// should push to all three themselves.
    pub fn push_vertex(&mut self, position: Vec3) -> u32 {
        let index = self.positions.len() as u32;
        self.positions.push(position);
        index
    }

    /// Push a triangle. Indices are not bounds-checked here — see
    /// [`validate`](Self::validate) for a post-hoc consistency sweep.
    pub fn push_triangle(&mut self, indices: [u32; 3]) {
        self.triangles.push(indices);
    }

    /// Axis-aligned bounding box of every position in the mesh, or
    /// `None` when the mesh is empty.
    pub fn bounds(&self) -> Option<Aabb> {
        let first = *self.positions.first()?;
        let mut min = first;
        let mut max = first;
        for p in &self.positions[1..] {
            min = min.min(*p);
            max = max.max(*p);
        }
        Some(Aabb { min, max })
    }

    /// Translate every vertex by `offset`.
    pub fn translate(&mut self, offset: Vec3) {
        for p in &mut self.positions {
            *p += offset;
        }
    }

    /// Scale every vertex component-wise by `factor`. Does not
    /// re-normalize normals — call [`recompute_normals`](Self::recompute_normals)
    /// if `factor` is non-uniform.
    pub fn scale(&mut self, factor: Vec3) {
        for p in &mut self.positions {
            *p *= factor;
        }
    }

    /// Apply a 3x3 rotation (or other linear transform) to every
    /// position and, if present, every normal.
    pub fn transform_linear(&mut self, m: Mat3) {
        for p in &mut self.positions {
            *p = m * *p;
        }
        for n in &mut self.normals {
            *n = (m * *n).normalize_or_zero();
        }
    }

    /// Merge `other` into `self`, appending positions / normals /
    /// uvs / triangles. Triangle indices from `other` are rebased
    /// onto the new position-vec length.
    pub fn merge(&mut self, other: &Mesh) {
        let offset = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.uvs.extend_from_slice(&other.uvs);
        self.triangles
            .extend(other.triangles.iter().map(|tri| {
                [tri[0] + offset, tri[1] + offset, tri[2] + offset]
            }));
    }

    /// Smooth-shaded per-vertex normals computed by averaging the
    /// unit-normal of every incident triangle. Overwrites any
    /// existing `normals` buffer.
    pub fn recompute_normals(&mut self) {
        self.normals.clear();
        self.normals.resize(self.positions.len(), Vec3::ZERO);
        for [i0, i1, i2] in &self.triangles {
            let p0 = self.positions[*i0 as usize];
            let p1 = self.positions[*i1 as usize];
            let p2 = self.positions[*i2 as usize];
            let n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
            self.normals[*i0 as usize] += n;
            self.normals[*i1 as usize] += n;
            self.normals[*i2 as usize] += n;
        }
        for n in &mut self.normals {
            *n = n.normalize_or_zero();
        }
    }

    /// Cheap consistency check: every triangle index is in range,
    /// and if attribute arrays are present they match the position
    /// array in length. Returns the first problem encountered.
    pub fn validate(&self) -> Result<(), MeshError> {
        let n = self.positions.len() as u32;
        if !self.normals.is_empty() && self.normals.len() != self.positions.len() {
            return Err(MeshError::AttributeLengthMismatch);
        }
        if !self.uvs.is_empty() && self.uvs.len() != self.positions.len() {
            return Err(MeshError::AttributeLengthMismatch);
        }
        for tri in &self.triangles {
            if tri[0] >= n || tri[1] >= n || tri[2] >= n {
                return Err(MeshError::IndexOutOfBounds);
            }
        }
        Ok(())
    }
}

/// Problems [`Mesh::validate`] can surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshError {
    /// A triangle index points past the end of `positions`.
    IndexOutOfBounds,
    /// `normals` or `uvs` is present but of the wrong length.
    AttributeLengthMismatch,
}

/// One step of midpoint triangle subdivision.
///
/// Splits every triangle into four by inserting new vertices at the
/// midpoint of each edge. Preserves the original positions and adds
/// new ones, so the mesh gets `4x` triangles for a `2x` vertex count
/// per pass. Good for quick smoothing; not a full Loop / Catmull-
/// Clark subdivision.
pub fn subdivide_midpoint(mesh: &Mesh) -> Mesh {
    use std::collections::HashMap;

    let mut out = Mesh {
        positions: mesh.positions.clone(),
        normals:   Vec::new(),
        uvs:       Vec::new(),
        triangles: Vec::with_capacity(mesh.triangles.len() * 4),
    };

    let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
    let mut midpoint = |out: &mut Mesh, a: u32, b: u32| -> u32 {
        let key = if a < b { (a, b) } else { (b, a) };
        if let Some(idx) = midpoints.get(&key) {
            return *idx;
        }
        let mid = (out.positions[a as usize] + out.positions[b as usize]) * 0.5;
        let idx = out.push_vertex(mid);
        midpoints.insert(key, idx);
        idx
    };

    for [i0, i1, i2] in &mesh.triangles {
        let m01 = midpoint(&mut out, *i0, *i1);
        let m12 = midpoint(&mut out, *i1, *i2);
        let m20 = midpoint(&mut out, *i2, *i0);
        out.push_triangle([*i0, m01, m20]);
        out.push_triangle([*i1, m12, m01]);
        out.push_triangle([*i2, m20, m12]);
        out.push_triangle([m01, m12, m20]);
    }
    out
}

/// Decimation placeholder — quadric-error edge-collapse is the
/// canonical implementation but well out-of-scope here. Currently
/// returns the input mesh unchanged.
///
/// Kept in the API as a named entry point so downstream code can
/// call `decimate(&mesh, ratio)` today and pick up a real
/// implementation later without a signature change.
pub fn decimate(mesh: &Mesh, _target_ratio: f32) -> Mesh {
    // TODO: implement quadric-error-metric edge collapse.
    mesh.clone()
}

/// Remesh placeholder — isotropic remeshing to a target edge length.
/// Currently a no-op; see [`decimate`] for the same rationale.
pub fn remesh(mesh: &Mesh, _target_edge_length: f32) -> Mesh {
    // TODO: implement isotropic remesh (split / collapse / flip +
    // tangential smoothing).
    mesh.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_triangle() -> Mesh {
        let mut mesh = Mesh::new();
        mesh.push_vertex(Vec3::ZERO);
        mesh.push_vertex(Vec3::X);
        mesh.push_vertex(Vec3::Y);
        mesh.push_triangle([0, 1, 2]);
        mesh
    }

    #[test]
    fn validate_catches_oob_index() {
        let mut mesh = unit_triangle();
        mesh.triangles.push([0, 1, 99]);
        assert_eq!(mesh.validate(), Err(MeshError::IndexOutOfBounds));
    }

    #[test]
    fn bounds_reports_unit_aabb() {
        let mesh = unit_triangle();
        let aabb = mesh.bounds().unwrap();
        assert!((aabb.min - Vec3::ZERO).length() < 1e-5);
        assert!((aabb.max - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn subdivide_quadruples_triangle_count() {
        let mesh = unit_triangle();
        let sub = subdivide_midpoint(&mesh);
        assert_eq!(sub.triangle_count(), 4);
        assert_eq!(sub.vertex_count(), 6); // 3 original + 3 midpoints
    }

    #[test]
    fn recompute_normals_points_z_for_xy_triangle() {
        let mut mesh = unit_triangle();
        mesh.recompute_normals();
        for n in &mesh.normals {
            assert!((n - &Vec3::Z).length() < 1e-5, "got {n:?}");
        }
    }

    #[test]
    fn merge_rebases_indices() {
        let a = unit_triangle();
        let b = unit_triangle();
        let mut merged = a;
        merged.merge(&b);
        assert_eq!(merged.vertex_count(), 6);
        assert_eq!(merged.triangle_count(), 2);
        assert_eq!(merged.triangles[1], [3, 4, 5]);
    }
}
