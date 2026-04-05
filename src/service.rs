use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::ServiceConfig;

const MANAGED_SERVICES_PATH: &str = "/etc/rscm/managed_services.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeclaredServices {
    pub services: Vec<String>,
}

pub struct ServiceTracker {
    path: PathBuf,
}

impl ServiceTracker {
    pub fn new() -> Self {
        Self {
            path: PathBuf::from(MANAGED_SERVICES_PATH),
        }
    }

    pub fn load(&self) -> DeclaredServices {
        if self.path.exists() {
            if let Ok(content) = fs::read_to_string(&self.path) {
                if let Ok(services) = toml::from_str(&content) {
                    return services;
                }
            }
        }
        DeclaredServices::default()
    }

    pub fn save(&self, services: &DeclaredServices) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(services)?;
        fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn compute_removed(
        &self,
        current_services: &HashMap<String, ServiceConfig>,
    ) -> Vec<String> {
        let declared = self.load();
        let declared_set: HashSet<&str> = declared.services.iter().map(|s| s.as_str()).collect();
        let current_set: HashSet<&str> = current_services.keys().map(|s| s.as_str()).collect();

        declared_set
            .difference(&current_set)
            .map(|s| s.to_string())
            .collect()
    }

    pub fn update_declared(&self, current_services: &HashMap<String, ServiceConfig>) -> Result<()> {
        let mut names: Vec<String> = current_services.keys().cloned().collect();
        names.sort();
        self.save(&DeclaredServices { services: names })
    }
}

pub struct ServiceApplier;

impl ServiceApplier {
    pub fn apply(services: &HashMap<String, ServiceConfig>) -> Result<()> {
        let tracker = ServiceTracker::new();
        let removed = tracker.compute_removed(services);

        let mut needs_daemon_reload = false;

        for name in &removed {
            Self::stop_and_disable_removed(name)?;
            needs_daemon_reload = true;
        }

        for (name, config) in services {
            if config.enable {
                Self::apply_service(name, config)?;
                needs_daemon_reload = true;
            } else {
                Self::disable_service(name)?;
            }
        }

        tracker.update_declared(services)?;

        if needs_daemon_reload {
            Command::new("systemctl").arg("daemon-reload").status().ok();
        }

        Ok(())
    }

    fn stop_and_disable_removed(name: &str) -> Result<()> {
        let service_name = if name.ends_with(".service") {
            name.to_string()
        } else {
            format!("{}.service", name)
        };

        Command::new("systemctl")
            .arg("stop")
            .arg(&service_name)
            .status()
            .ok();

        Command::new("systemctl")
            .arg("disable")
            .arg(&service_name)
            .status()
            .ok();

        let drop_in_dir = Path::new("/etc/systemd/system").join(format!("{}.service.d", name));
        if drop_in_dir.exists() {
            let _ = fs::remove_dir_all(&drop_in_dir);
        }

        let etc_config = Path::new("/etc").join(name);
        if etc_config.exists() {
            let _ = fs::remove_dir_all(&etc_config);
        }

        println!("Removed managed service: {}", name);

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
