use crate::config::Configuration;
use crate::lua::LuaEngine;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub struct ConfigLoader {
    engine: LuaEngine,
}

impl ConfigLoader {
    pub fn new() -> Result<Self> {
        let engine = LuaEngine::new()?;
        Ok(Self { engine })
    }

    pub fn load<P: AsRef<Path>>(&self, path: P) -> Result<Configuration> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        self.load_string(&content)
    }

    pub fn load_string(&self, content: &str) -> Result<Configuration> {
        self.engine.load_config(content)
    }

    pub fn validate(&self, config: &Configuration) -> Result<()> {
        if let Some(system) = &config.system {
            if let Some(hostname) = &system.hostname {
                if hostname.is_empty() {
                    anyhow::bail!("hostname cannot be empty");
                }
                if hostname.contains(' ') {
                    anyhow::bail!("hostname cannot contain spaces");
                }
            }
        }
        if let Some(network) = &config.network {
            for (name, interface) in &network.interfaces {
                if interface.dhcp == Some(true) {
                    continue;
                }
                if interface.address.is_none() && interface.dhcp.is_none() {
                    anyhow::bail!(
                        "interface '{}' must have either dhcp=true or an address",
                        name
                    );
                }
            }
        }
        for (name, user) in &config.users {
            if user.uid.is_none() && !user.system_user {
                anyhow::bail!(
                    "user '{}' must have a uid or be marked as system_user",
                    name
                );
            }
        }
        Ok(())
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new().expect("failed to create config loader")
    }
}
