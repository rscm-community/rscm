use super::{
    BuildType, InstalledPackage, PackageConfig, PackageInfo, PackageManager, PackageManagerType,
    PackageSource,
};
use crate::pkg::privilege::PrivilegeManager;
use crate::store::package::FileEntry;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const PACMAN_DB_PATH: &str = "/var/lib/pacman";
const ARCHIVE_URL: &str = "https://archive.archlinux.org";

#[derive(Debug, Clone)]
pub struct Pacman {
    isolated_root: Option<PathBuf>,
    isolated_db_path: Option<PathBuf>,
    cache_dir: PathBuf,
    archive_cache_dir: PathBuf,
    privilege: PrivilegeManager,
    store_root: PathBuf,
}

impl Pacman {
    pub fn new(store_root: PathBuf) -> Self {
        let isolated_root = store_root.join("tmp/pacman");
        let isolated_db_path = isolated_root.join("var/lib/pacman");
        let cache_dir = isolated_root.join("var/cache/pacman/pkg");
        let archive_cache_dir = store_root.join("cache/archive");

        fs::create_dir_all(&isolated_db_path).ok();
        fs::create_dir_all(&cache_dir).ok();
        fs::create_dir_all(&archive_cache_dir).ok();

        Self {
            isolated_root: Some(isolated_root.clone()),
            isolated_db_path: Some(isolated_db_path),
            cache_dir,
            archive_cache_dir,
            privilege: PrivilegeManager::new(),
            store_root,
        }
    }

    pub fn system(store_root: PathBuf) -> Self {
        let archive_cache_dir = store_root.join("cache/archive");
        fs::create_dir_all(&archive_cache_dir).ok();

        Self {
            isolated_root: None,
            isolated_db_path: None,
            cache_dir: PathBuf::from("/var/cache/pacman/pkg"),
            archive_cache_dir,
            privilege: PrivilegeManager::new(),
            store_root,
        }
    }

    fn get_isolated_root(&self) -> Option<&Path> {
        self.isolated_root.as_deref()
    }

    fn get_case_insensitive<'a>(map: &'a HashMap<String, String>, key: &str) -> Option<&'a String> {
        let key_lower = key.to_lowercase();
        map.iter()
            .find(|(k, _)| k.to_lowercase() == key_lower)
            .map(|(_, v)| v)
    }

    fn run_pacman_system(&self, args: &[&str]) -> Result<std::process::Output> {
        let mut cmd = if self.privilege.is_root() {
            Command::new("pacman")
        } else {
            Command::new("sudo")
        };

        if !self.privilege.is_root() {
            cmd.arg("pacman");
        }
        cmd.env("LC_ALL", "C");
        cmd.args(args);

        cmd.output()
            .map_err(|e| anyhow::anyhow!("Failed to run pacman: {}", e))
    }

    fn run_pacman_isolated(&self, args: &[&str]) -> Result<std::process::Output> {
        let root = self
            .get_isolated_root()
            .ok_or_else(|| anyhow::anyhow!("Pacman not configured for isolated operation"))?;

        let db_path = self
            .isolated_db_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(PACMAN_DB_PATH.to_string());

        let mut cmd = if self.privilege.is_root() {
            Command::new("pacman")
        } else {
            Command::new("sudo")
        };

        if !self.privilege.is_root() {
            cmd.arg("pacman");
        }

        cmd.env("LC_ALL", "C");
        cmd.arg("--root").arg(root);
        cmd.arg("--dbpath").arg(db_path);
        cmd.args(args);

        cmd.output()
            .map_err(|e| anyhow::anyhow!("Failed to run pacman in isolation: {}", e))
    }

    pub fn sync_database(&self) -> Result<()> {
        let output = if self.isolated_root.is_some() {
            self.run_pacman_isolated(&["-Sy"])?
        } else {
            self.run_pacman_system(&["-Sy"])?
        };

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "Failed to sync database: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    pub fn package_info_from_system(&self, name: &str) -> Result<Option<PackageInfo>> {
        let output = self.run_pacman_system(&["-Qi", name])?;

        if !output.status.success() {
            return Ok(None);
        }

        self.parse_pacman_qi_output(&String::from_utf8_lossy(&output.stdout), name)
    }

    pub fn package_info_from_sync_db(
        &self,
        name: &str,
        _version: Option<&str>,
    ) -> Result<Option<PackageInfo>> {
        let output = self.run_pacman_system(&["-Si", name])?;

        if !output.status.success() {
            return Ok(None);
        }

        self.parse_pacman_si_output(&String::from_utf8_lossy(&output.stdout), name)
    }

    fn parse_pacman_si_output(&self, output: &str, name: &str) -> Result<Option<PackageInfo>> {
        let mut info_map: HashMap<String, String> = HashMap::new();

        for line in output.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                info_map.insert(key, value);
            }
        }

        if info_map.is_empty() {
            return Ok(None);
        }

        let version = Self::get_case_insensitive(&info_map, "Version")
            .cloned()
            .unwrap_or_else(|| "0".to_string());

        let (ver, rel) = if let Some(pos) = version.rfind('-') {
            (version[..pos].to_string(), version[pos + 1..].to_string())
        } else {
            (version.clone(), "1".to_string())
        };

        let repository = Self::get_case_insensitive(&info_map, "Repository")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        let dependencies: Vec<String> = Self::get_case_insensitive(&info_map, "Depends On")
            .map(|s| {
                s.split_whitespace()
                    .map(String::from)
                    .filter(|s| s != "None")
                    .collect()
            })
            .unwrap_or_default();

        let optional_deps: Vec<String> = Self::get_case_insensitive(&info_map, "Optional Deps")
            .map(|s| {
                s.split_whitespace()
                    .map(String::from)
                    .filter(|s| s != "None")
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(PackageInfo {
            name: name.to_string(),
            version: ver,
            release: rel,
            description: Self::get_case_insensitive(&info_map, "Description").cloned(),
            dependencies,
            optional_deps,
            size: 0,
            installed: self.exists_in_db(name),
            manager: PackageManagerType::Pacman,
            build_date: None,
            source: PackageSource::Repository(repository),
        }))
    }

    fn parse_pacman_qi_output(&self, output: &str, name: &str) -> Result<Option<PackageInfo>> {
        let mut info_map: HashMap<String, String> = HashMap::new();

        for line in output.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                info_map.insert(key, value);
            }
        }

        if info_map.is_empty() {
            return Ok(None);
        }

        let version = Self::get_case_insensitive(&info_map, "Version")
            .cloned()
            .unwrap_or_else(|| "0".to_string());

        let (ver, rel) = if let Some(pos) = version.rfind('-') {
            (version[..pos].to_string(), version[pos + 1..].to_string())
        } else {
            (version.clone(), "1".to_string())
        };

        let dependencies: Vec<String> = Self::get_case_insensitive(&info_map, "Depends On")
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        let optional_deps: Vec<String> = Self::get_case_insensitive(&info_map, "Optional Deps")
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        let size: u64 = Self::get_case_insensitive(&info_map, "Installed Size")
            .and_then(|s| {
                s.replace(".", "")
                    .replace("K", "000")
                    .replace("M", "000000")
                    .replace("G", "000000000")
                    .parse()
                    .ok()
            })
            .unwrap_or(0);

        let build_date = Self::get_case_insensitive(&info_map, "Build Date")
            .and_then(|s| s.parse::<i64>().ok())
            .map(|t| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t as u64));

        Ok(Some(PackageInfo {
            name: name.to_string(),
            version: ver,
            release: rel,
            description: Self::get_case_insensitive(&info_map, "Description").cloned(),
            dependencies,
            optional_deps,
            size,
            installed: true,
            manager: PackageManagerType::Pacman,
            build_date,
            source: PackageSource::Repository("core".to_string()),
        }))
    }

    pub fn list_installed_packages(&self) -> Result<Vec<InstalledPackage>> {
        let output = self.run_pacman_system(&["-Q", "--info"])?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Failed to list installed packages"));
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        self.parse_installed_packages(&output_str)
    }

    fn parse_installed_packages(&self, output: &str) -> Result<Vec<InstalledPackage>> {
        let mut packages = Vec::new();
        let package_blocks: Vec<&str> = output.split("\n\n").collect();

        for block in package_blocks {
            if block.trim().is_empty() {
                continue;
            }

            let mut info_map: HashMap<String, String> = HashMap::new();
            for line in block.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_string();
                    let value = value.trim().to_string();
                    info_map.insert(key, value);
                }
            }

            if let Some(name) = Self::get_case_insensitive(&info_map, "Name") {
                let version_str = Self::get_case_insensitive(&info_map, "Version")
                    .cloned()
                    .unwrap_or_else(|| "0".to_string());

                let (version, release) = if let Some(pos) = version_str.rfind('-') {
                    (
                        version_str[..pos].to_string(),
                        version_str[pos + 1..].to_string(),
                    )
                } else {
                    (version_str.clone(), "1".to_string())
                };

                let install_time = Self::get_case_insensitive(&info_map, "Install Date")
                    .and_then(|s| self.parse_pacman_date(s))
                    .unwrap_or(SystemTime::UNIX_EPOCH);

                let dependencies: Vec<String> = Self::get_case_insensitive(&info_map, "Depends On")
                    .map(|s| s.split_whitespace().map(String::from).collect())
                    .unwrap_or_default();

                packages.push(InstalledPackage {
                    name: name.clone(),
                    version,
                    release,
                    install_time,
                    description: Self::get_case_insensitive(&info_map, "Description").cloned(),
                    dependencies,
                    install_root: self
                        .isolated_root
                        .clone()
                        .unwrap_or_else(|| PathBuf::from("/")),
                    files: vec![],
                    manager: PackageManagerType::Pacman,
                });
            }
        }

        Ok(packages)
    }

    fn parse_pacman_date(&self, date_str: &str) -> Option<SystemTime> {
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];

        let parts: Vec<&str> = date_str.split_whitespace().collect();

        let (year_idx, time_idx, month_idx, day_idx) = if parts.len() == 5 {
            (4, 3, 1, 2)
        } else if parts.len() == 6 {
            (5, 4, 1, 2)
        } else {
            return None;
        };

        let year = parts[year_idx].parse::<u16>().ok()?;
        let day = parts[day_idx].parse::<u8>().ok()?;
        let time_parts: Vec<&str> = parts[time_idx].split(':').collect();
        if time_parts.len() < 3 {
            return None;
        }
        let hour = time_parts[0].parse::<u8>().ok()?;
        let min = time_parts[1].parse::<u8>().ok()?;
        let sec = time_parts[2].parse::<u8>().ok()?;

        let month = months
            .iter()
            .position(|&m| m == parts[month_idx])
            .map(|p| p as u8 + 1)?;

        let days: i64 = (year as i64 - 1970) * 365 + (month as i64 - 1) * 30 + day as i64;
        let seconds: i64 = days * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64;
        Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds as u64))
    }

    pub fn list_package_files(&self, name: &str) -> Result<Vec<String>> {
        let output = self.run_pacman_system(&["-Ql", name])?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Failed to list files for package {}", name));
        }

        let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let path = line.split_once(' ').map(|(_, p)| p.to_string());
                if path.as_ref().map(|p| !p.ends_with('/')).unwrap_or(false) {
                    path
                } else {
                    None
                }
            })
            .collect();

        Ok(files)
    }

    pub fn scan_package_files(&self, pkg_name: &str, root: &Path) -> Result<Vec<FileEntry>> {
        let files = self.list_package_files(pkg_name)?;
        let mut entries = Vec::new();

        for file_path in files {
            let full_path = root.join(&file_path);

            if !full_path.exists() {
                continue;
            }

            let metadata = fs::metadata(&full_path)?;

            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&full_path)?.to_string_lossy().to_string();
                let target_hash = self.compute_hash_for_path(&full_path)?;

                entries.push(FileEntry {
                    path: file_path,
                    hash: target_hash,
                    size: metadata.len(),
                    mode: 0o120000,
                    symlink_target: Some(target),
                });
            } else {
                let hash = self.compute_hash_for_path(&full_path)?;
                let mode = metadata.permissions().mode() & 0o7777;

                entries.push(FileEntry {
                    path: file_path,
                    hash,
                    size: metadata.len(),
                    mode,
                    symlink_target: None,
                });
            }
        }

        Ok(entries)
    }

    fn compute_hash_for_path(&self, path: &Path) -> Result<String> {
        let mut file = fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        Ok(hex::encode(hasher.finalize()))
    }

    pub fn install_to_isolated(
        &self,
        packages: &[String],
    ) -> Result<(tempfile::TempDir, Vec<InstalledPackage>)> {
        if self.isolated_root.is_none() {
            return Err(anyhow::anyhow!(
                "Pacman not configured for isolated operation. Use new() with a root path."
            ));
        }

        if !self.privilege.is_root() && !self.privilege.test_sudo() {
            return Err(anyhow::anyhow!(
                "Root privileges required for package installation"
            ));
        }

        let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;

        let target_root = temp_dir.path();

        let mut args = vec![
            "-Sy".to_string(),
            "--root".to_string(),
            target_root.to_string_lossy().to_string(),
            "--noconfirm".to_string(),
        ];
        args.extend(packages.iter().cloned());

        let output =
            self.run_pacman_isolated(&args.iter().map(|s| s.as_str()).collect::<Vec<_>>())?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "Failed to install packages: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let mut installed = Vec::new();
        for pkg_name in packages {
            if let Some(info) = self.package_info_from_system(pkg_name)? {
                installed.push(InstalledPackage {
                    name: info.name,
                    version: info.version,
                    release: info.release,
                    install_time: SystemTime::now(),
                    description: info.description,
                    dependencies: info.dependencies,
                    install_root: target_root.to_path_buf(),
                    files: vec![],
                    manager: PackageManagerType::Pacman,
                });
            }
        }

        Ok((temp_dir, installed))
    }

    pub fn exists_in_system(&self, name: &str) -> bool {
        self.run_pacman_system(&["-Q", name])
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn exists_in_db(&self, name: &str) -> bool {
        self.exists_in_system(name)
    }

    pub fn search_packages(&self, query: &str) -> Result<Vec<PackageInfo>> {
        let output = self.run_pacman_system(&["-Ss", query])?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let mut results = Vec::new();
        let output_str = String::from_utf8_lossy(&output.stdout);

        for line in output_str.lines() {
            if line.starts_with("mesg/") || line.starts_with(":: ") {
                continue;
            }

            if let Some((repo_pkg, desc)) = line.split_once(" :: ") {
                let parts: Vec<&str> = repo_pkg.split('/').collect();
                if parts.len() >= 2 {
                    let repo = parts[0].to_string();
                    let name = parts[1].to_string();

                    let version_parts: Vec<&str> = desc.split_whitespace().collect();
                    let version = version_parts.first().unwrap_or(&"0").to_string();

                    results.push(PackageInfo {
                        name,
                        version,
                        release: "1".to_string(),
                        description: desc.lines().next().map(String::from),
                        dependencies: vec![],
                        optional_deps: vec![],
                        size: 0,
                        installed: self.exists_in_system(&parts[1]),
                        manager: PackageManagerType::Pacman,
                        build_date: None,
                        source: PackageSource::Repository(repo),
                    });
                }
            }
        }

        Ok(results)
    }

    pub fn get_package_size(&self, name: &str) -> Result<u64> {
        let output = self.run_pacman_system(&["-Si", name])?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Package {} not found", name));
        }

        let output_str = String::from_utf8_lossy(&output.stdout);

        for line in output_str.lines() {
            if line.to_lowercase().starts_with("download size") {
                if let Some(size_str) = line.split_once(':') {
                    let size_part = size_str.1.trim().to_lowercase();
                    let size_val: f64 = size_part
                        .replace("MiB", "")
                        .replace("KiB", "")
                        .replace("GiB", "")
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0.0);

                    let multiplier = if size_part.contains("KiB") {
                        1024.0
                    } else if size_part.contains("GiB") {
                        1024.0 * 1024.0 * 1024.0
                    } else {
                        1024.0 * 1024.0
                    };

                    return Ok((size_val * multiplier) as u64);
                }
            }
        }

        Ok(0)
    }
}

impl PackageManager for Pacman {
    fn is_available_in_store(&self, package: &PackageConfig) -> bool {
        self.exists_in_system(&package.name)
    }

    fn ensure_in_store(&self, package: &PackageConfig) -> Result<PackageInfo> {
        self.install(package, false)
    }

    fn install(&self, package: &PackageConfig, force: bool) -> Result<PackageInfo> {
        if !force && self.is_available_in_store(package) {
            return self
                .query_package_info(&package.name, package.version.as_deref())
                .and_then(|info| info.ok_or_else(|| anyhow::anyhow!("Package not found")));
        }

        if !self.privilege.is_root() && !self.privilege.test_sudo() {
            return Err(anyhow::anyhow!(
                "Root privileges required for package installation"
            ));
        }

        let (temp_dir, _installed) = self.install_to_isolated(&[package.name.clone()])?;
        let temp_root = temp_dir.path();

        let files = self.scan_package_files(&package.name, temp_root)?;

        let mut info = self
            .package_info_from_system(&package.name)?
            .ok_or_else(|| anyhow::anyhow!("Package {} not found", package.name))?;

        let build_hash = hex::encode(&Sha256::digest(format!(
            "{}-{}-{}",
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
        info.source = PackageSource::Repository("core".to_string());

        self.query_package_info(&package.name, package.version.as_deref())
            .and_then(|info| info.ok_or_else(|| anyhow::anyhow!("Package not found after install")))
    }

    fn remove(
        &self,
        package_name: &str,
        version: Option<&str>,
        recursive: bool,
    ) -> Result<super::RemoveResult> {
        if !self.privilege.is_root() && !self.privilege.test_sudo() {
            return Err(anyhow::anyhow!(
                "Root privileges required for package removal"
            ));
        }

        let packages_dir = self.store_root.join("packages");

        let mut found_packages = Vec::new();
        let pattern = match version {
            Some(v) => format!("{}-{}-*.json", package_name, v),
            None => format!("{}-*.json", package_name),
        };

        let glob_pattern = packages_dir.join(&pattern);
        for entry in glob::glob(glob_pattern.to_str().unwrap())? {
            let path = entry?;
            if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                found_packages.push((name.to_string(), path));
            }
        }

        if found_packages.is_empty() {
            return Err(anyhow::anyhow!(
                "Package {} not found in store",
                package_name
            ));
        }

        let mut removed_dependents = Vec::new();
        if recursive {
            let dependents = self.check_dependencies(package_name)?;
            let mut visited = std::collections::HashSet::new();
            visited.insert(package_name.to_string());

            for dep in &dependents {
                if !visited.contains(dep) {
                    let result = self.remove(dep, None, true)?;
                    removed_dependents.push(dep.clone());
                    removed_dependents.extend(result.removed_dependents);
                }
            }
        }

        let mut removed_versions = Vec::new();
        let mut files_removed = 0;
        let mut space_freed = 0u64;

        for (full_name, manifest_path) in &found_packages {
            let content = fs::read_to_string(manifest_path)?;
            let pkg: crate::store::Package = serde_json::from_str(&content)?;

            for file in &pkg.files {
                space_freed += file.size;
                files_removed += 1;
            }

            fs::remove_file(manifest_path)?;

            let parts: Vec<&str> = full_name.split('-').collect();
            if parts.len() >= 2 {
                removed_versions.push(format!(
                    "{}-{}",
                    parts[parts.len() - 2],
                    parts[parts.len() - 1]
                ));
            }
        }

        Ok(super::RemoveResult {
            package_name: package_name.to_string(),
            removed_versions,
            files_removed,
            space_freed,
            recursive,
            removed_dependents,
        })
    }

    fn query_package_info(&self, name: &str, version: Option<&str>) -> Result<Option<PackageInfo>> {
        if let Some(expected_version) = version {
            println!("Fetching {} from Arch Linux Archive...", expected_version);
            let archive_info = self.query_package_info_from_archive(name, expected_version)?;
            return Ok(Some(archive_info));
        }

        if let Some(info) = self.package_info_from_sync_db(name, None)? {
            return Ok(Some(info));
        }
        self.package_info_from_system(name)
    }

    fn list_installed(&self) -> Result<Vec<InstalledPackage>> {
        self.list_installed_packages()
    }

    fn build_type(&self) -> BuildType {
        BuildType::Pacman
    }

    fn manager_name(&self) -> &'static str {
        "pacman"
    }
}

impl Pacman {
    pub fn query_package_info_from_archive(
        &self,
        name: &str,
        version: &str,
    ) -> Result<PackageInfo> {
        let first_char = name.chars().next().unwrap_or('a');
        let archive_path = format!("{}/packages/{}/{}", ARCHIVE_URL, first_char, name);

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?;

        let response = client.get(&archive_path).send()?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Package {} not found in Arch Linux Archive",
                name
            ));
        }

        let body = response.text()?;

        let mut available_packages: Vec<String> = Vec::new();
        for line in body.lines() {
            if line.contains(".pkg.tar.zst") || line.contains(".pkg.tar.xz") {
                if let Some(start) = line.find("href=\"") {
                    let start = start + 6;
                    if let Some(end) = line[start..].find("\"") {
                        let filename = &line[start..start + end];
                        if !filename.ends_with(".sig") && filename.contains(name) {
                            available_packages.push(filename.to_string());
                        }
                    }
                }
            }
        }

        if available_packages.is_empty() {
            return Err(anyhow::anyhow!(
                "No packages found for {} in Arch Linux Archive",
                name
            ));
        }

        let target_prefix = format!("{}-{}", name, version);
        let package_filename = available_packages
            .iter()
            .find(|p| {
                p.starts_with(&target_prefix)
                    && (p.ends_with(".pkg.tar.zst") || p.ends_with(".pkg.tar.xz"))
            })
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Version {} of package {} not found in Arch Linux Archive",
                    version,
                    name
                )
            })?;

        let pkg_url = format!("{}/{}", archive_path, package_filename);

        let cached_pkg_path = self.archive_cache_dir.join(&package_filename);

        let pkg_path = if cached_pkg_path.exists() {
            println!("Using cached {}-{}", name, version);
            cached_pkg_path.clone()
        } else {
            let temp_dir = tempfile::tempdir()?;
            let temp_pkg_path = temp_dir.path().join(&package_filename);

            println!("Downloading {} from archive...", pkg_url);
            let mut response = client.get(&pkg_url).send()?;
            if !response.status().is_success() {
                return Err(anyhow::anyhow!(
                    "Failed to download package {} from archive",
                    name
                ));
            }

            let total_size = response.content_length().unwrap_or(0);
            let mut file = File::create(&temp_pkg_path)?;
            let mut downloaded: u64 = 0;
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                match response.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        file.write_all(&buffer[..n])?;
                        downloaded += n as u64;
                        if total_size > 0 {
                            let pct = (downloaded * 100) / total_size;
                            print!(
                                "\rDownloading {}-{}: {}/{} ({}%)",
                                name, version, downloaded, total_size, pct
                            );
                            std::io::stdout().flush().ok();
                        } else {
                            print!("\rDownloading {}: {} bytes", name, downloaded);
                            std::io::stdout().flush().ok();
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(anyhow::anyhow!("Download error: {}", e)),
                }
            }
            println!();

            fs::copy(&temp_pkg_path, &cached_pkg_path)?;
            println!("Cached to {}", cached_pkg_path.display());

            cached_pkg_path
        };

        let pkg_info = self.parse_package_file(&pkg_path, name, version)?;

        Ok(pkg_info)
    }

    fn parse_package_file(
        &self,
        pkg_path: &Path,
        name: &str,
        version: &str,
    ) -> Result<PackageInfo> {
        let file = File::open(pkg_path)?;
        let file_size = file.metadata()?.len();

        let decompressed: Box<dyn Read> = if pkg_path.to_string_lossy().ends_with(".zst") {
            let mut decoder = zstd::stream::Decoder::new(file)?;
            Box::new(std::io::BufReader::new(decoder))
        } else {
            Box::new(file)
        };

        let mut archive = tar::Archive::new(decompressed);

        let mut pkginfo_content = None;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();
            if path.ends_with(".pkginfo") || path == ".PKGINFO" {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                pkginfo_content = Some(content);
                break;
            }
        }

        let (ver, rel) = if let Some(pos) = version.rfind('-') {
            (version[..pos].to_string(), version[pos + 1..].to_string())
        } else {
            (version.to_string(), "1".to_string())
        };

        let mut dependencies = Vec::new();
        let mut optional_deps = Vec::new();
        let mut description = None;
        let mut provides = Vec::new();
        let mut conflicts = Vec::new();

        if let Some(content) = pkginfo_content {
            for line in content.lines() {
                let line = line.trim();
                if let Some(val) = line.strip_prefix("depend = ") {
                    let dep = val.trim().to_string();
                    if !dep.is_empty() {
                        dependencies.push(dep);
                    }
                } else if let Some(val) = line.strip_prefix("optdepend = ") {
                    let dep = val.trim().to_string();
                    if !dep.is_empty() {
                        optional_deps.push(dep);
                    }
                } else if let Some(val) = line.strip_prefix("provides = ") {
                    let prov = val.trim().to_string();
                    if !prov.is_empty() {
                        provides.push(prov);
                    }
                } else if let Some(val) = line.strip_prefix("conflicts = ") {
                    let conf = val.trim().to_string();
                    if !conf.is_empty() {
                        conflicts.push(conf);
                    }
                } else if let Some(val) = line.strip_prefix("replaces = ") {
                    let rep = val.trim().to_string();
                    if !rep.is_empty() {
                        conflicts.push(rep);
                    }
                } else if let Some(desc) = line.strip_prefix("pkgdesc = ") {
                    description = Some(desc.trim().to_string());
                }
            }
        }

        Ok(PackageInfo {
            name: name.to_string(),
            version: ver,
            release: rel,
            description,
            dependencies,
            optional_deps,
            size: file_size,
            installed: false,
            manager: PackageManagerType::Pacman,
            build_date: None,
            source: PackageSource::Repository("archive".to_string()),
        })
    }

    fn check_dependencies(&self, package_name: &str) -> Result<Vec<String>> {
        let mut dependents = Vec::new();

        let packages_dir = self.store_root.join("packages");
        if packages_dir.exists() {
            for entry in fs::read_dir(&packages_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    let content = fs::read_to_string(&path)?;
                    if let Ok(pkg) = serde_json::from_str::<crate::store::Package>(&content) {
                        if pkg.dependencies.contains(&package_name.to_string()) {
                            if !dependents.contains(&pkg.name) {
                                dependents.push(pkg.name.clone());
                            }
                        }
                    }
                }
            }
        }

        for installed_pkg in self.list_installed()? {
            if installed_pkg
                .dependencies
                .contains(&package_name.to_string())
            {
                if !dependents.contains(&installed_pkg.name) {
                    dependents.push(installed_pkg.name.clone());
                }
            }
        }

        let generations_dir = self.store_root.join("generations");
        if generations_dir.exists() {
            for entry in fs::read_dir(&generations_dir)? {
                let entry = entry?;
                let gen_path = entry.path();
                let manifest_path = gen_path.join("manifest.toml");
                if manifest_path.exists() {
                    let content = fs::read_to_string(&manifest_path)?;
                    if let Ok(manifest) =
                        toml::from_str::<crate::store::generation::GenerationManifest>(&content)
                    {
                        if manifest
                            .packages
                            .iter()
                            .any(|p| p.starts_with(package_name))
                        {
                            if !dependents.contains(&format!("generation-{}", manifest.id)) {
                                dependents.push(format!("generation-{}", manifest.id));
                            }
                        }
                    }
                }
            }
        }

        Ok(dependents)
    }
}
