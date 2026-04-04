pub mod resolver;
pub mod tracker;

pub use resolver::{LockedPackage, Resolver};
pub use tracker::{
    LockDelta, LockFile, LockTracker, PackageChange, PackageVersion, PackageVersions,
};

use crate::lua::LuaEngine;
use anyhow::{anyhow, Result};
use std::path::PathBuf;

pub struct LockManager {
    config_path: PathBuf,
    store_root: PathBuf,
}

impl LockManager {
    pub fn new(config_path: PathBuf, store_root: PathBuf) -> Self {
        Self {
            config_path,
            store_root,
        }
    }

    pub fn lock(&self, incremental: bool, force: bool) -> Result<LockDelta> {
        let config_dir = self
            .config_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid config path"))?;

        let config_content = std::fs::read_to_string(&self.config_path)?;
        let engine = LuaEngine::new()?;
        let config = engine.load_config(&config_content, &self.config_path)?;

        let tracker = LockTracker::new(config_dir);

        if !incremental && !force && tracker.exists() {
            return Err(anyhow!(
                "Lock file already exists. Use --update/-u to update incrementally or --force/-f to regenerate."
            ));
        }

        let old_lock = tracker.load()?;
        let mirrors = config
            .system
            .as_ref()
            .and_then(|s| s.pacman_mirrors.clone());
        let lock_file = tracker.resolve(
            &config,
            &config_content,
            self.store_root.clone(),
            incremental,
            mirrors,
        )?;

        if let Some(old) = old_lock {
            let delta = tracker.compute_delta(&old, &lock_file);
            print_lock_summary(&delta);
            Ok(delta)
        } else {
            println!(
                "✓ Created lock file with {} packages",
                lock_file.packages.len()
            );
            Ok(LockDelta {
                added: lock_file.packages.keys().cloned().collect(),
                removed: vec![],
                changed: vec![],
            })
        }
    }

    pub fn show(&self) -> Result<()> {
        let config_dir = self
            .config_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid config path"))?;

        let tracker = LockTracker::new(config_dir);

        match tracker.load()? {
            Some(lock) => {
                println!("Version: {}", lock.version);
                println!("Timestamp: {}", lock.timestamp);
                println!("Config hash: {}", lock.configuration_hash);
                println!("\nPackages ({}):", lock.packages.len());

                let mut names: Vec<_> = lock.packages.keys().collect();
                names.sort();

                for name in names {
                    if let Some(pkg_versions) = lock.packages.get(name) {
                        let versions: Vec<String> = pkg_versions.versions.keys().cloned().collect();
                        let versions_str = versions.join(", ");
                        let hash = pkg_versions
                            .versions
                            .values()
                            .next()
                            .map(|v| &v.hash[..std::cmp::min(16, v.hash.len())])
                            .unwrap_or("N/A");
                        println!(
                            "  {} = {} ({}: {})",
                            name, versions_str, pkg_versions.source, hash
                        );
                    }
                }
            }
            None => {
                println!("No lock file found. Run 'rscm lock' to create one.");
            }
        }

        Ok(())
    }

    pub fn diff(&self) -> Result<()> {
        let config_dir = self
            .config_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid config path"))?;

        let config_content = std::fs::read_to_string(&self.config_path)?;
        let engine = LuaEngine::new()?;
        let config = engine.load_config(&config_content, &self.config_path)?;

        let tracker = LockTracker::new(config_dir);
        let current = tracker
            .load()?
            .ok_or_else(|| anyhow!("No lock file found"))?;

        let mirrors = config
            .system
            .as_ref()
            .and_then(|s| s.pacman_mirrors.clone());
        let new_lock = tracker.resolve(
            &config,
            &config_content,
            self.store_root.clone(),
            true,
            mirrors,
        )?;
        let delta = tracker.compute_delta(&current, &new_lock);

        if delta.is_empty() {
            println!("No changes detected.");
        } else {
            print_lock_summary(&delta);
        }

        Ok(())
    }
}

fn print_lock_summary(delta: &LockDelta) {
    println!("{}", delta.summary());

    if !delta.added.is_empty() {
        println!("\nAdded:");
        for name in &delta.added {
            println!("  + {}", name);
        }
    }

    if !delta.removed.is_empty() {
        println!("\nRemoved:");
        for name in &delta.removed {
            println!("  - {}", name);
        }
    }

    if !delta.changed.is_empty() {
        println!("\nChanged:");
        for change in &delta.changed {
            println!(
                "  ~ {} ({} -> {})",
                change.name, change.old_version, change.new_version
            );
        }
    }
}
