use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellEnv {
    pub id: u64,
    pub packages: Vec<String>,
    pub created: u64,
    pub path: PathBuf,
}

pub struct ShellEnvStore {
    root: PathBuf,
}

impl ShellEnvStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn next_id(&self) -> Result<u64> {
        let mut max_id: u64 = 0;
        
        if self.root.exists() {
            for entry in fs::read_dir(&self.root)? {
                let entry = entry?;
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Ok(id) = name.parse::<u64>() {
                            if id > max_id {
                                max_id = id;
                            }
                        }
                    }
                }
            }
        }
        
        Ok(max_id + 1)
    }

    pub fn create(&self, packages: &[String]) -> Result<ShellEnv> {
        let id = self.next_id()?;
        let shell_path = self.root.join(id.to_string());
        
        fs::create_dir_all(&shell_path)?;
        
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let shell_env = ShellEnv {
            id,
            packages: packages.to_vec(),
            created,
            path: shell_path,
        };

        self.save_manifest(&shell_env)?;
        
        Ok(shell_env)
    }

    fn save_manifest(&self, shell: &ShellEnv) -> Result<()> {
        let manifest_path = shell.path.join("manifest.toml");
        let content = toml::to_string_pretty(shell)?;
        fs::write(manifest_path, content)?;
        Ok(())
    }

    pub fn load(&self, id: u64) -> Result<Option<ShellEnv>> {
        let shell_path = self.root.join(id.to_string());
        
        if !shell_path.exists() {
            return Ok(None);
        }

        let manifest_path = shell_path.join("manifest.toml");
        if !manifest_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&manifest_path)?;
        let mut shell: ShellEnv = toml::from_str(&content)?;
        shell.path = shell_path;
        
        Ok(Some(shell))
    }

    pub fn list(&self) -> Result<Vec<ShellEnv>> {
        let mut shells = Vec::new();
        
        if !self.root.exists() {
            return Ok(shells);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Ok(id) = name.parse::<u64>() {
                        if let Some(shell) = self.load(id)? {
                            shells.push(shell);
                        }
                    }
                }
            }
        }

        Ok(shells)
    }

    pub fn delete(&self, id: u64) -> Result<()> {
        let shell_path = self.root.join(id.to_string());
        
        if shell_path.exists() {
            fs::remove_dir_all(&shell_path)?;
        }

        Ok(())
    }

    pub fn path(&self, id: u64) -> PathBuf {
        self.root.join(id.to_string())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
