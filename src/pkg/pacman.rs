use super::{
    BuildType, InstalledPackage, PackageConfig, PackageInfo, PackageManager, PackageSource,
    PackageType,
};
use crate::pkg::privilege::PrivilegeManager;
use crate::store::package::FileEntry;
use crate::store::{ContentStore, PackageStore};
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
const MIRROR_URL: &str = "https://geo.mirror.pkgbuild.com";
const REPOS: &[&str] = &["core", "extra", "multilib"];
const ARCH: &str = "x86_64";

#[derive(Debug, Clone)]
pub struct Pacman {
    isolated_root: Option<PathBuf>,
    isolated_db_path: Option<PathBuf>,
    cache_dir: PathBuf,
    archive_cache_dir: PathBuf,
    repo_db_cache_dir: PathBuf,
    privilege: PrivilegeManager,
    store_root: PathBuf,
    package_store: PackageStore,
    content_store: ContentStore,
}

impl Pacman {
    pub fn new(store_root: PathBuf) -> Self {
        let isolated_root = store_root.join("tmp/pacman");
        let isolated_db_path = isolated_root.join("var/lib/pacman");
        let cache_dir = isolated_root.join("var/cache/pacman/pkg");
        let archive_cache_dir = store_root.join("cache/archive");
        let repo_db_cache_dir = store_root.join("cache/repo");

        fs::create_dir_all(&isolated_db_path).ok();
        fs::create_dir_all(&cache_dir).ok();
        fs::create_dir_all(&archive_cache_dir).ok();
        fs::create_dir_all(&repo_db_cache_dir).ok();

        let package_store = PackageStore::new(store_root.join("packages")).unwrap();
        let content_store = ContentStore::new(store_root.join("content")).unwrap();

        Self {
            isolated_root: Some(isolated_root.clone()),
            isolated_db_path: Some(isolated_db_path),
            cache_dir,
            archive_cache_dir,
            repo_db_cache_dir,
            privilege: PrivilegeManager::new(),
            store_root,
            package_store,
            content_store,
        }
    }

    pub fn system(store_root: PathBuf) -> Self {
        let archive_cache_dir = store_root.join("cache/archive");
        let repo_db_cache_dir = store_root.join("cache/repo");
        fs::create_dir_all(&archive_cache_dir).ok();
        fs::create_dir_all(&repo_db_cache_dir).ok();

        let package_store = PackageStore::new(store_root.join("packages")).unwrap();
        let content_store = ContentStore::new(store_root.join("content")).unwrap();

        Self {
            isolated_root: None,
            isolated_db_path: None,
            cache_dir: PathBuf::from("/var/cache/pacman/pkg"),
            archive_cache_dir,
            repo_db_cache_dir,
            privilege: PrivilegeManager::new(),
            store_root,
            package_store,
            content_store,
        }
    }

    pub fn package_store(&self) -> &PackageStore {
        &self.package_store
    }

    pub fn content_store(&self) -> &ContentStore {
        &self.content_store
    }

    fn download_repo_db(&self, repo: &str) -> Result<PathBuf> {
        let db_filename = format!("{}.db", repo);
        let db_path = self.repo_db_cache_dir.join(&db_filename);

        if db_path.exists() {
            if let Ok(metadata) = fs::metadata(&db_path) {
                if let Ok(modified) = metadata.modified() {
                    if modified.elapsed().unwrap_or_default().as_secs() < 3600 {
                        return Ok(db_path);
                    }
                }
            }
        }

        let url = format!("{}/{}/os/{}/{}", MIRROR_URL, repo, ARCH, db_filename);
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        let response = client.get(&url).send()?;
        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to download repository database for {}: {}",
                repo,
                response.status()
            ));
        }

        let temp_path = db_path.with_extension("tmp");
        let mut file = File::create(&temp_path)?;
        let content = response.bytes()?;
        file.write_all(&content)?;

        fs::rename(&temp_path, &db_path)?;
        Ok(db_path)
    }

    fn parse_repo_db(&self, db_path: &Path, package_name: &str) -> Result<Option<PackageInfo>> {
        let mut raw_content = Vec::new();
        File::open(db_path)?.read_to_end(&mut raw_content)?;

        let decompressed: Box<dyn Read> = if db_path.to_string_lossy().ends_with(".zst") {
            let decoder = zstd::stream::Decoder::new(raw_content.as_slice())?;
            Box::new(std::io::BufReader::new(decoder))
        } else if db_path.to_string_lossy().ends_with(".gz") {
            let decoder = flate2::read::GzDecoder::new(raw_content.as_slice());
            Box::new(decoder)
        } else {
            // gzip
            if raw_content.len() >= 2 && raw_content[0] == 0x1f && raw_content[1] == 0x8b {
                let decoder = flate2::read::GzDecoder::new(raw_content.as_slice());
                Box::new(decoder)
            } else {
                // zstd
                if raw_content.len() >= 4
                    && raw_content[0] == 0xFD
                    && raw_content[1] == 0x2F
                    && raw_content[2] == 0xB5
                    && raw_content[3] == 0x28
                {
                    let decoder = zstd::stream::Decoder::new(raw_content.as_slice())?;
                    Box::new(std::io::BufReader::new(decoder))
                } else {
                    // tar
                    Box::new(std::io::Cursor::new(raw_content))
                }
            }
        };

        let mut archive = tar::Archive::new(decompressed);
        let entries = archive.entries()?;

        for entry in entries {
            let mut entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    continue;
                }
            };
            let path_bytes = entry.path_bytes();
            let path_str = String::from_utf8_lossy(&path_bytes);

            if path_str.ends_with("/desc") {
                if let Some(first_segment) = path_str.split('/').next() {
                    if first_segment.starts_with(&format!("{}-", package_name))
                        || first_segment == package_name
                    {
                        let mut content = Vec::new();
                        entry.read_to_end(&mut content)?;
                        let content_str = String::from_utf8_lossy(&content);
                        return self.parse_desc_file(&content_str, package_name);
                    }
                }
            }
        }

        Ok(None)
    }

    fn parse_desc_file(&self, content: &str, package_name: &str) -> Result<Option<PackageInfo>> {
        let mut info_map: HashMap<String, String> = HashMap::new();
        let mut current_key = String::new();
        let mut current_value = String::new();

        for line in content.lines() {
            if line.starts_with('%') && line.ends_with('%') {
                if !current_key.is_empty() {
                    info_map.insert(current_key.clone(), current_value.trim().to_string());
                }
                current_key = line.trim_matches('%').to_string();
                current_value.clear();
            } else if !current_key.is_empty() {
                if !current_value.is_empty() {
                    current_value.push('\n');
                }
                current_value.push_str(line);
            }
        }

        if !current_key.is_empty() {
            info_map.insert(current_key, current_value.trim().to_string());
        }

        if info_map.is_empty() {
            return Ok(None);
        }

        let version = info_map.get("VERSION").cloned().unwrap_or_default();
        let (ver, rel) = if let Some(pos) = version.rfind('-') {
            (version[..pos].to_string(), version[pos + 1..].to_string())
        } else {
            (version, "1".to_string())
        };

        let dependencies: Vec<String> = info_map
            .get("DEPENDS")
            .map(|s| {
                s.lines()
                    .filter(|line| !line.contains(".so"))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let provides: Vec<String> = info_map
            .get("PROVIDES")
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default();

        let optional_deps: Vec<String> = info_map
            .get("OPTDEPENDS")
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default();

        let size: u64 = info_map
            .get("SIZE")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Ok(Some(PackageInfo {
            name: package_name.to_string(),
            version: ver,
            release: rel,
            description: info_map.get("DESC").cloned(),
            dependencies,
            optional_deps,
            provides,
            size,
            installed: self.exists_in_db(package_name),
            ty: PackageType::Pacman,
            build_date: None,
            source: PackageSource::Repository("core".to_string()),
        }))
    }

    fn get_package_info_from_repo_db(&self, name: &str) -> Result<Option<PackageInfo>> {
        for repo in REPOS {
            let db_path = self.download_repo_db(repo)?;
            if let Some(info) = self.parse_repo_db(&db_path, name)? {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    pub fn find_package_by_provides(&self, virtual_name: &str) -> Result<Option<PackageInfo>> {
        for repo in REPOS {
            let db_path = self.download_repo_db(repo)?;
            if let Some(info) = self.find_package_in_db_by_provides(&db_path, virtual_name)? {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    fn find_package_in_db_by_provides(
        &self,
        db_path: &Path,
        virtual_name: &str,
    ) -> Result<Option<PackageInfo>> {
        let mut raw_content = Vec::new();
        File::open(db_path)?.read_to_end(&mut raw_content)?;

        let decompressed: Box<dyn Read> = if db_path.to_string_lossy().ends_with(".zst") {
            let decoder = zstd::stream::Decoder::new(raw_content.as_slice())?;
            Box::new(std::io::BufReader::new(decoder))
        } else if db_path.to_string_lossy().ends_with(".gz") {
            let decoder = flate2::read::GzDecoder::new(raw_content.as_slice());
            Box::new(decoder)
        } else {
            if raw_content.len() >= 2 && raw_content[0] == 0x1f && raw_content[1] == 0x8b {
                let decoder = flate2::read::GzDecoder::new(raw_content.as_slice());
                Box::new(decoder)
            } else {
                if raw_content.len() >= 4
                    && raw_content[0] == 0xFD
                    && raw_content[1] == 0x2F
                    && raw_content[2] == 0xB5
                    && raw_content[3] == 0x28
                {
                    let decoder = zstd::stream::Decoder::new(raw_content.as_slice())?;
                    Box::new(std::io::BufReader::new(decoder))
                } else {
                    Box::new(std::io::Cursor::new(raw_content))
                }
            }
        };

        let mut archive = tar::Archive::new(decompressed);
        let entries = archive.entries()?;

        for entry in entries {
            let mut entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path_str = entry.path_bytes().to_vec();
            let path_str = String::from_utf8_lossy(&path_str);

            if path_str.ends_with("/desc") {
                let mut content = Vec::new();
                entry.read_to_end(&mut content)?;
                let content_str = String::from_utf8_lossy(&content);

                let mut info_map: HashMap<String, String> = HashMap::new();
                let mut current_key = String::new();
                let mut current_value = String::new();

                for line in content_str.lines() {
                    if line.starts_with('%') && line.ends_with('%') {
                        if !current_key.is_empty() {
                            info_map.insert(current_key.clone(), current_value.trim().to_string());
                        }
                        current_key = line.trim_matches('%').to_string();
                        current_value.clear();
                    } else if !current_key.is_empty() {
                        if !current_value.is_empty() {
                            current_value.push('\n');
                        }
                        current_value.push_str(line);
                    }
                }
                if !current_key.is_empty() {
                    info_map.insert(current_key, current_value.trim().to_string());
                }

                if let Some(provides_str) = info_map.get("PROVIDES") {
                    for prov in provides_str.lines() {
                        let prov_clean: String = prov
                            .split(|c: char| c == '<' || c == '>' || c == '=' || c == ' ')
                            .next()
                            .unwrap_or(prov)
                            .trim()
                            .to_string();
                        if prov_clean == virtual_name {
                            let actual_name = info_map
                                .get("NAME")
                                .map(|s| s.trim().to_string())
                                .unwrap_or_else(|| {
                                    let path_str_owned = path_str.to_string();
                                    if let Some(pkg_dir) = path_str_owned.split('/').next() {
                                        pkg_dir
                                            .rsplitn(3, '-')
                                            .last()
                                            .unwrap_or(pkg_dir)
                                            .to_string()
                                    } else {
                                        String::new()
                                    }
                                });
                            return self.parse_desc_file(&content_str, &actual_name);
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    pub fn sync_database(&self) -> Result<()> {
        for repo in REPOS {
            self.download_repo_db(repo)?;
        }
        Ok(())
    }

    pub fn package_info_from_system(&self, name: &str) -> Result<Option<PackageInfo>> {
        if let Some(pkg) = self.package_store.get(name)? {
            Ok(Some(PackageInfo {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                release: pkg.release.clone(),
                description: None,
                dependencies: pkg.dependencies.clone(),
                optional_deps: vec![],
                provides: vec![],
                size: 0,
                installed: true,
                ty: PackageType::Pacman,
                build_date: None,
                source: PackageSource::Repository("local".to_string()),
            }))
        } else {
            Ok(None)
        }
    }

    pub fn package_info_from_sync_db(
        &self,
        name: &str,
        _version: Option<&str>,
    ) -> Result<Option<PackageInfo>> {
        self.get_package_info_from_repo_db(name)
    }

    pub fn list_installed_packages(&self) -> Result<Vec<InstalledPackage>> {
        let generations_dir = self.store_root.join("generations");
        if !generations_dir.exists() {
            return Ok(vec![]);
        }

        let mut packages = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for entry in fs::read_dir(&generations_dir)? {
            let entry = entry?;
            let gen_path = entry.path();
            let manifest_path = gen_path.join("manifest.toml");
            if manifest_path.exists() {
                let content = fs::read_to_string(&manifest_path)?;
                if let Ok(manifest) =
                    toml::from_str::<crate::store::generation::GenerationManifest>(&content)
                {
                    for package_name in &manifest.packages {
                        if seen.insert(package_name.clone()) {
                            if let Some(pkg) = self.package_store.get(package_name)? {
                                let files: Vec<String> =
                                    pkg.files.iter().map(|f| f.path.clone()).collect();
                                packages.push(InstalledPackage {
                                    name: pkg.name.clone(),
                                    version: pkg.version.clone(),
                                    release: pkg.release.clone(),
                                    install_time: pkg.install_time,
                                    description: None,
                                    dependencies: pkg.dependencies.clone(),
                                    install_root: self
                                        .isolated_root
                                        .clone()
                                        .unwrap_or_else(|| PathBuf::from("/")),
                                    files,
                                    ty: PackageType::Pacman,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(packages)
    }

    pub fn list_package_files(&self, name: &str) -> Result<Vec<String>> {
        if let Some(pkg) = self.package_store.get(name)? {
            Ok(pkg.files.iter().map(|f| f.path.clone()).collect())
        } else {
            Ok(vec![])
        }
    }

    pub fn scan_package_files(&self, pkg_name: &str, root: &Path) -> Result<Vec<FileEntry>> {
        let files = self.list_package_files(pkg_name)?;
        let mut entries = Vec::new();

        for file_path in files {
            let full_path = root.join(&file_path);

            let metadata = fs::symlink_metadata(&full_path)?;

            if !metadata.is_file() && !metadata.is_symlink() {
                continue;
            }

            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&full_path)?.to_string_lossy().to_string();
                let target_hash = self.compute_hash_for_path(&full_path)?;

                entries.push(FileEntry {
                    path: file_path,
                    hash: target_hash,
                    size: 0,
                    mode: 0o120000,
                    symlink_target: Some(target),
                    source_path: Some(full_path.to_string_lossy().to_string()),
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
                    source_path: Some(full_path.to_string_lossy().to_string()),
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

    pub fn exists_in_system(&self, name: &str) -> bool {
        self.package_store
            .get(name)
            .map(|p| p.is_some())
            .unwrap_or(false)
    }

    pub fn exists_in_db(&self, name: &str) -> bool {
        self.exists_in_system(name)
    }

    pub fn search_packages(&self, query: &str) -> Result<Vec<PackageInfo>> {
        let packages = self.package_store.list_all()?;
        let mut results = Vec::new();

        for pkg in packages {
            if pkg.name.contains(query) {
                results.push(PackageInfo {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    release: pkg.release.clone(),
                    description: None,
                    dependencies: pkg.dependencies.clone(),
                    optional_deps: vec![],
                    provides: vec![],
                    size: 0,
                    installed: true,
                    ty: PackageType::Pacman,
                    build_date: None,
                    source: PackageSource::Repository("local".to_string()),
                });
            }
        }

        Ok(results)
    }

    pub fn get_package_size(&self, name: &str) -> Result<u64> {
        if let Some(info) = self.get_package_info_from_repo_db(name)? {
            return Ok(info.size);
        }

        if let Some(pkg) = self.package_store.get(name)? {
            let total_size: u64 = pkg.files.iter().map(|f| f.size).sum();
            return Ok(total_size);
        }

        Ok(0)
    }
}

impl PackageManager for Pacman {
    fn is_available_in_store(&self, package: &PackageConfig) -> bool {
        self.package_store
            .get(&package.name)
            .map(|p| p.is_some())
            .unwrap_or(false)
    }

    fn ensure_in_store(&self, package: &PackageConfig) -> Result<PackageInfo> {
        self.install(package, false)
    }

    fn install(&self, package: &PackageConfig, force: bool) -> Result<PackageInfo> {
        let mut package_with_version = package.clone();
        if package_with_version.version.is_none() {
            if let Some(info) = self.package_info_from_sync_db(&package.name, None)? {
                package_with_version.version = Some(format!("{}-{}", info.version, info.release));
            } else {
                return Err(anyhow::anyhow!(
                    "Package {} not found in repository and no version specified",
                    package.name
                ));
            }
        }

        self.install_from_archive_to_store(&package_with_version, force)
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

        let pkg = match self.package_store.get(package_name)? {
            Some(p) => p,
            None => {
                return Err(anyhow::anyhow!(
                    "Package {} not found in store",
                    package_name
                ));
            }
        };

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

        for file in &pkg.files {
            space_freed += file.size;
            files_removed += 1;
        }

        let full_name = format!("{}-{}-{}", pkg.name, pkg.version, pkg.release);
        removed_versions.push(full_name.clone());

        let pkg_path = self
            .store_root
            .join("packages")
            .join(format!("{}.json", full_name));
        if pkg_path.exists() {
            fs::remove_file(pkg_path)?;
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

fn decode_url_encoded(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next();
            let h2 = chars.next();

            if let (Some(h1), Some(h2)) = (h1, h2) {
                let hex_str: String = vec![h1, h2].iter().collect();
                if let Ok(val) = u8::from_str_radix(&hex_str, 16) {
                    result.push(val as char);
                } else {
                    result.push('%');
                    result.push(h1);
                    result.push(h2);
                }
            } else {
                result.push('%');
                if let Some(h) = h1 {
                    result.push(h);
                }
                if let Some(h) = h2 {
                    result.push(h);
                }
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }

    result
}

impl Pacman {
    pub fn install_from_archive_to_store(
        &self,
        package: &PackageConfig,
        force: bool,
    ) -> Result<PackageInfo> {
        let name = &package.name;
        let version = package.version.as_deref().unwrap_or("");

        if version.is_empty() {
            return Err(anyhow::anyhow!(
                "Version is required for archive installation"
            ));
        }

        let pkg_info = self.query_package_info_from_archive(name, version)?;

        if !force && self.is_available_in_store(package) {
            if let Ok(Some(pkg)) = self.package_store.get(name) {
                return Ok(PackageInfo {
                    name: name.to_string(),
                    version: pkg.version,
                    release: pkg.release,
                    description: pkg_info.description,
                    dependencies: pkg_info.dependencies,
                    optional_deps: pkg_info.optional_deps,
                    provides: pkg_info.provides,
                    size: pkg_info.size,
                    installed: true,
                    ty: PackageType::Pacman,
                    build_date: pkg_info.build_date,
                    source: PackageSource::Repository("archive".to_string()),
                });
            }
        }

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
        let mut available_packages: Vec<(String, String)> = Vec::new();
        for line in body.lines() {
            if line.contains(".pkg.tar.zst") || line.contains(".pkg.tar.xz") {
                if let Some(start) = line.find("href=\"") {
                    let start = start + 6;
                    if let Some(end) = line[start..].find("\"") {
                        let filename_encoded = &line[start..start + end];
                        if !filename_encoded.ends_with(".sig") {
                            let filename_decoded = decode_url_encoded(filename_encoded);
                            if filename_decoded.contains(name) {
                                available_packages
                                    .push((filename_decoded, filename_encoded.to_string()));
                            }
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
        let (package_filename, package_filename_encoded) = available_packages
            .iter()
            .find(|(decoded, _)| {
                decoded.starts_with(&target_prefix)
                    && (decoded.ends_with(".pkg.tar.zst") || decoded.ends_with(".pkg.tar.xz"))
            })
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Version {} of package {} not found in Arch Linux Archive",
                    version,
                    name
                )
            })?;

        let pkg_url = format!("{}/{}", archive_path, package_filename_encoded);
        let cached_pkg_path = self.archive_cache_dir.join(&package_filename);

        let pkg_path = if cached_pkg_path.exists() {
            println!("Using cached {}-{}", name, version);
            cached_pkg_path.clone()
        } else {
            println!("Downloading {} from archive...", pkg_url);
            let mut response = client.get(&pkg_url).send()?;
            if !response.status().is_success() {
                return Err(anyhow::anyhow!(
                    "Failed to download package {} from archive",
                    name
                ));
            }

            let total_size = response.content_length().unwrap_or(0);
            let mut file = File::create(&cached_pkg_path)?;
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
            cached_pkg_path.clone()
        };

        let (ver, rel) = if let Some(pos) = version.rfind('-') {
            (version[..pos].to_string(), version[pos + 1..].to_string())
        } else {
            (version.to_string(), "1".to_string())
        };

        let temp_dir = tempfile::tempdir()?;
        let temp_extract_dir = temp_dir.path();

        let file = File::open(&pkg_path)?;
        let decompressed: Box<dyn Read> = if pkg_path.to_string_lossy().ends_with(".zst") {
            let decoder = zstd::stream::Decoder::new(file)?;
            Box::new(std::io::BufReader::new(decoder))
        } else if pkg_path.to_string_lossy().ends_with(".xz") {
            let decoder = xz2::read::XzDecoder::new(file);
            Box::new(std::io::BufReader::new(decoder))
        } else {
            Box::new(file)
        };

        let mut archive = tar::Archive::new(decompressed);
        let mut files = Vec::new();

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();

            if path.ends_with(".pkginfo") || path == ".PKGINFO" {
                continue;
            }

            let entry_type = entry.header().entry_type();
            if entry_type == tar::EntryType::Link {
                continue;
            }

            let full_path = temp_extract_dir.join(&path);

            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }

            entry.unpack(&full_path)?;

            if let Ok(metadata) = fs::symlink_metadata(&full_path) {
                if metadata.is_dir() {
                    continue;
                }
                let hash = self.content_store.add_file(&full_path)?;
                let mode = metadata.permissions().mode() & 0o7777;
                let symlink_target = if metadata.file_type().is_symlink() {
                    fs::read_link(&full_path)
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                };
                let source = self.content_store.content_path(&hash);

                files.push(FileEntry {
                    path,
                    hash,
                    size: metadata.len(),
                    mode,
                    symlink_target,
                    source_path: Some(source.to_string_lossy().to_string()),
                });
            }
        }

        let pkg = crate::store::Package {
            name: name.to_string(),
            version: ver.clone(),
            release: rel.clone(),
            files,
            dependencies: pkg_info.dependencies.clone(),
            install_time: SystemTime::now(),
        };

        self.package_store.save(&pkg)?;

        Ok(PackageInfo {
            name: name.to_string(),
            version: ver,
            release: rel,
            description: pkg_info.description,
            dependencies: pkg_info.dependencies,
            optional_deps: pkg_info.optional_deps,
            provides: pkg_info.provides,
            size: pkg_info.size,
            installed: true,
            ty: PackageType::Pacman,
            build_date: pkg_info.build_date,
            source: PackageSource::Repository("archive".to_string()),
        })
    }

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
        let mut available_packages: Vec<(String, String)> = Vec::new();
        for line in body.lines() {
            if line.contains(".pkg.tar.zst") || line.contains(".pkg.tar.xz") {
                if let Some(start) = line.find("href=\"") {
                    let start = start + 6;
                    if let Some(end) = line[start..].find("\"") {
                        let filename_encoded = &line[start..start + end];
                        if !filename_encoded.ends_with(".sig") {
                            let filename_decoded = decode_url_encoded(filename_encoded);
                            if filename_decoded.contains(name) {
                                available_packages
                                    .push((filename_decoded, filename_encoded.to_string()));
                            }
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
        let (package_filename, package_filename_encoded) = available_packages
            .iter()
            .find(|(decoded, _)| {
                decoded.starts_with(&target_prefix)
                    && (decoded.ends_with(".pkg.tar.zst") || decoded.ends_with(".pkg.tar.xz"))
            })
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Version {} of package {} not found in Arch Linux Archive",
                    version,
                    name
                )
            })?;

        let pkg_url = format!("{}/{}", archive_path, package_filename_encoded);

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
            let decoder = zstd::stream::Decoder::new(file)?;
            Box::new(std::io::BufReader::new(decoder))
        } else if pkg_path.to_string_lossy().ends_with(".xz") {
            let decoder = xz2::read::XzDecoder::new(file);
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
                    if !dep.is_empty() && !dep.contains(".so") {
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
            provides,
            size: file_size,
            installed: false,
            ty: PackageType::Pacman,
            build_date: None,
            source: PackageSource::Repository("archive".to_string()),
        })
    }

    fn check_dependencies(&self, package_name: &str) -> Result<Vec<String>> {
        let mut dependents = Vec::new();

        let packages = self.package_store.list_all()?;
        for pkg in packages {
            if pkg.dependencies.contains(&package_name.to_string()) {
                if !dependents.contains(&pkg.name) {
                    dependents.push(pkg.name.clone());
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
