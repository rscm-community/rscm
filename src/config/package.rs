use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PackageOptions {
    pub version: Option<String>,
    pub source: Option<String>,
    pub versions: Option<HashMap<String, VersionOptions>>,
    pub default_version: Option<String>,
    pub groups: Vec<String>,
    pub dependencies: Vec<String>,
    pub build: Option<BuildOptions>,
    pub env: HashMap<String, String>,
    pub hooks: PackageHooks,
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub custom: HashMap<String, toml::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionOptions {
    pub version: String,
    pub default: bool,
    pub source: Option<String>,
    pub build: Option<BuildOptions>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildOptions {
    pub ty: BuildType,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub host_tools: Vec<String>,
    pub sandbox: Option<SandboxOptions>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildType {
    #[default]
    Standard,
    Aur,
    Source,
    Custom(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxOptions {
    pub network: bool,
    pub ro_paths: Vec<String>,
    pub rw_paths: Vec<String>,
    pub tmpfs: Vec<String>,
    pub cpu_limit: Option<String>,
    pub mem_limit: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageHooks {
    pub pre_install: Vec<String>,
    pub post_install: Vec<String>,
    pub pre_remove: Vec<String>,
    pub post_remove: Vec<String>,
    pub pre_update: Vec<String>,
    pub post_update: Vec<String>,
}
