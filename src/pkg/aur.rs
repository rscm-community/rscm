use super::{
    pacman::Pacman, BuildType, InstalledPackage, PackageConfig, PackageInfo, PackageManager,
    PackageSource, PackageType, SandboxConfig,
};
use crate::store::package::FileEntry;
use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;
use which::which;

pub const AUR_DB_URL: &str = "https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD";
pub const AUR_RPC_URL: &str = "https://aur.archlinux.org/rpc.php";

#[derive(Debug, Clone)]
pub struct PkgBuildInfo {
    pub name: String,
    pub version: String,
    pub release: String,
    pub description: Option<String>,
    pub depends: Vec<String>,
    pub makedepends: Vec<String>,
    pub source: Vec<(String, Option<String>)>,
    pub md5sums: Vec<Option<String>>,
}

impl PkgBuildInfo {
    fn parse(content: &str, name: &str) -> Option<Self> {
        let mut pkgname = name.to_string();
        let mut pkgver = String::new();
        let mut pkgrel = String::new();
        let mut description = None;
        let mut depends = Vec::new();
        let mut makedepends = Vec::new();
        let mut source = Vec::new();
        let mut md5sums = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("pkgname=") {
                pkgname = line
                    .trim_start_matches("pkgname=")
                    .trim()
                    .trim_matches('\'')
                    .to_string();
            } else if line.starts_with("pkgver=") {
                pkgver = line
                    .trim_start_matches("pkgver=")
                    .trim()
                    .trim_matches('\'')
                    .to_string();
            } else if line.starts_with("pkgrel=") {
                pkgrel = line
                    .trim_start_matches("pkgrel=")
                    .trim()
                    .trim_matches('\'')
                    .to_string();
            } else if line.starts_with("pkgdesc=") {
                let desc = line.trim_start_matches("pkgdesc=").trim();
                if !desc.starts_with('$') {
                    description = Some(desc.trim_matches('"').trim_matches('\'').to_string());
                }
            } else if line.starts_with("depends=") {
                let deps = line.trim_start_matches("depends=").trim();
                depends = Self::parse_array(deps);
            } else if line.starts_with("makedepends=") {
                let deps = line.trim_start_matches("makedepends=").trim();
                makedepends = Self::parse_array(deps);
            } else if line.starts_with("source=") {
                let src = line.trim_start_matches("source=").trim();
                source = Self::parse_array(src)
                    .into_iter()
                    .map(|s| {
                        let s = Self::expand_variables(&s, &pkgname, &pkgver);
                        if s.starts_with("git+") {
                            let url = s.strip_prefix("git+").unwrap_or(&s).to_string();
                            (url, Some("git".to_string()))
                        } else {
                            (s, None)
                        }
                    })
                    .collect();
            } else if line.starts_with("md5sums=") {
                let sums = line.trim_start_matches("md5sums=").trim();
                md5sums = Self::parse_array(sums)
                    .into_iter()
                    .map(|s| if s.is_empty() { None } else { Some(s) })
                    .collect();
            }
        }

        if pkgver.is_empty() {
            return None;
        }

        Some(Self {
            name: pkgname,
            version: pkgver,
            release: pkgrel,
            description,
            depends,
            makedepends,
            source,
            md5sums,
        })
    }

    fn parse_array(arr: &str) -> Vec<String> {
        let arr = arr.trim_start_matches('(').trim_end_matches(')');
        arr.split_whitespace()
            .map(|s| s.trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn expand_variables(s: &str, pkgname: &str, pkgver: &str) -> String {
        s.replace("$pkgname", pkgname)
            .replace("${pkgname}", pkgname)
            .replace("$pkgver", pkgver)
            .replace("${pkgver}", pkgver)
            .replace("$url", "")
            .replace("${url}", "")
    }
}

#[derive(Debug, Clone)]
pub struct AurHelper {
    build_dir: PathBuf,
    pkg_dest: PathBuf,
    cache_dir: PathBuf,
    store_root: PathBuf,
    makedepends_root: PathBuf,
    pacman: Pacman,
}

impl AurHelper {
    pub fn new(build_dir: PathBuf, pkg_dest: PathBuf, store_root: PathBuf) -> Self {
        let cache_dir = store_root.join("cache/aur");
        let makedepends_root = store_root.join("tmp/makedepends");
        let _ = fs::create_dir_all(&cache_dir);
        let _ = fs::create_dir_all(&makedepends_root);

        let pacman = Pacman::system(store_root.clone());

        Self {
            build_dir,
            pkg_dest,
            cache_dir,
            store_root,
            makedepends_root,
            pacman,
        }
    }

    pub fn detect(store_root: PathBuf) -> Option<Self> {
        let build_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("rscm")
            .join("aur-build");

        let pkg_dest = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("rscm")
            .join("aur-packages");

        Some(Self::new(build_dir, pkg_dest, store_root))
    }

    pub fn build_dir(&self) -> &PathBuf {
        &self.build_dir
    }

    pub fn pkg_dest(&self) -> &PathBuf {
        &self.pkg_dest
    }

    pub fn exists_in_store(&self, name: &str) -> bool {
        let packages_dir = self.store_root.join("packages");
        let pattern = format!("{}-*.toml", name);
        if let Ok(paths) = glob::glob(packages_dir.join(&pattern).to_str().unwrap()) {
            for entry in paths.flatten() {
                if entry.exists() {
                    return true;
                }
            }
        }
        false
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
            installed: self.exists_in_store(name),
            ty: PackageType::Aur,
            build_date: None,
            source: PackageSource::Aur,
        }))
    }

    fn get_specific_version(&self, name: &str, version: &str) -> Result<Option<PackageInfo>> {
        let repo_url = format!("https://aur.archlinux.org/{}.git", name);
        let cache_dir = self.cache_dir.join(name);

        let clone_dir = if cache_dir.exists() {
            println!("Using cached AUR repository for {}", name);
            cache_dir
        } else {
            println!(
                "Cloning AUR repository for {} to fetch version {}...",
                name, version
            );
            fs::create_dir_all(&self.cache_dir)?;
            let status = Command::new("git")
                .args(["clone", &repo_url, cache_dir.to_str().unwrap()])
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .context("Failed to clone AUR repository")?;

            if !status.success() {
                return Err(anyhow!("Failed to clone AUR repository for {}", name));
            }
            cache_dir
        };
        let tags_output = Command::new("git")
            .current_dir(&clone_dir)
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
                        let pkgbuild = self.fetch_pkgbuild_at_tag(&clone_dir, tag)?;
                        let info = self.parse_pkgbuild(&pkgbuild, name)?;
                        return Ok(info.map(|i| self.pkgbuild_to_package_info(&i)));
                    }
                }

                for line in tags_content.lines() {
                    let tag = line.trim();
                    if tag.contains(version) {
                        let pkgbuild = self.fetch_pkgbuild_at_tag(&clone_dir, tag)?;
                        let info = self.parse_pkgbuild(&pkgbuild, name)?;
                        return Ok(info.map(|i| self.pkgbuild_to_package_info(&i)));
                    }
                }
            }
        }

        if let Some(commit_pkgbuild) = self.find_commit_with_version(&clone_dir, name, version)? {
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
                let info = self.parse_pkgbuild(&pkgbuild, name)?;
                if let Some(info) = info {
                    if info.version == version {
                        return Ok(Some(self.pkgbuild_to_package_info(&info)));
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
                    let info = self.parse_pkgbuild(&pkgbuild, name)?;
                    if let Some(info) = info {
                        if info.version == version {
                            return Ok(Some(self.pkgbuild_to_package_info(&info)));
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

    fn parse_pkgbuild(&self, content: &str, name: &str) -> Result<Option<PkgBuildInfo>> {
        Ok(PkgBuildInfo::parse(content, name))
    }

    fn pkgbuild_to_package_info(&self, info: &PkgBuildInfo) -> PackageInfo {
        let mut all_deps = info.depends.clone();
        all_deps.extend(info.makedepends.clone());

        PackageInfo {
            name: info.name.clone(),
            version: info.version.clone(),
            release: info.release.clone(),
            description: info.description.clone(),
            dependencies: all_deps,
            optional_deps: vec![],
            size: 0,
            installed: self.exists_in_store(&info.name),
            ty: PackageType::Aur,
            build_date: None,
            source: PackageSource::Aur,
        }
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

        println!("Cloning AUR repository for {}...", name);
        let status = Command::new("git")
            .args(["clone", &format!("https://aur.archlinux.org/{}.git", name)])
            .current_dir(&self.build_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("Failed to clone AUR repository")?;

        if !status.success() {
            return Err(anyhow!("Failed to clone AUR package {}", name));
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

        let pkgbuild_script = self.generate_pkgbuild_script(pkg_dir)?;

        bwrap_cmd
            .arg("--tmpfs")
            .arg("/tmp")
            .arg("--tmpfs")
            .arg("/build")
            .arg("--chdir")
            .arg("/build")
            .arg("/bin/bash")
            .arg("-c")
            .arg(&pkgbuild_script);

        let output = bwrap_cmd
            .current_dir(pkg_dir)
            .stderr(Stdio::inherit())
            .output()
            .context("Failed to build package in sandbox")?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to build package: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let pkg_file = self.find_built_package(pkg_dir)?;
        Ok(pkg_file)
    }

    pub fn build_package(
        &self,
        name: &str,
        sandbox_config: Option<&SandboxConfig>,
    ) -> Result<PathBuf> {
        let clone_dir = self.clone_aur_package(name)?;
        println!("Building AUR package {}...", name);

        let sandbox = sandbox_config.cloned().unwrap_or_else(|| SandboxConfig {
            network: false,
            ro_paths: vec![
                "/usr".to_string(),
                "/etc/pacman.conf".to_string(),
                "/var/cache/pacman/pkg".to_string(),
            ],
            rw_paths: vec![],
        });

        let pkgbuild_path = clone_dir.join("PKGBUILD");
        let content = fs::read_to_string(&pkgbuild_path).context("Failed to read PKGBUILD")?;
        let pkg_info = PkgBuildInfo::parse(&content, "unknown")
            .ok_or_else(|| anyhow!("Failed to parse PKGBUILD"))?;

        let installed_makedeps = self.ensure_makedepends(&pkg_info.makedepends)?;

        let result = self.build_direct(&clone_dir);

        if !installed_makedeps.is_empty() {
            self.cleanup_makedepends(&installed_makedeps)?;
        }

        let pkg_file = result?;
        println!("Successfully built AUR package {}", name);
        Ok(pkg_file)
    }

    fn ensure_makedepends(&self, makedepends: &[String]) -> Result<Vec<String>> {
        let mut installed = Vec::new();

        for dep in makedepends {
            let dep_name = Self::normalize_dep_name(dep);
            if !self.is_package_installed(&dep_name) {
                println!(
                    "Build dependency {} not installed, installing temporarily...",
                    dep_name
                );
                self.install_makedepend(&dep_name)?;
                installed.push(dep_name);
            } else {
                println!("Build dependency {} already installed, skipping", dep_name);
            }
        }

        Ok(installed)
    }

    fn is_package_installed(&self, name: &str) -> bool {
        self.pacman
            .package_info_from_system(name)
            .is_ok_and(|p| p.is_some())
            || self
                .pacman
                .package_info_from_sync_db(name, None)
                .is_ok_and(|p| p.is_some())
    }

    fn install_makedepend(&self, name: &str) -> Result<()> {
        fs::create_dir_all(&self.makedepends_root)?;

        let config = PackageConfig {
            name: name.to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        self.pacman.install(&config, false)?;

        println!("Successfully installed temporary build dependency {}", name);
        Ok(())
    }

    fn cleanup_makedepends(&self, installed: &[String]) -> Result<()> {
        if !self.makedepends_root.exists() {
            return Ok(());
        }

        println!("Cleaning up temporarily installed build dependencies...");
        for name in installed {
            let _ = self.pacman.remove(name, None, false);
        }
        println!("Cleaned up temporary build dependencies");

        Ok(())
    }

    fn normalize_dep_name(dep: &str) -> String {
        dep.split(|c: char| c == '<' || c == '>' || c == '=' || c == ' ')
            .next()
            .unwrap_or(dep)
            .trim()
            .to_string()
    }
    fn build_direct(&self, pkg_dir: &Path) -> Result<PathBuf> {
        let pkgbuild_path = pkg_dir.join("PKGBUILD");
        let content = fs::read_to_string(&pkgbuild_path).context("Failed to read PKGBUILD")?;

        let pkg_info = PkgBuildInfo::parse(&content, "unknown")
            .ok_or_else(|| anyhow!("Failed to parse PKGBUILD"))?;

        println!("Building {} v{}...", pkg_info.name, pkg_info.version);

        self.download_sources(pkg_dir, &pkg_info)?;

        let src_dir = pkg_dir.join("src");
        let _ = fs::remove_dir_all(&src_dir);
        fs::create_dir_all(&src_dir)?;

        self.extract_sources(&src_dir)?;

        self.run_pkgbuild_functions(pkg_dir)?;

        self.find_built_package(pkg_dir)
    }

    fn download_sources(&self, pkg_dir: &Path, info: &PkgBuildInfo) -> Result<()> {
        for (i, (url, _hash)) in info.source.iter().enumerate() {
            if url.is_empty() {
                continue;
            }

            let filename = url
                .split('/')
                .last()
                .unwrap_or(&format!("source_{}", i))
                .to_string();

            let dest = pkg_dir.join(&filename);

            if dest.exists() {
                println!("Source {} already exists, skipping", filename);
                continue;
            }

            println!("Downloading {}...", filename);

            if url.starts_with("http://") || url.starts_with("https://") {
                let output = Command::new("curl")
                    .args(["-L", "-o", dest.to_str().unwrap(), url])
                    .output()
                    .context(format!("Failed to download {}", url))?;

                if !output.status.success() {
                    return Err(anyhow!("Failed to download {}", url));
                }
            } else if !url.contains("://") {
                let src_path = pkg_dir.join(url);
                if src_path.exists() {
                    fs::copy(&src_path, &dest).context(format!("Failed to copy {}", url))?;
                }
            }
        }

        Ok(())
    }

    fn extract_sources(&self, src_dir: &Path) -> Result<()> {
        let entries = fs::read_dir(src_dir.parent().unwrap())?;

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if filename.ends_with(".tar.gz")
                || filename.ends_with(".tar.bz2")
                || filename.ends_with(".tar.xz")
                || filename.ends_with(".tar.zst")
                || filename.ends_with(".tgz")
                || filename.ends_with(".zip")
            {
                println!("Extracting {}...", filename);

                let output = if filename.ends_with(".zip") {
                    Command::new("unzip")
                        .arg("-o")
                        .arg(path.to_str().unwrap())
                        .arg("-d")
                        .arg(src_dir.to_str().unwrap())
                        .output()
                } else {
                    Command::new("tar")
                        .args([
                            "-xf",
                            path.to_str().unwrap(),
                            "-C",
                            src_dir.to_str().unwrap(),
                        ])
                        .output()
                };

                if let Ok(output) = output {
                    if !output.status.success() {
                        eprintln!("Warning: Failed to extract {}", filename);
                    }
                }
            }
        }

        Ok(())
    }

    fn generate_pkgbuild_script(&self, pkg_dir: &Path) -> Result<String> {
        let pkgbuild_path = pkg_dir.join("PKGBUILD");
        let content = fs::read_to_string(&pkgbuild_path).context("Failed to read PKGBUILD")?;

        let mut script = String::new();
        script.push_str("set -e\n");
        script.push_str("export PKGDEST=$(pwd)\n");
        script.push_str("export SRCDEST=$(pwd)/src\n");
        script.push_str("export LOGDEST=$(pwd)/logs\n");
        script.push_str("mkdir -p \"$SRCDEST\" \"$LOGDEST\"\n");

        script.push_str("source PKGBUILD\n");

        script.push_str("if declare -f prepare >/dev/null 2>&1; then\n");
        script.push_str("  echo 'Running prepare()...'\n");
        script.push_str("  prepare\n");
        script.push_str("fi\n");

        script.push_str("if declare -f build >/dev/null 2>&1; then\n");
        script.push_str("  echo 'Running build()...'\n");
        script.push_str("  build\n");
        script.push_str("fi\n");

        script.push_str("if declare -f package >/dev/null 2>&1; then\n");
        script.push_str("  echo 'Running package()...'\n");
        script.push_str("  export pkgdir=$(pwd)/pkg\n");
        script.push_str("  mkdir -p \"$pkgdir\"\n");
        script.push_str("  package\n");
        script.push_str("fi\n");

        script.push_str("echo 'Build completed successfully'\n");

        Ok(script)
    }

    fn run_pkgbuild_functions(&self, pkg_dir: &Path) -> Result<()> {
        let pkgbuild_path = pkg_dir.join("PKGBUILD");
        let content = fs::read_to_string(&pkgbuild_path).context("Failed to read PKGBUILD")?;

        let src_dir = pkg_dir.join("src");
        let build_dir = if src_dir.exists() {
            let entries: Vec<_> = fs::read_dir(&src_dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect();
            if entries.len() == 1 {
                entries[0].path()
            } else {
                src_dir.clone()
            }
        } else {
            pkg_dir.to_path_buf()
        };

        let mut script = String::new();
        script.push_str("set -e\n");
        script.push_str(&format!("cd {}\n", pkg_dir.display()));
        script.push_str("export PKGDEST=$(pwd)\n");
        script.push_str("export SRCDEST=$(pwd)/src\n");
        script.push_str("export LOGDEST=$(pwd)/logs\n");
        script.push_str("mkdir -p \"$SRCDEST\" \"$LOGDEST\"\n");

        script.push_str("source PKGBUILD\n");

        script.push_str("if declare -f prepare >/dev/null 2>&1; then\n");
        script.push_str("  echo 'Running prepare()...'\n");
        script.push_str(&format!("  cd {}\n", src_dir.display()));
        script.push_str("  prepare\n");
        script.push_str(&format!("  cd {}\n", pkg_dir.display()));
        script.push_str("fi\n");

        script.push_str("if declare -f build >/dev/null 2>&1; then\n");
        script.push_str("  echo 'Running build()...'\n");
        script.push_str(&format!("  cd {}\n", src_dir.display()));
        script.push_str("  build\n");
        script.push_str(&format!("  cd {}\n", pkg_dir.display()));
        script.push_str("fi\n");

        script.push_str("if declare -f package >/dev/null 2>&1; then\n");
        script.push_str("  echo 'Running package()...'\n");
        script.push_str("  export pkgdir=$(pwd)/pkg\n");
        script.push_str("  mkdir -p \"$pkgdir\"\n");
        script.push_str(&format!("  cd {}\n", src_dir.display()));
        script.push_str("  package\n");
        script.push_str("fi\n");

        script.push_str("echo 'Build completed successfully'\n");

        let output = Command::new("bash")
            .arg("-c")
            .arg(&script)
            .current_dir(pkg_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .context("Failed to execute PKGBUILD functions")?;

        if !output.status.success() {
            return Err(anyhow!(
                "PKGBUILD execution failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    fn find_built_package(&self, pkg_dir: &Path) -> Result<PathBuf> {
        let entries = fs::read_dir(pkg_dir)?;
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "zst" && path.to_string_lossy().contains(".pkg.tar.") {
                    return Ok(path);
                }
            }
        }

        let pkg_subdir = pkg_dir.join("pkg");
        if pkg_subdir.exists() {
            return Ok(pkg_subdir);
        }

        Err(anyhow!("No built package found in {}", pkg_dir.display()))
    }

    fn run_build(&self, src_dir: &Path, pkg_dir: &Path) -> Result<()> {
        let entries: Vec<_> = fs::read_dir(src_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        let build_dir = if entries.len() == 1 {
            entries[0].path()
        } else {
            src_dir.to_path_buf()
        };

        let configure_exists = build_dir.join("configure").exists();
        let cmakelists_exists = build_dir.join("CMakeLists.txt").exists();
        let makefile_exists = build_dir.join("Makefile").exists();
        let meson_build_exists = build_dir.join("meson.build").exists();

        let prefix = pkg_dir.join("pkg");
        let mut build_cmd = Command::new("bash");
        build_cmd.arg("-c");

        if configure_exists {
            build_cmd.arg(&format!(
                "cd {} && ./configure --prefix={} && make && make install",
                build_dir.display(),
                prefix.display()
            ));
        } else if cmakelists_exists {
            build_cmd.arg(&format!(
                "cd {} && cmake -B build -DCMAKE_INSTALL_PREFIX={} && cmake --build build && cmake --install build",
                build_dir.display(),
                prefix.display()
            ));
        } else if makefile_exists {
            build_cmd.arg(&format!(
                "cd {} && make && DESTDIR={} make install",
                build_dir.display(),
                prefix.display()
            ));
        } else if meson_build_exists {
            build_cmd.arg(&format!(
                "cd {} && meson setup build --prefix={} && meson compile -C build && meson install -C build",
                build_dir.display(),
                prefix.display()
            ));
        } else {
            return Err(anyhow!(
                "Cannot determine build system (no configure, CMakeLists.txt, Makefile, or meson.build found)"
            ));
        }

        build_cmd
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .current_dir(pkg_dir)
            .spawn()
            .context("Failed to start build")?
            .wait()
            .context("Build failed")?;

        Ok(())
    }

    fn extract_package_files(
        &self,
        pkg_file: &Path,
        store_pkg_dir: &Path,
    ) -> Result<Vec<FileEntry>> {
        let file = std::fs::File::open(pkg_file)?;

        let decompressed: Box<dyn Read> = if pkg_file.to_string_lossy().ends_with(".zst") {
            let mut decoder = zstd::stream::Decoder::new(file)?;
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

            let full_path = store_pkg_dir.join(&path);

            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }

            entry.unpack(&full_path)?;

            if let Ok(metadata) = fs::metadata(&full_path) {
                let hash = self.compute_hash_for_path(&full_path)?;
                let mode = metadata.permissions().mode() & 0o7777;
                let symlink_target = if metadata.file_type().is_symlink() {
                    fs::read_link(&full_path)
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                };

                files.push(FileEntry {
                    path,
                    hash,
                    size: metadata.len(),
                    mode,
                    symlink_target,
                    source_path: Some(full_path.to_string_lossy().to_string()),
                });
            }
        }

        Ok(files)
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

    fn check_dependencies(&self, package_name: &str) -> Result<Vec<String>> {
        let mut dependents = Vec::new();

        let packages_dir = self.store_root.join("packages");
        if packages_dir.exists() {
            for entry in fs::read_dir(&packages_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    let content = fs::read_to_string(&path)?;
                    if let Ok(pkg) = toml::from_str::<crate::store::Package>(&content) {
                        if pkg.dependencies.contains(&package_name.to_string()) {
                            if !dependents.contains(&pkg.name) {
                                dependents.push(pkg.name.clone());
                            }
                        }
                    }
                }
            }
        }

        Ok(dependents)
    }

    fn copy_directory_contents(&self, src_dir: &Path, dst_dir: &Path) -> Result<Vec<FileEntry>> {
        let mut files = Vec::new();
        self.copy_dir_recursive_with_entries(src_dir, dst_dir, &mut files, dst_dir)?;
        Ok(files)
    }

    fn copy_dir_recursive_with_entries(
        &self,
        src: &Path,
        dst: &Path,
        files: &mut Vec<FileEntry>,
        base_dir: &Path,
    ) -> Result<()> {
        fs::create_dir_all(dst)?;

        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if src_path.is_dir() {
                self.copy_dir_recursive_with_entries(&src_path, &dst_path, files, base_dir)?;
            } else {
                fs::copy(&src_path, &dst_path)?;

                if let Ok(metadata) = fs::metadata(&dst_path) {
                    let hash = self.compute_hash_for_path(&dst_path)?;
                    let mode = metadata.permissions().mode() & 0o7777;
                    let symlink_target = if metadata.file_type().is_symlink() {
                        fs::read_link(&dst_path)
                            .ok()
                            .map(|p| p.to_string_lossy().to_string())
                    } else {
                        None
                    };

                    let relative_path = dst_path
                        .strip_prefix(base_dir)
                        .unwrap_or(&dst_path)
                        .to_string_lossy()
                        .to_string();

                    files.push(FileEntry {
                        path: relative_path,
                        hash,
                        size: metadata.len(),
                        mode,
                        symlink_target,
                        source_path: Some(dst_path.to_string_lossy().to_string()),
                    });
                }
            }
        }

        Ok(())
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

    fn extract_package_files(
        &self,
        pkg_file: &Path,
        store_pkg_dir: &Path,
    ) -> Result<Vec<FileEntry>> {
        let file = std::fs::File::open(pkg_file)?;

        let decompressed: Box<dyn Read> = if pkg_file.to_string_lossy().ends_with(".zst") {
            let mut decoder = zstd::stream::Decoder::new(file)?;
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

            let full_path = store_pkg_dir.join(&path);

            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }

            entry.unpack(&full_path)?;

            if let Ok(metadata) = fs::metadata(&full_path) {
                let hash = self.compute_hash_for_path(&full_path)?;
                let mode = metadata.permissions().mode() & 0o7777;
                let symlink_target = if metadata.file_type().is_symlink() {
                    fs::read_link(&full_path)
                        .ok()
                        .map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                };

                files.push(FileEntry {
                    path,
                    hash,
                    size: metadata.len(),
                    mode,
                    symlink_target,
                    source_path: Some(full_path.to_string_lossy().to_string()),
                });
            }
        }

        Ok(files)
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
}

impl Default for Bubblewrap {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageManager for AurHelper {
    fn is_available_in_store(&self, package: &PackageConfig) -> bool {
        self.exists_in_store(&package.name)
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
        let build_output = self.build_package(&package.name, sandbox)?;

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

        let files =
            if build_output.is_file() && build_output.to_string_lossy().ends_with(".pkg.tar.zst") {
                self.extract_package_files(&build_output, &store_pkg_dir)?
            } else if build_output.is_dir() {
                self.copy_directory_contents(&build_output, &store_pkg_dir)?
            } else {
                return Err(anyhow!(
                    "Build output is neither a package file nor a directory: {}",
                    build_output.display()
                ));
            };

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

        Ok(info)
    }

    fn remove(
        &self,
        package_name: &str,
        version: Option<&str>,
        recursive: bool,
    ) -> Result<super::RemoveResult> {
        let packages_dir = self.store_root.join("packages");

        let mut found_packages = Vec::new();
        let pattern = match version {
            Some(v) => format!("{}-{}-*.toml", package_name, v),
            None => format!("{}-*.toml", package_name),
        };

        let glob_pattern = packages_dir.join(&pattern);
        for entry in glob::glob(glob_pattern.to_str().unwrap())? {
            let path = entry?;
            if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                found_packages.push((name.to_string(), path));
            }
        }

        if found_packages.is_empty() {
            return Err(anyhow!("Package {} not found in store", package_name));
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
            if let Ok(content) = fs::read_to_string(manifest_path) {
                if let Ok(pkg) = toml::from_str::<crate::store::Package>(&content) {
                    for file in &pkg.files {
                        space_freed += file.size;
                        files_removed += 1;
                    }
                }
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
        if let Some(ver) = version {
            println!("Fetching {} version {} from AUR...", name, ver);
        } else {
            println!("Fetching {} from AUR...", name);
        }
        self.get_aur_info(name, version)
    }

    fn list_installed(&self) -> Result<Vec<InstalledPackage>> {
        let mut installed = Vec::new();
        let packages_dir = self.store_root.join("packages");

        if packages_dir.exists() {
            for entry in fs::read_dir(&packages_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(pkg) = toml::from_str::<crate::store::Package>(&content) {
                            let name_parts: Vec<&str> = path
                                .file_stem()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .rsplit('-')
                                .collect();

                            let version = if name_parts.len() >= 2 {
                                name_parts[name_parts.len() - 1]
                            } else {
                                &pkg.version
                            };

                            installed.push(InstalledPackage {
                                name: pkg.name.clone(),
                                version: pkg.version.clone(),
                                release: pkg.release.clone(),
                                install_time: pkg.install_time,
                                description: None,
                                dependencies: pkg.dependencies.clone(),
                                install_root: PathBuf::from("/"),
                                files: pkg.files.iter().map(|f| f.path.clone()).collect(),
                                ty: PackageType::Aur,
                            });
                        }
                    }
                }
            }
        }

        Ok(installed)
    }

    fn build_type(&self) -> BuildType {
        BuildType::Aur
    }

    fn manager_name(&self) -> &'static str {
        "aur"
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
