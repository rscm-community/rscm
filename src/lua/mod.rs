pub mod builtins;

use crate::config::package::{BuildOptions, BuildType, PackageOptions, VersionOptions};
use crate::config::{
    Configuration, DotfilesConfig, FirewallConfig, InterfaceConfig, NetworkConfig, PackageConfig,
    ServiceConfig, SystemConfig, SystemProfile, UserConfig,
};
use crate::lua::builtins::register_builtins;
use anyhow::Result;
use mlua::serde::LuaSerdeExt;
use mlua::{Lua, Table, Value};
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
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

        let outputs_result = self.lua.create_table()?;
        globals.set("__outputs_result", outputs_result.clone())?;

        for section in &[
            "system",
            "packages",
            "services",
            "users",
            "boot",
            "network",
            "sources",
            "systems",
            "programs",
            "hardware",
            "security",
            "filesystems",
            "swapdevices",
            "environment",
            "plugins",
            "imports",
            "overlays",
        ] {
            let name = section.to_string();

            let setter = self.lua.create_function(move |lua: &Lua, table: Table| {
                let root: Table = lua.globals().get("__config_root")?;
                let existing: Option<Table> = root.get(name.clone()).ok();

                if let Some(existing_table) = existing {
                    for pair in table.pairs::<Value, Value>() {
                        let (key, value) = pair?;
                        match key {
                            Value::Integer(_) => {
                                let len = existing_table.len().unwrap_or(0) as usize;
                                existing_table.set(len + 1, value)?;
                            }
                            Value::String(s) => {
                                if let Some(new_table) = value.as_table() {
                                    if let Ok(existing_val) =
                                        existing_table.get::<Value>(s.to_str()?)
                                    {
                                        if let Some(old_table) = existing_val.as_table() {
                                            for p in new_table.clone().pairs::<Value, Value>() {
                                                let (k, v) = p?;
                                                old_table.set(k, v)?;
                                            }
                                            existing_table.set(s.to_str()?, old_table)?;
                                            continue;
                                        }
                                    }
                                }
                                existing_table.set(s.to_str()?, value)?;
                            }
                            _ => {
                                existing_table.set(key, value)?;
                            }
                        }
                    }
                } else {
                    root.set(name.clone(), table)?;
                }
                Ok(())
            })?;

            globals.set(*section, setter)?;
        }

        let outputs_fn = self
            .lua
            .create_function(move |lua: &Lua, arg: Value| match arg {
                Value::Function(func) => {
                    let globals = lua.globals();
                    let sources_table = lua.create_table()?;
                    if let Ok(srcs) = globals.get::<Table>("__sources_parsed") {
                        for pair in srcs.pairs::<Value, Value>() {
                            let (k, v) = pair?;
                            sources_table.set(k, v)?;
                        }
                    }

                    let result: Table = func.call(sources_table)?;

                    let outputs_result: Table = globals.get("__outputs_result")?;
                    for pair in result.pairs::<Value, Value>() {
                        let (k, v) = pair?;
                        outputs_result.set(k, v)?;
                    }
                    Ok(())
                }
                Value::Table(table) => {
                    let globals = lua.globals();
                    let outputs_result: Table = globals.get("__outputs_result")?;
                    for pair in table.pairs::<Value, Value>() {
                        let (k, v) = pair?;
                        outputs_result.set(k, v)?;
                    }
                    Ok(())
                }
                _ => Err(mlua::Error::RuntimeError(
                    "outputs expects a function or table".to_string(),
                )),
            })?;

        globals.set("outputs", outputs_fn)?;

        Ok(())
    }

    pub fn load_config(&self, content: &str, config_path: &Path) -> Result<Configuration> {
        self.load_config_recursive(content, config_path, &mut HashSet::new())
    }

    fn load_config_recursive(
        &self,
        content: &str,
        config_path: &Path,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<Configuration> {
        let canonical = config_path
            .canonicalize()
            .unwrap_or_else(|_| config_path.to_path_buf());
        if !visited.insert(canonical.clone()) {
            return Ok(Configuration::default());
        }

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

        if let Ok(sources_section) = root.get::<Table>("sources") {
            config.sources = self.parse_sources(sources_section)?;
            let sources_parsed = self.lua.create_table()?;
            for (k, v) in &config.sources {
                let src_table = self.lua.create_table()?;
                if let Some(ref owner) = v.owner {
                    src_table.set("owner", owner.as_str())?;
                }
                if let Some(ref repo) = v.repo {
                    src_table.set("repo", repo.as_str())?;
                }
                if let Some(ref r#ref) = v.r#ref {
                    src_table.set("ref", r#ref.as_str())?;
                }
                if let Some(ref path) = v.path {
                    src_table.set("path", path.as_str())?;
                }
                if let Some(track_git) = v.track_git {
                    src_table.set("track_git", track_git)?;
                }
                sources_parsed.set(k.as_str(), src_table)?;
            }
            self.lua.globals().set("__sources_parsed", sources_parsed)?;
        }

        if let Ok(systems_section) = root.get::<Table>("systems") {
            let systems = self.parse_systems(systems_section)?;
            config.systems = self.resolve_system_inheritance(systems)?;
        }

        if let Ok(boot_section) = root.get::<Table>("boot") {
            config.boot = Some(self.parse_boot(boot_section)?);
        }

        if let Ok(programs_section) = root.get::<Table>("programs") {
            config.programs = self.parse_programs(programs_section)?;
        }

        if let Ok(hardware_section) = root.get::<Table>("hardware") {
            config.hardware = self.parse_hardware(hardware_section)?;
        }

        if let Ok(security_section) = root.get::<Table>("security") {
            config.security = self.parse_security(security_section)?;
        }

        if let Ok(filesystems_section) = root.get::<Table>("filesystems") {
            config.filesystems = self.parse_filesystems(filesystems_section)?;
        }

        if let Ok(swapdevices_section) = root.get::<Table>("swapdevices") {
            config.swapdevices = self.parse_swapdevices(swapdevices_section)?;
        }

        if let Ok(environment_section) = root.get::<Table>("environment") {
            config.environment = self.parse_environment(environment_section)?;
        }

        if let Ok(plugins_section) = root.get::<Table>("plugins") {
            config.plugins = self.parse_plugins(plugins_section)?;
        }

        let outputs_result: Table = self.lua.globals().get("__outputs_result")?;
        if !outputs_result.is_empty() {
            config.outputs = self.parse_outputs_table(outputs_result)?;
        }

        if let Ok(overlays_section) = root.get::<Table>("overlays") {
            config.overlays = Some(self.parse_overlays(overlays_section)?);
        }

        if let Ok(imports_section) = root.get::<Table>("imports") {
            config.imports = imports_section
                .get::<Vec<String>>("imports")
                .unwrap_or_default();
        } else if let Ok(imports_array) = root.get::<Vec<String>>("imports") {
            config.imports = imports_array;
        }

        let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));

        for import_path in &config.imports.clone() {
            let import_path = import_path.trim();
            let resolved = if import_path.starts_with('/') {
                PathBuf::from(import_path)
            } else {
                config_dir.join(import_path)
            };

            if resolved.exists() {
                let import_content = std::fs::read_to_string(&resolved).map_err(|e| {
                    anyhow::anyhow!("Cannot read import file {}: {}", resolved.display(), e)
                })?;

                let imported_config =
                    self.load_config_recursive(&import_content, &resolved, visited)?;
                config.merge(imported_config);
            } else {
                eprintln!("Warning: import file not found: {}", resolved.display());
            }
        }

        Ok(config)
    }

    fn parse_outputs_table(&self, section: Table) -> Result<Option<crate::config::OutputsConfig>> {
        let mut outputs = crate::config::OutputsConfig::default();

        if let Ok(systems_table) = section.get::<Table>("systems") {
            outputs.systems = Some(self.parse_outputs_systems(systems_table)?);
        }

        if let Ok(users_table) = section.get::<Table>("users") {
            outputs.users = Some(self.parse_outputs_users(users_table)?);
        }

        if let Ok(packages_table) = section.get::<Table>("packages") {
            outputs.packages = Some(self.parse_outputs_packages(packages_table)?);
        }

        if let Ok(dev_envs_table) = section.get::<Table>("devEnvs") {
            outputs.dev_envs = Some(self.parse_dev_envs(dev_envs_table)?);
        }

        if let Ok(modules_table) = section.get::<Table>("modules") {
            outputs.modules = Some(self.parse_outputs_modules(modules_table)?);
        }

        if let Ok(overlays_table) = section.get::<Table>("overlays") {
            outputs.overlays = Some(self.parse_overlays(overlays_table)?);
        }

        if let Ok(templates_table) = section.get::<Table>("templates") {
            outputs.templates = Some(self.parse_templates(templates_table)?);
        }

        if let Ok(apps_table) = section.get::<Table>("apps") {
            outputs.apps = Some(self.parse_apps(apps_table)?);
        }

        Ok(Some(outputs))
    }

    fn resolve_system_inheritance(
        &self,
        systems: HashMap<String, SystemProfile>,
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
        config.pacman_mirrors = section.get::<Vec<String>>("pacman_mirrors").ok();
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
                                "versions" => {
                                    if let Some(versions_table) = v.as_table() {
                                        let mut versions = HashMap::new();
                                        for ver_pair in versions_table.pairs::<String, Table>() {
                                            if let Ok((ver_key, ver_table)) = ver_pair {
                                                let mut ver_opts = VersionOptions {
                                                    version: ver_table
                                                        .get::<String>("version")
                                                        .unwrap_or_else(|_| ver_key.clone()),
                                                    default: ver_table
                                                        .get::<bool>("default")
                                                        .unwrap_or(false),
                                                    ..Default::default()
                                                };
                                                if let Ok(source) =
                                                    ver_table.get::<String>("source")
                                                {
                                                    ver_opts.source = Some(source);
                                                }
                                                if let Ok(deps_table) =
                                                    ver_table.get::<Table>("dependencies")
                                                {
                                                    let mut deps = Vec::new();
                                                    for item in
                                                        deps_table.sequence_values::<String>()
                                                    {
                                                        if let Ok(s) = item {
                                                            deps.push(s);
                                                        }
                                                    }
                                                    ver_opts.dependencies = deps;
                                                }
                                                versions.insert(ver_key, ver_opts);
                                            }
                                        }
                                        pkg.versions = Some(versions);
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

    pub fn parse_sources(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::SourceConfig>> {
        let mut sources = HashMap::new();

        for pair in section.pairs::<String, Table>() {
            let (name, source_table) = pair?;
            let mut config = crate::config::SourceConfig::default();

            if let Ok(source_type) = source_table.get::<String>("type") {
                config.source_type = match source_type.as_str() {
                    "github" => crate::config::SourceType::GitHub,
                    "path" => crate::config::SourceType::Path,
                    "directurl" => crate::config::SourceType::DirectUrl,
                    _ => crate::config::SourceType::Path,
                };
            }
            config.owner = source_table.get::<String>("owner").ok();
            config.repo = source_table.get::<String>("repo").ok();
            config.r#ref = source_table.get::<String>("ref").ok();
            config.path = source_table.get::<String>("path").ok();
            config.track_git = source_table.get::<bool>("track_git").ok();
            config.url = source_table.get::<String>("url").ok();
            config.hash = source_table.get::<String>("hash").ok();

            sources.insert(name, config);
        }

        Ok(sources)
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

    pub fn parse_boot(&self, section: Table) -> Result<crate::config::BootConfig> {
        let mut config = crate::config::BootConfig::default();

        if let Ok(kernel_section) = section.get::<Table>("kernel") {
            config.kernel = Some(self.parse_kernel(kernel_section)?);
        }

        if let Ok(kernel_modules_section) = section.get::<Table>("kernelModules") {
            config.kernel_modules = Some(self.parse_kernel_modules(kernel_modules_section)?);
        }

        if let Ok(initrd_section) = section.get::<Table>("initrd") {
            config.initrd = Some(self.parse_initrd(initrd_section)?);
        }

        if let Ok(loader_section) = section.get::<Table>("loader") {
            config.loader = Some(self.parse_loader(loader_section)?);
        }

        config.console_log_level = section
            .get::<f64>("consoleLogLevel")
            .ok()
            .map(|v| v as u32)
            .or_else(|| section.get::<u32>("consoleLogLevel").ok());

        config.dev_shm_size = section.get::<String>("devShmSize").ok();
        config.grow_partition = section.get::<bool>("growPartition").ok();

        Ok(config)
    }

    fn parse_kernel(&self, section: Table) -> Result<crate::config::KernelConfig> {
        let mut config = crate::config::KernelConfig::default();
        config.package = section.get::<String>("package").ok();
        config.packages = section.get::<Vec<String>>("packages").ok();
        config.params = section.get::<Vec<String>>("params").ok();
        Ok(config)
    }

    fn parse_kernel_modules(&self, section: Table) -> Result<crate::config::KernelModulesConfig> {
        let mut config = crate::config::KernelModulesConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.blacklist = section.get::<Vec<String>>("blacklist").ok();
        config.extra = section.get::<Vec<String>>("extra").ok();
        Ok(config)
    }

    fn parse_initrd(&self, section: Table) -> Result<crate::config::InitrdConfig> {
        let mut config = crate::config::InitrdConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.kernel_modules = section.get::<Vec<String>>("kernelModules").ok();
        config.available_kernel_modules = section.get::<Vec<String>>("availableKernelModules").ok();

        if let Ok(luks_section) = section.get::<Table>("luks") {
            config.luks = Some(self.parse_luks(&luks_section)?);
        }

        if let Ok(systemd_section) = section.get::<Table>("systemd") {
            config.systemd = Some(self.parse_systemd_initrd(&systemd_section)?);
        }

        Ok(config)
    }

    fn parse_luks(&self, section: &Table) -> Result<crate::config::LuksConfig> {
        let mut config = crate::config::LuksConfig::default();
        if let Ok(devices_section) = section.get::<Table>("devices") {
            let mut devices = HashMap::new();
            for pair in devices_section.pairs::<String, Table>() {
                let (name, dev_table) = pair?;
                let mut device_config = crate::config::LuksDeviceConfig::default();
                device_config.device = dev_table.get::<String>("device").ok();
                device_config.allow_discards = dev_table.get::<bool>("allowDiscards").ok();
                devices.insert(name, device_config);
            }
            config.devices = Some(devices);
        }
        Ok(config)
    }

    fn parse_systemd_initrd(&self, section: &Table) -> Result<crate::config::SystemdInitrdConfig> {
        let mut config = crate::config::SystemdInitrdConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        Ok(config)
    }

    fn parse_loader(&self, section: Table) -> Result<crate::config::LoaderConfig> {
        let mut config = crate::config::LoaderConfig::default();
        if let Ok(systemd_boot_section) = section.get::<Table>("systemdBoot") {
            config.systemd_boot = Some(self.parse_systemd_boot(systemd_boot_section)?);
        }
        Ok(config)
    }

    fn parse_systemd_boot(&self, section: Table) -> Result<crate::config::SystemdBootConfig> {
        let mut config = crate::config::SystemdBootConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.configuration_limit = section
            .get::<f64>("configurationLimit")
            .map(|v| v as u32)
            .ok()
            .or_else(|| section.get::<u32>("configurationLimit").ok());
        config.timeout = section
            .get::<f64>("timeout")
            .map(|v| v as u32)
            .ok()
            .or_else(|| section.get::<u32>("timeout").ok());
        Ok(config)
    }
    pub fn parse_programs(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::ProgramConfig>> {
        let mut programs = HashMap::new();
        for pair in section.pairs::<String, Value>() {
            let (key, value) = pair?;
            let mut program = crate::config::ProgramConfig::default();

            match value {
                Value::Boolean(true) => {
                    program.enable = Some(true);
                }
                Value::Boolean(false) => {
                    program.enable = Some(false);
                }
                Value::Table(table) => {
                    program.enable = table.get::<bool>("enable").ok();
                    program.config = if let Ok(config_table) = table.get::<Table>("config") {
                        let mut cfg = HashMap::new();
                        for pair in config_table.pairs::<String, String>() {
                            let (k, v) = pair?;
                            cfg.insert(k, v);
                        }
                        Some(cfg)
                    } else {
                        None
                    };
                    program.aliases = if let Ok(aliases_table) = table.get::<Table>("aliases") {
                        let mut al = HashMap::new();
                        for pair in aliases_table.pairs::<String, String>() {
                            let (k, v) = pair?;
                            al.insert(k, v);
                        }
                        Some(al)
                    } else {
                        None
                    };
                    program.package = table.get::<String>("package").ok();
                    program.default_editor = table.get::<bool>("defaultEditor").ok();

                    if let Ok(oh_my_zsh_table) = table.get::<Table>("oh-my-zsh") {
                        program.oh_my_zsh = Some(self.parse_oh_my_zsh(&oh_my_zsh_table)?);
                    }
                }
                _ => {}
            }

            programs.insert(key.to_string(), program);
        }
        Ok(programs)
    }

    fn parse_nested_table<T: serde::de::DeserializeOwned>(
        &self,
        table: &Table,
        key: &str,
    ) -> Result<Option<T>> {
        if let Ok(value) = table.get::<Value>(key) {
            match value {
                Value::Table(nested_table) => {
                    let lua_value: Value = self.lua.to_value(&nested_table)?;
                    let result: T = self.lua.from_value(lua_value)?;
                    Ok(Some(result))
                }
                _ => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    fn parse_oh_my_zsh(&self, section: &Table) -> Result<crate::config::OhMyZshConfig> {
        let mut config = crate::config::OhMyZshConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.theme = section.get::<String>("theme").ok();
        config.plugins = section.get::<Vec<String>>("plugins").ok();
        Ok(config)
    }

    pub fn parse_hardware(&self, section: Table) -> Result<crate::config::HardwareConfig> {
        let mut config = crate::config::HardwareConfig::default();

        if let Ok(graphics_section) = section.get::<Table>("graphics") {
            config.graphics = Some(self.parse_graphics(&graphics_section)?);
        }

        if let Ok(bluetooth_section) = section.get::<Table>("bluetooth") {
            config.bluetooth = Some(self.parse_bluetooth(&bluetooth_section)?);
        }

        if let Ok(pulseaudio_section) = section.get::<Table>("pulseaudio") {
            config.pulseaudio = Some(self.parse_pulseaudio(&pulseaudio_section)?);
        }

        if let Ok(opengl_section) = section.get::<Table>("opengl") {
            config.opengl = Some(self.parse_opengl(&opengl_section)?);
        }

        if let Ok(bluetooth_section) = section.get::<Table>("bluetooth") {
            config.bluetooth = Some(self.parse_bluetooth(&bluetooth_section)?);
        }

        if let Ok(pulseaudio_section) = section.get::<Table>("pulseaudio") {
            config.pulseaudio = Some(self.parse_pulseaudio(&pulseaudio_section)?);
        }

        config.firmware = section.get::<Vec<String>>("firmware").ok();

        if let Ok(opengl_section) = section.get::<Table>("opengl") {
            config.opengl = Some(self.parse_opengl(&opengl_section)?);
        }

        Ok(config)
    }

    fn parse_graphics(&self, section: &Table) -> Result<crate::config::GraphicsConfig> {
        let mut config = crate::config::GraphicsConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.driver = section.get::<String>("driver").ok();
        config.extra_packages = section.get::<Vec<String>>("extraPackages").ok();
        Ok(config)
    }

    fn parse_bluetooth(&self, section: &Table) -> Result<crate::config::BluetoothConfig> {
        let mut config = crate::config::BluetoothConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.package = section.get::<String>("package").ok();

        if let Ok(settings_table) = section.get::<Table>("settings") {
            let mut settings = HashMap::new();
            for pair in settings_table.pairs::<String, Table>() {
                let (section_name, section_table) = pair?;
                let mut section_map = HashMap::new();
                for inner_pair in section_table.pairs::<String, String>() {
                    let (k, v) = inner_pair?;
                    section_map.insert(k, v);
                }
                settings.insert(section_name, section_map);
            }
            config.settings = Some(settings);
        }

        Ok(config)
    }

    fn parse_pulseaudio(&self, section: &Table) -> Result<crate::config::PulseAudioConfig> {
        let mut config = crate::config::PulseAudioConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.package = section.get::<String>("package").ok();
        Ok(config)
    }

    fn parse_opengl(&self, section: &Table) -> Result<crate::config::OpenGLConfig> {
        let mut config = crate::config::OpenGLConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.dri_drivers = section.get::<Vec<String>>("driDrivers").ok();
        Ok(config)
    }

    pub fn parse_security(&self, section: Table) -> Result<crate::config::SecurityConfig> {
        let mut config = crate::config::SecurityConfig::default();

        if let Ok(sudo_section) = section.get::<Table>("sudo") {
            config.sudo = Some(self.parse_sudo(&sudo_section)?);
        }

        if let Ok(pam_section) = section.get::<Table>("pam") {
            config.pam = Some(self.parse_pam(&pam_section)?);
        }

        if let Ok(polkit_section) = section.get::<Table>("polkit") {
            config.polkit = Some(self.parse_polkit(&polkit_section)?);
        }

        Ok(config)
    }

    fn parse_sudo(&self, section: &Table) -> Result<crate::config::SudoConfig> {
        let mut config = crate::config::SudoConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        config.wheel_needs_password = section.get::<bool>("wheelNeedsPassword").ok();
        config.exec_wheel_only = section.get::<bool>("execWheelOnly").ok();
        config.keep_terminfo = section.get::<bool>("keepTerminfo").ok();
        config.extra_config = section.get::<String>("extraConfig").ok();

        if let Ok(extra_rules_table) = section.get::<Table>("extraRules") {
            let mut rules = Vec::new();
            for item in extra_rules_table.sequence_values::<Table>() {
                if let Ok(rule_table) = item {
                    let mut rule = crate::config::SudoRule::default();
                    rule.users = rule_table.get::<Vec<String>>("users").ok();
                    rule.groups = rule_table.get::<Vec<String>>("groups").ok();
                    rule.commands = rule_table.get::<Vec<String>>("commands").ok();
                    rule.options = rule_table.get::<Vec<String>>("options").ok();
                    rules.push(rule);
                }
            }
            config.extra_rules = Some(rules);
        }

        Ok(config)
    }

    fn parse_pam(&self, section: &Table) -> Result<crate::config::PamConfig> {
        let mut config = crate::config::PamConfig::default();
        config.enable = section.get::<bool>("enable").ok();

        if let Ok(services_table) = section.get::<Table>("services") {
            let mut services = HashMap::new();
            for pair in services_table.pairs::<String, Table>() {
                let (name, service_table) = pair?;
                let mut service = crate::config::PamServiceConfig::default();
                service.enable = service_table.get::<bool>("enable").ok();
                service.touch_id_auth = service_table.get::<bool>("touchIdAuth").ok();
                services.insert(name, service);
            }
            config.services = Some(services);
        }

        Ok(config)
    }

    fn parse_polkit(&self, section: &Table) -> Result<crate::config::PolkitConfig> {
        let mut config = crate::config::PolkitConfig::default();
        config.enable = section.get::<bool>("enable").ok();
        Ok(config)
    }

    pub fn parse_filesystems(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::FilesystemConfig>> {
        let mut filesystems = HashMap::new();

        for pair in section.pairs::<Value, Value>() {
            let (key, value) = pair?;
            let mount_point = key
                .as_string()
                .and_then(|s| s.to_str().ok())
                .map(|s| s.to_string());

            if mount_point.as_ref().map_or(false, |mp| *mp == "tmpfs") {
                if let Some(tmpfs_table) = value.as_table() {
                    for inner_pair in tmpfs_table.pairs::<Value, Value>() {
                        let (mp_key, fs_table) = inner_pair?;
                        let mp = mp_key
                            .as_string()
                            .and_then(|s| s.to_str().ok())
                            .map(|s| s.to_string());
                        if let Some(mp_path) = mp {
                            if let Some(fs_table_val) = fs_table.as_table() {
                                if let Ok(fs_config) = self.parse_filesystem_config(&fs_table_val) {
                                    filesystems.insert(mp_path, fs_config);
                                }
                            }
                        }
                    }
                }
            } else if let Some(fs_table) = value.as_table() {
                if let Some(mp) = mount_point {
                    if let Ok(fs_config) = self.parse_filesystem_config(fs_table) {
                        filesystems.insert(mp, fs_config);
                    }
                }
            }
        }

        Ok(filesystems)
    }

    fn parse_filesystem_config(&self, section: &Table) -> Result<crate::config::FilesystemConfig> {
        let mut config = crate::config::FilesystemConfig::default();
        config.device = section.get::<String>("device").ok();
        config.fs_type = section.get::<String>("fsType").ok();
        config.options = section.get::<Vec<String>>("options").ok();
        config.mount_point = section.get::<String>("mountPoint").ok();
        config.needed_for_boot = section.get::<bool>("neededForBoot").ok();

        if let Ok(subvolumes_table) = section.get::<Table>("subvolumes") {
            let mut subvolumes = HashMap::new();
            for pair in subvolumes_table.pairs::<String, Table>() {
                let (name, subvol_table) = pair?;
                let mut subvol = crate::config::SubvolumeConfig::default();
                subvol.mount_point = subvol_table.get::<String>("mountPoint").ok();
                subvolumes.insert(name, subvol);
            }
            config.subvolumes = Some(subvolumes);
        }

        Ok(config)
    }

    pub fn parse_swapdevices(
        &self,
        section: Table,
    ) -> Result<Vec<crate::config::SwapDeviceConfig>> {
        let mut swapdevices = Vec::new();

        for item in section.sequence_values::<Value>() {
            if let Ok(value) = item {
                if let Some(table) = value.as_table() {
                    let mut device = crate::config::SwapDeviceConfig::default();
                    device.device = table.get::<String>("device").ok();
                    device.label = table.get::<String>("label").ok();
                    device.size = table
                        .get::<f64>("size")
                        .map(|v| v as i64)
                        .ok()
                        .or_else(|| table.get::<i64>("size").ok());
                    device.priority = table
                        .get::<f64>("priority")
                        .map(|v| v as i64)
                        .ok()
                        .or_else(|| table.get::<i64>("priority").ok());
                    device.options = table.get::<Vec<String>>("options").ok();
                    swapdevices.push(device);
                }
            }
        }

        Ok(swapdevices)
    }

    pub fn parse_environment(&self, section: Table) -> Result<crate::config::EnvironmentConfig> {
        let mut config = crate::config::EnvironmentConfig::default();

        if let Ok(variables_table) = section.get::<Table>("variables") {
            let mut variables = HashMap::new();
            for pair in variables_table.pairs::<String, String>() {
                let (k, v) = pair?;
                variables.insert(k, v);
            }
            config.variables = Some(variables);
        }

        if let Ok(session_vars_table) = section.get::<Table>("sessionVariables") {
            let mut session_vars = HashMap::new();
            for pair in session_vars_table.pairs::<String, String>() {
                let (k, v) = pair?;
                session_vars.insert(k, v);
            }
            config.session_variables = Some(session_vars);
        }

        config.shell_init = section.get::<String>("shellInit").ok();
        config.login_shell_init = section.get::<String>("loginShellInit").ok();
        config.paths_to_link = section.get::<Vec<String>>("pathsToLink").ok();

        Ok(config)
    }

    pub fn parse_plugins(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::PluginConfig>> {
        let mut plugins = HashMap::new();

        for pair in section.pairs::<String, Value>() {
            let (name, value) = pair?;
            let mut plugin = crate::config::PluginConfig::default();

            match value {
                Value::Boolean(b) => {
                    plugin.enable = Some(b);
                }
                Value::String(_s) => {
                    plugin.enable = Some(true);
                }
                Value::Table(table) => {
                    plugin.enable = table.get::<bool>("enable").ok();
                    plugin.version = table.get::<String>("version").ok();

                    if let Ok(source_table) = table.get::<Table>("source") {
                        plugin.source = Some(self.parse_source_config(&source_table)?);
                    }

                    if let Ok(config_table) = table.get::<Table>("config") {
                        plugin.config = Some(self.parse_plugin_config(&config_table)?);
                    }

                    if table.contains_key("github").unwrap_or(false) {
                        if let Ok(github_table) = table.get::<Table>("github") {
                            plugin.source = Some(self.parse_github_source(&github_table)?);
                        }
                    } else if table.contains_key("path").unwrap_or(false) {
                        if let Ok(path_table) = table.get::<Table>("path") {
                            plugin.source = Some(self.parse_path_source(&path_table)?);
                        }
                    } else if table.contains_key("url").unwrap_or(false) {
                        if let Ok(url_table) = table.get::<Table>("url") {
                            plugin.source = Some(self.parse_url_source(&url_table)?);
                        }
                    }
                }
                _ => {}
            }

            plugins.insert(name, plugin);
        }

        Ok(plugins)
    }

    fn parse_source_config(&self, section: &Table) -> Result<crate::config::SourceConfig> {
        let mut config = crate::config::SourceConfig::default();
        config.source_type = section
            .get::<String>("type")
            .ok()
            .map(|t| match t.as_str() {
                "github" => crate::config::SourceType::GitHub,
                "path" => crate::config::SourceType::Path,
                "directurl" => crate::config::SourceType::DirectUrl,
                _ => crate::config::SourceType::Path,
            })
            .unwrap_or_default();
        config.owner = section.get::<String>("owner").ok();
        config.repo = section.get::<String>("repo").ok();
        config.r#ref = section.get::<String>("ref").ok();
        config.path = section.get::<String>("path").ok();
        config.track_git = section.get::<bool>("track_git").ok();
        config.url = section.get::<String>("url").ok();
        config.hash = section.get::<String>("hash").ok();
        Ok(config)
    }

    fn parse_github_source(&self, section: &Table) -> Result<crate::config::SourceConfig> {
        let mut config = crate::config::SourceConfig::default();
        config.source_type = crate::config::SourceType::GitHub;
        config.owner = section.get::<String>("owner").ok();
        config.repo = section.get::<String>("repo").ok();
        config.r#ref = section.get::<String>("ref").ok();
        Ok(config)
    }

    fn parse_path_source(&self, section: &Table) -> Result<crate::config::SourceConfig> {
        let mut config = crate::config::SourceConfig::default();
        config.source_type = crate::config::SourceType::Path;
        config.path = section.get::<String>("path").ok();
        config.track_git = section.get::<bool>("track_git").ok();
        Ok(config)
    }

    fn parse_url_source(&self, section: &Table) -> Result<crate::config::SourceConfig> {
        let mut config = crate::config::SourceConfig::default();
        config.source_type = crate::config::SourceType::DirectUrl;
        config.url = section.get::<String>("url").ok();
        config.hash = section.get::<String>("hash").ok();
        Ok(config)
    }

    fn parse_plugin_config(&self, section: &Table) -> Result<HashMap<String, serde_json::Value>> {
        let mut config = HashMap::new();
        for pair in section.clone().pairs::<String, Value>() {
            let (k, v) = pair?;
            let json_value = match v {
                Value::String(s) => serde_json::Value::String(s.to_str()?.to_string()),
                Value::Number(n) => {
                    use serde_json::Number;
                    serde_json::Value::Number(Number::from_f64(n).expect("Invalid float"))
                }
                Value::Boolean(b) => serde_json::Value::Bool(b),
                Value::Nil => serde_json::Value::Null,
                _ => serde_json::Value::String(v.to_string().unwrap_or_default()),
            };
            config.insert(k, json_value);
        }
        Ok(config)
    }

    pub fn parse_outputs(&self, section: Table) -> Result<Option<crate::config::OutputsConfig>> {
        let outputs_table = if section.is_empty() {
            return Ok(None);
        } else {
            section
        };

        self.parse_outputs_table(outputs_table)
    }

    pub fn parse_overlays(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::OverlayConfig>> {
        let mut overlays = HashMap::new();

        for pair in section.pairs::<String, Value>() {
            let (name, value) = pair?;
            let mut overlay = crate::config::OverlayConfig::default();

            match value {
                Value::Function(_) => {
                    overlay.config = Some("function".to_string());
                }
                Value::Table(table) => {
                    overlay.config = table.get::<String>("config").ok();
                }
                _ => {}
            }

            overlays.insert(name, overlay);
        }

        Ok(overlays)
    }

    fn parse_outputs_systems(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::SystemProfile>> {
        let mut systems = HashMap::new();

        for pair in section.pairs::<String, Table>() {
            let (name, sys_table) = pair?;
            let mut profile = crate::config::SystemProfile::default();

            profile.description = sys_table.get::<String>("description").ok();

            if let Ok(import_list) = sys_table.get::<Table>("imports") {
                let mut imports = Vec::new();
                for item in import_list.sequence_values::<String>() {
                    if let Ok(s) = item {
                        imports.push(s);
                    }
                }
                profile.imports = imports;
            }

            if let Ok(system_section) = sys_table.get::<Table>("system") {
                profile.system = Some(self.parse_system(system_section)?);
            }

            if let Ok(packages_section) = sys_table.get::<Table>("packages") {
                profile.packages = Some(self.parse_packages(packages_section)?);
            }

            if let Ok(users_section) = sys_table.get::<Table>("users") {
                profile.users = Some(self.parse_users_simple(users_section)?);
            }

            systems.insert(name, profile);
        }

        Ok(systems)
    }

    fn parse_users_simple(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::UserConfig>> {
        let mut users = HashMap::new();

        for pair in section.pairs::<String, Value>() {
            let (name, value) = pair?;
            let mut user = crate::config::UserConfig::default();

            if let Some(user_table) = value.as_table() {
                user.description = user_table.get::<String>("description").ok();
                user.shell = user_table.get::<String>("shell").ok();

                if let Ok(home_table) = user_table.get::<Table>("home") {
                    user.home = home_table.get::<String>("homeDirectory").ok();
                }

                if let Ok(pkgs_table) = user_table.get::<Table>("packages") {
                    user.groups = pkgs_table
                        .clone()
                        .sequence_values::<String>()
                        .filter_map(|s| s.ok())
                        .collect();
                }
            }

            users.insert(name, user);
        }

        Ok(users)
    }

    fn parse_outputs_users(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::OutputUserConfig>> {
        let mut users = HashMap::new();

        for pair in section.pairs::<String, Table>() {
            let (name, user_table) = pair?;
            let mut user = crate::config::OutputUserConfig::default();

            user.description = user_table.get::<String>("description").ok();

            if let Ok(home_table) = user_table.get::<Table>("home") {
                let mut home = crate::config::HomeConfig::default();
                home.username = home_table.get::<String>("username").ok();
                home.home_directory = home_table.get::<String>("homeDirectory").ok();
                home.state_version = home_table.get::<String>("stateVersion").ok();
                user.home = Some(home);
            }

            user.packages = user_table.get::<Vec<String>>("packages").ok();

            if let Ok(programs_table) = user_table.get::<Table>("programs") {
                user.programs = Some(self.parse_programs(programs_table)?);
            }

            users.insert(name, user);
        }

        Ok(users)
    }

    fn parse_outputs_packages(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::OutputPackageConfig>> {
        let mut packages = HashMap::new();

        for pair in section.pairs::<String, Table>() {
            let (name, pkg_table) = pair?;
            let mut pkg = crate::config::OutputPackageConfig::default();

            pkg.src = pkg_table.get::<String>("src").ok();

            if let Ok(build_table) = pkg_table.get::<Table>("build") {
                let mut build = crate::config::BuildConfig::default();
                build.r#type = build_table.get::<String>("type").ok();
                build.pname = build_table.get::<String>("pname").ok();
                build.version = build_table.get::<String>("version").ok();
                pkg.build = Some(build);
            }

            packages.insert(name, pkg);
        }

        Ok(packages)
    }

    fn parse_dev_envs(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::DevEnvConfig>> {
        let mut envs = HashMap::new();

        for pair in section.pairs::<String, Table>() {
            let (name, env_table) = pair?;
            let mut env = crate::config::DevEnvConfig::default();

            env.description = env_table.get::<String>("description").ok();
            env.packages = env_table.get::<Vec<String>>("packages").ok();
            env.shell_hook = env_table.get::<String>("shellHook").ok();

            if let Ok(env_vars_table) = env_table.get::<Table>("env") {
                let mut env_vars = HashMap::new();
                for pair in env_vars_table.pairs::<String, String>() {
                    let (k, v) = pair?;
                    env_vars.insert(k, v);
                }
                env.env = Some(env_vars);
            }

            env.build_inputs = env_table.get::<Vec<String>>("buildInputs").ok();

            if let Ok(services_table) = env_table.get::<Table>("services") {
                let mut services = HashMap::new();
                for pair in services_table.pairs::<String, Table>() {
                    let (svc_name, svc_table) = pair?;
                    let mut svc = crate::config::DevEnvServiceConfig::default();
                    svc.enable = svc_table.get::<bool>("enable").ok();
                    svc.port = svc_table
                        .get::<f64>("port")
                        .map(|v| v as u16)
                        .ok()
                        .or_else(|| svc_table.get::<u16>("port").ok());
                    services.insert(svc_name, svc);
                }
                env.services = Some(services);
            }

            envs.insert(name, env);
        }

        Ok(envs)
    }

    fn parse_outputs_modules(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::ModuleConfig>> {
        let mut modules = HashMap::new();

        for pair in section.pairs::<String, Table>() {
            let (name, mod_table) = pair?;
            let mut module = crate::config::ModuleConfig::default();

            module.description = mod_table.get::<String>("description").ok();
            module.config = mod_table.get::<String>("config").ok();

            if let Ok(import_list) = mod_table.get::<Table>("imports") {
                let mut imports = Vec::new();
                for item in import_list.sequence_values::<String>() {
                    if let Ok(s) = item {
                        imports.push(s);
                    }
                }
                module.imports = Some(imports);
            }

            modules.insert(name, module);
        }

        Ok(modules)
    }

    fn parse_templates(
        &self,
        section: Table,
    ) -> Result<HashMap<String, crate::config::TemplateConfig>> {
        let mut templates = HashMap::new();

        for pair in section.pairs::<String, Table>() {
            let (name, tmpl_table) = pair?;
            let mut template = crate::config::TemplateConfig::default();

            template.description = tmpl_table.get::<String>("description").ok();
            template.path = tmpl_table.get::<String>("path").ok();
            template.files = tmpl_table.get::<Vec<String>>("files").ok();

            if let Ok(vars_table) = tmpl_table.get::<Table>("variables") {
                let mut vars = HashMap::new();
                for pair in vars_table.pairs::<String, String>() {
                    let (k, v) = pair?;
                    vars.insert(k, v);
                }
                template.variables = Some(vars);
            }

            templates.insert(name, template);
        }

        Ok(templates)
    }

    fn parse_apps(&self, section: Table) -> Result<HashMap<String, crate::config::AppConfig>> {
        let mut apps = HashMap::new();

        for pair in section.pairs::<String, Table>() {
            let (name, app_table) = pair?;
            let mut app = crate::config::AppConfig::default();

            app.r#type = app_table.get::<String>("type").ok();
            app.program = app_table.get::<String>("program").ok();
            app.description = app_table.get::<String>("description").ok();

            apps.insert(name, app);
        }

        Ok(apps)
    }
}
