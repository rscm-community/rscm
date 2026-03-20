use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct GenerationManifest {
    pub id: u64,
    pub packages: Vec<String>,
    pub created: SystemTime,
}

#[derive(Debug)]
pub struct Generation {
    pub id: u64,
    pub path: PathBuf,
    pub manifest: GenerationManifest,
}

pub struct GenerationStore {
    root: PathBuf,
}

impl GenerationStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn create<F>(
        &self,
        package_names: &[String],
        files: &[super::package::FileEntry],
        linker: F,
    ) -> Result<u64>
    where
        F: Fn(&str, &Path) -> Result<()>,
    {
        let id = self.next_id()?;
        let gen_path = self.path(id);
        let files_path = gen_path.join("files");

        fs::create_dir_all(&files_path)?;

        for file in files {
            let target = files_path.join(&file.path);
            if let Some(target_path) = &file.symlink_target {
                std::os::unix::fs::symlink(target_path, &target)?;
            } else {
                linker(&file.hash, &target)?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&target, fs::Permissions::from_mode(file.mode))?;
                }
            }
        }
        let manifest = GenerationManifest {
            id,
            packages: package_names.to_vec(),
            created: SystemTime::now(),
        };
        fs::write(
            gen_path.join("manifest.json"),
            serde_json::to_string_pretty(&manifest)?,
        )?;
        Ok(id)
    }
    pub fn path(&self, id: u64) -> PathBuf {
        self.root.join(id.to_string())
    }
    pub fn list(&self) -> Result<Vec<Generation>> {
        let mut generations = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(id_str) = path.file_name().and_then(|n| n.to_str()) {
                if let Ok(id) = id_str.parse::<u64>() {
                    let manifest_path = path.join("manifest.json");
                    if manifest_path.exists() {
                        let content = fs::read_to_string(manifest_path)?;
                        let manifest = serde_json::from_str(&content)?;

                        generations.push(Generation { id, path, manifest });
                    }
                }
            }
        }
        generations.sort_by_key(|g| g.id);
        Ok(generations)
    }
    fn next_id(&self) -> Result<u64> {
        let mut max_id = 0;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(id) = name.parse::<u64>() {
                    max_id = max_id.max(id);
                }
            }
        }
        Ok(max_id + 1)
    }
}
