use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use engine::assets::{AssetKind, AssetMeta};
use engine::scene::{
    ComponentData, PrefabDocument, PrimitiveValue, SceneDocument, SceneEntity, SceneId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const PROJECT_MANIFEST_FILE: &str = "rustforge-project.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub name: String,
    pub startup_scene: String,
    pub asset_roots: Vec<String>,
}

impl Default for ProjectManifest {
    fn default() -> Self {
        Self {
            name: "RustForge Sandbox".into(),
            startup_scene: "assets/scenes/sandbox.scene.ron".into(),
            asset_roots: vec!["assets".into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectWorkspace {
    pub root: PathBuf,
    pub manifest: ProjectManifest,
    pub active_scene_path: PathBuf,
    pub scene: SceneDocument,
    pub assets: Vec<AssetMeta>,
}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("failed to read `{path}`")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write `{path}`")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse project manifest")]
    Manifest(#[from] serde_json::Error),
    #[error("failed to parse scene file `{path}`")]
    SceneParse {
        path: String,
        #[source]
        source: ron::error::SpannedError,
    },
    #[error("failed to serialize scene")]
    SceneSerialize(#[from] ron::Error),
    /// I-23: distinguish prefab parse failures from scene parse
    /// failures so the editor can surface targeted diagnostics
    /// ("which .prefab.ron file is broken?") without string-matching.
    #[error("failed to parse prefab file `{path}`")]
    PrefabParse {
        path: String,
        #[source]
        source: ron::error::SpannedError,
    },
}

impl ProjectWorkspace {
    pub fn load_or_bootstrap(root: PathBuf) -> Result<Self, ProjectError> {
        bootstrap_project(&root)?;
        Self::load(root)
    }

    pub fn load(root: PathBuf) -> Result<Self, ProjectError> {
        let manifest_path = root.join(PROJECT_MANIFEST_FILE);
        let manifest_source = fs::read_to_string(&manifest_path).map_err(|source| ProjectError::Read {
            path: manifest_path.display().to_string(),
            source,
        })?;
        let manifest: ProjectManifest = serde_json::from_str(&manifest_source)?;
        let active_scene_path = root.join(&manifest.startup_scene);
        let scene_source = fs::read_to_string(&active_scene_path).map_err(|source| ProjectError::Read {
            path: active_scene_path.display().to_string(),
            source,
        })?;
        let scene = SceneDocument::from_ron_string(&scene_source).map_err(|source| {
            ProjectError::SceneParse {
                path: active_scene_path.display().to_string(),
                source,
            }
        })?;
        let assets = scan_assets(&root, &manifest.asset_roots)?;

        Ok(Self {
            root,
            manifest,
            active_scene_path,
            scene,
            assets,
        })
    }

    pub fn save_scene(&mut self, scene: &SceneDocument) -> Result<PathBuf, ProjectError> {
        let serialized = scene.to_ron_string()?;
        if let Some(parent) = self.active_scene_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ProjectError::Write {
                path: parent.display().to_string(),
                source,
            })?;
        }
        fs::write(&self.active_scene_path, serialized).map_err(|source| ProjectError::Write {
            path: self.active_scene_path.display().to_string(),
            source,
        })?;
        self.scene = scene.clone();
        self.assets = scan_assets(&self.root, &self.manifest.asset_roots)?;
        Ok(self.active_scene_path.clone())
    }

    /// I-23: resolve a project-relative prefab path and parse its RON.
    /// `relative` is whatever the content browser shows (e.g.
    /// `"assets/prefabs/player.prefab.ron"`) — we join against the
    /// project root so the same call works regardless of CWD.
    pub fn load_prefab(&self, relative: &Path) -> Result<PrefabDocument, ProjectError> {
        let absolute = self.root.join(relative);
        let source = fs::read_to_string(&absolute).map_err(|source| ProjectError::Read {
            path: absolute.display().to_string(),
            source,
        })?;
        PrefabDocument::from_ron_string(&source).map_err(|source| ProjectError::PrefabParse {
            path: absolute.display().to_string(),
            source,
        })
    }

    /// I-24: re-scan the asset tree from disk and replace
    /// `self.assets`. Used by the hot-reload watcher when it detects a
    /// filesystem change — the asset list is authoritative, so a full
    /// re-scan guarantees renames, deletions, and creations are all
    /// reflected without having to track per-event diffs.
    pub fn rescan_assets(&mut self) -> Result<(), ProjectError> {
        self.assets = scan_assets(&self.root, &self.manifest.asset_roots)?;
        Ok(())
    }

    pub fn set_active_scene_file(&mut self, file_name: &str) {
        self.active_scene_path = self.scene_root().join(file_name);
    }

    pub fn active_scene_file_name(&self) -> String {
        self.active_scene_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("untitled.scene.ron")
            .to_owned()
    }

    pub fn relative_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn scene_root(&self) -> PathBuf {
        let relative_parent = Path::new(&self.manifest.startup_scene)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("assets/scenes"));
        self.root.join(relative_parent)
    }
}

fn bootstrap_project(root: &Path) -> Result<(), ProjectError> {
    fs::create_dir_all(root.join("assets/scenes")).map_err(|source| ProjectError::Write {
        path: root.join("assets/scenes").display().to_string(),
        source,
    })?;
    fs::create_dir_all(root.join("assets/prefabs")).map_err(|source| ProjectError::Write {
        path: root.join("assets/prefabs").display().to_string(),
        source,
    })?;
    fs::create_dir_all(root.join("assets/materials")).map_err(|source| ProjectError::Write {
        path: root.join("assets/materials").display().to_string(),
        source,
    })?;
    fs::create_dir_all(root.join("assets/audio")).map_err(|source| ProjectError::Write {
        path: root.join("assets/audio").display().to_string(),
        source,
    })?;
    fs::create_dir_all(root.join("assets/meshes")).map_err(|source| ProjectError::Write {
        path: root.join("assets/meshes").display().to_string(),
        source,
    })?;
    // I-32: textures live alongside other assets. Creating the dir
    // unconditionally even when no textures have been authored yet
    // means the file tree the editor's asset browser walks looks
    // the same in every project — one less source of "why is this
    // folder missing" friction.
    fs::create_dir_all(root.join("assets/textures")).map_err(|source| ProjectError::Write {
        path: root.join("assets/textures").display().to_string(),
        source,
    })?;

    let manifest_path = root.join(PROJECT_MANIFEST_FILE);
    if !manifest_path.exists() {
        let manifest = serde_json::to_string_pretty(&ProjectManifest::default())?;
        fs::write(&manifest_path, manifest).map_err(|source| ProjectError::Write {
            path: manifest_path.display().to_string(),
            source,
        })?;
    }

    let scene_path = root.join("assets/scenes/sandbox.scene.ron");
    if !scene_path.exists() {
        let scene = default_scene().to_ron_string()?;
        fs::write(&scene_path, scene).map_err(|source| ProjectError::Write {
            path: scene_path.display().to_string(),
            source,
        })?;
    }

    let prefab_path = root.join("assets/prefabs/player.prefab.ron");
    if !prefab_path.exists() {
        // I-23: seed a spawnable prefab that carries Transform +
        // Mesh so an instantiated copy renders as a visible cube
        // the moment the user clicks "Spawn" in the content browser.
        // Authored ids (1, 2) are template-only — `SpawnPrefabCommand`
        // remaps every id at instantiation time.
        fs::write(
            &prefab_path,
            "(\n    root: (\n        id: (1),\n        name: \"Player\",\n        components: [\n            (\n                type_name: \"Transform\",\n                fields: {\n                    \"x\": F64(0.0),\n                    \"y\": F64(0.0),\n                    \"z\": F64(0.0),\n                },\n            ),\n            (\n                type_name: \"Mesh\",\n                fields: {\n                    \"primitive\": String(\"cube\"),\n                },\n            ),\n        ],\n        children: [\n            (\n                id: (2),\n                name: \"Weapon\",\n                components: [\n                    (\n                        type_name: \"Transform\",\n                        fields: {\n                            \"x\": F64(0.8),\n                        },\n                    ),\n                ],\n                children: [],\n            ),\n        ],\n    ),\n)\n",
        )
        .map_err(|source| ProjectError::Write {
            path: prefab_path.display().to_string(),
            source,
        })?;
    }

    let material_path = root.join("assets/materials/default.material.ron");
    if !material_path.exists() {
        fs::write(
            &material_path,
            "(\n    base_color: (0.85, 0.88, 0.94),\n    roughness: 0.45,\n    metallic: 0.02,\n)\n",
        )
        .map_err(|source| ProjectError::Write {
            path: material_path.display().to_string(),
            source,
        })?;
    }

    // I-29: ship a minimal real PCM WAV so the audio pipeline has
    // something to decode on first run. A 300ms 440Hz sine tone is
    // short enough to avoid being annoying on repeat and small enough
    // (~13KB) not to bloat the bootstrap. Before I-29 this path held a
    // text placeholder, which rodio would reject with a decode error
    // the moment the user clicked Play — the editor would log the
    // failure and run silent. Shipping a real file means the whole
    // scene-to-sink path round-trips immediately.
    let audio_path = root.join("assets/audio/impact.wav");
    if !audio_path.exists() || {
        // Upgrade any legacy placeholder files left over from pre-I-29
        // bootstraps so existing project roots get the real tone too.
        fs::read(&audio_path)
            .map(|bytes| bytes.starts_with(b"RUSTFORGE_AUDIO_PLACEHOLDER"))
            .unwrap_or(false)
    } {
        let wav = sine_tone_wav(440.0, 0.3, 22_050);
        fs::write(&audio_path, wav).map_err(|source| ProjectError::Write {
            path: audio_path.display().to_string(),
            source,
        })?;
    }

    // I-27: seed a tiny glTF asset so a freshly bootstrapped project
    // has something the MeshRegistry import path can chew on. Without
    // this, "open project → see imported mesh render" requires the
    // user to hand-author a .gltf file before they've even tried
    // anything. A 4-vertex / 4-face tetrahedron is the smallest
    // non-trivial closed mesh and proves the whole pipeline works:
    //   scene doc `Mesh { source }` →
    //   ECS `MeshHandle` + `MeshSource` →
    //   disk read + gltf parse →
    //   PendingMeshUpload →
    //   MeshRegistry GPU buffers →
    //   MeshInstanceRenderer draw call.
    let tetra_path = root.join("assets/meshes/tetrahedron.gltf");
    if !tetra_path.exists() {
        let gltf = tetrahedron_gltf();
        fs::write(&tetra_path, gltf).map_err(|source| ProjectError::Write {
            path: tetra_path.display().to_string(),
            source,
        })?;
    }

    // I-32: seed a checkerboard PNG so the default scene has a real
    // texture to sample through the I-32 albedo pipeline. A 64×64
    // 8×8-cell checker reads clearly against any camera distance and
    // proves the whole path:
    //   scene `Material { albedo_texture }` →
    //   ECS `TextureSource` → PNG decode →
    //   `PendingTextureUpload` → `TextureRegistry::upload` →
    //   triplanar sample in the cube shader.
    // We write it with the `image` crate (same one the bridge uses to
    // decode on load), so the bootstrap and the runtime share exactly
    // one codec. Tiny file (~200B after PNG compression), no runtime
    // dependency that isn't already in the tree.
    let checker_path = root.join("assets/textures/checker.png");
    if !checker_path.exists() {
        let png = checkerboard_png(64, 8);
        fs::write(&checker_path, png).map_err(|source| ProjectError::Write {
            path: checker_path.display().to_string(),
            source,
        })?;
    }

    Ok(())
}

/// I-32: encode a checkerboard `side×side` RGBA8 image with `cells`
/// per axis as PNG bytes. The two cells alternate between opaque
/// magenta (0xE4, 0x44, 0xB5) and near-white (0xEE, 0xEE, 0xE8) — high
/// contrast so the triplanar sampling is easy to eyeball across all
/// three cube-face orientations, and intentionally not a pure R/G/B so
/// it doesn't get confused with the gizmo handles.
fn checkerboard_png(side: u32, cells: u32) -> Vec<u8> {
    use std::io::Cursor;

    // Color A (magenta) and Color B (off-white). RGBA8.
    const A: [u8; 4] = [0xE4, 0x44, 0xB5, 0xFF];
    const B: [u8; 4] = [0xEE, 0xEE, 0xE8, 0xFF];

    let cell_px = side / cells.max(1);
    let mut img = image::RgbaImage::new(side, side);
    for y in 0..side {
        for x in 0..side {
            let cx = x / cell_px.max(1);
            let cy = y / cell_px.max(1);
            let on = ((cx + cy) & 1) == 0;
            img.put_pixel(x, y, image::Rgba(if on { A } else { B }));
        }
    }

    // PNG is the universal browser-supported format and `image`'s
    // default encoder. Writing into a Cursor<Vec<u8>> dodges the
    // dance around `Write + Seek` bounds on a raw Vec.
    let mut bytes = Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png)
        .expect("in-memory PNG encode must not fail");
    bytes.into_inner()
}

fn scan_assets(root: &Path, asset_roots: &[String]) -> Result<Vec<AssetMeta>, ProjectError> {
    let mut assets = Vec::new();

    for asset_root in asset_roots {
        let directory = root.join(asset_root);
        if directory.exists() {
            collect_assets(root, &directory, &mut assets)?;
        }
    }

    assets.sort_by(|left, right| left.source.cmp(&right.source));
    Ok(assets)
}

fn collect_assets(root: &Path, directory: &Path, assets: &mut Vec<AssetMeta>) -> Result<(), ProjectError> {
    for entry in fs::read_dir(directory).map_err(|source| ProjectError::Read {
        path: directory.display().to_string(),
        source,
    })? {
        let entry = entry.map_err(|source| ProjectError::Read {
            path: directory.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_assets(root, &path, assets)?;
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "meta") {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_path_buf();
        assets.push(AssetMeta::new(relative, infer_asset_kind(&path)));
    }

    Ok(())
}

fn infer_asset_kind(path: &Path) -> AssetKind {
    match path.extension().and_then(OsStr::to_str) {
        Some("ron") if path.to_string_lossy().contains(".scene.") => AssetKind::Scene,
        Some("ron") if path.to_string_lossy().contains(".prefab.") => AssetKind::Prefab,
        Some("ron") if path.to_string_lossy().contains(".material.") => AssetKind::Material,
        Some("wav") | Some("ogg") | Some("mp3") => AssetKind::Audio,
        Some("png") | Some("jpg") | Some("jpeg") => AssetKind::Texture,
        Some("wasm") => AssetKind::PluginManifest,
        Some("gltf") | Some("glb") => AssetKind::Unknown("mesh".into()),
        Some("txt") | Some("json") | Some("toml") => AssetKind::Script,
        Some(other) => AssetKind::Unknown(other.into()),
        None => AssetKind::Unknown("file".into()),
    }
}

/// I-29: synthesize a mono 16-bit PCM WAV containing a sine tone.
/// Written directly into the bootstrap asset folder so the audio
/// pipeline has something real to decode without the project crate
/// taking a dependency on `hound` or another wav writer.
///
/// The RIFF/WAVE header layout is stable across the spec — 12 bytes
/// of RIFF wrapper + 24 bytes of `fmt ` chunk + 8 bytes of `data`
/// header + N * 2 bytes of samples.
fn sine_tone_wav(frequency_hz: f32, duration_seconds: f32, sample_rate: u32) -> Vec<u8> {
    let sample_count = (sample_rate as f32 * duration_seconds) as u32;
    let data_bytes = sample_count as usize * 2;
    let mut out: Vec<u8> = Vec::with_capacity(44 + data_bytes);

    // RIFF header.
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32 + data_bytes as u32).to_le_bytes()); // file size - 8
    out.extend_from_slice(b"WAVE");

    // `fmt ` subchunk — 16 bytes of PCM format descriptor.
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());        // subchunk size
    out.extend_from_slice(&1u16.to_le_bytes());         // audio format = PCM
    out.extend_from_slice(&1u16.to_le_bytes());         // channels = mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2; // mono * 16-bit / 8
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());         // block align (mono*16-bit/8)
    out.extend_from_slice(&16u16.to_le_bytes());        // bits per sample

    // `data` subchunk.
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());

    // Sine samples — clamp amplitude to 0.5 so output isn't harsh.
    let tau = std::f32::consts::TAU;
    let amplitude = 0.5 * i16::MAX as f32;
    for n in 0..sample_count {
        let t = n as f32 / sample_rate as f32;
        let value = (tau * frequency_hz * t).sin() * amplitude;
        let sample = value as i16;
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// I-27: generate a minimal tetrahedron glTF 2.0 file for the
/// bootstrapped project. The buffer holds 4 positions (VEC3 f32) then
/// 12 indices (u16) back-to-back, base64-encoded into the single
/// buffer URI so the file has no `.bin` sidecar.
///
/// Geometry: a regular-ish tetrahedron centered near the origin, sized
/// so it sits comfortably next to the starter cubes (~1.2 units tall).
/// Winding is CCW viewed from outside so the lit shader's back-face
/// culling doesn't eat the visible faces.
fn tetrahedron_gltf() -> String {
    use std::io::Write;

    // Vertices — the peak is (0, 1.2, 0); the base is a slightly
    // compressed equilateral triangle in the XZ plane.
    let verts: [[f32; 3]; 4] = [
        [0.0, 1.2, 0.0],
        [-1.0, -0.5, -0.6],
        [1.0, -0.5, -0.6],
        [0.0, -0.5, 1.2],
    ];
    // CCW outward-facing triangles. The four faces together form a
    // closed solid so every vertex participates in three triangles.
    let indices: [u16; 12] = [0, 3, 1, 0, 2, 3, 0, 1, 2, 1, 3, 2];

    let mut buffer: Vec<u8> = Vec::with_capacity(48 + 24);
    for v in &verts {
        for f in v {
            buffer.write_all(&f.to_le_bytes()).unwrap();
        }
    }
    for i in &indices {
        buffer.write_all(&i.to_le_bytes()).unwrap();
    }

    // Per-axis mins/maxes for the POSITION accessor — required by the
    // glTF spec for POSITION to satisfy validators.
    let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for v in &verts {
        for axis in 0..3 {
            min[axis] = min[axis].min(v[axis]);
            max[axis] = max[axis].max(v[axis]);
        }
    }

    let base64 = encode_base64(&buffer);

    // Hand-rolled JSON: the shape is tiny and stable, so pulling in a
    // templating layer would be overkill. `{{` / `}}` escape braces in
    // the format string.
    format!(
        r#"{{
    "asset": {{ "version": "2.0", "generator": "rustforge-bootstrap" }},
    "scene": 0,
    "scenes": [ {{ "nodes": [ 0 ] }} ],
    "nodes": [ {{ "mesh": 0, "name": "Tetrahedron" }} ],
    "meshes": [
        {{
            "name": "Tetrahedron",
            "primitives": [
                {{ "attributes": {{ "POSITION": 0 }}, "indices": 1, "mode": 4 }}
            ]
        }}
    ],
    "buffers": [ {{ "byteLength": {buf_len}, "uri": "data:application/octet-stream;base64,{base64}" }} ],
    "bufferViews": [
        {{ "buffer": 0, "byteOffset": 0,  "byteLength": 48, "target": 34962 }},
        {{ "buffer": 0, "byteOffset": 48, "byteLength": 24, "target": 34963 }}
    ],
    "accessors": [
        {{
            "bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
            "min": [{min0}, {min1}, {min2}], "max": [{max0}, {max1}, {max2}]
        }},
        {{ "bufferView": 1, "componentType": 5123, "count": 12, "type": "SCALAR" }}
    ]
}}
"#,
        buf_len = buffer.len(),
        base64 = base64,
        min0 = min[0], min1 = min[1], min2 = min[2],
        max0 = max[0], max1 = max[1], max2 = max[2],
    )
}

/// Minimal standard-alphabet base64 encoder. Inline so the project
/// crate doesn't gain a `base64` dependency just for a one-shot
/// bootstrap write.
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    let mut chunks = bytes.chunks_exact(3);
    for chunk in chunks.by_ref() {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32;
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(n & 0x3F) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        0 => {}
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => unreachable!(),
    }
    out
}

fn default_scene() -> SceneDocument {
    let camera = SceneEntity::new(SceneId::new(1), "Editor Camera").with_component(
        ComponentData::new("Transform")
            .with_field("x", PrimitiveValue::F64(0.0))
            .with_field("y", PrimitiveValue::F64(2.5))
            .with_field("z", PrimitiveValue::F64(-6.0)),
    );
    // I-26 + I-28: ship the default Player with Mover + Collider +
    // RigidBody so Play mode demonstrates WASD movement *and* gravity
    // the moment a new project is opened. The player spawns a few
    // units above the ground plane so the first thing a user sees in
    // Play mode is the cube falling and settling on the floor — the
    // whole physics pipeline running end to end with zero authoring.
    // Speed is tuned to feel responsive in the starter scene (cubes
    // span ~5 units) without overshooting the camera framing.
    let player = SceneEntity::new(SceneId::new(2), "Player")
        .with_component(
            ComponentData::new("Transform")
                .with_field("x", PrimitiveValue::F64(1.0))
                .with_field("y", PrimitiveValue::F64(3.0))
                .with_field("z", PrimitiveValue::F64(0.0)),
        )
        .with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        )
        .with_component(
            ComponentData::new("Mover")
                .with_field("speed", PrimitiveValue::F64(3.0)),
        )
        .with_component(ComponentData::new("Collider"))
        .with_component(ComponentData::new("RigidBody"))
        // I-31: tint the Player so the new material pipeline is
        // immediately visible in a freshly bootstrapped project.
        // Saturated red reads clearly against the neutral gray
        // ground and doesn't collide with the R/G/B gizmo handles.
        // I-32: sample the checkerboard texture too — the albedo
        // color multiplies the sampled texel, so the Player ends
        // up with a red-tinted checker pattern that visually
        // confirms every stage of the texture pipeline.
        .with_component(
            ComponentData::new("Material")
                .with_field("color_r", PrimitiveValue::F64(0.90))
                .with_field("color_g", PrimitiveValue::F64(0.25))
                .with_field("color_b", PrimitiveValue::F64(0.20))
                .with_field("color_a", PrimitiveValue::F64(1.0))
                .with_field(
                    "albedo_texture",
                    PrimitiveValue::String("textures/checker.png".into()),
                ),
        )
        // I-29: autoplay a short tone when the user enters Play mode
        // so the audio pipeline round-trips visibly on first run. The
        // bootstrap writes `assets/audio/impact.wav` as a real PCM
        // sine tone — the Play button is the only trigger needed.
        .with_component(
            ComponentData::new("AudioSource")
                .with_field("source", PrimitiveValue::String("audio/impact.wav".into()))
                .with_field("volume", PrimitiveValue::F64(0.6))
                .with_field("autoplay", PrimitiveValue::Bool(true)),
        );
    let light = SceneEntity::new(SceneId::new(3), "Key Light").with_component(
        ComponentData::new("Light")
            .with_field("intensity", PrimitiveValue::F64(4500.0))
            .with_field("casts_shadows", PrimitiveValue::Bool(true)),
    );
    // I-25: bootstrap a primary gameplay camera. Shares the `Editor
    // Camera` framing (slightly elevated, looking toward origin) so
    // Play mode renders a recognizable view of the starter tableau
    // out of the box. `is_primary: true` wins against any future
    // authored cameras unless the user explicitly downgrades it.
    let play_camera = SceneEntity::new(SceneId::new(20), "Play Camera")
        .with_component(
            ComponentData::new("Transform")
                .with_field("x", PrimitiveValue::F64(0.0))
                .with_field("y", PrimitiveValue::F64(2.0))
                .with_field("z", PrimitiveValue::F64(-8.0)),
        )
        .with_component(
            ComponentData::new("Camera")
                .with_field("fov", PrimitiveValue::F64(60.0))
                .with_field("near", PrimitiveValue::F64(0.1))
                .with_field("far", PrimitiveValue::F64(500.0))
                .with_field("is_primary", PrimitiveValue::Bool(true)),
        );
    // I-5: seed the default project with a trio of cubes so a freshly
    // bootstrapped workspace renders something the moment the editor
    // opens it. Names starting with "Spin" pick up the I-4 Spin demo
    // component via `ViewportBridge::rebuild_world_from_scene`.
    let center = SceneEntity::new(SceneId::new(4), "SpinCube Center")
        .with_component(ComponentData::new("Transform"))
        .with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        );
    let right = SceneEntity::new(SceneId::new(5), "SpinCube Right")
        .with_component(
            ComponentData::new("Transform")
                .with_field("x", PrimitiveValue::F64(2.5))
                .with_field("scale", PrimitiveValue::F64(0.6)),
        )
        .with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        )
        // I-31: warm amber so the three starter cubes read as
        // distinct pieces rather than a monochrome cluster.
        .with_component(
            ComponentData::new("Material")
                .with_field("color_r", PrimitiveValue::F64(0.95))
                .with_field("color_g", PrimitiveValue::F64(0.75))
                .with_field("color_b", PrimitiveValue::F64(0.20)),
        );
    let left_block = SceneEntity::new(SceneId::new(6), "Static Block Left")
        .with_component(
            ComponentData::new("Transform")
                .with_field("x", PrimitiveValue::F64(-2.5))
                .with_field("scale", PrimitiveValue::F64(0.8)),
        )
        .with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        )
        // I-31: cool teal — the static block now reads as "not the
        // thing you can control" at a glance. No physics meaning;
        // it's purely a visual affordance the color system enables.
        .with_component(
            ComponentData::new("Material")
                .with_field("color_r", PrimitiveValue::F64(0.15))
                .with_field("color_g", PrimitiveValue::F64(0.60))
                .with_field("color_b", PrimitiveValue::F64(0.70)),
        );
    // I-28: a wide-and-thin static Collider anchored below the starter
    // tableau. No RigidBody → treated as immovable ground by the
    // physics integrator. A Mesh cube scaled to match gives designers
    // a visible reference for where the collision volume actually
    // sits (invisible colliders are a notorious source of "why isn't
    // my character falling?" confusion).
    let ground = SceneEntity::new(SceneId::new(8), "Ground")
        .with_component(
            ComponentData::new("Transform")
                .with_field("y", PrimitiveValue::F64(-1.0))
                .with_field("scale", PrimitiveValue::F64(1.0)),
        )
        .with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        )
        .with_component(
            ComponentData::new("Collider")
                .with_field("x", PrimitiveValue::F64(10.0))
                .with_field("y", PrimitiveValue::F64(0.5))
                .with_field("z", PrimitiveValue::F64(10.0)),
        );

    // I-27: drop the bootstrap glTF asset into the scene at a visible
    // offset. The bootstrap writes `assets/meshes/tetrahedron.gltf`
    // alongside this file so the editor can demonstrate the full
    // import-to-draw pipeline immediately on first open.
    let tetra = SceneEntity::new(SceneId::new(7), "Imported Tetrahedron")
        .with_component(
            ComponentData::new("Transform")
                .with_field("x", PrimitiveValue::F64(0.0))
                .with_field("y", PrimitiveValue::F64(0.0))
                .with_field("z", PrimitiveValue::F64(2.5)),
        )
        .with_component(
            ComponentData::new("Mesh")
                .with_field("source", PrimitiveValue::String("meshes/tetrahedron.gltf".into())),
        );

    SceneDocument::new("Sandbox")
        .with_root(camera)
        .with_root(player)
        .with_root(light)
        .with_root(play_camera)
        .with_root(center)
        .with_root(right)
        .with_root(left_block)
        .with_root(ground)
        .with_root(tetra)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::ProjectWorkspace;

    fn unique_temp_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustforge_project_test_{nanos}"))
    }

    #[test]
    fn bootstrap_project_creates_manifest_scene_and_assets() {
        let root = unique_temp_dir();
        let project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        assert_eq!(project.manifest.name, "RustForge Sandbox");
        assert_eq!(project.scene.name, "Sandbox");
        assert!(project.assets.len() >= 4);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_bootstrapped_prefab_into_document() {
        // I-23: the bootstrap path writes a player.prefab.ron with
        // Transform + Mesh; `load_prefab` must parse it back into a
        // PrefabDocument the command layer can instantiate.
        let root = unique_temp_dir();
        let project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let prefab = project
            .load_prefab(std::path::Path::new("assets/prefabs/player.prefab.ron"))
            .expect("bootstrapped prefab must parse");
        assert_eq!(prefab.root.name, "Player");
        // Transform + Mesh authored components survive the round-trip.
        let component_types: Vec<&str> = prefab
            .root
            .components
            .iter()
            .map(|c| c.type_name.as_str())
            .collect();
        assert!(component_types.contains(&"Transform"));
        assert!(component_types.contains(&"Mesh"));
        assert_eq!(prefab.root.children.len(), 1);
        assert_eq!(prefab.root.children[0].name, "Weapon");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bootstrapped_tetrahedron_gltf_parses_through_core_importer() {
        // I-27: the bootstrap-seeded tetrahedron.gltf must round-trip
        // cleanly through the core glTF importer with no sidecar files
        // (it ships as a single JSON with a data-URI buffer). Four
        // vertices, twelve indices — no more, no less.
        use engine::mesh::gltf;

        let root = unique_temp_dir();
        let _project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let gltf_path = root.join("assets/meshes/tetrahedron.gltf");
        assert!(gltf_path.exists(), "bootstrap should write tetrahedron.gltf");

        let bytes = std::fs::read(&gltf_path).unwrap();
        let meshes = gltf::import_from_slice(&bytes, |_| None)
            .expect("tetrahedron gltf must parse without sidecar files");

        assert_eq!(meshes.len(), 1, "one primitive");
        let mesh = &meshes[0];
        assert_eq!(mesh.positions.len(), 4, "4 tetrahedron vertices");
        assert_eq!(mesh.indices.len(), 12, "4 triangles × 3 indices");
        // Flat-face normals are synthesized by the importer when the
        // file has no NORMAL accessor — parity must hold.
        assert_eq!(mesh.normals.len(), mesh.positions.len());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_scene_references_tetrahedron_source_asset() {
        // I-27: the default scene must include a Mesh { source } entry
        // pointing at the bootstrap glTF so "open project → see
        // imported mesh" works zero-click. We walk the scene doc
        // rather than the ECS because the scene-doc layer is what
        // persists to disk.
        let root = unique_temp_dir();
        let project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let found = project
            .scene
            .root_entities
            .iter()
            .flat_map(|e| e.components.iter())
            .any(|c| {
                c.type_name == "Mesh"
                    && matches!(
                        c.fields.get("source"),
                        Some(engine::scene::PrimitiveValue::String(s))
                            if s == "meshes/tetrahedron.gltf"
                    )
            });
        assert!(
            found,
            "default scene must reference meshes/tetrahedron.gltf through a Mesh{{ source }} field",
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bootstrap_writes_real_wav_that_decodes_through_rodio() {
        // I-29: the bootstrap-seeded impact.wav must be a valid PCM
        // WAV rodio's decoder accepts. Pre-I-29 this path wrote a
        // text placeholder that would fail at Play time; the upgrade
        // path also rewrites that placeholder into a real tone.
        use rodio::Decoder;
        use std::io::Cursor;

        let root = unique_temp_dir();
        let _project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let wav_path = root.join("assets/audio/impact.wav");
        let bytes = std::fs::read(&wav_path).unwrap();
        assert!(bytes.starts_with(b"RIFF"), "WAV header present");
        // Decode check — any rodio supported format returns Ok here.
        Decoder::new(Cursor::new(bytes)).expect("bootstrap WAV must decode");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_scene_player_carries_autoplay_audio_source() {
        // The starter scene must drive the audio pipeline on entry to
        // Play mode out of the box — no authoring required. Walking
        // the scene doc (rather than the runtime ECS) because this
        // is what persists to disk.
        let root = unique_temp_dir();
        let project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let has_autoplay = project
            .scene
            .root_entities
            .iter()
            .flat_map(|e| e.components.iter())
            .any(|c| {
                c.type_name == "AudioSource"
                    && matches!(
                        c.fields.get("autoplay"),
                        Some(engine::scene::PrimitiveValue::Bool(true))
                    )
            });
        assert!(has_autoplay, "default scene must contain an autoplay AudioSource");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_scene_player_carries_material_tint() {
        // The starter scene must ship a Material on the Player so the
        // I-31 pipeline has something to show out of the box.
        // Walking the scene doc (not the runtime World) because this
        // is the authored state that persists to disk — if it round-
        // trips through RON without a Material, opening the project
        // in a future editor build would lose the visual cue.
        let root = unique_temp_dir();
        let project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let player_has_material = project
            .scene
            .root_entities
            .iter()
            .find(|e| e.name == "Player")
            .map(|e| e.components.iter().any(|c| c.type_name == "Material"))
            .unwrap_or(false);
        assert!(
            player_has_material,
            "default scene Player must ship with a Material component"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bootstrap_writes_real_png_that_decodes_through_image_crate() {
        // I-32: the seeded `assets/textures/checker.png` must be a
        // valid PNG — the bridge's `load_texture_from_disk` path
        // rejects anything the `image` crate can't decode, and the
        // renderer would silently fall back to white. Verifying the
        // file header + round-tripping through the decoder catches
        // any encoder regression at its earliest point.
        let root = unique_temp_dir();
        let _project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let png_path = root.join("assets/textures/checker.png");
        assert!(png_path.exists(), "bootstrap should write checker.png");
        let bytes = std::fs::read(&png_path).expect("read checker.png");
        // PNG signature is the canonical 8-byte header.
        assert!(
            bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
            "bootstrap PNG must start with the PNG signature"
        );
        // Decode end-to-end and check dimensions match our 64×64
        // checker.
        let decoded = image::load_from_memory(&bytes).expect("image crate decodes bootstrap PNG");
        let rgba = decoded.to_rgba8();
        assert_eq!(rgba.dimensions(), (64, 64));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_scene_player_carries_albedo_texture_reference() {
        // I-32: the starter Material must also reference the bootstrap
        // checkerboard so users see a real textured material on first
        // launch. Guards against someone swapping the checker for a
        // different path and forgetting to regenerate the PNG.
        let root = unique_temp_dir();
        let project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();

        let has_texture_field = project
            .scene
            .root_entities
            .iter()
            .find(|e| e.name == "Player")
            .and_then(|e| e.components.iter().find(|c| c.type_name == "Material"))
            .and_then(|c| c.fields.get("albedo_texture"))
            .map(|v| matches!(
                v,
                engine::scene::PrimitiveValue::String(s)
                    if s == "textures/checker.png"
            ))
            .unwrap_or(false);
        assert!(
            has_texture_field,
            "default Player Material must reference textures/checker.png"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saving_scene_persists_ron_to_disk() {
        let root = unique_temp_dir();
        let mut project = ProjectWorkspace::load_or_bootstrap(root.clone()).unwrap();
        project.scene.name = "SandboxSaved".into();

        let saved_path = project.save_scene(&project.scene.clone()).unwrap();
        let source = std::fs::read_to_string(saved_path).unwrap();

        assert!(source.contains("SandboxSaved"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
