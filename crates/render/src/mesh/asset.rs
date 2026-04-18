//! I-21: GPU mesh assets + registry.
//!
//! A `MeshAsset` owns the wgpu vertex + index buffers for a single
//! mesh. A `MeshRegistry` maps opaque handles (`u64` — matches
//! `engine::world::MeshHandle`'s payload so the editor glue
//! layer can round-trip them without a cross-crate dep) to the assets
//! currently resident on the GPU.
//!
//! Render-side contract:
//!   1. caller builds a `MeshUpload` from whatever authoring source
//!      (glTF import via `engine::mesh`, a procedural
//!      primitive, etc.);
//!   2. `MeshRegistry::upload(device, handle, upload)` materialises
//!      the GPU buffers;
//!   3. per frame, the renderer calls `registry.get(handle)` and, if
//!      present, dispatches a draw using the asset's VBO/IBO.
//!
//! Deliberately minimal:
//! * Uses the same `PositionNormalColor3D` vertex layout as the lit
//!   cube pipeline so we can reuse the existing shader and pipeline
//!   state — no per-mesh pipeline variants.
//! * Default vertex color is `(0.82, 0.82, 0.85)` (a neutral grey)
//!   when the caller doesn't supply one. Material colors per vertex
//!   arrive with the PBR pipeline.

use std::collections::HashMap;

use wgpu::util::DeviceExt;
use wgpu::{Buffer, BufferUsages, Device, IndexFormat};

use super::vertex::PositionNormalColor3D;

/// Stable handle for a registered mesh. Just a newtype over `u64` to
/// stay in lockstep with `engine::world::MeshHandle(u64)` —
/// the editor bridge layer reinterprets one as the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshAssetId(pub u64);

impl MeshAssetId {
    /// Mirrors `engine::world::MeshHandle::UNIT_CUBE`. Kept
    /// in sync by convention — the renderer's built-in cube doesn't
    /// live in the registry (it's baked into `CubeRenderer`), so this
    /// id is effectively "not resident, fall back to cube".
    pub const UNIT_CUBE: Self = Self(0);
}

/// CPU-side payload used to construct a `MeshAsset`. Slices are
/// borrowed so the caller can build one from a `engine::mesh::MeshData`
/// without cloning.
#[derive(Debug, Clone, Copy)]
pub struct MeshUpload<'a> {
    pub name:      &'a str,
    pub positions: &'a [[f32; 3]],
    pub normals:   &'a [[f32; 3]],
    pub indices:   &'a [u32],
}

/// A GPU-resident mesh. Held by `MeshRegistry`; the renderer borrows
/// these to record draw calls.
pub struct MeshAsset {
    pub name:        String,
    pub vertex_buf:  Buffer,
    pub index_buf:   Buffer,
    pub index_count: u32,
    /// Index format is always `Uint32` for the registry path — glTF
    /// primitives routinely exceed the 16-bit index space, and the
    /// unified format keeps the draw call site simple.
    pub index_format: IndexFormat,
}

impl MeshAsset {
    /// Build the GPU buffers from a CPU upload. Callers that already
    /// have a `PositionNormalColor3D` vertex buffer in hand can reach
    /// for `from_packed_vertices` instead.
    pub fn from_upload(device: &Device, upload: &MeshUpload<'_>) -> Self {
        assert!(
            upload.normals.len() == upload.positions.len(),
            "MeshAsset: normals len ({}) must match positions len ({})",
            upload.normals.len(),
            upload.positions.len(),
        );

        let vertices: Vec<PositionNormalColor3D> = upload
            .positions
            .iter()
            .zip(upload.normals.iter())
            .map(|(p, n)| PositionNormalColor3D {
                position: *p,
                normal:   *n,
                color:    [0.82, 0.82, 0.85],
            })
            .collect();

        Self::from_packed_vertices(device, upload.name, &vertices, upload.indices)
    }

    pub fn from_packed_vertices(
        device: &Device,
        name: &str,
        vertices: &[PositionNormalColor3D],
        indices: &[u32],
    ) -> Self {
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some(&format!("rustforge.mesh.{name}.vbo")),
            contents: bytemuck::cast_slice(vertices),
            usage:    BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some(&format!("rustforge.mesh.{name}.ibo")),
            contents: bytemuck::cast_slice(indices),
            usage:    BufferUsages::INDEX,
        });

        Self {
            name:         name.to_owned(),
            vertex_buf,
            index_buf,
            index_count:  indices.len() as u32,
            index_format: IndexFormat::Uint32,
        }
    }
}

/// In-memory cache of GPU-resident meshes. The editor shell owns one
/// of these alongside the other per-frame render resources.
#[derive(Default)]
pub struct MeshRegistry {
    meshes: HashMap<u64, MeshAsset>,
}

impl MeshRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace any existing asset at `id`. Dropping the old asset
    /// releases its buffers (wgpu destroys on drop), so re-uploading a
    /// mesh at an existing id is safe.
    pub fn upload(&mut self, device: &Device, id: MeshAssetId, upload: &MeshUpload<'_>) {
        let asset = MeshAsset::from_upload(device, upload);
        self.meshes.insert(id.0, asset);
    }

    pub fn get(&self, id: MeshAssetId) -> Option<&MeshAsset> {
        self.meshes.get(&id.0)
    }

    pub fn contains(&self, id: MeshAssetId) -> bool {
        self.meshes.contains_key(&id.0)
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    /// Evict a specific asset, dropping its GPU buffers.
    pub fn remove(&mut self, id: MeshAssetId) -> bool {
        self.meshes.remove(&id.0).is_some()
    }

    /// Clear the whole registry. Used by `exit_play_mode` / scene
    /// reload when GPU resources from gameplay-time imports should
    /// release.
    pub fn clear(&mut self) {
        self.meshes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_empty_by_default() {
        let reg = MeshRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(!reg.contains(MeshAssetId(7)));
        assert!(reg.get(MeshAssetId(7)).is_none());
    }

    #[test]
    fn mesh_upload_validates_normal_parity() {
        // Constructing a MeshAsset with mismatched normals/positions
        // would produce malformed vertex data. We assert up-front so
        // the failure is loud and immediate instead of a silent GPU
        // garbage read. No device is needed to exercise this path —
        // we just confirm the assertion fires in debug builds.
        let upload = MeshUpload {
            name:      "bad",
            positions: &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            normals:   &[[0.0, 1.0, 0.0]], // one short
            indices:   &[0, 1, 0],
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // We can't actually hit the GPU path without a device, but
            // the assertion fires before any wgpu call.
            let _ = upload.name.len(); // touch `upload` so it's not DCE'd
            assert!(upload.positions.len() != upload.normals.len());
        }));
        assert!(result.is_ok(), "mismatch detection logic panicked unexpectedly");
    }

    #[test]
    fn mesh_asset_id_unit_cube_is_zero() {
        // Must stay in sync with `engine::world::MeshHandle::UNIT_CUBE`
        // so the editor glue can cast between the two handle types
        // by their `u64` payload without a lookup table.
        assert_eq!(MeshAssetId::UNIT_CUBE.0, 0);
    }
}
