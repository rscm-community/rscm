use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use which::which;

use crate::config::UserConfig;
use crate::pkg::pacman::Pacman;
use crate::pkg::{BuildType, PackageConfig, PackageManager};

pub struct UserApplier;

const MANAGED_USERS_FILE: &str = "managed_users.toml";
const MANAGED_GROUPS_FILE: &str = "managed_groups.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManagedUsers {
    pub users: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManagedGroups {
    pub groups: Vec<String>,
}

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
    pub fn apply(users: &HashMap<String, UserConfig>, store_root: &Path) -> Result<()> {
        let managed_users = Self::load_managed_users(store_root);
        let managed_groups = Self::load_managed_groups(store_root);

        Self::remove_undeclared_users(users, &managed_users, store_root)?;
        Self::remove_undeclared_groups(users, &managed_groups, store_root)?;

        for (username, config) in users {
            Self::apply_user(username, config)?;
            Self::mark_user_managed(store_root, username)?;

            for group in &config.groups {
                Self::mark_group_managed(store_root, group)?;
            }
        }

        Ok(())
    }

    fn load_managed_groups(store_root: &Path) -> HashSet<String> {
        let managed_file = store_root.join(MANAGED_GROUPS_FILE);
        if managed_file.exists() {
            if let Ok(content) = fs::read_to_string(&managed_file) {
                if let Ok(data) = toml::from_str::<ManagedGroups>(&content) {
                    return data.groups.into_iter().collect();
                }
            }
        }
        HashSet::new()
    }

    fn save_managed_groups(store_root: &Path, groups: &HashSet<String>) -> Result<()> {
        let managed_file = store_root.join(MANAGED_GROUPS_FILE);
        if let Some(parent) = managed_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut group_list: Vec<String> = groups.iter().cloned().collect();
        group_list.sort();
        let content = toml::to_string_pretty(&ManagedGroups { groups: group_list })?;
        fs::write(managed_file, content)?;
        Ok(())
    }

    fn mark_group_managed(store_root: &Path, group: &str) -> Result<()> {
        if group == "root" {
            return Ok(());
        }
        let mut managed = Self::load_managed_groups(store_root);
        managed.insert(group.to_string());
        Self::save_managed_groups(store_root, &managed)?;
        Ok(())
    }

    fn is_managed_group(group: &str, managed_groups: &HashSet<String>) -> bool {
        managed_groups.contains(group)
    }

    fn load_managed_users(store_root: &Path) -> HashSet<String> {
        let managed_file = store_root.join(MANAGED_USERS_FILE);
        if managed_file.exists() {
            if let Ok(content) = fs::read_to_string(&managed_file) {
                if let Ok(data) = toml::from_str::<ManagedUsers>(&content) {
                    return data.users.into_iter().collect();
                }
            }
        }
        HashSet::new()
    }

    fn save_managed_users(store_root: &Path, users: &HashSet<String>) -> Result<()> {
        let managed_file = store_root.join(MANAGED_USERS_FILE);
        if let Some(parent) = managed_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut user_list: Vec<String> = users.iter().cloned().collect();
        user_list.sort();
        let content = toml::to_string_pretty(&ManagedUsers { users: user_list })?;
        fs::write(managed_file, content)?;
        Ok(())
    }

    fn mark_user_managed(store_root: &Path, username: &str) -> Result<()> {
        if username == "root" {
            return Ok(());
        }
        let mut managed = Self::load_managed_users(store_root);
        managed.insert(username.to_string());
        Self::save_managed_users(store_root, &managed)?;
        Ok(())
    }

    fn is_managed_user(username: &str, managed_users: &HashSet<String>) -> bool {
        managed_users.contains(username)
    }

    fn get_system_users() -> Result<Vec<String>> {
        let output = Command::new("awk")
            .args(["-F:", "{print $1}", "/etc/passwd"])
            .output()?;
        let users: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(users)
    }

    fn remove_undeclared_users(
        config_users: &HashMap<String, UserConfig>,
        managed_users: &HashSet<String>,
        store_root: &Path,
    ) -> Result<()> {
        let system_users = Self::get_system_users()?;
        let config_usernames: HashSet<&String> = config_users.keys().collect();

        for username in system_users {
            if !Self::is_managed_user(&username, managed_users) {
                continue;
            }

            if !config_usernames.contains(&username) {
                println!("Removing undeclared user: {}", username);
                let status = Command::new("userdel").arg("-r").arg(&username).status()?;

                if !status.success() {
                    eprintln!("Warning: failed to remove user {}", username);
                } else {
                    let mut managed = managed_users.clone();
                    managed.remove(&username);
                    Self::save_managed_users(store_root, &managed)?;
                }
            }
        }

        Ok(())
    }

    fn get_system_groups() -> Result<Vec<String>> {
        let output = Command::new("awk")
            .args(["-F:", "{print $1}", "/etc/group"])
            .output()?;
        let groups: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(groups)
    }

    fn remove_undeclared_groups(
        config_users: &HashMap<String, UserConfig>,
        managed_groups: &HashSet<String>,
        store_root: &Path,
    ) -> Result<()> {
        let mut declared_groups: HashSet<String> = HashSet::new();
        for config in config_users.values() {
            for group in &config.groups {
                declared_groups.insert(group.clone());
            }
        }

        let system_groups = Self::get_system_groups()?;

        for group in system_groups {
            if !Self::is_managed_group(&group, managed_groups) {
                continue;
            }

            if !declared_groups.contains(&group) {
                let output = Command::new("getent").arg("group").arg(&group).output()?;
                let has_members = String::from_utf8_lossy(&output.stdout).contains(":");

                if has_members {
                    let members = Command::new("getent")
                        .args(["group", &group])
                        .output()?
                        .stdout;
                    let members_str = String::from_utf8_lossy(&members);
                    if members_str.contains(':') {
                        let parts: Vec<&str> = members_str.split(':').collect();
                        if parts.len() >= 4 {
                            let member_list = parts[3];
                            if !member_list.is_empty() {
                                println!("Skipping group {} (has members: {})", group, member_list);
                                continue;
                            }
                        }
                    }
                }

                println!("Removing undeclared group: {}", group);
                let status = Command::new("groupdel").arg(&group).status()?;

                if !status.success() {
                    eprintln!("Warning: failed to remove group {}", group);
                } else {
                    let mut managed = managed_groups.clone();
                    managed.remove(&group);
                    Self::save_managed_groups(store_root, &managed)?;
                }
            }
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

    fn ensure_groups(name: &str, config_groups: &[String]) -> Result<()> {
        let default_groups = [
            "root", "wheel", "sudo", "adm", "audio", "video", "disk", "lp", "tty",
        ];

        for group in config_groups {
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

        let output = Command::new("id").arg("-nG").arg(name).output()?;
        let current_groups_str = String::from_utf8_lossy(&output.stdout);
        let current_groups: Vec<&str> = current_groups_str.split_whitespace().collect();

        for group in current_groups {
            if default_groups.contains(&group) {
                continue;
            }

            if group == name {
                continue;
            }

            if !config_groups.iter().any(|g| g.as_str() == group) {
                Command::new("gpasswd")
                    .arg("-d")
                    .arg(name)
                    .arg(group)
                    .status()
                    .ok();
                println!("Removed {} from group: {}", name, group);
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
