use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::store::package::InstallScript;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstallHookType {
    PreInstall,
    PostInstall,
    PreUpgrade,
    PostUpgrade,
    PreRemove,
    PostRemove,
}

impl InstallHookType {
    pub fn function_name(&self) -> &'static str {
        match self {
            InstallHookType::PreInstall => "pre_install",
            InstallHookType::PostInstall => "post_install",
            InstallHookType::PreUpgrade => "pre_upgrade",
            InstallHookType::PostUpgrade => "post_upgrade",
            InstallHookType::PreRemove => "pre_remove",
            InstallHookType::PostRemove => "post_remove",
        }
    }
}

pub fn parse_install_script(content: &str) -> Vec<String> {
    let mut functions = Vec::new();
    let known_functions = [
        "pre_install",
        "post_install",
        "pre_upgrade",
        "post_upgrade",
        "pre_remove",
        "post_remove",
    ];

    for line in content.lines() {
        let trimmed = line.trim();
        for func in &known_functions {
            if trimmed.starts_with(&format!("{}()", func))
                || trimmed.starts_with(&format!("{} ()", func))
            {
                if !functions.contains(&func.to_string()) {
                    functions.push(func.to_string());
                }
            }
        }
    }

    functions
}

pub fn extract_install_script_from_tar(
    archive: &mut tar::Archive<impl std::io::Read>,
    temp_extract_dir: &Path,
) -> Result<Option<InstallScript>> {
    let mut install_script_content = None;

    let entries: Vec<_> = archive.entries()?.filter_map(|e| e.ok()).collect();

    for mut entry in entries {
        let path = entry.path()?.to_string_lossy().to_string();

        if path.ends_with(".INSTALL") || path == ".INSTALL" {
            let full_path = temp_extract_dir.join(&path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(&full_path)?;
            install_script_content = Some(fs::read_to_string(&full_path)?);
            break;
        }
    }

    if let Some(content) = install_script_content {
        let functions = parse_install_script(&content);
        Ok(Some(InstallScript { content, functions }))
    } else {
        Ok(None)
    }
}

pub fn execute_install_hook(
    script: &InstallScript,
    hook_type: InstallHookType,
    pkg_name: &str,
    pkg_version: &str,
) -> Result<()> {
    let func_name = hook_type.function_name();

    if !script.functions.iter().any(|f| f == func_name) {
        return Ok(());
    }

    println!(
        "Running {} hook for package {}-{}...",
        func_name, pkg_name, pkg_version
    );

    let script_content = format!(
        "pkgname='{}'\npkgver='{}'\n\n{}\n\n{} $@\n",
        pkg_name, pkg_version, script.content, func_name
    );

    let output = Command::new("bash")
        .arg("-c")
        .arg(&script_content)
        .output()
        .context(format!("Failed to execute {} hook", func_name))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "Warning: {} hook for {}-{} failed with exit code {}: {}",
            func_name,
            pkg_name,
            pkg_version,
            output.status.code().unwrap_or(-1),
            stderr
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        println!("{}", stdout.trim());
    }

    Ok(())
}

pub fn execute_post_install(
    script: &InstallScript,
    pkg_name: &str,
    pkg_version: &str,
) -> Result<()> {
    execute_install_hook(script, InstallHookType::PostInstall, pkg_name, pkg_version)
}

pub fn execute_pre_install(
    script: &InstallScript,
    pkg_name: &str,
    pkg_version: &str,
) -> Result<()> {
    execute_install_hook(script, InstallHookType::PreInstall, pkg_name, pkg_version)
}
