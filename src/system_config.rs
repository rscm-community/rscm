use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::config::SystemConfig;

pub struct SystemConfigApplier;

impl SystemConfigApplier {
    pub fn apply(config: &SystemConfig) -> Result<()> {
        Self::apply_hostname(config)?;
        Self::apply_timezone(config)?;
        Self::apply_locale(config)?;
        Self::apply_keymap(config)?;
        Self::apply_sysctl(config)?;
        Self::apply_limits(config)?;
        Ok(())
    }

    fn apply_hostname(config: &SystemConfig) -> Result<()> {
        if let Some(ref hostname) = config.hostname {
            let hostname_path = Path::new("/etc/hostname");
            fs::write(hostname_path, format!("{}\n", hostname))?;

            Command::new("hostname").arg(hostname).status().ok();

            let hosts_path = Path::new("/etc/hosts");
            if hosts_path.exists() {
                let content = fs::read_to_string(hosts_path)?;
                let new_content = Self::update_hosts_with_hostname(&content, hostname);
                fs::write(hosts_path, new_content)?;
            }

            println!("Applied hostname: {}", hostname);
        }
        Ok(())
    }

    fn update_hosts_with_hostname(content: &str, hostname: &str) -> String {
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let mut found = false;

        for line in &mut lines {
            if line.starts_with("127.0.1.1") || line.starts_with("127.0.0.1") {
                if line.contains("localhost") {
                    continue;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let ip = parts[0];
                    *line = format!("{}\t{}", ip, hostname);
                    found = true;
                    break;
                }
            }
        }

        if !found {
            lines.push(format!("127.0.1.1\t{}", hostname));
        }

        lines.join("\n")
    }

    fn apply_timezone(config: &SystemConfig) -> Result<()> {
        if let Some(ref timezone) = config.timezone {
            let tz_path = Path::new("/etc/localtime");
            let zone_path = Path::new("/usr/share/zoneinfo").join(timezone);

            if !zone_path.exists() {
                eprintln!("Warning: timezone zoneinfo not found: {}", timezone);
                return Ok(());
            }

            if tz_path.exists() || tz_path.is_symlink() {
                fs::remove_file(tz_path)?;
            }

            std::os::unix::fs::symlink(&zone_path, tz_path)?;

            if let Some(tz_content) = timezone.strip_prefix('/') {
                let etc_timezone = Path::new("/etc/timezone");
                fs::write(etc_timezone, format!("{}\n", tz_content))?;
            } else {
                let etc_timezone = Path::new("/etc/timezone");
                fs::write(etc_timezone, format!("{}\n", timezone))?;
            }

            println!("Applied timezone: {}", timezone);
        }
        Ok(())
    }

    fn apply_locale(config: &SystemConfig) -> Result<()> {
        Self::apply_locale_gen(config)?;
        Self::apply_locale_conf(config)?;
        Ok(())
    }

    fn apply_locale_gen(config: &SystemConfig) -> Result<()> {
        if let Some(ref locales) = config.locales {
            let locale_gen_path = Path::new("/etc/locale.gen");
            let mut content = String::new();

            if locale_gen_path.exists() {
                content = fs::read_to_string(locale_gen_path)?;
            }

            for locale_entry in locales {
                let locale_line = locale_entry.trim();
                if locale_line.is_empty() {
                    continue;
                }

                let locale_name: String = locale_line
                    .split_whitespace()
                    .next()
                    .unwrap_or(locale_line)
                    .to_string();

                let already_enabled = content
                    .lines()
                    .any(|l| l.trim() == locale_line || l.trim() == locale_name);

                if !already_enabled {
                    let commented_pattern = format!("#{}", locale_line);
                    let commented_with_space = format!("# {}", locale_line);
                    let commented_name = format!("#{}", locale_name);
                    let commented_name_space = format!("# {}", locale_name);

                    if content.contains(&commented_pattern) {
                        content = content.replace(&commented_pattern, locale_line);
                    } else if content.contains(&commented_with_space) {
                        content = content.replace(&commented_with_space, locale_line);
                    } else if content.contains(&commented_name) {
                        content = content.replace(&commented_name, locale_line);
                    } else if content.contains(&commented_name_space) {
                        content = content.replace(&commented_name_space, locale_line);
                    } else {
                        if !content.ends_with('\n') && !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(locale_line);
                        content.push('\n');
                    }
                }
            }

            fs::write(locale_gen_path, content)?;

            Command::new("locale-gen").status().ok();

            println!("Applied locales: {:?}", locales);
        }
        Ok(())
    }

    fn apply_locale_conf(config: &SystemConfig) -> Result<()> {
        let locale_conf_path = Path::new("/etc/locale.conf");
        let mut existing = Vec::new();

        if locale_conf_path.exists() {
            let content = fs::read_to_string(locale_conf_path)?;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    existing.push(line.to_string());
                }
            }
        }

        let mut lines = existing.clone();

        if let Some(ref locale_conf) = config.locale_conf {
            let mut handled_keys = Vec::new();

            for (key, value) in locale_conf {
                handled_keys.push(key.clone());
                let new_entry = format!("{}={}", key, value);

                let mut found = false;
                for line in &mut lines {
                    if line.starts_with(&format!("{}=", key))
                        || line.starts_with(&format!("{} =", key))
                    {
                        *line = new_entry.clone();
                        found = true;
                        break;
                    }
                }

                if !found {
                    lines.push(new_entry);
                }
            }

            for key in &handled_keys {
                if let Some(lang) = locale_conf.get(key) {
                    if key == "LANG" {
                        Command::new("localectl")
                            .args(["set-locale", lang])
                            .status()
                            .ok();
                    }
                }
            }

            println!("Applied locale.conf: {:?}", locale_conf);
        } else if let Some(ref locale) = config.locale {
            let mut found = false;
            for line in &mut lines {
                if line.starts_with("LANG=") || line.starts_with("LANG =") {
                    *line = format!("LANG={}", locale);
                    found = true;
                    break;
                }
            }
            if !found {
                lines.push(format!("LANG={}", locale));
            }

            Command::new("localectl")
                .args(["set-locale", locale])
                .status()
                .ok();

            println!("Applied locale: {}", locale);
        }

        let output = lines.join("\n");
        fs::write(locale_conf_path, format!("{}\n", output))?;

        Ok(())
    }

    fn apply_keymap(config: &SystemConfig) -> Result<()> {
        if let Some(ref keymap) = config.keymap {
            Command::new("localectl")
                .args(["set-keymap", keymap])
                .status()
                .ok();

            let vconsole_path = Path::new("/etc/vconsole.conf");
            let mut content = String::new();
            if vconsole_path.exists() {
                content = fs::read_to_string(vconsole_path)?;
            }

            let mut updated = false;
            let mut lines: Vec<String> = Vec::new();
            for line in content.lines() {
                if line.starts_with("KEYMAP=") {
                    lines.push(format!("KEYMAP={}", keymap));
                    updated = true;
                } else {
                    lines.push(line.to_string());
                }
            }
            if !updated {
                lines.push(format!("KEYMAP={}", keymap));
            }

            fs::write(vconsole_path, format!("{}\n", lines.join("\n")))?;

            println!("Applied keymap: {}", keymap);
        }
        Ok(())
    }

    fn apply_sysctl(config: &SystemConfig) -> Result<()> {
        if let Some(ref sysctl) = config.sysctl {
            let sysctl_conf_path = Path::new("/etc/sysctl.d/99-rscm.conf");

            if let Some(parent) = sysctl_conf_path.parent() {
                fs::create_dir_all(parent)?;
            }

            let mut lines = Vec::new();
            lines.push("# Managed by rscm - do not edit manually".to_string());
            for (key, value) in sysctl {
                lines.push(format!("{} = {}", key, value));
            }

            fs::write(sysctl_conf_path, format!("{}\n", lines.join("\n")))?;

            Command::new("sysctl").arg("--system").status().ok();

            println!("Applied sysctl settings");
        }
        Ok(())
    }

    fn apply_limits(config: &SystemConfig) -> Result<()> {
        if let Some(ref limits) = config.limits {
            let limits_conf_path = Path::new("/etc/security/limits.d/99-rscm.conf");

            if let Some(parent) = limits_conf_path.parent() {
                fs::create_dir_all(parent)?;
            }

            let mut lines = Vec::new();
            lines.push("# Managed by rscm - do not edit manually".to_string());
            for (key, value) in limits {
                let (domain, limit_type) = if key.contains(':') {
                    let parts: Vec<&str> = key.splitn(2, ':').collect();
                    (parts[0].to_string(), parts[1].to_string())
                } else {
                    ("*".to_string(), key.clone())
                };

                lines.push(format!("{} {} {}", domain, limit_type, value));
            }

            fs::write(limits_conf_path, format!("{}\n", lines.join("\n")))?;

            println!("Applied limits settings");
        }
        Ok(())
    }
}
