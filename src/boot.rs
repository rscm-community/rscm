use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{BootConfig, InitrdConfig, KernelConfig, SystemdBootConfig};

const SYSTEMD_BOOT_LOADER_CONF: &str = "/boot/loader/loader.conf";
const SYSTEMD_BOOT_ENTRIES_DIR: &str = "/boot/loader/entries";
const RSCM_ESP_SUBDIR: &str = "rscm";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootEntry {
    pub title: String,
    pub linux: String,
    pub initrd: Option<String>,
    pub options: Vec<String>,
    pub machine_id: Option<String>,
}

pub struct BootApplier;

impl BootApplier {
    pub fn apply(boot_config: &BootConfig, generation_id: u64, gen_path: &Path) -> Result<()> {
        if let Some(ref loader) = boot_config.loader {
            if let Some(ref systemd_boot) = loader.systemd_boot {
                if systemd_boot.enable.unwrap_or(true) {
                    if !Self::is_systemd_boot_installed() {
                        println!("systemd-boot not installed, installing...");
                        Self::install_systemd_boot()?;
                    }
                    Self::apply_systemd_boot(boot_config, systemd_boot, generation_id, gen_path)?;
                    Self::install_switch_service()?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn is_systemd_boot_installed() -> bool {
        let output = std::process::Command::new("bootctl")
            .arg("--path=/boot")
            .arg("is-installed")
            .output();
        match output {
            Ok(o) => {
                if o.status.success() {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    stdout.trim() == "yes"
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }

    fn install_systemd_boot() -> Result<()> {
        if Self::is_systemd_boot_installed() {
            println!("systemd-boot is already installed, skipping installation");
            return Ok(());
        }

        println!("Installing systemd-boot...");

        let output = std::process::Command::new("bootctl")
            .arg("--path=/boot")
            .arg("install")
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to run bootctl install: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("bootctl install failed: {}", stderr));
        }

        println!("systemd-boot installed successfully");
        Ok(())
    }

    fn install_switch_service() -> Result<()> {
        let service_content = r#"[Unit]
Description=Switch to rscm generation specified in boot parameters
DefaultDependencies=no
After=sysinit.target
Before=basic.target

[Service]
Type=oneshot
ExecStart=/rscm/scripts/rscm-auto-switch
RemainAfterExit=yes

[Install]
WantedBy=sysinit.target
"#;
        let service_path = Path::new("/etc/systemd/system/rscm-switch.service");
        fs::write(service_path, service_content)?;
        println!("Installed rscm-switch.service");

        let script_content = r#"#!/bin/sh
# Auto-switch to the generation specified in kernel boot parameters
GENERATION=$(cat /proc/cmdline | tr ' ' '\n' | grep '^rscm.generation=' | cut -d'=' -f2)
if [ -n "$GENERATION" ]; then
    CURRENT=$(readlink -f /rscm/current-system 2>/dev/null | xargs basename 2>/dev/null)
    if [ "$CURRENT" != "$GENERATION" ]; then
        rscm switch "$GENERATION"
    fi
fi
"#;
        let scripts_dir = Path::new("/rscm/scripts");
        fs::create_dir_all(scripts_dir)?;
        let script_path = scripts_dir.join("rscm-auto-switch");
        fs::write(&script_path, script_content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            fs::set_permissions(&script_path, perms)?;
        }
        println!(
            "Installed rscm-auto-switch script to {}",
            script_path.display()
        );

        if std::process::Command::new("systemctl")
            .arg("daemon-reload")
            .status()
            .is_err()
        {
            eprintln!("Warning: systemctl daemon-reload failed");
        }

        if std::process::Command::new("systemctl")
            .arg("enable")
            .arg("rscm-switch.service")
            .status()
            .is_err()
        {
            eprintln!("Warning: failed to enable rscm-switch.service");
        }

        Ok(())
    }

    fn apply_systemd_boot(
        boot_config: &BootConfig,
        systemd_boot: &SystemdBootConfig,
        generation_id: u64,
        gen_path: &Path,
    ) -> Result<()> {
        println!(
            "Applying systemd-boot configuration for generation {}...",
            generation_id
        );

        let entries_dir = Path::new(SYSTEMD_BOOT_ENTRIES_DIR);
        fs::create_dir_all(entries_dir)?;

        let rscm_esp_dir = Path::new(RSCM_ESP_SUBDIR);
        fs::create_dir_all(rscm_esp_dir)?;

        Self::write_loader_conf(systemd_boot)?;

        let kernel_info = Self::discover_kernel(gen_path, &boot_config.kernel)?;
        let initrd_info = Self::discover_initrd(gen_path, &boot_config.initrd)?;

        let esp_kernel_path = Self::copy_to_esp(&kernel_info.path, RSCM_ESP_SUBDIR, generation_id)?;
        let esp_initrd_path = if let Some(ref initrd) = initrd_info {
            Some(Self::copy_to_esp(
                &initrd.path,
                RSCM_ESP_SUBDIR,
                generation_id,
            )?)
        } else {
            None
        };

        println!("Copied kernel to {}", esp_kernel_path);
        if let Some(ref p) = esp_initrd_path {
            println!("Copied initramfs to {}", p);
        }

        let entry = Self::build_boot_entry(
            &esp_kernel_path,
            &esp_initrd_path,
            boot_config,
            generation_id,
            &kernel_info.version,
        )?;

        let entry_filename = format!("rscm-generation-{}.conf", generation_id);
        let entry_path = entries_dir.join(&entry_filename);
        let entry_content = Self::format_entry(&entry);
        fs::write(&entry_path, entry_content)?;
        println!("Created boot entry: {}", entry_path.display());

        if let Some(limit) = systemd_boot.configuration_limit {
            Self::enforce_configuration_limit(limit, generation_id)?;
        }

        Self::set_default_entry(generation_id)?;

        if let Err(e) = std::process::Command::new("bootctl")
            .arg("--path=/boot")
            .arg("update")
            .status()
        {
            eprintln!("Warning: bootctl update failed: {}", e);
        }

        Ok(())
    }

    fn copy_to_esp(src: &str, esp_subdir: &str, generation_id: u64) -> Result<String> {
        let src_path = Path::new(src);
        if !src_path.exists() {
            return Err(anyhow::anyhow!("Source file does not exist: {}", src));
        }

        let orig_name = src_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine filename from {}", src))?
            .to_string_lossy()
            .to_string();

        let (stem, ext) = if let Some(dot_pos) = orig_name.rfind('.') {
            (&orig_name[..dot_pos], &orig_name[dot_pos..])
        } else {
            (orig_name.as_str(), "")
        };
        let esp_filename = format!("{}-gen-{}{}", stem, generation_id, ext);

        let dest_dir = Path::new("/boot").join(esp_subdir);
        fs::create_dir_all(&dest_dir)?;

        let dest_path = dest_dir.join(&esp_filename);
        fs::copy(src_path, &dest_path)?;

        let esp_path = format!("{}/{}", esp_subdir, esp_filename);
        Ok(esp_path)
    }

    fn write_loader_conf(systemd_boot: &SystemdBootConfig) -> Result<()> {
        let loader_path = Path::new(SYSTEMD_BOOT_LOADER_CONF);

        let timeout = systemd_boot.timeout.unwrap_or(5);

        let mut conf = String::new();
        conf.push_str("# rscm managed - do not edit manually\n");
        conf.push_str("default rscm-*.conf\n");
        conf.push_str(&format!("timeout {}\n", timeout));
        conf.push_str("editor no\n");
        conf.push_str("console-mode keep\n");

        fs::write(loader_path, conf)?;
        println!("Updated loader.conf at {}", loader_path.display());

        Ok(())
    }

    fn discover_kernel(
        gen_path: &Path,
        kernel_config: &Option<KernelConfig>,
    ) -> Result<KernelInfo> {
        let mut kernel_path: Option<String> = None;
        let mut kernel_version: Option<String> = None;

        let gen_boot = gen_path.join("boot");
        if gen_boot.exists() {
            if let Some(cfg) = kernel_config {
                if let Some(ref package) = cfg.package {
                    let image_name = match package.as_str() {
                        "linux" => "vmlinuz-linux",
                        "linux-lts" => "vmlinuz-linux-lts",
                        "linux-hardened" => "vmlinuz-linux-hardened",
                        "linux-zen" => "vmlinuz-linux-zen",
                        other => &format!(
                            "vmlinuz-{}",
                            other.replace("linux-", "").replace("linux_", "")
                        ),
                    };
                    let path = gen_boot.join(image_name);
                    if path.exists() {
                        kernel_path = Some(path.to_string_lossy().to_string());
                        kernel_version = Some(package.clone());
                    }
                }
            }
        }

        if kernel_path.is_none() && gen_boot.exists() {
            if let Ok(entries) = fs::read_dir(&gen_boot) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("vmlinuz") {
                        kernel_path = Some(entry.path().to_string_lossy().to_string());
                        kernel_version = Some(
                            name_str
                                .strip_prefix("vmlinuz-")
                                .unwrap_or("linux")
                                .to_string(),
                        );
                        break;
                    }
                }
            }
        }

        if kernel_version.is_none() {
            let modules_dir = gen_path.join("usr/lib/modules");
            if modules_dir.exists() {
                if let Ok(entries) = fs::read_dir(&modules_dir) {
                    for entry in entries.flatten() {
                        if entry.path().is_dir() {
                            kernel_version = Some(entry.file_name().to_string_lossy().to_string());
                            break;
                        }
                    }
                }
            }
        }

        if kernel_path.is_none() {
            if let Ok(entries) = fs::read_dir("/boot") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("vmlinuz-") {
                        kernel_path = Some(format!("/boot/{}", name_str));
                        if kernel_version.is_none() {
                            kernel_version =
                                Some(name_str.strip_prefix("vmlinuz-").unwrap_or("").to_string());
                        }
                        break;
                    }
                }
            }
        }

        let kernel_path = kernel_path.unwrap_or_else(|| "/boot/vmlinuz-linux".to_string());
        let kernel_version = kernel_version.unwrap_or_else(|| "linux".to_string());

        Ok(KernelInfo {
            path: kernel_path,
            version: kernel_version,
        })
    }

    fn discover_initrd(
        gen_path: &Path,
        initrd_config: &Option<InitrdConfig>,
    ) -> Result<Option<InitrdInfo>> {
        let use_systemd_init = initrd_config
            .as_ref()
            .and_then(|c| c.systemd.as_ref())
            .and_then(|s| s.enable)
            .unwrap_or(false);

        if use_systemd_init {
            let systemd_initrd = "/boot/initrd-systemd";
            if Path::new(systemd_initrd).exists() {
                return Ok(Some(InitrdInfo {
                    path: systemd_initrd.to_string(),
                }));
            }
        }

        let gen_boot = gen_path.join("boot");
        if gen_boot.exists() {
            if let Ok(entries) = fs::read_dir(&gen_boot) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if (name_str.starts_with("initramfs") || name_str.starts_with("initrd"))
                        && !name_str.contains("fallback")
                    {
                        return Ok(Some(InitrdInfo {
                            path: entry.path().to_string_lossy().to_string(),
                        }));
                    }
                }
            }
        }

        if gen_boot.exists() {
            if let Ok(entries) = fs::read_dir(&gen_boot) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("initramfs") || name_str.starts_with("initrd") {
                        return Ok(Some(InitrdInfo {
                            path: entry.path().to_string_lossy().to_string(),
                        }));
                    }
                }
            }
        }

        if let Ok(entries) = fs::read_dir("/boot") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("initramfs")
                    && name_str.ends_with(".img")
                    && !name_str.contains("fallback")
                {
                    return Ok(Some(InitrdInfo {
                        path: format!("/boot/{}", name_str),
                    }));
                }
            }
        }

        Ok(None)
    }

    fn build_boot_entry(
        esp_kernel_path: &str,
        esp_initrd_path: &Option<String>,
        boot_config: &BootConfig,
        generation_id: u64,
        kernel_version: &str,
    ) -> Result<BootEntry> {
        let mut options = Vec::new();

        if let Some(ref kernel) = boot_config.kernel {
            if let Some(ref params) = kernel.params {
                options.extend(params.clone());
            }
        }

        let root_device = Self::get_root_device();
        if let Some(ref root) = root_device {
            options.push(root.clone());
        }

        if let Some(ref initrd) = boot_config.initrd {
            if let Some(ref modules) = initrd.kernel_modules {
                if !modules.is_empty() {
                    options.push(format!("rd.modules={}", modules.join(",")));
                }
            }
            if let Some(ref luks) = initrd.luks {
                if let Some(ref devices) = luks.devices {
                    for (name, device) in devices {
                        if let Some(ref dev_path) = device.device {
                            options.push(format!("rd.luks.name={}={}", name, dev_path));
                        }
                        if device.allow_discards.unwrap_or(false) {
                            options.push(format!("rd.luks.options={}={}", name, "discard"));
                        }
                    }
                }
            }
        }

        if let Some(log_level) = boot_config.console_log_level {
            options.push(format!("loglevel={}", log_level));
        }

        if let Some(ref shm_size) = boot_config.dev_shm_size {
            options.push(format!("tmpfs.size={}", shm_size));
        }

        let title = format!("rscm generation {} ({})", generation_id, kernel_version);

        options.push(format!("rscm.generation={}", generation_id));

        Ok(BootEntry {
            title,
            linux: esp_kernel_path.to_string(),
            initrd: esp_initrd_path.clone(),
            options,
            machine_id: Self::read_machine_id(),
        })
    }

    fn format_entry(entry: &BootEntry) -> String {
        let mut content = String::new();
        content.push_str(&format!("title {}\n", entry.title));

        if let Some(ref machine_id) = entry.machine_id {
            content.push_str(&format!("machine-id {}\n", machine_id));
        }

        content.push_str(&format!("linux {}\n", entry.linux));

        if let Some(ref initrd) = entry.initrd {
            content.push_str(&format!("initrd {}\n", initrd));
        }

        if !entry.options.is_empty() {
            content.push_str(&format!("options {}\n", entry.options.join(" ")));
        }

        content
    }

    fn read_machine_id() -> Option<String> {
        fs::read_to_string("/etc/machine-id")
            .ok()
            .map(|s| s.trim().to_string())
    }

    fn get_root_device() -> Option<String> {
        fs::read_to_string("/proc/cmdline")
            .ok()
            .and_then(|cmdline| {
                cmdline
                    .split_whitespace()
                    .find(|p| p.starts_with("root="))
                    .map(|s| s.to_string())
            })
    }

    fn set_default_entry(generation_id: u64) -> Result<()> {
        let entry_name = format!("rscm-generation-{}.conf", generation_id);
        let loader_path = Path::new(SYSTEMD_BOOT_LOADER_CONF);

        let mut lines: Vec<String> = if loader_path.exists() {
            fs::read_to_string(loader_path)?
                .lines()
                .map(|l| l.to_string())
                .collect()
        } else {
            vec![]
        };

        let mut found = false;
        for line in &mut lines {
            if line.starts_with("default ") {
                *line = format!("default {}", entry_name);
                found = true;
                break;
            }
        }

        if !found {
            lines.push(format!("default {}", entry_name));
        }

        fs::write(loader_path, lines.join("\n") + "\n")?;
        println!("Set default boot entry to: {}", entry_name);

        Ok(())
    }

    fn enforce_configuration_limit(limit: u32, current_id: u64) -> Result<()> {
        let entries_dir = Path::new(SYSTEMD_BOOT_ENTRIES_DIR);
        if !entries_dir.exists() {
            return Ok(());
        }

        let mut entries: Vec<(u64, PathBuf)> = Vec::new();
        for entry in fs::read_dir(entries_dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str.starts_with("rscm-generation-") && name_str.ends_with(".conf") {
                let id_str = name_str
                    .strip_prefix("rscm-generation-")
                    .and_then(|s| s.strip_suffix(".conf"))
                    .unwrap_or("");
                if let Ok(id) = id_str.parse::<u64>() {
                    entries.push((id, path));
                }
            }
        }

        entries.sort_by(|a, b| b.0.cmp(&a.0));

        if entries.len() > limit as usize {
            for (id, path) in entries.iter().skip(limit as usize) {
                if *id != current_id {
                    println!(
                        "Removing old boot entry for generation {} (limit: {})",
                        id, limit
                    );
                    fs::remove_file(path)?;
                }
            }
        }

        Self::clean_old_esp_copies(current_id, limit)?;

        Ok(())
    }

    fn clean_old_esp_copies(current_id: u64, limit: u32) -> Result<()> {
        let rscm_esp_dir = Path::new(RSCM_ESP_SUBDIR);
        if !rscm_esp_dir.exists() {
            return Ok(());
        }

        let mut gen_files: std::collections::HashMap<u64, Vec<PathBuf>> =
            std::collections::HashMap::new();
        for entry in fs::read_dir(rscm_esp_dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if let Some(id) = Self::extract_gen_id_from_esp_filename(&name_str) {
                gen_files.entry(id).or_default().push(path);
            }
        }

        let mut gen_ids: Vec<u64> = gen_files.keys().cloned().collect();
        gen_ids.sort_by(|a, b| b.cmp(a));

        for id in gen_ids.iter().skip(limit as usize) {
            if *id != current_id {
                if let Some(files) = gen_files.get(id) {
                    for file in files {
                        if file.exists() {
                            fs::remove_file(file)?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn extract_gen_id_from_esp_filename(name: &str) -> Option<u64> {
        // Pattern: *-gen-<id> or *-gen-<id>-<suffix>
        if let Some(pos) = name.rfind("-gen-") {
            let after = &name[pos + 5..];
            let id_str = after.split('-').next()?;
            id_str.parse::<u64>().ok()
        } else {
            None
        }
    }

    pub fn remove_entry(generation_id: u64) -> Result<()> {
        let entry_path = Path::new(SYSTEMD_BOOT_ENTRIES_DIR)
            .join(format!("rscm-generation-{}.conf", generation_id));
        if entry_path.exists() {
            fs::remove_file(&entry_path)?;
            println!("Removed boot entry for generation {}", generation_id);
        }

        let rscm_esp_dir = Path::new(RSCM_ESP_SUBDIR);
        if rscm_esp_dir.exists() {
            for entry in fs::read_dir(rscm_esp_dir)? {
                let entry = entry?;
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if Self::extract_gen_id_from_esp_filename(&name_str) == Some(generation_id) {
                    fs::remove_file(entry.path())?;
                }
            }
        }

        Ok(())
    }

    pub fn remove_generation_boot_entry(generation_id: u64) -> Result<()> {
        if Self::is_systemd_boot_installed() {
            Self::remove_entry(generation_id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct KernelInfo {
    path: String,
    version: String,
}

#[derive(Debug, Clone)]
struct InitrdInfo {
    path: String,
}
