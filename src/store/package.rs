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

pub struct PackageStore {
    root: PathBuf,
}

impl PackageStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    pub fn save(&self, pkg: &Package) -> Result<()> {
        let path = self.package_path(&pkg.name, &pkg.version, &pkg.release);
        let json = serde_json::to_string_pretty(pkg)?;
        fs::write(path, json)?;
        Ok(())
    }
    pub fn get(&self, name: &str) -> Result<Option<Package>> {
        let pattern = self.root.join(format!("{}*.json", name));
        let mut entries: Vec<_> = glob::glob(pattern.to_str().unwrap())?
            .filter_map(Result::ok)
            .collect();
        if entries.is_empty() {
            return Ok(None);
        }
        entries.sort();
        let latest = entries.last().unwrap();
        let content = fs::read_to_string(latest)?;
        let pkg = serde_json::from_str(&content)?;
        Ok(Some(pkg))
    }
    pub fn list(&self) -> Result<Vec<String>> {
        let mut packages = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                    packages.push(name.to_string());
                }
            }
        }
        Ok(packages)
    }
    fn package_path(&self, name: &str, version: &str, release: &str) -> PathBuf {
        self.root
            .join(format!("{}-{}-{}.json", name, version, release))
    }
}
