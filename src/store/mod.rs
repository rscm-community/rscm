pub mod content;
pub mod generation;
pub mod package;
pub mod reference;

pub use content::ContentStore;
pub use generation::{Generation, GenerationStore};
pub use package::{FileEntry, Package, PackageStore};
pub use reference::{RefKind, ReferenceCounter};

use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Configuration;
use crate::lock::LockFile;
use crate::pkg::{BuildType, PackageConfig, PackageManagerFactory};
use crate::store::generation::GenerationManifest;
use crate::system_config::SystemConfigApplier;

#[derive(Debug, Default)]
pub struct GcResult {
    pub collected_contents: usize,
    pub collected_packages: usize,
    pub freed_space: u64,
}

pub struct Store {
    root: PathBuf,
    content: ContentStore,
    packages: PackageStore,
    generations: GenerationStore,
    reference: ReferenceCounter,
    pkg_factory: PackageManagerFactory,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        if !root.exists() {
            anyhow::bail!(
                "Store directory {} does not exist. Run 'rscm init' first to initialize storage.",
                root.display()
            );
        }
        let content = ContentStore::new(root.join("content"))?;
        let packages = PackageStore::new(root.join("packages"))?;
        let generations = GenerationStore::new(root.join("generations"))?;
        let reference = ReferenceCounter::new(root.join("references.json"))?;
        let pkg_factory = PackageManagerFactory::new(root.clone());

        Ok(Self {
            root,
            content,
            packages,
            generations,
            reference,
            pkg_factory,
        })
    }
    pub fn register_package(&mut self, pkg: Package) -> Result<()> {
        self.packages.save(&pkg)?;
        for file in &pkg.files {
            self.reference.add(&file.hash, RefKind::Content)?;
        }
        self.reference.add(&pkg.name, RefKind::Package)?;
        Ok(())
    }

    pub fn create_generation(
        &mut self,
        configuration: Configuration,
        lock: &LockFile,
    ) -> Result<u64> {
        let mut files = Vec::new();
        let mut package_names = Vec::new();

        for (name, pkg_versions) in &lock.packages {
            for (version_key, pkg_version) in &pkg_versions.versions {
                let full_name = format!("{}-{}", name, version_key);
                package_names.push(full_name.clone());

                if let Some(pkg) = self.packages.get(name)? {
                    for file in &pkg.files {
                        files.push(file.clone());
                    }
                } else {
                    println!("Installing package: {} ({})", name, version_key);
                    let is_aur = pkg_versions.source == "aur";
                    let build_type = if is_aur {
                        BuildType::Aur
                    } else {
                        BuildType::Pacman
                    };

                    let config = PackageConfig {
                        name: name.clone(),
                        version: Some(version_key.clone()),
                        build_type,
                        dependencies: pkg_version.dependencies.clone(),
                        sandbox_config: None,
                    };

                    let manager = self.pkg_factory.for_package(&config)?;
                    let info = manager.install(&config, false)?;

                    if let Some(pkg) = self.packages.get(name)? {
                        for file in &pkg.files {
                            files.push(file.clone());
                        }
                    }
                }
            }
        }

        self.generations.create(
            &package_names,
            &files,
            configuration.environment,
            configuration.system,
            |src, dst| self.content.link_to(src, dst),
        )
    }

    pub fn activate_generation(&self, id: u64) -> Result<()> {
        let gen_path = self.generations.path(id);
        let current = Path::new("/rscm/current-system");
        if current.exists() {
            fs::remove_file(current)?;
        }
        std::os::unix::fs::symlink(&gen_path, current)?;
        let manifest_path = current.join("manifest.toml");
        let manifest: GenerationManifest = toml::from_str(&fs::read_to_string(manifest_path)?)?;

        let mut path_dirs = Vec::new();
        for dir in &["bin", "sbin", "usr/bin", "usr/sbin"] {
            let full_path = gen_path.join(dir);
            if full_path.exists() {
                path_dirs.push(format!("/rscm/current-system/{}", dir));
            }
        }
        let rscm_path = path_dirs.join(":");

        let mut lib_dirs = Vec::new();
        for dir in &["lib", "lib64", "usr/lib", "usr/lib64"] {
            let full_path = gen_path.join(dir);
            if full_path.exists() {
                lib_dirs.push(format!("/rscm/current-system/{}", dir));
            }
        }
        let rscm_ld_path = lib_dirs.join(":");

        let mut include_dirs = Vec::new();
        for dir in &["usr/include"] {
            let full_path = gen_path.join(dir);
            if full_path.exists() {
                include_dirs.push(format!("/rscm/current-system/{}", dir));
            }
        }
        let rscm_include_path = include_dirs.join(":");

        let mut pkgconfig_dirs = Vec::new();
        for dir in &[
            "lib/pkgconfig",
            "lib64/pkgconfig",
            "usr/lib/pkgconfig",
            "usr/lib64/pkgconfig",
            "usr/share/pkgconfig",
        ] {
            let full_path = gen_path.join(dir);
            if full_path.exists() {
                pkgconfig_dirs.push(format!("/rscm/current-system/{}", dir));
            }
        }
        let rscm_pkgconfig_path = pkgconfig_dirs.join(":");

        let session_env_path = current.join("session.env");
        let variables_sh_path = current.join("variables.sh");

        let mut session_env_content = if session_env_path.exists() {
            fs::read_to_string(&session_env_path)?
        } else {
            String::new()
        };

        let mut variables_sh_content = if variables_sh_path.exists() {
            fs::read_to_string(&variables_sh_path)?
        } else {
            String::from("# This file was generated by rscm.\n")
        };

        if !rscm_path.is_empty() {
            session_env_content.push_str(&format!("\nPATH={}:$PATH\n", rscm_path));
        }
        if !rscm_ld_path.is_empty() {
            session_env_content.push_str(&format!(
                "\nLD_LIBRARY_PATH={}${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}\n",
                rscm_ld_path
            ));
        }
        if !rscm_include_path.is_empty() {
            session_env_content.push_str(&format!(
                "\nCPATH={}${{CPATH:+:$CPATH}}\n",
                rscm_include_path
            ));
        }
        if !rscm_pkgconfig_path.is_empty() {
            session_env_content.push_str(&format!(
                "\nPKG_CONFIG_PATH={}${{PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}}\n",
                rscm_pkgconfig_path
            ));
        }
        fs::write(&session_env_path, &session_env_content)?;

        if !rscm_path.is_empty() {
            variables_sh_content.push_str(&format!("export PATH={}:$PATH\n", rscm_path));
        }
        if !rscm_ld_path.is_empty() {
            variables_sh_content.push_str(&format!(
                "export LD_LIBRARY_PATH={}:${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}\n",
                rscm_ld_path
            ));
        }
        if !rscm_include_path.is_empty() {
            variables_sh_content.push_str(&format!(
                "export CPATH={}:${{CPATH:+:$CPATH}}\n",
                rscm_include_path
            ));
        }
        if !rscm_pkgconfig_path.is_empty() {
            variables_sh_content.push_str(&format!(
                "export PKG_CONFIG_PATH={}${{PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}}\n",
                rscm_pkgconfig_path
            ));
        }
        fs::write(&variables_sh_path, &variables_sh_content)?;

        let env_d_path = Path::new("/etc/environment.d");
        fs::create_dir_all(env_d_path)?;
        let rscm_env_link = env_d_path.join("rscm.conf");
        if rscm_env_link.exists() || rscm_env_link.is_symlink() {
            fs::remove_file(&rscm_env_link)?;
        }
        std::os::unix::fs::symlink(&session_env_path, &rscm_env_link)?;

        let env_path = Path::new("/etc/profile.d/rscm.sh");
        if env_path.exists() || env_path.is_symlink() {
            fs::remove_file(env_path)?;
        }
        std::os::unix::fs::symlink(variables_sh_path, env_path)?;

        let system_config_path = gen_path.join("system_config.toml");
        if system_config_path.exists() {
            let sys_config_content = fs::read_to_string(&system_config_path)?;
            let system_config: crate::config::SystemConfig = toml::from_str(&sys_config_content)?;
            SystemConfigApplier::apply(&system_config)?;
        }

        println!("Switched to generation {}", id);
        if !rscm_path.is_empty() {
            println!("PATH prepended: {}", rscm_path);
        }
        if !rscm_ld_path.is_empty() {
            println!("LD_LIBRARY_PATH prepended: {}", rscm_ld_path);
        }
        if !rscm_include_path.is_empty() {
            println!("CPATH prepended: {}", rscm_include_path);
        }
        if !rscm_pkgconfig_path.is_empty() {
            println!("PKG_CONFIG_PATH prepended: {}", rscm_pkgconfig_path);
        }

        Ok(())
    }

    pub fn delete_generation(&mut self, id: u64) -> Result<()> {
        let current_link = Path::new("/rscm/current-system");
        if current_link.exists() {
            let current_target = fs::read_link(current_link)?;
            if let Some(current_name) = current_target.file_name().and_then(|n| n.to_str()) {
                if current_name == id.to_string() {
                    anyhow::bail!(
                        "Cannot delete generation {}: it is currently active. Use 'rscm switch' to switch to another generation first.",
                        id
                    );
                }
            }
        }

        let generation = self.generations.get(id)?;
        if let Some(generation) = generation {
            for pkg_name in &generation.manifest.packages {
                if let Some(pkg) = self.packages.get(pkg_name)? {
                    for file in &pkg.files {
                        self.reference.remove(&file.hash)?;
                    }
                    self.reference.remove(pkg_name)?;
                } else {
                    self.reference.remove(pkg_name)?;
                }
            }
        }

        self.generations.delete(id)
    }

    pub fn list_generations(&self) -> Result<Vec<Generation>> {
        self.generations.list()
    }

    pub fn gc(&mut self, dry_run: bool) -> Result<GcResult> {
        let mut result = GcResult::default();

        let mut reachable_contents: HashSet<String> = HashSet::new();
        let mut reachable_pkg_prefixes: HashSet<String> = HashSet::new();

        let generations = self.generations.list()?;
        for generation in &generations {
            for pkg_full_name in &generation.manifest.packages {
                if let Some(pkg) = self.packages.get_by_full_name(pkg_full_name)? {
                    for file in &pkg.files {
                        reachable_contents.insert(file.hash.clone());
                    }
                    let dir_name = format!("{}-{}-", pkg.name, pkg.version);
                    reachable_pkg_prefixes.insert(dir_name);
                }
            }
        }

        let all_content_hashes = self.content.list_all_hashes()?;
        for hash in &all_content_hashes {
            if !reachable_contents.contains(hash) {
                let path = self.content.content_path(hash);
                if let Ok(metadata) = fs::metadata(&path) {
                    result.freed_space += metadata.len();
                }
                if !dry_run {
                    self.content.remove(hash)?;
                    self.reference.remove_entry(hash);
                }
                result.collected_contents += 1;
            }
        }

        if !self.packages.root().exists() {
            return Ok(result);
        }
        for entry in fs::read_dir(self.packages.root())? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            let is_reachable = reachable_pkg_prefixes
                .iter()
                .any(|prefix| dir_name.starts_with(prefix));
            if !is_reachable {
                if let Ok(content) = fs::read_to_string(path.join("manifest.toml")) {
                    if let Ok(pkg) = toml::from_str::<Package>(&content) {
                        for file in &pkg.files {
                            result.freed_space += file.size;
                        }
                    }
                }
                if !dry_run {
                    fs::remove_dir_all(&path)?;
                    self.reference.remove_entry(&dir_name);
                }
                result.collected_packages += 1;
            }
        }

        Ok(result)
    }
}
