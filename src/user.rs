use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use which::which;

use crate::config::UserConfig;
use crate::pkg::pacman::Pacman;
use crate::pkg::{BuildType, PackageConfig, PackageManager};

pub struct UserApplier;

fn ensure_openssl() -> Result<()> {
    if which("openssl").is_ok() {
        return Ok(());
    }

    println!("openssl not found, installing temporarily...");

    let temp_store_root = std::env::temp_dir().join("rscm-temp-openssl");
    let pacman = Pacman::system(temp_store_root);

    let config = PackageConfig {
        name: "openssl".to_string(),
        version: None,
        build_type: BuildType::Pacman,
        dependencies: vec![],
        sandbox_config: None,
    };

    pacman.install(&config, false)?;

    if which("openssl").is_err() {
        return Err(anyhow::anyhow!("Failed to install openssl"));
    }

    println!("openssl installed successfully");
    Ok(())
}

impl UserApplier {
    pub fn apply(users: &HashMap<String, UserConfig>) -> Result<()> {
        for (username, config) in users {
            Self::apply_user(username, config)?;
        }
        Ok(())
    }

    fn apply_user(name: &str, config: &UserConfig) -> Result<()> {
        let home_dir = config
            .home
            .clone()
            .unwrap_or_else(|| format!("/home/{}", name));

        if Self::user_exists(name)? {
            Self::update_user(name, config, &home_dir)?;
        } else {
            Self::create_user(name, config, &home_dir)?;
        }

        Self::ensure_groups(name, &config.groups)?;

        if let Some(ref shell) = config.shell {
            Self::set_user_shell(name, shell)?;
        }

        if config.create_home {
            Self::ensure_home_directory(&home_dir, name)?;
        }

        if !config.ssh_keys.is_empty() {
            Self::setup_ssh_keys(&home_dir, &config.ssh_keys)?;
        }

        Ok(())
    }

    fn user_exists(name: &str) -> Result<bool> {
        let output = Command::new("id").arg(name).output()?;
        Ok(output.status.success())
    }

    fn create_user(name: &str, config: &UserConfig, home_dir: &str) -> Result<()> {
        let mut cmd = Command::new("useradd");
        cmd.arg("--no-create-home");

        if config.system_user {
            cmd.arg("--system");
        }

        if let Some(uid) = config.uid {
            cmd.arg("--uid").arg(uid.to_string());
        }

        if let Some(ref desc) = config.description {
            cmd.arg("--comment").arg(desc);
        }

        if let Some(ref shell) = config.shell {
            cmd.arg("--shell").arg(shell);
        }

        if let Some(ref password) = config.password {
            ensure_openssl()?;
            let password_hash = if password.starts_with('$') {
                password.clone()
            } else {
                let output = Command::new("openssl")
                    .args(["passwd", "-6", password])
                    .output()?;
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            };
            cmd.arg("--password").arg(password_hash);
        }

        cmd.arg(name);

        let status = cmd.status()?;
        if status.success() {
            println!("Created user: {}", name);
        } else {
            eprintln!("Warning: failed to create user {}", name);
        }

        Ok(())
    }

    fn update_user(name: &str, config: &UserConfig, home_dir: &str) -> Result<()> {
        let mut cmd = Command::new("usermod");

        let mut has_option = false;

        if let Some(ref shell) = config.shell {
            cmd.arg("--shell").arg(shell);
            has_option = true;
        }

        if let Some(ref desc) = config.description {
            cmd.arg("--comment").arg(desc);
            has_option = true;
        }

        if let Some(uid) = config.uid {
            cmd.arg("--uid").arg(uid.to_string());
            has_option = true;
        }

        if config.home.is_some() {
            cmd.arg("--home").arg(home_dir);
            has_option = true;
        }

        if !config.groups.is_empty() {
            cmd.arg("--append")
                .arg("--groups")
                .arg(config.groups.join(","));
            has_option = true;
        }

        if let Some(ref password) = config.password {
            ensure_openssl()?;
            let password_hash = if password.starts_with('$') {
                password.clone()
            } else {
                let output = Command::new("openssl")
                    .args(["passwd", "-6", password])
                    .output()?;
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            };
            cmd.arg("--password").arg(password_hash);
            has_option = true;
        }

        if !has_option {
            return Ok(());
        }

        cmd.arg(name);

        let status = cmd.status()?;
        if !status.success() {
            eprintln!("Warning: failed to update user {}", name);
        }

        Ok(())
    }

    fn ensure_groups(name: &str, groups: &[String]) -> Result<()> {
        for group in groups {
            let group_exists = Command::new("getent")
                .arg("group")
                .arg(group)
                .output()?
                .status
                .success();

            if !group_exists {
                Command::new("groupadd").arg(group).status().ok();
                println!("Created group: {}", group);
            }

            let output = Command::new("id").arg("-nG").arg(name).output()?;
            let current_groups = String::from_utf8_lossy(&output.stdout);

            if !current_groups.split_whitespace().any(|g| g == group) {
                Command::new("gpasswd")
                    .arg("-a")
                    .arg(name)
                    .arg(group)
                    .status()
                    .ok();
                println!("Added {} to group: {}", name, group);
            }
        }
        Ok(())
    }

    fn set_user_shell(name: &str, shell: &str) -> Result<()> {
        Command::new("chsh")
            .arg("-s")
            .arg(shell)
            .arg(name)
            .status()
            .ok();

        println!("Set shell for {}: {}", name, shell);

        Ok(())
    }

    fn ensure_home_directory(home_dir: &str, name: &str) -> Result<()> {
        let path = Path::new(home_dir);

        if !path.exists() {
            fs::create_dir_all(path)?;
            println!("Created home directory: {}", home_dir);
        }

        let uid_output = Command::new("id").arg("-u").arg(name).output()?;
        let uid = String::from_utf8_lossy(&uid_output.stdout)
            .trim()
            .to_string();

        let gid_output = Command::new("id").arg("-g").arg(name).output()?;
        let gid = String::from_utf8_lossy(&gid_output.stdout)
            .trim()
            .to_string();

        Command::new("chown")
            .arg("-R")
            .arg(format!("{}:{}", uid, gid))
            .arg(home_dir)
            .status()
            .ok();

        Command::new("chmod").arg("755").arg(home_dir).status().ok();

        Ok(())
    }

    fn setup_ssh_keys(home_dir: &str, ssh_keys: &[String]) -> Result<()> {
        let ssh_dir = Path::new(home_dir).join(".ssh");
        fs::create_dir_all(&ssh_dir)?;

        Command::new("chmod").arg("700").arg(&ssh_dir).status().ok();

        let authorized_keys_path = ssh_dir.join("authorized_keys");

        let mut keys_content = String::new();
        for key in ssh_keys {
            keys_content.push_str(key.trim());
            keys_content.push('\n');
        }

        fs::write(&authorized_keys_path, keys_content)?;

        Command::new("chmod")
            .arg("600")
            .arg(&authorized_keys_path)
            .status()
            .ok();

        println!("Configured SSH keys for home: {}", home_dir);

        Ok(())
    }
}
