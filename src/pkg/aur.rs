use super::pacman::Pacman;
use super::{InstalledPackage, PackageManager};
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AurHelper {
    pacman: Pacman,
    helper_type: AurHelperType,
    build_dir: PathBuf,
    pkg_dest: PathBuf,
}
#[derive(Debug, Clone, Copy)]
pub enum AurHelperType {
    Yay,
    Paru,
}
impl AurHelper {
    pub fn new(
        pacman: Pacman,
        helper_type: AurHelperType,
        build_dir: PathBuf,
        pkg_dest: PathBuf,
    ) -> Self {
        Self {
            pacman,
            helper_type,
            build_dir,
            pkg_dest,
        }
    }
    pub(crate) fn is_aur_package(&self, name: &str) -> Result<bool> {
        todo!()
    }
}
impl PackageManager for AurHelper {
    fn install(&self, packages: &[String]) -> Result<Vec<InstalledPackage>> {
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
