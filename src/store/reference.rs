use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ReferenceCounter {
    counts: HashMap<String, usize>,
    path: PathBuf,
}

impl ReferenceCounter {
    pub fn new(path: PathBuf) -> Result<Self> {
        let counts = if path.exists() {
            let content = fs::read_to_string(&path)?;
            serde_json::from_str(&content)?
        } else {
            HashMap::new()
        };

        Ok(Self { counts, path })
    }

    pub fn add(&mut self, hash: &str) -> Result<()> {
        *self.counts.entry(hash.to_string()).or_insert(0) += 1;
        self.save()?;
        Ok(())
    }

    pub fn remove(&mut self, hash: &str) -> Result<usize> {
        if let Some(count) = self.counts.get_mut(hash) {
            *count -= 1;
            let new_count = *count;

            if *count == 0 {
                self.counts.remove(hash);
            }

            self.save()?;
            Ok(new_count)
        } else {
            Ok(0)
        }
    }
    pub fn get_unreferenced(&self) -> Vec<String> {
        self.counts
            .iter()
            .filter(|&(_, &count)| count == 0)
            .map(|(hash, _)| hash.clone())
            .collect()
    }
    fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.counts)?;
        fs::write(&self.path, json)?;
        Ok(())
    }
}
