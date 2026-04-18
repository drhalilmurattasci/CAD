use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    ReadAssets,
    WriteScene,
    SpawnProcess,
    FileSystemRead { roots: Vec<String> },
    FileSystemWrite { roots: Vec<String> },
    NetworkEgress { hosts: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub entry: String,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModManifest {
    pub name: String,
    pub version: String,
    pub root_asset: String,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAdapterManifest {
    pub name: String,
    pub version: String,
    pub endpoint: String,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("name must not be empty")]
    EmptyName,
    #[error("version must not be empty")]
    EmptyVersion,
    #[error("entry/endpoint must not be empty")]
    EmptyEntrypoint,
    #[error("capability `{0}` has an empty scope")]
    EmptyScope(&'static str),
}

pub trait CapabilityManifest {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn entrypoint(&self) -> &str;
    fn capabilities(&self) -> &[Capability];

    fn validate(&self) -> Result<(), ManifestError> {
        if self.name().trim().is_empty() {
            return Err(ManifestError::EmptyName);
        }
        if self.version().trim().is_empty() {
            return Err(ManifestError::EmptyVersion);
        }
        if self.entrypoint().trim().is_empty() {
            return Err(ManifestError::EmptyEntrypoint);
        }

        for capability in self.capabilities() {
            match capability {
                Capability::FileSystemRead { roots } | Capability::FileSystemWrite { roots }
                    if roots.is_empty() =>
                {
                    return Err(ManifestError::EmptyScope("filesystem"));
                }
                Capability::NetworkEgress { hosts } if hosts.is_empty() => {
                    return Err(ManifestError::EmptyScope("network"));
                }
                _ => {}
            }
        }

        Ok(())
    }
}

impl CapabilityManifest for PluginManifest {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn entrypoint(&self) -> &str {
        &self.entry
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

impl CapabilityManifest for ModManifest {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn entrypoint(&self) -> &str {
        &self.root_asset
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

impl CapabilityManifest for ServiceAdapterManifest {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn entrypoint(&self) -> &str {
        &self.endpoint
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[cfg(test)]
mod tests {
    use super::{Capability, CapabilityManifest, ManifestError, PluginManifest};

    #[test]
    fn manifest_validation_rejects_empty_scopes() {
        let manifest = PluginManifest {
            name: "example".into(),
            version: "0.1.0".into(),
            entry: "plugin.wasm".into(),
            capabilities: vec![Capability::FileSystemRead { roots: Vec::new() }],
        };

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::EmptyScope("filesystem"))
        );
    }

    #[test]
    fn manifest_validation_accepts_scoped_capabilities() {
        let manifest = PluginManifest {
            name: "example".into(),
            version: "0.1.0".into(),
            entry: "plugin.wasm".into(),
            capabilities: vec![
                Capability::ReadAssets,
                Capability::NetworkEgress {
                    hosts: vec!["localhost".into()],
                },
            ],
        };

        assert!(manifest.validate().is_ok());
    }
}
