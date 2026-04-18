//! glTF → `MeshData` importer.
//!
//! Handles both `.glb` (single binary blob with embedded buffers) and
//! `.gltf` + external `.bin` (separate-files form). The caller hands
//! us the top-level bytes and a resolver closure for external buffer
//! URIs; we return one `MeshData` per primitive.
//!
//! Intentional simplifications for this landing:
//! * Triangles only — strips / fans rejected up-front. Most DCC tools
//!   export triangles by default, and supporting strips means
//!   triangulating on import which is a separate concern.
//! * No materials, UVs, tangents, or skinning attributes yet. The lit
//!   shader only consumes position + normal today.
//! * Data URIs and embedded GLB chunks work out-of-the-box. External
//!   `.bin` sidecars need the resolver callback the caller provides.

use gltf::buffer::Source;
use gltf::mesh::util::ReadIndices;
use gltf::mesh::Mode;
use gltf::{Accessor, Gltf};

use super::{MeshData, MeshImportError};

/// Import every primitive from every mesh in a glTF document.
///
/// `resolver` is called for external URIs (non-data, non-GLB-embedded
/// buffers); it receives the URI string and must return the raw bytes
/// or `None` if the sidecar can't be found.
pub fn import_from_slice<F>(bytes: &[u8], mut resolver: F) -> Result<Vec<MeshData>, MeshImportError>
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    let Gltf { document, blob } =
        Gltf::from_slice(bytes).map_err(|e| MeshImportError::GltfParse(e.to_string()))?;

    // Resolve every buffer up front into a flat `Vec<Vec<u8>>` indexed
    // by buffer index — downstream accessor reads then become pure
    // indexing and don't need to re-check the source each time.
    let mut buffers: Vec<Vec<u8>> = Vec::with_capacity(document.buffers().count());
    for buffer in document.buffers() {
        let data = match buffer.source() {
            Source::Bin => blob
                .clone()
                .ok_or(MeshImportError::MissingBufferData { index: buffer.index() })?,
            Source::Uri(uri) => {
                if let Some(data) = try_decode_data_uri(uri) {
                    data
                } else {
                    resolver(uri)
                        .ok_or(MeshImportError::MissingBufferData { index: buffer.index() })?
                }
            }
        };
        buffers.push(data);
    }

    if document.meshes().len() == 0 {
        return Err(MeshImportError::NoMeshes);
    }

    let mut out = Vec::new();
    for mesh in document.meshes() {
        let mesh_name = mesh
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("mesh_{}", mesh.index()));
        let primitive_count = mesh.primitives().count();
        if primitive_count == 0 {
            return Err(MeshImportError::EmptyMesh { mesh: mesh_name });
        }

        for primitive in mesh.primitives() {
            if primitive.mode() != Mode::Triangles {
                return Err(MeshImportError::UnsupportedTopology {
                    topology: format!("{:?}", primitive.mode()),
                });
            }

            let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));

            let positions: Vec<[f32; 3]> = reader
                .read_positions()
                .ok_or(MeshImportError::MissingPositions)?
                .collect();

            // Prefer authored normals; fall back to flat-face if the
            // exporter dropped them.
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|iter| iter.collect())
                .unwrap_or_default();

            // Non-indexed primitives get a synthetic 0..N sequence so
            // downstream code only has one path.
            let indices: Vec<u32> = match reader.read_indices() {
                Some(ReadIndices::U8(iter)) => iter.map(u32::from).collect(),
                Some(ReadIndices::U16(iter)) => iter.map(u32::from).collect(),
                Some(ReadIndices::U32(iter)) => iter.collect(),
                None => (0..positions.len() as u32).collect(),
            };

            let name = if primitive_count == 1 {
                mesh_name.clone()
            } else {
                format!("{}#prim{}", mesh_name, primitive.index())
            };

            let mut data = MeshData {
                name,
                positions,
                normals,
                indices,
            };
            if data.normals.len() != data.positions.len() {
                data.generate_flat_normals();
            }
            out.push(data);
        }
    }

    Ok(out)
}

/// Decode inline `data:` URIs (base64 buffer payloads). Non-data URIs
/// are the caller's problem — we return `None` so the resolver runs.
fn try_decode_data_uri(uri: &str) -> Option<Vec<u8>> {
    // glTF spec only permits `data:application/octet-stream;base64,`
    // (and a couple of MIME aliases). Parse in a way that tolerates
    // any MIME prefix ending in `;base64,`.
    let prefix = "data:";
    let body = uri.strip_prefix(prefix)?;
    let comma = body.find(',')?;
    let meta = &body[..comma];
    let payload = &body[comma + 1..];
    if !meta.ends_with(";base64") {
        // We could handle percent-encoded text payloads here, but
        // every real-world glTF exporter uses base64 for binary data.
        return None;
    }
    decode_base64(payload)
}

/// Minimal base64 decoder — avoids pulling in `base64` just for the
/// one inline-URI use-case. Standard alphabet, padding optional.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut table = [255u8; 256];
    for (i, &c) in ALPHABET.iter().enumerate() {
        table[c as usize] = i as u8;
    }

    let bytes: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();

    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for b in bytes {
        let v = table[b as usize];
        if v == 255 {
            return None;
        }
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

// Expose `Accessor` for debug/logging in higher layers; not required
// for import itself but useful while iterating on format support.
#[allow(dead_code)]
fn primitive_accessor_debug(acc: Accessor) -> String {
    format!(
        "accessor#{} count={} kind={:?} dim={:?}",
        acc.index(),
        acc.count(),
        acc.data_type(),
        acc.dimensions()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal hand-rolled single-triangle glTF 2.0 in the JSON+data-URI
    /// form. We keep it tiny and inline so the test has zero filesystem
    /// dependencies.
    const TRIANGLE_GLTF: &str = r#"{
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [ { "nodes": [ 0 ] } ],
        "nodes":  [ { "mesh": 0 } ],
        "meshes": [
            {
                "name": "Triangle",
                "primitives": [
                    {
                        "attributes": { "POSITION": 0 },
                        "indices": 1,
                        "mode": 4
                    }
                ]
            }
        ],
        "buffers": [
            {
                "byteLength": 42,
                "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA"
            }
        ],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0,  "byteLength": 36, "target": 34962 },
            { "buffer": 0, "byteOffset": 36, "byteLength": 6,  "target": 34963 }
        ],
        "accessors": [
            {
                "bufferView": 0, "componentType": 5126, "count": 3,
                "type": "VEC3",
                "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]
            },
            { "bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR" }
        ]
    }"#;

    #[test]
    fn base64_decoder_matches_known_vector() {
        // "Hello" in standard base64.
        assert_eq!(decode_base64("SGVsbG8="), Some(b"Hello".to_vec()));
        assert_eq!(decode_base64("SGVsbG8"), Some(b"Hello".to_vec()));
    }

    #[test]
    fn imports_inline_triangle_primitive() {
        let meshes = import_from_slice(TRIANGLE_GLTF.as_bytes(), |_| None)
            .expect("single triangle glTF must import cleanly");
        assert_eq!(meshes.len(), 1);
        let mesh = &meshes[0];
        assert_eq!(mesh.name, "Triangle");
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        // No NORMAL attribute → importer falls back to flat face
        // normals. The triangle lies on the Z=0 plane with CCW
        // winding viewed from +Z, so the normal is (0, 0, 1).
        assert_eq!(mesh.normals.len(), 3);
        for n in &mesh.normals {
            assert!((n[2] - 1.0).abs() < 1e-5, "got {:?}", n);
        }
        assert_eq!(mesh.triangle_count(), 1);
    }

    #[test]
    fn unsupported_topology_is_rejected() {
        // Swap mode=4 (triangles) for mode=1 (lines). Everything else
        // stays valid so we isolate the topology check.
        let lines = TRIANGLE_GLTF.replace("\"mode\": 4", "\"mode\": 1");
        let err = import_from_slice(lines.as_bytes(), |_| None)
            .expect_err("line topology must be rejected for now");
        matches!(err, MeshImportError::UnsupportedTopology { .. });
    }

    #[test]
    fn missing_buffer_uri_is_surfaced() {
        // Replace the inline data: URI with a name that has no
        // resolver — importer must report MissingBufferData instead
        // of panicking on `None`.
        let external = TRIANGLE_GLTF.replace(
            "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA",
            "triangle.bin",
        );
        let err = import_from_slice(external.as_bytes(), |_| None)
            .expect_err("unresolved external buffer must error");
        matches!(err, MeshImportError::MissingBufferData { .. });
    }

    #[test]
    fn resolver_is_called_for_external_buffers() {
        // Same substitution as the previous test, but this time the
        // resolver returns the expected bytes — import must succeed.
        // The byte stream matches the TRIANGLE_GLTF base64 payload.
        let buffer = decode_base64(
            "AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA",
        )
        .unwrap();
        let external = TRIANGLE_GLTF.replace(
            "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA",
            "triangle.bin",
        );
        let meshes = import_from_slice(external.as_bytes(), |uri| {
            (uri == "triangle.bin").then(|| buffer.clone())
        })
        .expect("resolver path must succeed");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].positions.len(), 3);
    }
}
