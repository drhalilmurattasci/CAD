//! Renderer-agnostic draw-data + picking bridge.
//!
//! The CAD stack outputs meshes ([`crate::cad::mesh`]); GPU backends
//! (Bevy, raw wgpu, Vulkan, WebGPU) expect a slightly tighter packed
//! format — position + normal + uv per vertex, indexed triangles.
//! This module owns the canonical vertex layout ([`DrawVertex`]) and
//! the conversion from [`Mesh`] ([`mesh_to_draw_data`]), plus the
//! ray/mesh picking helpers that every editor viewport needs.
//!
//! Nothing here opens a window or binds a GPU buffer — that's the
//! job of the host engine. The layer is deliberately shaped as
//! passive POD so a renderer can memcpy it straight into a
//! `wgpu::Buffer`.

use glam::{Vec2, Vec3};

use super::core::EntityId;
use super::mesh::Mesh;
use crate::math::Ray;

/// Canonical per-vertex GPU layout used by the CAD stack.
///
/// Layout is `position (3) + normal (3) + uv (2)` — 8 floats,
/// 32 bytes. Matches what `bevy_render`'s default mesh vertex
/// attributes expect, and unpacks cleanly into a raw wgpu layout
/// without reshuffling.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawVertex {
    /// World-space position.
    pub position: Vec3,
    /// Shading normal.
    pub normal:   Vec3,
    /// Texture coordinate.
    pub uv:       Vec2,
}

/// Indexed triangle-list bundle for a single drawable.
#[derive(Debug, Clone, Default)]
pub struct DrawData {
    /// Packed vertex array.
    pub vertices: Vec<DrawVertex>,
    /// Triangle indices — every three is one triangle.
    pub indices:  Vec<u32>,
}

impl DrawData {
    /// Number of triangles (`indices.len() / 3`).
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Convert a [`Mesh`] into renderer-ready [`DrawData`].
///
/// - Missing normals are filled from a fresh `recompute_normals`.
/// - Missing uvs are filled with zero — downstream pipelines that
///   need real uvs should compute them before calling this.
pub fn mesh_to_draw_data(mesh: &Mesh) -> DrawData {
    let mut work = mesh.clone();
    if work.normals.len() != work.positions.len() {
        work.recompute_normals();
    }
    let vertex_count = work.positions.len();
    let mut vertices = Vec::with_capacity(vertex_count);
    for i in 0..vertex_count {
        let position = work.positions[i];
        let normal = *work.normals.get(i).unwrap_or(&Vec3::Z);
        let uv = *work.uvs.get(i).unwrap_or(&Vec2::ZERO);
        vertices.push(DrawVertex {
            position,
            normal,
            uv,
        });
    }
    let indices = work
        .triangles
        .iter()
        .flat_map(|tri| tri.iter().copied())
        .collect();
    DrawData { vertices, indices }
}

/// Outcome of a successful ray/mesh hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickHit {
    /// Distance from the ray origin.
    pub t:        f32,
    /// Hit point in world space.
    pub point:    Vec3,
    /// Triangle index within the mesh's `triangles` array.
    pub triangle: usize,
}

/// Möller-Trumbore ray/triangle intersection, returning the closest
/// positive hit across every triangle in `mesh`.
///
/// An AABB pre-pass (via [`Mesh::bounds`] + [`Aabb::ray_hit`]) skips
/// the full sweep when the ray misses the mesh outright.
pub fn pick_mesh(ray: &Ray, mesh: &Mesh) -> Option<PickHit> {
    if let Some(aabb) = mesh.bounds() {
        if aabb.ray_hit(ray).is_none() {
            return None;
        }
    } else {
        return None;
    }

    let mut closest: Option<PickHit> = None;
    for (i, tri) in mesh.triangles.iter().enumerate() {
        let v0 = mesh.positions[tri[0] as usize];
        let v1 = mesh.positions[tri[1] as usize];
        let v2 = mesh.positions[tri[2] as usize];
        if let Some(t) = ray_triangle(ray, v0, v1, v2) {
            let point = ray.origin + ray.direction * t;
            if closest.as_ref().map_or(true, |best| t < best.t) {
                closest = Some(PickHit {
                    t,
                    point,
                    triangle: i,
                });
            }
        }
    }
    closest
}

fn ray_triangle(ray: &Ray, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = ray.direction.cross(edge2);
    let a = edge1.dot(h);
    if a.abs() < 1e-6 {
        return None;
    }
    let f = 1.0 / a;
    let s = ray.origin - v0;
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = f * ray.direction.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * edge2.dot(q);
    if t >= 0.0 { Some(t) } else { None }
}

/// Receiver that a CAD-aware renderer implements to keep GPU state
/// in sync with the authoring side. The kernel / feature-tree layer
/// calls these as meshes appear / change / disappear; the renderer
/// decides how to encode them into its own scene graph.
pub trait SceneSync {
    /// Upload or replace the drawable for `entity`.
    fn upload(&mut self, entity: EntityId, data: &DrawData);
    /// Drop the drawable for `entity`.
    fn remove(&mut self, entity: EntityId);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_mesh() -> Mesh {
        let mut mesh = Mesh::new();
        mesh.push_vertex(Vec3::new(-1.0, -1.0, 0.0));
        mesh.push_vertex(Vec3::new(1.0, -1.0, 0.0));
        mesh.push_vertex(Vec3::new(1.0, 1.0, 0.0));
        mesh.push_vertex(Vec3::new(-1.0, 1.0, 0.0));
        mesh.push_triangle([0, 1, 2]);
        mesh.push_triangle([0, 2, 3]);
        mesh
    }

    #[test]
    fn mesh_to_draw_data_matches_counts() {
        let mesh = square_mesh();
        let data = mesh_to_draw_data(&mesh);
        assert_eq!(data.vertices.len(), 4);
        assert_eq!(data.indices.len(), 6);
        assert_eq!(data.triangle_count(), 2);
    }

    #[test]
    fn pick_mesh_hits_plane_at_expected_t() {
        let mesh = square_mesh();
        let ray = Ray::new(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z);
        let hit = pick_mesh(&ray, &mesh).expect("hit");
        assert!((hit.t - 5.0).abs() < 1e-4);
    }

    #[test]
    fn pick_mesh_misses_outside_bounds() {
        let mesh = square_mesh();
        let ray = Ray::new(Vec3::new(10.0, 10.0, 5.0), -Vec3::Z);
        assert!(pick_mesh(&ray, &mesh).is_none());
    }

    #[test]
    fn draw_data_fills_defaults_when_attributes_missing() {
        let mut mesh = Mesh::new();
        mesh.push_vertex(Vec3::ZERO);
        mesh.push_vertex(Vec3::X);
        mesh.push_vertex(Vec3::Y);
        mesh.push_triangle([0, 1, 2]);
        // No normals / uvs set — mesh_to_draw_data should still
        // produce a fully populated vertex array.
        let data = mesh_to_draw_data(&mesh);
        assert_eq!(data.vertices.len(), 3);
    }
}
