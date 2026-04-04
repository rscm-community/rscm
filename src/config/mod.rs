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

    #[serde(default)]
    pub boot: Option<BootConfig>,

    #[serde(default)]
    pub programs: HashMap<String, ProgramConfig>,

    #[serde(default)]
    pub hardware: HardwareConfig,

    #[serde(default)]
    pub security: SecurityConfig,

    #[serde(default)]
    pub filesystems: HashMap<String, FilesystemConfig>,

    #[serde(default)]
    pub swapdevices: Vec<SwapDeviceConfig>,

    #[serde(default)]
    pub environment: EnvironmentConfig,

    #[serde(default)]
    pub plugins: HashMap<String, PluginConfig>,

    #[serde(default)]
    pub outputs: Option<OutputsConfig>,

    #[serde(default)]
    pub overlays: Option<HashMap<String, OverlayConfig>>,

    #[serde(default)]
    pub imports: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemConfig {
    pub hostname: Option<String>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub keymap: Option<String>,
    pub locales: Option<Vec<String>>,
    pub locale_conf: Option<HashMap<String, String>>,
    pub limits: Option<HashMap<String, String>>,
    pub sysctl: Option<HashMap<String, String>>,
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
    #[serde(default)]
    pub imports: Vec<String>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BootConfig {
    pub kernel: Option<KernelConfig>,
    pub kernel_modules: Option<KernelModulesConfig>,
    pub initrd: Option<InitrdConfig>,
    pub loader: Option<LoaderConfig>,
    pub console_log_level: Option<u32>,
    pub dev_shm_size: Option<String>,
    pub grow_partition: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KernelConfig {
    pub package: Option<String>,
    pub packages: Option<Vec<String>>,
    pub params: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KernelModulesConfig {
    pub enable: Option<bool>,
    pub blacklist: Option<Vec<String>>,
    pub extra: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InitrdConfig {
    pub enable: Option<bool>,
    pub kernel_modules: Option<Vec<String>>,
    pub available_kernel_modules: Option<Vec<String>>,
    pub luks: Option<LuksConfig>,
    pub systemd: Option<SystemdInitrdConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LuksConfig {
    pub devices: Option<HashMap<String, LuksDeviceConfig>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LuksDeviceConfig {
    pub device: Option<String>,
    pub allow_discards: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemdInitrdConfig {
    pub enable: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoaderConfig {
    pub ty: Option<String>,
    pub systemd_boot: Option<SystemdBootConfig>,
    pub grub: Option<GrubConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemdBootConfig {
    pub enable: Option<bool>,
    pub configuration_limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrubConfig {
    pub enable: Option<bool>,
    pub device: Option<String>,
    pub efi_support: Option<bool>,
    pub configuration_limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProgramConfig {
    pub enable: Option<bool>,
    pub config: Option<HashMap<String, String>>,
    pub aliases: Option<HashMap<String, String>>,
    pub package: Option<String>,
    pub default_editor: Option<bool>,
    pub oh_my_zsh: Option<OhMyZshConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OhMyZshConfig {
    pub enable: Option<bool>,
    pub theme: Option<String>,
    pub plugins: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardwareConfig {
    pub graphics: Option<GraphicsConfig>,
    pub bluetooth: Option<BluetoothConfig>,
    pub pulseaudio: Option<PulseAudioConfig>,
    pub firmware: Option<Vec<String>>,
    pub opengl: Option<OpenGLConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphicsConfig {
    pub enable: Option<bool>,
    pub driver: Option<String>,
    pub extra_packages: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BluetoothConfig {
    pub enable: Option<bool>,
    pub package: Option<String>,
    pub settings: Option<HashMap<String, HashMap<String, String>>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PulseAudioConfig {
    pub enable: Option<bool>,
    pub package: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenGLConfig {
    pub enable: Option<bool>,
    pub dri_drivers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub sudo: Option<SudoConfig>,
    pub pam: Option<PamConfig>,
    pub polkit: Option<PolkitConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SudoConfig {
    pub enable: Option<bool>,
    pub wheel_needs_password: Option<bool>,
    pub exec_wheel_only: Option<bool>,
    pub keep_terminfo: Option<bool>,
    pub extra_rules: Option<Vec<SudoRule>>,
    pub extra_config: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SudoRule {
    pub users: Option<Vec<String>>,
    pub groups: Option<Vec<String>>,
    pub commands: Option<Vec<String>>,
    pub options: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PamConfig {
    pub enable: Option<bool>,
    pub services: Option<HashMap<String, PamServiceConfig>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PamServiceConfig {
    pub enable: Option<bool>,
    pub touch_id_auth: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolkitConfig {
    pub enable: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilesystemConfig {
    pub device: Option<String>,
    pub fs_type: Option<String>,
    pub options: Option<Vec<String>>,
    pub mount_point: Option<String>,
    pub subvolumes: Option<HashMap<String, SubvolumeConfig>>,
    pub needed_for_boot: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubvolumeConfig {
    pub mount_point: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SwapDeviceConfig {
    pub device: Option<String>,
    pub label: Option<String>,
    pub size: Option<i64>,
    pub priority: Option<i64>,
    pub options: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvironmentConfig {
    pub variables: Option<HashMap<String, String>>,
    pub session_variables: Option<HashMap<String, String>>,
    pub shell_init: Option<String>,
    pub login_shell_init: Option<String>,
    pub paths_to_link: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginConfig {
    pub enable: Option<bool>,
    pub source: Option<SourceConfig>,
    pub version: Option<String>,
    pub config: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputsConfig {
    pub systems: Option<HashMap<String, SystemProfile>>,
    pub users: Option<HashMap<String, OutputUserConfig>>,
    pub packages: Option<HashMap<String, OutputPackageConfig>>,
    pub dev_envs: Option<HashMap<String, DevEnvConfig>>,
    pub modules: Option<HashMap<String, ModuleConfig>>,
    pub overlays: Option<HashMap<String, OverlayConfig>>,
    pub templates: Option<HashMap<String, TemplateConfig>>,
    pub apps: Option<HashMap<String, AppConfig>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OverlayConfig {
    pub config: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputUserConfig {
    pub description: Option<String>,
    pub home: Option<HomeConfig>,
    pub packages: Option<Vec<String>>,
    pub programs: Option<HashMap<String, ProgramConfig>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HomeConfig {
    pub username: Option<String>,
    pub home_directory: Option<String>,
    pub state_version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputPackageConfig {
    pub src: Option<String>,
    pub build: Option<BuildConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildConfig {
    pub r#type: Option<String>,
    pub pname: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevEnvConfig {
    pub description: Option<String>,
    pub packages: Option<Vec<String>>,
    pub shell_hook: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub build_inputs: Option<Vec<String>>,
    pub services: Option<HashMap<String, DevEnvServiceConfig>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevEnvServiceConfig {
    pub enable: Option<bool>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleConfig {
    pub description: Option<String>,
    pub config: Option<String>,
    pub imports: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemplateConfig {
    pub description: Option<String>,
    pub path: Option<String>,
    pub files: Option<Vec<String>>,
    pub variables: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub r#type: Option<String>,
    pub program: Option<String>,
    pub description: Option<String>,
}
