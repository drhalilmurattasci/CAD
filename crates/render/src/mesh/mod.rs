//! Vertex types and primitive mesh data.
//!
//! As the engine grows this module will hold:
//! - Vertex struct definitions (one per layout).
//! - Built-in primitive meshes (triangle, quad, cube, sphere).
//! - Mesh loading utilities (glTF import lands in I-15).
//!
//! For now (I-2.1) only the 2D position-color vertex + its canonical
//! triangle fixture are here.

pub mod asset;
pub mod cube;
pub mod primitives;
pub mod vertex;

pub use asset::{MeshAsset, MeshAssetId, MeshRegistry, MeshUpload};
pub use cube::{CUBE_INDICES, CUBE_LIT_VERTICES, CUBE_VERTICES};
pub use primitives::TRIANGLE_2D;
pub use vertex::{PositionColor2D, PositionColor3D, PositionNormalColor3D};
