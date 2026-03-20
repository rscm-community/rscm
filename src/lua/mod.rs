pub mod builtins;

use crate::config::package::{BuildOptions, BuildType, PackageOptions};
use crate::config::{
    Configuration, DotfilesConfig, FirewallConfig, InterfaceConfig, NetworkConfig, PackageConfig,
    ServiceConfig, SystemConfig, SystemProfile, UserConfig,
};
use crate::lua::builtins::register_builtins;
use anyhow::Result;
use mlua::{Lua, Table, Value};
use std::collections::HashMap;
use std::sync::Arc;

pub struct LuaEngine {
    lua: Lua,
    state: Arc<builtins::BuiltinState>,
}

impl<'a> LuaEngine {
    pub fn new() -> Result<Self> {
        let lua = Lua::new();

        lua.set_memory_limit(128 * 1024 * 1024)
            .expect("ERROR: set memory limit failed");

        let state = Arc::new(builtins::BuiltinState::default());
        register_builtins(&lua, state.clone())?;

        let engine = Self { lua, state };
        engine.register_sections()?;

        Ok(engine)
    }
    fn register_sections(&self) -> Result<()> {
        let globals = self.lua.globals();
        let config_root = self.lua.create_table()?;
        globals.set("__config_root", config_root.clone())?;

        for section in &[
            "system", "packages", "services", "users", "kernel", "boot", "network", "sources",
            "systems",
        ] {
            let name = section.to_string();

            let setter = self.lua.create_function(move |lua: &Lua, table: Table| {
                let root: Table = lua.globals().get("__config_root")?;
                root.set(name.clone(), table)?;
                Ok(())
            })?;

            globals.set(*section, setter)?;
        }

        Ok(())
    }

    pub fn load_config(&self, content: &str) -> Result<Configuration> {
        let fresh = self.lua.create_table()?;
        self.lua.globals().set("__config_root", fresh)?;

        let systems_collector = self.lua.create_table()?;
        self.lua
            .globals()
            .set("__systems_collector", systems_collector)?;

        self.lua.load(content).exec()?;

        let root: Table = self.lua.globals().get("__config_root")?;

        let mut config = Configuration::default();

        if let Ok(system_section) = root.get::<Table>("system") {
            config.system = Some(self.parse_system(system_section)?);
        }

        if let Ok(packages_section) = root.get::<Table>("packages") {
            config.packages = self.parse_packages(packages_section)?;
        }

        if let Ok(services_section) = root.get::<Table>("services") {
            config.services = self.parse_services(services_section)?;
        }

        if let Ok(users_section) = root.get::<Table>("users") {
            config.users = self.parse_users(users_section)?;
        }

        if let Ok(network_section) = root.get::<Table>("network") {
            config.network = Some(self.parse_network(network_section)?);
        }

        if let Ok(systems_section) = root.get::<Table>("systems") {
            let systems = self.parse_systems(systems_section)?;
            config.systems = self.resolve_system_inheritance(systems)?;
        }

        Ok(config)
    }

    fn resolve_system_inheritance(
        &self,
        mut systems: HashMap<String, SystemProfile>,
    ) -> Result<HashMap<String, SystemProfile>> {
        let mut resolved = HashMap::new();
        let mut visiting = Vec::new();

        fn resolve_one(
            name: &str,
            systems: &HashMap<String, SystemProfile>,
            resolved: &mut HashMap<String, SystemProfile>,
            visiting: &mut Vec<String>,
            merge_fn: fn(SystemProfile, SystemProfile) -> Result<SystemProfile>,
        ) -> Result<SystemProfile> {
            if let Some(p) = resolved.get(name) {
                return Ok(p.clone());
            }
            if visiting.iter().any(|n| n == name) {
                anyhow::bail!("circular inheritance detected: {}", name);
            }
            let profile = systems
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("system '{}' not found", name))?;

            visiting.push(name.to_string());
            let mut result = profile.clone();
            if !profile.inherits.is_empty() {
                for parent_name in &profile.inherits {
                    let parent = resolve_one(parent_name, systems, resolved, visiting, merge_fn)?;
                    result = merge_fn(parent, result)?;
                }
            }
            visiting.pop();
            resolved.insert(name.to_string(), result.clone());
            Ok(result)
        }

        for name in systems.keys() {
            let name = name.clone();
            let result = resolve_one(
                &name,
                &systems,
                &mut resolved,
                &mut visiting,
                Self::merge_profiles,
            )?;
            resolved.insert(name, result);
        }

        Ok(resolved)
    }

    fn merge_profiles(base: SystemProfile, override_: SystemProfile) -> Result<SystemProfile> {
        let mut result = base;
        if override_.description.is_some() {
            result.description = override_.description;
        }
        if override_.system.is_some() {
            result.system = override_.system;
        }
        if override_.packages.is_some() {
            if result.packages.is_none() {
                result.packages = override_.packages;
            } else if let Some(override_pkgs) = override_.packages {
                let mut merged_pkgs = result.packages.take().unwrap();
                merged_pkgs.list.extend(override_pkgs.list);
                for (k, v) in override_pkgs.map {
                    merged_pkgs.map.insert(k, v);
                }
                result.packages = Some(merged_pkgs);
            }
        }
        if override_.services.is_some() {
            if result.services.is_none() {
                result.services = override_.services;
            } else if let Some(override_svcs) = override_.services {
                let merged_svcs = result.services.take().unwrap();
                let mut merged = merged_svcs;
                merged.extend(override_svcs);
                result.services = Some(merged);
            }
        }
        if override_.users.is_some() {
            if result.users.is_none() {
                result.users = override_.users;
            } else if let Some(override_usrs) = override_.users {
                let merged_usrs = result.users.take().unwrap();
                let mut merged = merged_usrs;
                merged.extend(override_usrs);
                result.users = Some(merged);
            }
        }
        if override_.network.is_some() {
            result.network = override_.network;
        }
        result.inherits = override_.inherits;
        Ok(result)
    }
    pub fn parse_system(&self, section: Table) -> Result<SystemConfig> {
        let mut config = SystemConfig::default();
        config.hostname = section.get::<String>("hostname").ok();
        config.timezone = section.get::<String>("timezone").ok();
        config.locale = section.get::<String>("locale").ok();
        config.keymap = section.get::<String>("keymap").ok();
        config.architecture = section.get::<String>("architecture").ok();
        config.locales = section.get::<Vec<String>>("locales").ok();
        config.locale_conf = section
            .get::<std::collections::HashMap<String, String>>("locale_conf")
            .ok();
        config.limits = section
            .get::<std::collections::HashMap<String, String>>("limits")
            .ok();
        config.sysctl = section
            .get::<std::collections::HashMap<String, String>>("sysctl")
            .ok();
        config.cleanup = section
            .get::<std::collections::HashMap<String, String>>("cleanup")
            .ok();
        Ok(config)
    }
    pub fn parse_packages(&self, section: Table) -> Result<PackageConfig> {
        let mut config = PackageConfig::default();
        for pair in section.pairs::<Value, Value>() {
            let (key, value) = pair?;
            match key {
                Value::Integer(_) => {
                    if let Some(s) = value.as_string() {
                        config.list.push(s.to_str()?.to_string());
                    }
                }
                Value::String(key) => {
                    let mut pkg = PackageOptions::default();
                    if let Some(table) = value.as_table() {
                        for p in table.pairs::<String, Value>() {
                            let (k, v) = p?;
                            match k.as_str() {
                                "version" => {
                                    pkg.version = v
                                        .as_string()
                                        .and_then(|s| s.to_str().ok())
                                        .map(|s| s.to_string())
                                        .or_else(|| v.to_string().ok());
                                }
                                "source" => {
                                    pkg.source = v
                                        .as_string()
                                        .and_then(|s| s.to_str().ok())
                                        .map(|s| s.to_string())
                                        .or_else(|| v.to_string().ok());
                                }
                                "dependencies" => {
                                    if let Some(deps_table) = v.as_table() {
                                        let mut deps = Vec::new();
                                        for item in deps_table.sequence_values::<String>() {
                                            if let Ok(s) = item {
                                                deps.push(s);
                                            }
                                        }
                                        pkg.dependencies = deps;
                                    }
                                }
                                "build" => {
                                    if let Some(table) = v.as_table() {
                                        let mut build_options = BuildOptions::default();
                                        for p in table.pairs::<String, Value>() {
                                            let (k, v) = p?;
                                            match k.as_str() {
                                                "type" => {
                                                    if let Some(s) = v.as_string() {
                                                        if let Ok(s_str) = s.to_str() {
                                                            let s_str: &str = &s_str;
                                                            build_options.ty = match s_str {
                                                                "standard" => BuildType::Standard,
                                                                "aur" => BuildType::Aur,
                                                                "source" => BuildType::Source,
                                                                _ => BuildType::Custom(
                                                                    s_str.to_string(),
                                                                ),
                                                            };
                                                        }
                                                    }
                                                }
                                                "args" => {
                                                    if let Some(args_table) = v.as_table() {
                                                        let mut args = Vec::new();
                                                        for item in
                                                            args_table.sequence_values::<String>()
                                                        {
                                                            if let Ok(s) = item {
                                                                args.push(s);
                                                            }
                                                        }
                                                        build_options.args = args;
                                                    }
                                                }
                                                "env" => {
                                                    if let Some(env_table) = v.as_table() {
                                                        let mut env = HashMap::new();
                                                        for pair in
                                                            env_table.pairs::<String, String>()
                                                        {
                                                            if let Ok((key, val)) = pair {
                                                                env.insert(key, val);
                                                            }
                                                        }
                                                        build_options.env = env;
                                                    }
                                                }
                                                "sandbox" => {
                                                    if let Some(sandbox_table) = v.as_table() {
                                                        let mut sandbox = crate::config::package::SandboxOptions::default();
                                                        sandbox.network = sandbox_table
                                                            .get::<bool>("network")
                                                            .unwrap_or(false);
                                                        sandbox.ro_paths = sandbox_table
                                                            .get::<Vec<String>>("ro_paths")
                                                            .unwrap_or_default();
                                                        sandbox.rw_paths = sandbox_table
                                                            .get::<Vec<String>>("rw_paths")
                                                            .unwrap_or_default();
                                                        build_options.sandbox = Some(sandbox);
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                        pkg.build = Some(build_options);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    config.map.insert(key.to_str()?.to_string(), pkg);
                }
                _ => {}
            }
        }
        Ok(config)
    }

    pub fn parse_services(&self, section: Table) -> Result<HashMap<String, ServiceConfig>> {
        let mut services = HashMap::new();
        for pair in section.pairs::<String, Table>() {
            let (key, value) = pair?;
            let mut service = ServiceConfig::default();
            service.enable = value.get::<bool>("enable").unwrap_or(false);
            service.start_now = value.get::<bool>("start_now").unwrap_or(false);
            if let Ok(config_table) = value.get::<Table>("config") {
                for pair in config_table.pairs::<String, String>() {
                    let (k, v) = pair?;
                    service.config.insert(k, toml::Value::String(v));
                }
            }
            service.wanted_by = value.get::<Vec<String>>("wanted_by").unwrap_or_default();
            service.required_by = value.get::<Vec<String>>("required_by").unwrap_or_default();
            service.after = value.get::<Vec<String>>("after").unwrap_or_default();
            service.before = value.get::<Vec<String>>("before").unwrap_or_default();
            services.insert(key, service);
        }
        Ok(services)
    }

    pub fn parse_users(&self, section: Table) -> Result<HashMap<String, UserConfig>> {
        let mut users = HashMap::new();
        for pair in section.pairs::<String, Table>() {
            let (key, value) = pair?;
            let mut user = UserConfig::default();
            user.uid = value.get::<u32>("uid").ok();
            user.groups = value.get::<Vec<String>>("groups").unwrap_or_default();
            user.ssh_keys = value.get::<Vec<String>>("ssh_keys").unwrap_or_default();
            user.system_user = value.get::<bool>("system_user").unwrap_or(false);
            user.shell = value.get::<String>("shell").ok();
            user.home = value.get::<String>("home").ok();
            user.create_home = value.get::<bool>("create_home").unwrap_or(false);
            user.description = value.get::<String>("description").ok();
            if let Ok(dotfiles_table) = value.get::<Table>("dotfiles") {
                let mut dotfiles = DotfilesConfig::default();
                dotfiles.source = dotfiles_table.get::<String>("source").ok();
                dotfiles.files = dotfiles_table
                    .get::<Vec<String>>("files")
                    .unwrap_or_default();
                dotfiles.exclude = dotfiles_table
                    .get::<Vec<String>>("exclude")
                    .unwrap_or_default();
                user.dotfiles = Some(dotfiles);
            }
            users.insert(key, user);
        }
        Ok(users)
    }

    pub fn parse_network(&self, section: Table) -> Result<NetworkConfig> {
        let mut config = NetworkConfig::default();
        config.hostname = section.get::<String>("hostname").ok();
        if let Ok(interfaces_section) = section.get::<Table>("interfaces") {
            for pair in interfaces_section.pairs::<String, Table>() {
                let (key, value) = pair?;
                let mut interface = InterfaceConfig::default();
                interface.dhcp = value.get::<bool>("dhcp").ok();
                interface.address = value.get::<String>("address").ok();
                interface.gateway = value.get::<String>("gateway").ok();
                interface.dns = value.get::<Vec<String>>("dns").unwrap_or_default();
                interface.ssid = value.get::<String>("ssid").ok();
                interface.password = value.get::<String>("password").ok();
                config.interfaces.insert(key, interface);
            }
        }
        if let Ok(firewall_section) = section.get::<Table>("firewall") {
            let mut firewall = FirewallConfig::default();
            firewall.enable = firewall_section.get::<bool>("enable").unwrap_or(false);
            firewall.open_ports = firewall_section
                .get::<Vec<u16>>("open_ports")
                .unwrap_or_default();
            firewall.allowed_services = firewall_section
                .get::<Vec<String>>("allowed_services")
                .unwrap_or_default();
            firewall.trusted_interfaces = firewall_section
                .get::<Vec<String>>("trusted_interfaces")
                .unwrap_or_default();
            config.firewall = Some(firewall);
        }
        Ok(config)
    }

    pub fn parse_systems(&self, section: Table) -> Result<HashMap<String, SystemProfile>> {
        let mut systems = HashMap::new();
        for pair in section.pairs::<String, Table>() {
            let (key, value) = pair?;
            let mut profile = SystemProfile::default();
            profile.description = value.get::<String>("description").ok();
            if let Ok(system_section) = value.get::<Table>("system") {
                profile.system = Some(self.parse_system(system_section)?);
            }
            if let Ok(packages_section) = value.get::<Table>("packages") {
                profile.packages = Some(self.parse_packages(packages_section)?);
            }
            if let Ok(services_section) = value.get::<Table>("services") {
                profile.services = Some(self.parse_services(services_section)?);
            }
            if let Ok(users_section) = value.get::<Table>("users") {
                profile.users = Some(self.parse_users(users_section)?);
            }
            if let Ok(network_section) = value.get::<Table>("network") {
                profile.network = Some(self.parse_network(network_section)?);
            }
            profile.inherits = value.get::<Vec<String>>("inherits").unwrap_or_default();
            systems.insert(key, profile);
        }
        Ok(systems)
    }
}
