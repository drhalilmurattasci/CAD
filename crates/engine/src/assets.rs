use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetKind {
    Scene,
    Prefab,
    Material,
    Texture,
    Audio,
    Script,
    PluginManifest,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetMeta {
    pub guid: Uuid,
    pub source: PathBuf,
    pub kind: AssetKind,
    pub tags: Vec<String>,
}

impl AssetMeta {
    pub fn new(source: impl Into<PathBuf>, kind: AssetKind) -> Self {
        Self {
            guid: Uuid::new_v4(),
            source: source.into(),
            kind,
            tags: Vec::new(),
        }
    }

    pub fn sidecar_path_for(source: &Path) -> PathBuf {
        let mut sidecar = source.as_os_str().to_os_string();
        sidecar.push(".meta");
        PathBuf::from(sidecar)
    }
}

#[cfg(test)]
mod tests {
    use super::{AssetKind, AssetMeta};

    #[test]
    fn asset_sidecar_uses_meta_suffix() {
        let meta = AssetMeta::new("assets/test.scene.ron", AssetKind::Scene);
        assert_eq!(
            AssetMeta::sidecar_path_for(&meta.source).to_string_lossy(),
            "assets/test.scene.ron.meta"
        );
    }
}
