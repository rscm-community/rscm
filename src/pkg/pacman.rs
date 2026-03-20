use super::{InstalledPackage, PackageInfo, PackageManager};
use crate::store::package::FileEntry;
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Pacman {
    root_dir: PathBuf,
    db_path: PathBuf,
    cache_dir: PathBuf,
}

impl Pacman {
    pub fn new(root_dir: PathBuf, db_path: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            root_dir,
            db_path,
            cache_dir,
        }
    }
    pub fn scan_installed_files(&self, package: &str) -> Result<Vec<FileEntry>> {
        Ok(vec![])
    }

    pub fn package_info(&self, name: &str) -> Result<PackageInfo> {
        todo!()
    }
}
impl PackageManager for Pacman {
    fn install(&self, packages: &[String]) -> Result<Vec<InstalledPackage>> {
        let mut cmd = if unsafe { libc::geteuid() == 0 } {
            std::process::Command::new("pacman")
        } else {
            let mut c = std::process::Command::new("sudo");
            c.arg("pacman");
            c
        };
        cmd.arg("-r")
            .arg(self.root_dir.to_str().unwrap())
            .arg("-S")
            .arg("--noconfirm")
            .args(packages);
        Ok(vec![])
    }

    fn remove(&self, packages: &[String]) -> Result<()> {
        todo!()
    }

    fn list_installed(&self) -> Result<Vec<InstalledPackage>> {
        todo!()
    }

    fn exists(&self, name: &str) -> Result<bool> {
        todo!()
    }
}
