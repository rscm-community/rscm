use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub mode: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub release: String,
    pub files: Vec<FileEntry>,
    pub dependencies: Vec<String>,
    pub install_time: SystemTime,
}

#[derive(Debug, Clone)]
pub struct PackageStore {
    root: PathBuf,
}

impl PackageStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn save(&self, pkg: &Package) -> Result<()> {
        let dir_name = format!("{}-{}-{}", pkg.name, pkg.version, pkg.release);
        let dir_path = self.root.join(&dir_name);
        fs::create_dir_all(&dir_path)?;

        let manifest_path = dir_path.join("manifest.toml");
        let content = toml::to_string_pretty(pkg)?;
        fs::write(manifest_path, content)?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Option<Package>> {
        self.get_version(name, None)
    }

    pub fn get_version(&self, name: &str, version: Option<&str>) -> Result<Option<Package>> {
        let mut matching_dirs: Vec<PathBuf> = Vec::new();

        if !self.root.exists() {
            return Ok(None);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if !dir_name.starts_with(&format!("{}-", name)) {
                continue;
            }

            if let Some(ver) = version {
                let expected_prefix = format!("{}-{}-", name, ver);
                if !dir_name.starts_with(&expected_prefix) {
                    continue;
                }
            }

            matching_dirs.push(path);
        }

        if matching_dirs.is_empty() {
            return Ok(None);
        }

        matching_dirs.sort();

        for dir in matching_dirs.iter().rev() {
            let manifest_path = dir.join("manifest.toml");
            if manifest_path.exists() {
                let content = fs::read_to_string(&manifest_path)?;
                let pkg: Package = toml::from_str(&content)?;
                return Ok(Some(pkg));
            }
        }

        Ok(None)
    }

    pub fn list(&self) -> Result<Vec<String>> {
        let mut packages = Vec::new();
        if !self.root.exists() {
            return Ok(packages);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if let Some(name) = dir_name.split('-').next() {
                if !packages.contains(&name.to_string()) {
                    packages.push(name.to_string());
                }
            }
        }
        Ok(packages)
    }

    pub fn list_all(&self) -> Result<Vec<Package>> {
        let mut packages = Vec::new();
        if !self.root.exists() {
            return Ok(packages);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("manifest.toml");
            if manifest_path.exists() {
                if let Ok(content) = fs::read_to_string(&manifest_path) {
                    if let Ok(pkg) = toml::from_str::<Package>(&content) {
                        packages.push(pkg);
                    }
                }
            }
        }
        Ok(packages)
    }

    pub fn list_versions(&self, name: &str) -> Result<Vec<Package>> {
        let mut packages = Vec::new();
        if !self.root.exists() {
            return Ok(packages);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if !dir_name.starts_with(&format!("{}-", name)) {
                continue;
            }

            let manifest_path = path.join("manifest.toml");
            if manifest_path.exists() {
                if let Ok(content) = fs::read_to_string(&manifest_path) {
                    if let Ok(pkg) = toml::from_str::<Package>(&content) {
                        packages.push(pkg);
                    }
                }
            }
        }
        Ok(packages)
    }

    pub fn contains(&self, name: &str) -> Result<bool> {
        Ok(self.get(name)?.is_some())
    }

    pub fn remove(&self, name: &str, version: Option<&str>) -> Result<bool> {
        let mut removed = false;

        if !self.root.exists() {
            return Ok(false);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if !dir_name.starts_with(&format!("{}-", name)) {
                continue;
            }

            if let Some(ver) = version {
                let expected_prefix = format!("{}-{}-", name, ver);
                if !dir_name.starts_with(&expected_prefix) {
                    continue;
                }
            }

            fs::remove_dir_all(&path)?;
            removed = true;
        }

        Ok(removed)
    }

    pub fn get_all_package_names(&self) -> Result<Vec<String>> {
        let mut packages = Vec::new();
        if !self.root.exists() {
            return Ok(packages);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if let Some(name) = dir_name.split('-').next() {
                if !packages.contains(&name.to_string()) {
                    packages.push(name.to_string());
                }
            }
        }
        Ok(packages)
    }
}
