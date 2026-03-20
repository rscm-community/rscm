pub mod loader;
pub mod package;

use crate::config::package::PackageOptions;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceConfig {
    pub source_type: SourceType,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub r#ref: Option<String>,
    pub path: Option<String>,
    pub track_git: Option<bool>,
    pub url: Option<String>,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    GitHub,
    Path,
    DirectUrl,
}

impl Default for SourceType {
    fn default() -> Self {
        SourceType::Path
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Configuration {
    #[serde(default)]
    pub system: Option<SystemConfig>,

    #[serde(default)]
    pub packages: PackageConfig,

    #[serde(default)]
    pub services: HashMap<String, ServiceConfig>,

    #[serde(default)]
    pub users: HashMap<String, UserConfig>,

    #[serde(default)]
    pub network: Option<NetworkConfig>,

    #[serde(default)]
    pub sources: HashMap<String, SourceConfig>,

    #[serde(default)]
    pub systems: HashMap<String, SystemProfile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemConfig {
    pub hostname: Option<String>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub keymap: Option<String>,
    pub architecture: Option<String>,
    pub locales: Option<Vec<String>>,
    pub locale_conf: Option<HashMap<String, String>>,
    pub limits: Option<HashMap<String, String>>,
    pub sysctl: Option<HashMap<String, String>>,
    pub swap: Option<HashMap<String, String>>,
    pub filesystems: Option<HashMap<String, String>>,
    pub cleanup: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageConfig {
    pub list: Vec<String>,
    pub map: HashMap<String, PackageOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub enable: bool,
    #[serde(default)]
    pub start_now: bool,
    #[serde(default)]
    pub config: HashMap<String, toml::Value>,
    #[serde(default)]
    pub wanted_by: Vec<String>,
    #[serde(default)]
    pub required_by: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub before: Vec<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub unit: Option<UnitConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnitConfig {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub documentation: Vec<String>,
    #[serde(default)]
    pub part_of: Vec<String>,
    #[serde(default)]
    pub binds_to: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    pub uid: Option<u32>,
    pub groups: Vec<String>,
    #[serde(default)]
    pub ssh_keys: Vec<String>,
    #[serde(default)]
    pub system_user: bool,
    #[serde(default)]
    pub dotfiles: Option<DotfilesConfig>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub home: Option<String>,
    #[serde(default)]
    pub create_home: bool,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DotfilesConfig {
    pub source: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub hostname: Option<String>,
    #[serde(default)]
    pub interfaces: HashMap<String, InterfaceConfig>,
    #[serde(default)]
    pub firewall: Option<FirewallConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InterfaceConfig {
    pub dhcp: Option<bool>,
    pub address: Option<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    pub ssid: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FirewallConfig {
    pub enable: bool,
    #[serde(default)]
    pub open_ports: Vec<u16>,
    #[serde(default)]
    pub allowed_services: Vec<String>,
    #[serde(default)]
    pub trusted_interfaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemProfile {
    pub description: Option<String>,
    pub system: Option<SystemConfig>,
    pub packages: Option<PackageConfig>,
    pub services: Option<HashMap<String, ServiceConfig>>,
    pub users: Option<HashMap<String, UserConfig>>,
    pub network: Option<NetworkConfig>,
    #[serde(default)]
    pub inherits: Vec<String>,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            enable: false,
            start_now: false,
            config: HashMap::new(),
            wanted_by: Vec::new(),
            required_by: Vec::new(),
            after: Vec::new(),
            before: Vec::new(),
            environment: HashMap::new(),
            unit: None,
        }
    }
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            uid: None,
            groups: Vec::new(),
            ssh_keys: Vec::new(),
            system_user: false,
            dotfiles: None,
            shell: None,
            home: None,
            create_home: false,
            description: None,
        }
    }
}
