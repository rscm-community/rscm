pub mod pacman;
pub mod aur;

use std::path::PathBuf;
use std::time::SystemTime;
use pacman::Pacman;
use aur::AurHelper;
use anyhow::Result;
use crate::store::Package;
use crate::store::package::FileEntry;

#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub release: String,
    pub install_time: SystemTime,
    pub description: Option<String>,
    pub dependencies: Vec<String>,
    pub install_root: PathBuf,
    pub files: Vec<String>,
    pub manager: PackageManagerType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackageManagerType {
    Pacman,
    Yay,
    Paru,
    Other(String),
}

impl InstalledPackage {
    pub fn full_name(&self) -> String {
        format!("{}-{}-{}", self.name, self.version, self.release)
    }
    pub fn to_store_package(&self, file_entries: Vec<FileEntry>) -> Package {
        Package {
            name: self.name.clone(),
            version: self.version.clone(),
            release: self.release.clone(),
            files: file_entries,
            dependencies: self.dependencies.clone(),
            install_time: self.install_time,
        }
    }
}
#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub release: String,
    pub description: Option<String>,
    pub dependencies: Vec<String>,
    pub optional_deps: Vec<String>,
    pub size: u64,
    pub installed: bool,
    pub manager: PackageManagerType,
    pub build_date: Option<SystemTime>,
    pub source: PackageSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackageSource {
    Repository(String),
    Aur,
    Local,
    Other(String),
}

impl PackageInfo {
    pub fn full_name(&self) -> String {
        format!("{}-{}-{}", self.name, self.version, self.release)
    }
    pub fn is_aur(&self) -> bool {
        matches!(self.source, PackageSource::Aur) ||
            matches!(self.manager, PackageManagerType::Yay | PackageManagerType::Paru)
    }
}
pub trait PackageManager {
    fn install(&self, packages: &[String]) -> Result<Vec<InstalledPackage>>;
    fn remove(&self, packages: &[String]) -> Result<()>;
    fn list_installed(&self) -> Result<Vec<InstalledPackage>>;
    fn exists(&self, name: &str) -> Result<bool>;
}

pub struct PackageManagerFactory {
    pacman: Pacman,
    aur_helper: Option<AurHelper>,
}

impl PackageManagerFactory {
    pub fn for_package(&self, name: &str) -> Result<Box<dyn PackageManager>> {
        if let Some(aur_helper) = &self.aur_helper {
            if aur_helper.is_aur_package(name)? {
                return Ok(Box::new(aur_helper.clone()));
            }
        }
        Ok(Box::new(self.pacman.clone()))
    }
}