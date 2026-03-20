pub mod aur;
pub mod lock;
pub mod pacman;
pub mod privilege;

use crate::store::package::FileEntry;
use crate::store::Package;
use anyhow::{anyhow, Result};
use aur::AurHelper;
use pacman::Pacman;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorePath {
    pub package_name: String,
    pub package_version: String,
    pub package_release: String,
    pub build_hash: String,
    pub root: PathBuf,
}

impl StorePath {
    pub fn new(name: &str, version: &str, release: &str, build_hash: &str) -> Self {
        Self {
            package_name: name.to_string(),
            package_version: version.to_string(),
            package_release: release.to_string(),
            build_hash: build_hash.to_string(),
            root: PathBuf::from(format!(
                "/rscm/store/packages/{}-{}-{}/",
                name, version, build_hash
            )),
        }
    }

    pub fn from_package(pkg: &Package, build_hash: &str) -> Self {
        Self::new(&pkg.name, &pkg.version, &pkg.release, build_hash)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.toml")
    }

    pub fn content_dir(&self) -> PathBuf {
        self.root.join("content")
    }
}

impl fmt::Display for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}-{}",
            self.package_name,
            self.package_version,
            self.package_release,
            &self.build_hash[..8]
        )
    }
}

#[derive(Debug, Clone)]
pub struct RemoveResult {
    pub package_name: String,
    pub removed_versions: Vec<String>,
    pub files_removed: usize,
    pub space_freed: u64,
    pub recursive: bool,
    pub removed_dependents: Vec<String>,
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PackageSource {
    Repository(String),
    Aur,
    Local,
    Other(String),
}

impl PackageSource {
    pub fn as_str(&self) -> &str {
        match self {
            PackageSource::Repository(name) => name.as_str(),
            PackageSource::Aur => "aur",
            PackageSource::Local => "local",
            PackageSource::Other(s) => s.as_str(),
        }
    }
}

impl PackageInfo {
    pub fn full_name(&self) -> String {
        format!("{}-{}-{}", self.name, self.version, self.release)
    }

    pub fn is_aur(&self) -> bool {
        matches!(self.source, PackageSource::Aur)
            || matches!(
                self.manager,
                PackageManagerType::Yay | PackageManagerType::Paru
            )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageConfig {
    pub name: String,
    pub version: Option<String>,
    pub build_type: BuildType,
    pub dependencies: Vec<String>,
    pub sandbox_config: Option<SandboxConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildType {
    Pacman,
    Aur,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub network: bool,
    pub ro_paths: Vec<String>,
    pub rw_paths: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            network: false,
            ro_paths: vec![],
            rw_paths: vec![],
        }
    }
}

pub trait PackageManager: Send + Sync {
    fn is_available_in_store(&self, package: &PackageConfig) -> bool;

    fn ensure_in_store(&self, package: &PackageConfig) -> Result<PackageInfo>;

    fn install(&self, package: &PackageConfig, force: bool) -> Result<PackageInfo>;

    fn remove(
        &self,
        package_name: &str,
        version: Option<&str>,
        recursive: bool,
    ) -> Result<RemoveResult>;

    fn query_package_info(&self, name: &str) -> Result<Option<PackageInfo>>;

    fn list_installed(&self) -> Result<Vec<InstalledPackage>>;

    fn build_type(&self) -> BuildType;

    fn manager_name(&self) -> &'static str;
}

pub struct PackageManagerFactory {
    pacman: Pacman,
    aur_helper: Option<AurHelper>,
}

impl PackageManagerFactory {
    pub fn new(store_root: PathBuf) -> Self {
        let pacman = Pacman::new(store_root.clone());
        let aur_helper = AurHelper::detect(store_root.clone()).map(|helper| {
            AurHelper::new(
                Pacman::new(store_root.clone()),
                helper.helper_type(),
                helper.build_dir().clone(),
                helper.pkg_dest().clone(),
                store_root,
            )
        });

        Self { pacman, aur_helper }
    }

    pub fn for_package(&self, config: &PackageConfig) -> Result<&dyn PackageManager> {
        let is_aur = match config.build_type {
            BuildType::Aur => true,
            BuildType::Pacman => {
                if let Some(ref aur) = self.aur_helper {
                    aur.is_aur_package(&config.name)?
                } else {
                    false
                }
            }
        };

        if is_aur {
            if let Some(ref aur) = self.aur_helper {
                return Ok(aur as &dyn PackageManager);
            }
            return Err(anyhow!("AUR package requested but no AUR helper available"));
        }

        Ok(&self.pacman as &dyn PackageManager)
    }

    pub fn pacman_manager(&self) -> &dyn PackageManager {
        &self.pacman
    }

    pub fn aur_manager(&self) -> Option<&dyn PackageManager> {
        self.aur_helper.as_ref().map(|h| h as &dyn PackageManager)
    }

    pub fn has_aur_helper(&self) -> bool {
        self.aur_helper.is_some()
    }

    pub fn aur_helper_type(&self) -> Option<&str> {
        self.aur_helper
            .as_ref()
            .map(|h| h.helper_type().binary_name())
    }
}
