use crate::lock::resolver::{LockedPackage, Resolver};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockFile {
    pub version: u32,
    pub timestamp: String,
    pub configuration_hash: String,
    pub packages: HashMap<String, PackageVersions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageVersions {
    pub source: String,
    pub versions: HashMap<String, PackageVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageVersion {
    pub version: String,
    pub release: String,
    pub hash: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl From<&LockedPackage> for PackageVersion {
    fn from(locked: &LockedPackage) -> Self {
        Self {
            version: locked.version.clone(),
            release: locked.release.clone(),
            hash: locked.hash.clone(),
            dependencies: locked.dependencies.clone(),
        }
    }
}

impl From<&PackageVersion> for LockedPackage {
    fn from(entry: &PackageVersion) -> Self {
        Self {
            name: String::new(),
            version: entry.version.clone(),
            release: entry.release.clone(),
            source: String::new(),
            hash: entry.hash.clone(),
            dependencies: entry.dependencies.clone(),
        }
    }
}

pub struct LockTracker {
    lock_path: PathBuf,
}

impl LockTracker {
    pub fn new(config_dir: &Path) -> Self {
        let lock_path = config_dir.join("rscm.lock");
        Self { lock_path }
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    pub fn exists(&self) -> bool {
        self.lock_path.exists()
    }

    pub fn load(&self) -> Result<Option<LockFile>> {
        if !self.lock_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.lock_path)?;
        let lock: LockFile = toml::from_str(&content)?;

        Ok(Some(lock))
    }

    pub fn save(&self, lock: &LockFile) -> Result<()> {
        let content = toml::to_string_pretty(lock)?;

        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&self.lock_path, content)?;

        Ok(())
    }

    pub fn update(
        &self,
        current: Option<&LockFile>,
        resolved: HashMap<String, Vec<LockedPackage>>,
        config_content: &str,
    ) -> Result<LockFile> {
        let timestamp = chrono::Utc::now().to_rfc3339();
        let config_hash = Self::compute_config_hash(config_content);

        let version = current.map(|c| c.version).unwrap_or(0) + 1;

        let mut packages: HashMap<String, PackageVersions> = HashMap::new();

        for (name, pkgs) in resolved {
            let source = pkgs.first().map(|p| p.source.clone()).unwrap_or_default();

            let mut versions = HashMap::new();
            for pkg in pkgs {
                let pkg_version = PackageVersion::from(&pkg);
                let version_key = pkg.version.clone();
                versions.insert(version_key, pkg_version);
            }

            packages.insert(name, PackageVersions { source, versions });
        }

        let lock = LockFile {
            version,
            timestamp,
            configuration_hash: config_hash,
            packages,
        };

        self.save(&lock)?;

        Ok(lock)
    }

    pub fn compute_delta(&self, old: &LockFile, new: &LockFile) -> LockDelta {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut changed = Vec::new();

        for (name, new_pkg) in &new.packages {
            if let Some(old_pkg) = old.packages.get(name) {
                let old_versions: std::collections::HashSet<_> = old_pkg.versions.keys().collect();
                let new_versions: std::collections::HashSet<_> = new_pkg.versions.keys().collect();

                if old_versions != new_versions || old_pkg.source != new_pkg.source {
                    changed.push(PackageChange {
                        name: name.clone(),
                        old_version: old_versions
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        new_version: new_versions
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    });
                }
            } else {
                added.push(name.clone());
            }
        }

        for name in old.packages.keys() {
            if !new.packages.contains_key(name) {
                removed.push(name.clone());
            }
        }

        LockDelta {
            added,
            removed,
            changed,
        }
    }

    pub fn resolve(
        &self,
        config: &crate::config::Configuration,
        config_content: &str,
        store_root: PathBuf,
        incremental: bool,
        mirrors: Option<Vec<String>>,
    ) -> Result<LockFile> {
        let current = if incremental && self.exists() {
            self.load()?
        } else {
            None
        };

        let mut resolver = Resolver::new(store_root, mirrors);
        let resolved = resolver.resolve_config(config)?;
        self.update(current.as_ref(), resolved, config_content)
    }

    fn compute_config_hash(content: &str) -> String {
        let hash = Sha256::digest(content);
        format!("sha256:{}", hex::encode(&hash))
    }
}

#[derive(Debug, Clone)]
pub struct LockDelta {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<PackageChange>,
}

#[derive(Debug, Clone)]
pub struct PackageChange {
    pub name: String,
    pub old_version: String,
    pub new_version: String,
}

impl LockDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();

        if !self.added.is_empty() {
            parts.push(format!("+{} added", self.added.len()));
        }
        if !self.removed.is_empty() {
            parts.push(format!("-{} removed", self.removed.len()));
        }
        if !self.changed.is_empty() {
            parts.push(format!("~{} changed", self.changed.len()));
        }

        if parts.is_empty() {
            "No changes".to_string()
        } else {
            parts.join(", ")
        }
    }
}
