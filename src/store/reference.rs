use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RefKind {
    Content,
    Package,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefEntry {
    pub count: usize,
    pub kind: RefKind,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ReferenceCounter {
    refs: HashMap<String, RefEntry>,
    path: PathBuf,
}

impl ReferenceCounter {
    pub fn new(path: PathBuf) -> Result<Self> {
        let refs = if path.exists() {
            let content = fs::read_to_string(&path)?;
            serde_json::from_str(&content)?
        } else {
            HashMap::new()
        };

        Ok(Self { refs, path })
    }

    pub fn add(&mut self, hash: &str, kind: RefKind) -> Result<()> {
        let entry = self
            .refs
            .entry(hash.to_string())
            .or_insert(RefEntry { count: 0, kind });
        entry.count += 1;
        self.save()?;
        Ok(())
    }

    pub fn remove(&mut self, hash: &str) -> Result<()> {
        if let Some(entry) = self.refs.get_mut(hash) {
            if entry.count > 0 {
                entry.count -= 1;
            }
            if entry.count == 0 {
                self.refs.remove(hash);
            }
            self.save()?;
        }
        Ok(())
    }

    pub fn get_count(&self, hash: &str) -> usize {
        self.refs.get(hash).map(|e| e.count).unwrap_or(0)
    }

    pub fn get_unreferenced(&self, kind: Option<RefKind>) -> Vec<String> {
        self.refs
            .iter()
            .filter(|(_, entry)| match &kind {
                Some(k) => entry.kind == *k,
                None => true,
            })
            .filter(|(_, entry)| entry.count == 0)
            .map(|(hash, _)| hash.clone())
            .collect()
    }

    pub fn get_all_hashes(&self, kind: Option<RefKind>) -> Vec<String> {
        self.refs
            .iter()
            .filter(|(_, entry)| match &kind {
                Some(k) => entry.kind == *k,
                None => true,
            })
            .map(|(hash, _)| hash.clone())
            .collect()
    }

    pub fn remove_entry(&mut self, hash: &str) {
        self.refs.remove(hash);
        let _ = self.save();
    }

    fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.refs)?;
        fs::write(&self.path, json)?;
        Ok(())
    }
}
