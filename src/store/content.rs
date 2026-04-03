use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ContentStore {
    root: PathBuf,
}
impl ContentStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    pub fn add_file(&self, path: &Path) -> Result<String> {
        let hash = if path.is_symlink() {
            let target = fs::read_link(path)?;
            let hash = Sha256::digest(target.to_string_lossy().as_bytes());
            hex::encode(hash)
        } else {
            self.calculate_hash(path)?
        };
        let dest = self.content_path(&hash);
        if dest.exists() {
            return Ok(hash);
        }
        fs::create_dir_all(dest.parent().unwrap())?;
        if path.is_symlink() {
            let target = fs::read_link(path)?;
            fs::write(&dest, target.to_string_lossy().as_bytes())?;
        } else {
            fs::copy(path, &dest).with_context(|| format!("Failed to copy to {:?}", dest))?;
        }
        Ok(hash)
    }
    pub fn link_to(&self, src: &Option<String>, target: &Path) -> Result<()> {
        let src_path = match src {
            Some(path) => std::path::PathBuf::from(path),
            None => anyhow::bail!("No source path available for file"),
        };

        if !src_path.exists() {
            anyhow::bail!("Source file not found: {}", src_path.display());
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        if src_path == target {
            return Ok(());
        }

        if let Err(e) = fs::hard_link(&src_path, target) {
            if e.raw_os_error() == Some(libc::EXDEV) {
                std::os::unix::fs::symlink(&src_path, target)?;
            } else if e.raw_os_error() != Some(libc::EEXIST) {
                return Err(e.into());
            }
        }

        Ok(())
    }
    pub fn content_path(&self, hash: &str) -> PathBuf {
        self.root.join(&hash[0..2]).join(&hash[2..4]).join(hash)
    }
    fn calculate_hash(&self, path: &Path) -> Result<String> {
        let mut file = fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0; 8192];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        Ok(hex::encode(hasher.finalize()))
    }
    pub fn verify(&self, hash: &str) -> Result<bool> {
        let path = self.content_path(hash);
        if !path.exists() {
            return Ok(false);
        }
        let actual = self.calculate_hash(&path)?;
        Ok(actual == hash)
    }

    pub fn remove(&self, hash: &str) -> Result<bool> {
        let path = self.content_path(hash);
        if path.exists() {
            fs::remove_file(&path)?;
            if let Some(parent) = path.parent() {
                if parent.exists() {
                    let _ = fs::remove_dir(parent);
                    if let Some(grandparent) = parent.parent() {
                        if grandparent.exists() {
                            let _ = fs::remove_dir(grandparent);
                        }
                    }
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn list_all_hashes(&self) -> Result<Vec<String>> {
        let mut hashes = Vec::new();
        if !self.root.exists() {
            return Ok(hashes);
        }
        for entry1 in fs::read_dir(&self.root)? {
            let entry1 = entry1?;
            if !entry1.file_type()?.is_dir() {
                continue;
            }
            for entry2 in fs::read_dir(entry1.path())? {
                let entry2 = entry2?;
                if !entry2.file_type()?.is_dir() {
                    continue;
                }
                for entry3 in fs::read_dir(entry2.path())? {
                    let entry3 = entry3?;
                    if entry3.file_type()?.is_file() {
                        if let Some(name) = entry3.file_name().to_str() {
                            hashes.push(name.to_string());
                        }
                    }
                }
            }
        }
        Ok(hashes)
    }
}
