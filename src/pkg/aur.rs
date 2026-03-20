use super::pacman::Pacman;
use super::{
    BuildType, InstalledPackage, PackageConfig, PackageInfo, PackageManager, PackageManagerType,
    PackageSource, SandboxConfig,
};
use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use which::which;

pub const AUR_DB_URL: &str = "https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD";

#[derive(Debug, Clone, Copy)]
pub enum AurHelperType {
    Yay,
    Paru,
}

impl AurHelperType {
    pub fn binary_name(&self) -> &'static str {
        match self {
            AurHelperType::Yay => "yay",
            AurHelperType::Paru => "paru",
        }
    }

    pub fn detect() -> Option<Self> {
        if which("paru").is_ok() {
            return Some(AurHelperType::Paru);
        }
        if which("yay").is_ok() {
            return Some(AurHelperType::Yay);
        }
        None
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::derive_partial_eq_without_hash)]
pub struct AurHelper {
    pacman: Pacman,
    helper_type: AurHelperType,
    build_dir: PathBuf,
    pkg_dest: PathBuf,
    cache_dir: PathBuf,
    store_root: PathBuf,
}

impl AurHelper {
    pub fn new(
        pacman: Pacman,
        helper_type: AurHelperType,
        build_dir: PathBuf,
        pkg_dest: PathBuf,
        store_root: PathBuf,
    ) -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("rscm")
            .join("aur");

        Self {
            pacman,
            helper_type,
            build_dir,
            pkg_dest,
            cache_dir,
            store_root,
        }
    }

    pub fn detect(store_root: PathBuf) -> Option<Self> {
        let helper_type = AurHelperType::detect()?;
        let pacman = Pacman::new(store_root.clone());

        let build_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("rscm")
            .join("aur-build");

        let pkg_dest = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("rscm")
            .join("aur-packages");

        Some(Self::new(
            pacman,
            helper_type,
            build_dir,
            pkg_dest,
            store_root,
        ))
    }

    pub fn helper_type(&self) -> AurHelperType {
        self.helper_type
    }

    pub fn build_dir(&self) -> &PathBuf {
        &self.build_dir
    }

    pub fn pkg_dest(&self) -> &PathBuf {
        &self.pkg_dest
    }

    pub fn binary_path(&self) -> Result<PathBuf> {
        which(self.helper_type.binary_name())
            .map_err(|_| anyhow!("{} not found in PATH", self.helper_type.binary_name()))
    }

    pub fn is_aur_package(&self, name: &str) -> Result<bool> {
        let output = Command::new(self.binary_path()?)
            .args(["-Si", name])
            .output()?;

        if !output.status.success() {
            return Ok(false);
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        Ok(output_str.contains("Repository      : AUR"))
    }

    pub fn search_aur(&self, query: &str) -> Result<Vec<PackageInfo>> {
        let output = Command::new(self.binary_path()?)
            .args(["-Ss", query])
            .output()
            .context("Failed to search AUR")?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        self.parse_aur_search_output(&output_str)
    }

    fn parse_aur_search_output(&self, output: &str) -> Result<Vec<PackageInfo>> {
        let mut results = Vec::new();

        for line in output.lines() {
            if line.starts_with("mesg/") || line.starts_with(":: ") {
                continue;
            }

            if let Some((repo_pkg, desc)) = line.split_once(" :: ") {
                let parts: Vec<&str> = repo_pkg.split('/').collect();
                if parts.len() >= 2 && parts[0] == "aur" {
                    let name = parts[1].to_string();

                    let desc_parts: Vec<&str> = desc.split_whitespace().collect();
                    let version = desc_parts.first().unwrap_or(&"0").to_string();

                    results.push(PackageInfo {
                        name,
                        version,
                        release: "1".to_string(),
                        description: Some(desc.to_string()),
                        dependencies: vec![],
                        optional_deps: vec![],
                        size: 0,
                        installed: false,
                        manager: match self.helper_type {
                            AurHelperType::Yay => PackageManagerType::Yay,
                            AurHelperType::Paru => PackageManagerType::Paru,
                        },
                        build_date: None,
                        source: PackageSource::Aur,
                    });
                }
            }
        }

        Ok(results)
    }

    pub fn get_aur_info(&self, name: &str) -> Result<Option<PackageInfo>> {
        let output = Command::new(self.binary_path()?)
            .args(["-Si", name])
            .output()
            .context("Failed to get AUR info")?;

        if !output.status.success() {
            return Ok(None);
        }

        self.parse_aur_info_output(&String::from_utf8_lossy(&output.stdout), name)
    }

    fn parse_aur_info_output(&self, output: &str, name: &str) -> Result<Option<PackageInfo>> {
        let mut info_map: HashMap<String, String> = HashMap::new();

        for line in output.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_lowercase();
                let value = value.trim().to_string();
                info_map.insert(key, value);
            }
        }

        if info_map.is_empty() {
            return Ok(None);
        }

        let version = info_map
            .get("version")
            .cloned()
            .unwrap_or_else(|| "0".to_string());

        let (ver, rel) = if let Some(pos) = version.rfind('-') {
            (version[..pos].to_string(), version[pos + 1..].to_string())
        } else {
            (version.clone(), "1".to_string())
        };

        let dependencies: Vec<String> = info_map
            .get("depends on")
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        Ok(Some(PackageInfo {
            name: name.to_string(),
            version: ver,
            release: rel,
            description: info_map.get("description").cloned(),
            dependencies,
            optional_deps: vec![],
            size: 0,
            installed: self.pacman.exists_in_db(name),
            manager: match self.helper_type {
                AurHelperType::Yay => PackageManagerType::Yay,
                AurHelperType::Paru => PackageManagerType::Paru,
            },
            build_date: None,
            source: PackageSource::Aur,
        }))
    }

    pub fn clone_aur_package(&self, name: &str) -> Result<PathBuf> {
        let clone_dir = self.build_dir.join(name);

        if clone_dir.exists() {
            fs::remove_dir_all(&clone_dir)?;
        }

        fs::create_dir_all(&self.build_dir)?;

        let output = Command::new("git")
            .args(["clone", &format!("https://aur.archlinux.org/{}.git", name)])
            .current_dir(&self.build_dir)
            .output()
            .context("Failed to clone AUR repository")?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to clone AUR package {}: {}",
                name,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(clone_dir)
    }

    pub fn build_in_sandbox(&self, pkg_dir: &Path, sandbox: &SandboxConfig) -> Result<PathBuf> {
        let bwrap = Bubblewrap::new();

        let mut bwrap_cmd = bwrap.command();

        if sandbox.network {
            bwrap_cmd.arg("--share-net");
        } else {
            bwrap_cmd.arg("--unshare-net");
        }

        for path in &sandbox.ro_paths {
            bwrap_cmd.arg("--ro-bind").arg(path).arg(path);
        }

        for path in &sandbox.rw_paths {
            bwrap_cmd.arg("--bind").arg(path).arg(path);
        }

        bwrap_cmd
            .arg("--tmpfs")
            .arg("/tmp")
            .arg("--tmpfs")
            .arg("/build")
            .arg("--chdir")
            .arg("/build")
            .arg("/bin/bash")
            .arg("-c")
            .arg(&format!("makepkg -s --noconfirm && ls *.pkg.tar.zst"));

        let output = bwrap_cmd
            .current_dir(pkg_dir)
            .output()
            .context("Failed to build package in sandbox")?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to build package: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let pkg_file = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|l| l.ends_with(".pkg.tar.zst"))
            .ok_or_else(|| anyhow!("No package file produced"))?
            .to_string();

        Ok(PathBuf::from(pkg_file))
    }

    pub fn build_package(
        &self,
        name: &str,
        sandbox_config: Option<&SandboxConfig>,
    ) -> Result<PathBuf> {
        let clone_dir = self.clone_aur_package(name)?;

        let sandbox = sandbox_config.cloned().unwrap_or_else(|| SandboxConfig {
            network: false,
            ro_paths: vec![
                "/usr".to_string(),
                "/etc/pacman.conf".to_string(),
                "/var/cache/pacman/pkg".to_string(),
            ],
            rw_paths: vec![],
        });

        if which("bubblewrap").is_ok() {
            self.build_in_sandbox(&clone_dir, &sandbox)
        } else {
            self.build_direct(&clone_dir)
        }
    }

    fn build_direct(&self, pkg_dir: &Path) -> Result<PathBuf> {
        let output = Command::new("makepkg")
            .args(["-s", "--noconfirm"])
            .current_dir(pkg_dir)
            .output()
            .context("Failed to build package")?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to build package: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let pkg_files: Vec<PathBuf> = fs::read_dir(pkg_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "zst").unwrap_or(false))
            .collect();

        pkg_files
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No package file produced"))
    }

    pub fn install_built_package(&self, pkg_file: &Path) -> Result<InstalledPackage> {
        let output = Command::new(self.binary_path()?)
            .args(["-U", "--noconfirm", &pkg_file.to_string_lossy()])
            .output()
            .context("Failed to install built package")?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to install package: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let pkg_name = pkg_file
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit('-').nth(1))
            .unwrap_or("unknown")
            .to_string();

        let info = self
            .get_aur_info(&pkg_name)?
            .ok_or_else(|| anyhow!("Failed to get package info after installation"))?;

        Ok(InstalledPackage {
            name: info.name,
            version: info.version,
            release: info.release,
            install_time: SystemTime::now(),
            description: info.description,
            dependencies: info.dependencies,
            install_root: PathBuf::from("/"),
            files: vec![],
            manager: match self.helper_type {
                AurHelperType::Yay => PackageManagerType::Yay,
                AurHelperType::Paru => PackageManagerType::Paru,
            },
        })
    }
}

pub struct Bubblewrap {
    sandbox_dir: PathBuf,
}

impl Bubblewrap {
    pub fn new() -> Self {
        let sandbox_dir = std::env::temp_dir().join("rscm-sandbox");
        let _ = fs::create_dir_all(&sandbox_dir);
        Self { sandbox_dir }
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new("bwrap");

        cmd.arg("--die-with-parent")
            .arg("--unshare-pid")
            .arg("--unshare-user")
            .arg("--unshareUTS")
            .arg("--cap-add")
            .arg("all")
            .arg("--dir")
            .arg("/tmp")
            .arg("--dir")
            .arg("/build")
            .arg("--dir")
            .arg("/var")
            .arg("--dir")
            .arg("/var/cache")
            .arg("--dir")
            .arg("/var/lib")
            .arg("--symlink")
            .arg("../var/tmp")
            .arg("/tmp")
            .arg("--proc")
            .arg("/proc")
            .arg("--dev")
            .arg("/dev")
            .arg("--ro-bind")
            .arg("/sys")
            .arg("/sys")
            .arg("--bind")
            .arg("/sys/class/net")
            .arg("/sys/class/net")
            .arg("--setenv")
            .arg("HOME")
            .arg("/build")
            .arg("--setenv")
            .arg("PKGDEST")
            .arg("/build")
            .arg("--setenv")
            .arg("SRCDEST")
            .arg("/build/src")
            .arg("--setenv")
            .arg("srcdest")
            .arg("/build/src")
            .arg("--setenv")
            .arg("LOGDEST")
            .arg("/build/logs")
            .arg("--setenv")
            .arg("PACKAGER")
            .arg("rscm");

        cmd
    }

    pub fn is_available() -> bool {
        which("bubblewrap").is_ok()
    }

    pub fn check_capabilities() -> Result<()> {
        if !Self::is_available() {
            return Err(anyhow!("bubblewrap is not installed"));
        }

        let output = Command::new("bwrap")
            .args(["--unshare-user", "true"])
            .output()?;

        if !output.status.success() {
            return Err(anyhow!(
                "bubblewrap cannot create user namespaces: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }
}

impl Default for Bubblewrap {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageManager for AurHelper {
    fn is_available_in_store(&self, package: &PackageConfig) -> bool {
        self.pacman.exists_in_db(&package.name)
    }

    fn ensure_in_store(&self, package: &PackageConfig) -> Result<PackageInfo> {
        self.install(package, false)
    }

    fn install(&self, package: &PackageConfig, force: bool) -> Result<PackageInfo> {
        if !force && self.is_available_in_store(package) {
            return self
                .query_package_info(&package.name)
                .and_then(|info| info.ok_or_else(|| anyhow!("Package not found")));
        }

        let sandbox = package.sandbox_config.as_ref();
        let pkg_file = self.build_package(&package.name, sandbox)?;

        let temp_dir = tempfile::tempdir()?;
        let temp_root = temp_dir.path();

        Command::new(self.binary_path()?)
            .args([
                "-U",
                "--noconfirm",
                "--root",
                &temp_root.to_string_lossy(),
                &pkg_file.to_string_lossy(),
            ])
            .output()?;

        let files = self.pacman.scan_package_files(&package.name, temp_root)?;

        let mut info = self
            .get_aur_info(&package.name)?
            .ok_or_else(|| anyhow!("Failed to get package info"))?;

        let build_hash = hex::encode(&Sha256::digest(format!(
            "{}-{}-{}-aur",
            info.name, info.version, info.release
        )));

        let store_pkg_dir = self.store_root.join("packages").join(format!(
            "{}-{}-{}-{}",
            info.name,
            info.version,
            info.release,
            &build_hash[..8]
        ));
        fs::create_dir_all(&store_pkg_dir)?;

        let pkg = crate::store::Package {
            name: info.name.clone(),
            version: info.version.clone(),
            release: info.release.clone(),
            files,
            dependencies: info.dependencies.clone(),
            install_time: SystemTime::now(),
        };

        let manifest_path = store_pkg_dir.join("manifest.json");
        fs::write(manifest_path, serde_json::to_string_pretty(&pkg)?)?;

        info.installed = true;
        info.source = PackageSource::Aur;

        self.query_package_info(&package.name)
            .and_then(|info| info.ok_or_else(|| anyhow!("Package not found after install")))
    }

    fn remove(
        &self,
        package_name: &str,
        version: Option<&str>,
        recursive: bool,
    ) -> Result<super::RemoveResult> {
        self.pacman.remove(package_name, version, recursive)
    }

    fn query_package_info(&self, name: &str) -> Result<Option<PackageInfo>> {
        self.get_aur_info(name)
    }

    fn list_installed(&self) -> Result<Vec<InstalledPackage>> {
        self.pacman.list_installed_packages()
    }

    fn build_type(&self) -> BuildType {
        BuildType::Aur
    }

    fn manager_name(&self) -> &'static str {
        self.helper_type.binary_name()
    }
}
