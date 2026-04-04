use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::config::ServiceConfig;

pub struct ServiceApplier;

impl ServiceApplier {
    pub fn apply(services: &HashMap<String, ServiceConfig>) -> Result<()> {
        let mut needs_daemon_reload = false;

        for (name, config) in services {
            if config.enable {
                Self::apply_service(name, config)?;
                needs_daemon_reload = true;
            } else {
                Self::disable_service(name)?;
            }
        }

        if needs_daemon_reload {
            Command::new("systemctl").arg("daemon-reload").status().ok();
        }

        Ok(())
    }

    fn apply_service(name: &str, config: &ServiceConfig) -> Result<()> {
        if !config.config.is_empty() {
            Self::generate_config_files(name, config)?;
        }

        Self::generate_drop_in_unit(name, config)?;

        Self::enable_service(name)?;

        if config.start_now {
            Self::start_service(name)?;
        }

        Ok(())
    }

    fn generate_drop_in_unit(name: &str, config: &ServiceConfig) -> Result<()> {
        let has_unit_config = config.unit.is_some()
            || !config.wanted_by.is_empty()
            || !config.required_by.is_empty()
            || !config.after.is_empty()
            || !config.before.is_empty()
            || !config.environment.is_empty();

        if !has_unit_config {
            return Ok(());
        }

        let drop_in_dir = Path::new("/etc/systemd/system").join(format!("{}.service.d", name));
        fs::create_dir_all(&drop_in_dir)?;

        let mut conf_content = String::new();

        if let Some(ref unit) = config.unit {
            conf_content.push_str("[Unit]\n");
            if let Some(ref desc) = unit.description {
                conf_content.push_str(&format!("Description={}\n", desc));
            }
            for doc in &unit.documentation {
                conf_content.push_str(&format!("Documentation={}\n", doc));
            }
            if !unit.part_of.is_empty() {
                conf_content.push_str(&format!("PartOf={}\n", unit.part_of.join(" ")));
            }
            if !unit.binds_to.is_empty() {
                conf_content.push_str(&format!("BindsTo={}\n", unit.binds_to.join(" ")));
            }
            if !unit.conflicts.is_empty() {
                conf_content.push_str(&format!("Conflicts={}\n", unit.conflicts.join(" ")));
            }
        }

        if !config.after.is_empty() {
            conf_content.push_str(&format!("After={}\n", config.after.join(" ")));
        }
        if !config.before.is_empty() {
            conf_content.push_str(&format!("Before={}\n", config.before.join(" ")));
        }
        if !config.wanted_by.is_empty() {
            conf_content.push_str("[Install]\n");
            conf_content.push_str(&format!("WantedBy={}\n", config.wanted_by.join(" ")));
        }
        if !config.required_by.is_empty() {
            conf_content.push_str("[Install]\n");
            conf_content.push_str(&format!("RequiredBy={}\n", config.required_by.join(" ")));
        }

        if !config.environment.is_empty() {
            conf_content.push_str("[Service]\n");
            let env_parts: Vec<String> = config
                .environment
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            conf_content.push_str(&format!("Environment={}\n", env_parts.join(" ")));
        }

        let override_path = drop_in_dir.join("rscm.conf");
        fs::write(&override_path, conf_content)?;

        println!("Generated drop-in config for service: {}", name);

        Ok(())
    }

    fn generate_config_files(name: &str, config: &ServiceConfig) -> Result<()> {
        let etc_dir = Path::new("/etc").join(name);
        fs::create_dir_all(&etc_dir)?;

        for (key, value) in &config.config {
            let file_path = etc_dir.join(key);

            let content = Self::toml_value_to_string(value);
            fs::write(&file_path, content)?;

            println!("Generated config file: {}", file_path.display());
        }

        Ok(())
    }

    fn toml_value_to_string(value: &toml::Value) -> String {
        match value {
            toml::Value::String(s) => s.clone(),
            toml::Value::Integer(i) => i.to_string(),
            toml::Value::Float(f) => f.to_string(),
            toml::Value::Boolean(b) => b.to_string(),
            toml::Value::Datetime(dt) => dt.to_string(),
            toml::Value::Array(arr) => {
                let items: Vec<String> = arr.iter().map(Self::toml_value_to_string).collect();
                items.join("\n")
            }
            toml::Value::Table(table) => {
                let mut lines = Vec::new();
                for (k, v) in table {
                    lines.push(format!("{} = {}", k, Self::toml_value_to_string(v)));
                }
                lines.join("\n")
            }
        }
    }

    fn enable_service(name: &str) -> Result<()> {
        let service_name = if name.ends_with(".service") {
            name.to_string()
        } else {
            format!("{}.service", name)
        };

        Command::new("systemctl")
            .arg("enable")
            .arg(&service_name)
            .status()
            .ok();

        println!("Enabled service: {}", name);

        Ok(())
    }

    fn disable_service(name: &str) -> Result<()> {
        let service_name = if name.ends_with(".service") {
            name.to_string()
        } else {
            format!("{}.service", name)
        };

        Command::new("systemctl")
            .arg("disable")
            .arg(&service_name)
            .status()
            .ok();

        println!("Disabled service: {}", name);

        Ok(())
    }

    fn start_service(name: &str) -> Result<()> {
        let service_name = if name.ends_with(".service") {
            name.to_string()
        } else {
            format!("{}.service", name)
        };

        Command::new("systemctl")
            .arg("start")
            .arg(&service_name)
            .status()
            .ok();

        println!("Started service: {}", name);

        Ok(())
    }
}
