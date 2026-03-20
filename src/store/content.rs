use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct ContentStore {
    root: PathBuf,
}
impl ContentStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    pub fn add_file(&self, path: &Path) -> Result<String> {
        let hash = self.calculate_hash(path)?;
        let dest = self.content_path(&hash);
        if dest.exists() {
            return Ok(hash);
        }
        fs::create_dir_all(dest.parent().unwrap())?;
        fs::copy(path, &dest).with_context(|| format!("Failed to copy to {:?}", dest))?;
        Ok(hash)
    }
    pub fn link_to(&self, hash: &str, target: &Path) -> Result<()> {
        let src = self.content_path(hash);
        if !src.exists() {
            anyhow::bail!("Content not found: {}", hash);
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Err(e) = fs::hard_link(&src, target) {
            if e.raw_os_error() == Some(libc::EXDEV) {
                fs::copy(&src, target)?;
            } else {
                return Err(e.into());
            }
        }

        Ok(())
    }
    fn content_path(&self, hash: &str) -> PathBuf {
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
}
