use super::pacman::Pacman;
use super::{
    BuildType, InstalledPackage, PackageConfig, PackageInfo, PackageManager, PackageManagerType,
    PackageSource, SandboxConfig,
};
use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use which::which;

pub const AUR_DB_URL: &str = "https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD";
pub const AUR_RPC_URL: &str = "https://aur.archlinux.org/rpc.php";

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
        let cache_dir = store_root.join("cache/aur");
        let _ = fs::create_dir_all(&cache_dir);

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
            .env("LC_ALL", "C")
            .args(["-Si", name])
            .output()?;

        if !output.status.success() {
            return Ok(false);
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            if line.to_lowercase().starts_with("repository") {
                return Ok(line.to_lowercase().contains(": aur"));
            }
        }
        Ok(false)
    }

    pub fn get_aur_info(&self, name: &str, version: Option<&str>) -> Result<Option<PackageInfo>> {
        if let Some(ver) = version {
            return self.get_specific_version(name, ver);
        }

        self.get_aur_info_from_rpc(name)
    }

    fn get_aur_info_from_rpc(&self, name: &str) -> Result<Option<PackageInfo>> {
        let url = format!("{}?v=5&type=info&arg={}", AUR_RPC_URL, name);

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        let response = client.get(&url).send()?;
        if !response.status().is_success() {
            return Ok(None);
        }

        let body = response.text()?;
        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse AUR RPC response")?;

        if json["resultcount"].as_i64().unwrap_or(0) == 0 {
            return Ok(None);
        }

        let pkg = &json["results"][0];

        let version = pkg["Version"].as_str().unwrap_or("0");
        let (ver, rel) = if let Some(pos) = version.rfind('-') {
            (version[..pos].to_string(), version[pos + 1..].to_string())
        } else {
            (version.to_string(), "1".to_string())
        };

        let dependencies: Vec<String> = pkg["Depends"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let description = pkg["Description"].as_str().map(String::from);

        Ok(Some(PackageInfo {
            name: pkg["Name"].as_str().unwrap_or(name).to_string(),
            version: ver,
            release: rel,
            description,
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

    fn get_specific_version(&self, name: &str, version: &str) -> Result<Option<PackageInfo>> {
        let repo_url = format!("https://aur.archlinux.org/{}.git", name);
        let temp_dir = tempfile::tempdir()?;
        let clone_dir = temp_dir.path();

        Command::new("git")
            .args([
                "clone",
                "--bare",
                "--depth=500",
                &repo_url,
                clone_dir.to_str().unwrap(),
            ])
            .output()
            .context("Failed to clone AUR repository")?;
        let tags_output = Command::new("git")
            .current_dir(clone_dir)
            .args(["tag", "-l"])
            .output()
            .context("Failed to list tags");

        if let Ok(tags_output) = tags_output {
            let tags_content = String::from_utf8_lossy(&tags_output.stdout);
            if tags_content.trim().is_empty() {
                let target_tag = format!("{}-{}", name, version);
                for line in tags_content.lines() {
                    let tag = line.trim();
                    if tag == target_tag
                        || tag.starts_with(&target_tag)
                        || tag.ends_with(&format!("-{}", version))
                    {
                        let pkgbuild = self.fetch_pkgbuild_at_tag(clone_dir, tag)?;
                        return self.parse_pkgbuild(&pkgbuild, name);
                    }
                }

                for line in tags_content.lines() {
                    let tag = line.trim();
                    if tag.contains(version) {
                        let pkgbuild = self.fetch_pkgbuild_at_tag(clone_dir, tag)?;
                        return self.parse_pkgbuild(&pkgbuild, name);
                    }
                }
            }
        }

        if let Some(commit_pkgbuild) = self.find_commit_with_version(clone_dir, name, version)? {
            return Ok(Some(commit_pkgbuild));
        }

        Ok(None)
    }

    fn find_commit_with_version(
        &self,
        repo_dir: &std::path::Path,
        name: &str,
        version: &str,
    ) -> Result<Option<PackageInfo>> {
        let log_output = Command::new("git")
            .current_dir(repo_dir)
            .args(["log", "--all", "--format=%H %s"])
            .output()
            .context("Failed to get git log")?;

        if !log_output.status.success() {
            return Ok(None);
        }

        for line in String::from_utf8_lossy(&log_output.stdout).lines() {
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            if parts.len() < 2 {
                continue;
            }
            let commit_hash = parts[0];
            let commit_msg = parts[1];

            if Self::commit_contains_version(commit_msg, name, version) {
                let pkgbuild = self.fetch_pkgbuild_at_commit(repo_dir, commit_hash)?;
                if let Some(mut info) = self.parse_pkgbuild(&pkgbuild, name)? {
                    if info.version == version {
                        return Ok(Some(info));
                    }
                }
            }
        }

        let log_output2 = Command::new("git")
            .current_dir(repo_dir)
            .args([
                "log",
                "--all",
                "--format=%H",
                "-S",
                &format!("pkgver={}", version),
                "--",
                "PKGBUILD",
            ])
            .output()
            .context("Failed to search git log for version")?;

        if log_output2.status.success() {
            for line in String::from_utf8_lossy(&log_output2.stdout).lines() {
                let commit_hash = line.trim();
                if !commit_hash.is_empty() {
                    let pkgbuild = self.fetch_pkgbuild_at_commit(repo_dir, commit_hash)?;
                    if let Some(mut info) = self.parse_pkgbuild(&pkgbuild, name)? {
                        if info.version == version {
                            return Ok(Some(info));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    fn commit_contains_version(commit_msg: &str, name: &str, version: &str) -> bool {
        let patterns = [
            format!("{}-{}", name, version),
            format!("{} {}", name, version),
            format!("v{}", version),
            format!(
                "{}.{}",
                version.split('.').next().unwrap_or(version),
                version
            ),
            version.to_string(),
        ];

        for pattern in &patterns {
            if commit_msg.contains(pattern.as_str()) {
                return true;
            }
        }
        false
    }

    fn fetch_pkgbuild_at_commit(&self, repo_dir: &std::path::Path, commit: &str) -> Result<String> {
        let output = Command::new("git")
            .current_dir(repo_dir)
            .args(["show", &format!("{}:PKGBUILD", commit)])
            .output()
            .context("Failed to get PKGBUILD at commit")?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "Failed to fetch PKGBUILD at commit {}",
                commit
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn fetch_pkgbuild_at_tag(&self, repo_dir: &std::path::Path, tag: &str) -> Result<String> {
        let output = Command::new("git")
            .current_dir(repo_dir)
            .args(["show", &format!("{}:PKGBUILD", tag)])
            .output()
            .context("Failed to get PKGBUILD at tag")?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Failed to fetch PKGBUILD at tag {}", tag));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn parse_pkgbuild(&self, content: &str, name: &str) -> Result<Option<PackageInfo>> {
        let mut pkgver = String::new();
        let mut pkgrel = String::new();
        let mut depends: Vec<String> = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("pkgver=") {
                pkgver = line.trim_start_matches("pkgver=").trim().to_string();
            } else if line.starts_with("pkgrel=") {
                pkgrel = line.trim_start_matches("pkgrel=").trim().to_string();
            } else if line.starts_with("depends=") {
                let deps = line.trim_start_matches("depends=").trim();
                depends = self.parse_deps_array(deps);
            } else if line.starts_with("makedepends=") {
                let deps = line.trim_start_matches("makedepends=").trim();
                let makedeps: Vec<String> = self.parse_deps_array(deps);
                depends.extend(makedeps);
            }
        }

        if pkgver.is_empty() {
            return Ok(None);
        }

        Ok(Some(PackageInfo {
            name: name.to_string(),
            version: pkgver,
            release: pkgrel,
            description: None,
            dependencies: depends,
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

    fn parse_deps_array(&self, deps: &str) -> Vec<String> {
        let deps = deps.trim_start_matches('(').trim_end_matches(')');
        deps.split_whitespace()
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.split(|c| c == '<' || c == '>' || c == '=')
                    .next()
                    .unwrap_or(s)
                    .trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_matches('\'').to_string())
            .collect()
    }

    pub fn clone_aur_package(&self, name: &str) -> Result<PathBuf> {
        let cache_clone_dir = self.cache_dir.join(name);
        let clone_dir = self.build_dir.join(name);

        if cache_clone_dir.exists() {
            if clone_dir.exists() {
                fs::remove_dir_all(&clone_dir)?;
            }
            fs::create_dir_all(&self.build_dir)?;
            println!("Using cached AUR repo for {}", name);
            fs::create_dir_all(&cache_clone_dir)?;
            copy_dir_recursive(&cache_clone_dir, &clone_dir)?;
            return Ok(clone_dir);
        }

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

        fs::create_dir_all(&cache_clone_dir)?;
        let _ = Command::new("git")
            .args(["clone", &format!("https://aur.archlinux.org/{}.git", name)])
            .current_dir(&self.cache_dir)
            .output();
        println!(
            "Cached AUR repo for {} to {}",
            name,
            cache_clone_dir.display()
        );

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
            .env("LC_ALL", "C")
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
            .env("LC_ALL", "C")
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
            .get_aur_info(&pkg_name, None)?
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
                .query_package_info(&package.name, package.version.as_deref())
                .and_then(|info| info.ok_or_else(|| anyhow!("Package not found")));
        }

        let sandbox = package.sandbox_config.as_ref();
        let pkg_file = self.build_package(&package.name, sandbox)?;

        let temp_dir = tempfile::tempdir()?;
        let temp_root = temp_dir.path();

        Command::new(self.binary_path()?)
            .env("LC_ALL", "C")
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
            .get_aur_info(&package.name, package.version.as_deref())?
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

        let manifest_path = store_pkg_dir.join("manifest.toml");
        fs::write(manifest_path, toml::to_string_pretty(&pkg)?)?;

        info.installed = true;
        info.source = PackageSource::Aur;

        self.query_package_info(&package.name, package.version.as_deref())
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

    fn query_package_info(&self, name: &str, version: Option<&str>) -> Result<Option<PackageInfo>> {
        self.get_aur_info(name, version)
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

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
