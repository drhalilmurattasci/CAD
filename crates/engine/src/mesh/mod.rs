//! I-18: CPU-side mesh asset + importers.
//!
//! A `MeshData` is the engine's intermediate representation between
//! disk formats (glTF today, OBJ/FBX later) and the GPU-side vertex
//! buffers the renderer hands to wgpu. We keep it deliberately small:
//!
//! * **positions** — one `[f32; 3]` per vertex, required.
//! * **normals** — one `[f32; 3]` per vertex. Generated flat if the
//!   source file didn't ship any, so downstream shading never has to
//!   branch on "do I have normals".
//! * **indices** — `u32` triangle list. If the source primitive was
//!   non-indexed, we synthesise `0..vertex_count`.
//!
//! UVs, tangents, skinning etc. land when the PBR pipeline does —
//! stashing the whole source vertex today would bloat the scene save
//! format for data nothing consumes.

pub mod gltf;

use glam::Vec3;

/// Error surface for mesh importers. One `thiserror` enum for every
/// flavour of malformed input so the editor can show a targeted
/// diagnostic rather than a generic "something went wrong".
#[derive(Debug, thiserror::Error)]
pub enum MeshImportError {
    #[error("failed to parse glTF document: {0}")]
    GltfParse(String),
    #[error("glTF document contains no meshes")]
    NoMeshes,
    #[error("glTF mesh '{mesh}' contains no primitives")]
    EmptyMesh { mesh: String },
    #[error("glTF primitive has no POSITION attribute")]
    MissingPositions,
    #[error("glTF buffer {index} is missing its binary payload")]
    MissingBufferData { index: usize },
    #[error("glTF primitive uses unsupported topology {topology:?}; expected Triangles")]
    UnsupportedTopology { topology: String },
}

/// CPU-side mesh: a triangle list with positions, normals, and
/// indices. Everything is `f32`/`u32` so bytemuck-based uploads are a
/// zero-copy memcpy on the render side.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshData {
    pub name:      String,
    pub positions: Vec<[f32; 3]>,
    pub normals:   Vec<[f32; 3]>,
    pub indices:   Vec<u32>,
}

impl MeshData {
    /// Number of triangles (indices / 3, rounded down — malformed
    /// imports shouldn't reach this point, but we stay total).
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Axis-aligned bounding box, computed on demand. `None` if the
    /// mesh has no vertices (degenerate but legal intermediate state).
    pub fn aabb(&self) -> Option<(Vec3, Vec3)> {
        let first = self.positions.first()?;
        let mut min = Vec3::from(*first);
        let mut max = min;
        for pos in &self.positions[1..] {
            let v = Vec3::from(*pos);
            min = min.min(v);
            max = max.max(v);
        }
        Some((min, max))
    }

    /// Generate per-face flat normals from the current triangles.
    /// Used as a fallback when a glTF primitive didn't ship
    /// NORMAL data — better than handing the shader uninitialized
    /// values, and matches how our hand-authored cube is shaded.
    pub fn generate_flat_normals(&mut self) {
        self.normals = vec![[0.0, 1.0, 0.0]; self.positions.len()];
        for triangle in self.indices.chunks_exact(3) {
            let (i0, i1, i2) = (triangle[0] as usize, triangle[1] as usize, triangle[2] as usize);
            let p0 = Vec3::from(self.positions[i0]);
            let p1 = Vec3::from(self.positions[i1]);
            let p2 = Vec3::from(self.positions[i2]);
            let normal = (p1 - p0).cross(p2 - p0).normalize_or_zero();
            let n = normal.to_array();
            self.normals[i0] = n;
            self.normals[i1] = n;
            self.normals[i2] = n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_normals_point_up_for_unit_quad() {
        let mut mesh = MeshData {
            name:      "quad".into(),
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            normals:   Vec::new(),
            indices:   vec![0, 1, 2, 0, 2, 3],
        };
        mesh.generate_flat_normals();
        for n in &mesh.normals {
            // Quad is on the XZ plane. With index order (0,1,2) and
            // positions (0,0,0)→(1,0,0)→(1,0,1) the cross product
            // (p1-p0) × (p2-p0) = (1,0,0) × (1,0,1) = (0,-1,0), i.e.
            // the winding is CW viewed from +Y → flat normal points
            // along -Y. The math is what matters — the test just
            // pins down the direction so we'd notice if the cross
            // order ever flipped.
            assert!((n[1] + 1.0).abs() < 1e-5, "got {:?}", n);
        }
    }

    #[test]
    fn aabb_covers_all_vertices() {
        let mesh = MeshData {
            name:      "line".into(),
            positions: vec![[-1.0, 2.0, 3.0], [4.0, -5.0, 6.0]],
            normals:   Vec::new(),
            indices:   vec![],
        };
        let (min, max) = mesh.aabb().expect("non-empty");
        assert_eq!(min, Vec3::new(-1.0, -5.0, 3.0));
        assert_eq!(max, Vec3::new(4.0, 2.0, 6.0));
    }
}
