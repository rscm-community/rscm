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

use crate::boot::BootApplier;
use crate::config::Configuration;
use crate::lock::LockFile;
use crate::pkg::{BuildType, PackageConfig, PackageManagerFactory};
use crate::service::ServiceApplier;
use crate::store::generation::GenerationManifest;
use crate::system_config::SystemConfigApplier;
use crate::user::UserApplier;

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
        let mut kernel_packages: Vec<String> = Vec::new();

        for (name, pkg_versions) in &lock.packages {
            for (version_key, pkg_version) in &pkg_versions.versions {
                let full_name = format!("{}-{}", name, version_key);
                package_names.push(full_name.clone());

                let is_kernel =
                    name == "linux" || name.starts_with("linux-") || name.starts_with("linux_");
                if is_kernel {
                    kernel_packages.push(name.clone());
                }

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
                    let _info = manager.install(&config, false)?;

                    if let Some(pkg) = self.packages.get(name)? {
                        for file in &pkg.files {
                            files.push(file.clone());
                        }
                    }
                }
            }
        }

        let boot_config = configuration.boot.clone();
        let id = self.generations.create(
            &package_names,
            &files,
            configuration.environment,
            configuration.system,
            &configuration.services,
            &configuration.users,
            configuration.boot,
            |src, dst| self.content.link_to(src, dst),
        )?;

        if !kernel_packages.is_empty() {
            let gen_path = self.generations.path(id);
            if let Err(e) =
                Self::generate_initramfs(&gen_path, &kernel_packages, boot_config.as_ref())
            {
                eprintln!("Warning: initramfs generation failed: {}", e);
            }
        }

        Ok(id)
    }

    fn generate_initramfs(
        gen_path: &Path,
        kernel_packages: &[String],
        boot_config: Option<&crate::config::BootConfig>,
    ) -> Result<()> {
        let gen_modules_dir = gen_path.join("lib/modules");
        let system_modules_dir = Path::new("/lib/modules");

        let mut all_kernel_versions: Vec<String> = Vec::new();

        if gen_modules_dir.exists() {
            if let Ok(entries) = fs::read_dir(&gen_modules_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !all_kernel_versions.contains(&name.to_string()) {
                                all_kernel_versions.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        if system_modules_dir.exists() {
            if let Ok(entries) = fs::read_dir(system_modules_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !all_kernel_versions.contains(&name.to_string()) {
                                all_kernel_versions.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        if all_kernel_versions.is_empty() {
            return Err(anyhow::anyhow!(
                "No kernel modules found in generation or system"
            ));
        }

        for kernel_version in &all_kernel_versions {
            println!("Generating initramfs for kernel {}...", kernel_version);

            let gen_boot = gen_path.join("boot");
            fs::create_dir_all(&gen_boot)?;

            let temp_dir = gen_path.join("tmp_initramfs_modules");
            let temp_modules_dir = temp_dir.join("lib/modules").join(kernel_version);
            fs::create_dir_all(&temp_modules_dir)?;

            let gen_kernel_modules_dir = gen_modules_dir.join(kernel_version);
            let system_kernel_modules_dir = system_modules_dir.join(kernel_version);

            let mut module_sources: Vec<PathBuf> = Vec::new();
            if gen_kernel_modules_dir.exists() {
                module_sources.push(gen_kernel_modules_dir);
            }
            if system_kernel_modules_dir.exists() {
                module_sources.push(system_kernel_modules_dir);
            }

            for source_dir in module_sources {
                if let Ok(entries) = fs::read_dir(&source_dir) {
                    for entry in entries.flatten() {
                        let entry_name = entry.file_name();
                        let target_file = temp_modules_dir.join(&entry_name);
                        if !target_file.exists() {
                            let _ = std::os::unix::fs::symlink(&entry.path(), &target_file);
                        }
                    }
                }
            }

            let initramfs_name = if kernel_version.contains("lts") {
                "initramfs-linux-lts.img"
            } else if kernel_version.contains("hardened") {
                "initramfs-linux-hardened.img"
            } else if kernel_version.contains("zen") {
                "initramfs-linux-zen.img"
            } else if kernel_packages.iter().any(|p| p == "linux") {
                "initramfs-linux.img"
            } else {
                "initramfs.img"
            };
            let initramfs_path = gen_boot.join(initramfs_name);

            let mut cmd = std::process::Command::new("mkinitcpio");
            cmd.arg("-k").arg(&kernel_version);
            cmd.arg("-r").arg(&temp_dir);
            cmd.arg("-g").arg(&initramfs_path);

            if let Some(ref boot) = boot_config {
                if let Some(ref initrd) = boot.initrd {
                    if let Some(ref modules) = initrd.kernel_modules {
                        if !modules.is_empty() {
                            let temp_conf = gen_path.join("mkinitcpio.conf");
                            let mut conf_content = String::new();
                            conf_content.push_str("MODULES=(");
                            conf_content.push_str(&modules.join(" "));
                            conf_content.push_str(")\n");
                            conf_content.push_str("HOOKS=(base udev autodetect modconf block filesystems keyboard fsck)\n");
                            conf_content.push_str("BINARIES=()\n");
                            conf_content.push_str("FILES=()\n");
                            conf_content.push_str("COMPRESSION=\"zstd\"\n");
                            fs::write(&temp_conf, conf_content)?;
                            cmd.arg("-c").arg(&temp_conf);
                        }
                    }
                }
            }

            let output = cmd.output()?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow::anyhow!(
                    "mkinitcpio failed for kernel {}: {}",
                    kernel_version,
                    stderr
                ));
            }

            if !initramfs_path.exists() {
                return Err(anyhow::anyhow!(
                    "mkinitcpio succeeded but initramfs was not created at {}",
                    initramfs_path.display()
                ));
            }

            println!("Generated initramfs at {}", initramfs_path.display());

            let fallback_name = initramfs_name.replace(".img", "-fallback.img");
            let fallback_path = gen_boot.join(&fallback_name);

            let mut fallback_cmd = std::process::Command::new("mkinitcpio");
            fallback_cmd.arg("-k").arg(&kernel_version);
            fallback_cmd.arg("-g").arg(&fallback_path);
            fallback_cmd.arg("-S");

            if let Ok(output) = fallback_cmd.output() {
                if output.status.success() && fallback_path.exists() {
                    println!(
                        "Generated fallback initramfs at {}",
                        fallback_path.display()
                    );
                }
            }

            let _ = fs::remove_dir_all(&temp_dir);
        }

        Ok(())
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

        let services_config_path = gen_path.join("services.toml");
        if services_config_path.exists() {
            let services_content = fs::read_to_string(&services_config_path)?;
            let services: std::collections::HashMap<String, crate::config::ServiceConfig> =
                toml::from_str(&services_content)?;
            ServiceApplier::apply(&services)?;
        }

        let users_config_path = gen_path.join("users.toml");
        if users_config_path.exists() {
            let users_content = fs::read_to_string(&users_config_path)?;
            let users: std::collections::HashMap<String, crate::config::UserConfig> =
                toml::from_str(&users_content)?;
            UserApplier::apply(&users)?;
        }

        let boot_config_path = gen_path.join("boot_config.toml");
        if boot_config_path.exists() {
            let boot_content = fs::read_to_string(&boot_config_path)?;
            let boot_config: crate::config::BootConfig = toml::from_str(&boot_content)?;
            if let Err(e) = BootApplier::apply(&boot_config, id, &gen_path) {
                eprintln!("Warning: Boot configuration failed: {}", e);
            }
        }

        Self::setup_fonts(&gen_path)?;

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

    fn setup_fonts(gen_path: &Path) -> Result<()> {
        let fonts_dir = gen_path.join("share/fonts");
        let local_fonts_dir = gen_path.join("share/local/fonts");

        let mut font_paths = Vec::new();
        if fonts_dir.exists() {
            font_paths.push(fonts_dir);
        }
        if local_fonts_dir.exists() {
            font_paths.push(local_fonts_dir);
        }

        if font_paths.is_empty() {
            return Ok(());
        }

        let fonts_conf_dir = Path::new("/etc/fonts/conf.d");
        fs::create_dir_all(fonts_conf_dir)?;

        let rscm_fonts_conf_path = fonts_conf_dir.join("99-rscm-fonts.conf");
        if rscm_fonts_conf_path.exists() || rscm_fonts_conf_path.is_symlink() {
            fs::remove_file(&rscm_fonts_conf_path)?;
        }

        let mut conf_content = String::from("<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:publicid:-//IDN fontconfig.org//DTD fontconfig files XML//1.0//EN\">\n<fontconfig>\n");
        conf_content.push_str("  <!-- Managed by rscm - do not edit manually -->\n");

        for font_path in &font_paths {
            let path_str = font_path.to_string_lossy();
            conf_content.push_str(&format!("  <dir>{}</dir>\n", path_str));
        }

        conf_content.push_str("</fontconfig>\n");

        fs::write(&rscm_fonts_conf_path, conf_content)?;

        println!("Added font directories:");
        for font_path in &font_paths {
            println!("  {}", font_path.display());
        }

        std::process::Command::new("fc-cache")
            .arg("-f")
            .status()
            .ok();

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

        BootApplier::remove_generation_boot_entry(id)?;

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
