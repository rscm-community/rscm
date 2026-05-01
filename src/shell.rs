use anyhow::{Result, anyhow};
use mlua::{Lua, Table};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{Configuration, ShellEnvironmentConfig};

#[derive(Default)]
struct ShellConfig {
    packages: Vec<String>,
    shell_hook: Option<String>,
    env: Vec<(String, String)>,
}

struct ResolvedShellEnv {
    name: String,
    packages: Vec<String>,
    shell_hook: Option<String>,
    variables: HashMap<String, String>,
}

pub fn enter_shell(
    cli_packages: &[String],
    config_path: Option<&str>,
    shell_name: Option<&str>,
    cli_vars: &[String],
    pure: bool,
    command: Option<&str>,
    shell_args: &[String],
) -> Result<()> {
    let mut all_packages: Vec<String> = cli_packages.to_vec();
    let mut env_vars_from_config: Vec<(String, String)> = Vec::new();
    let shell_hook: Option<String>;

    let shell_env_config = find_and_load_shell_environment(config_path, shell_name)?;

    if let Some(shell_cfg) = shell_env_config {
        all_packages.extend(shell_cfg.packages.clone());
        for (k, v) in &shell_cfg.variables {
            env_vars_from_config.push((k.clone(), v.clone()));
        }
        shell_hook = shell_cfg.shell_hook.clone();
        println!("Using shell environment: {}", shell_cfg.name);
    } else {
        let cfg = load_shell_config(config_path, cli_vars)?;
        all_packages.extend(cfg.packages);
        env_vars_from_config = cfg.env;
        shell_hook = cfg.shell_hook;
    }

    let mut seen = HashSet::new();
    let mut unique_packages: Vec<String> = all_packages
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect();

    let store_root = PathBuf::from("/rscm/store");

    if !unique_packages.is_empty() {
        unique_packages = ensure_packages_in_store(&store_root, &unique_packages)?;
    }

    let store = crate::store::Store::new(store_root.clone())?;
    let shell_env = store.create_shell_env(&unique_packages)?;
    let shell_id = shell_env.id;
    let shell_path = shell_env.path.clone();

    link_packages_to_shell_env(&shell_path, &unique_packages, &store_root)?;

    let (bin_paths, lib_paths) = collect_shell_env_paths(&shell_path)?;

    let mut env_vars = Vec::new();

    if pure {
        let mut all_paths = bin_paths.clone();
        add_generation_paths(&store_root, &mut all_paths)?;

        let new_path = all_paths.join(":");
        if !new_path.is_empty() {
            env_vars.push(("PATH".to_string(), new_path));
        }

        let mut all_lib_paths = lib_paths.clone();
        add_generation_lib_paths(&store_root, &mut all_lib_paths)?;

        let new_ld = all_lib_paths.join(":");
        if !new_ld.is_empty() {
            env_vars.push(("LD_LIBRARY_PATH".to_string(), new_ld));
        }

        if let Ok(home) = std::env::var("HOME") {
            env_vars.push(("HOME".to_string(), home));
        }
        if let Ok(user) = std::env::var("USER") {
            env_vars.push(("USER".to_string(), user));
        }
        if let Ok(logname) = std::env::var("LOGNAME") {
            env_vars.push(("LOGNAME".to_string(), logname));
        }
        if let Ok(shell) = std::env::var("SHELL") {
            env_vars.push(("SHELL".to_string(), shell));
        }
        if let Ok(term) = std::env::var("TERM") {
            env_vars.push(("TERM".to_string(), term));
        }
    } else {
        let existing_path = std::env::var("PATH").unwrap_or_default();
        let mut new_paths = bin_paths.clone();
        add_generation_paths(&store_root, &mut new_paths)?;

        let new_path = if new_paths.is_empty() {
            existing_path
        } else if existing_path.is_empty() {
            new_paths.join(":")
        } else {
            format!("{}:{}", new_paths.join(":"), existing_path)
        };

        if !new_path.is_empty() {
            env_vars.push(("PATH".to_string(), new_path));
        }

        let existing_ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let mut new_lib_paths = lib_paths.clone();
        add_generation_lib_paths(&store_root, &mut new_lib_paths)?;

        let new_ld = if new_lib_paths.is_empty() {
            existing_ld
        } else if existing_ld.is_empty() {
            new_lib_paths.join(":")
        } else {
            format!("{}:{}", new_lib_paths.join(":"), existing_ld)
        };

        if !new_ld.is_empty() {
            env_vars.push(("LD_LIBRARY_PATH".to_string(), new_ld));
        }
    }

    for (k, v) in env_vars_from_config {
        env_vars.push((k, v));
    }

    let pid = std::process::id();
    fs::write(shell_path.join("pid"), pid.to_string())?;

    let result = if let Some(cmd) = command {
        execute_command(cmd, &env_vars)
    } else {
        start_shell(&env_vars, shell_hook.as_deref(), shell_args, pure)
    };

    let _ = fs::remove_file(shell_path.join("pid"));
    let _ = store.delete_shell_env(shell_id);

    result
}

fn add_generation_paths(store_root: &Path, paths: &mut Vec<String>) -> Result<()> {
    let current_link = Path::new("/rscm/current-system");
    if current_link.exists() {
        if let Ok(gen_path) = std::fs::read_link(current_link) {
            for dir in &["bin", "usr/bin", "sbin", "usr/sbin"] {
                let full = gen_path.join(dir);
                if full.exists() {
                    let full_str = full.to_string_lossy().to_string();
                    if !paths.contains(&full_str) {
                        paths.push(full_str);
                    }
                }
            }
        }
    }

    let generations_dir = store_root.join("generations");
    if generations_dir.exists() {
        let mut latest_id: Option<u64> = None;
        if let Ok(entries) = std::fs::read_dir(&generations_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Ok(id) = name.parse::<u64>() {
                            latest_id = Some(latest_id.map_or(id, |old| old.max(id)));
                        }
                    }
                }
            }
        }
        if let Some(id) = latest_id {
            let gen_path = generations_dir.join(id.to_string());
            for dir in &["bin", "usr/bin", "sbin", "usr/sbin"] {
                let full = gen_path.join(dir);
                if full.exists() {
                    let full_str = full.to_string_lossy().to_string();
                    if !paths.contains(&full_str) {
                        paths.push(full_str);
                    }
                }
            }
        }
    }

    Ok(())
}

fn add_generation_lib_paths(store_root: &Path, paths: &mut Vec<String>) -> Result<()> {
    let current_link = Path::new("/rscm/current-system");
    if current_link.exists() {
        if let Ok(gen_path) = std::fs::read_link(current_link) {
            for dir in &["lib", "usr/lib", "lib64", "usr/lib64"] {
                let full = gen_path.join(dir);
                if full.exists() {
                    let full_str = full.to_string_lossy().to_string();
                    if !paths.contains(&full_str) {
                        paths.push(full_str);
                    }
                }
            }
        }
    }

    let generations_dir = store_root.join("generations");
    if generations_dir.exists() {
        let mut latest_id: Option<u64> = None;
        if let Ok(entries) = std::fs::read_dir(&generations_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Ok(id) = name.parse::<u64>() {
                            latest_id = Some(latest_id.map_or(id, |old| old.max(id)));
                        }
                    }
                }
            }
        }
        if let Some(id) = latest_id {
            let gen_path = generations_dir.join(id.to_string());
            for dir in &["lib", "usr/lib", "lib64", "usr/lib64"] {
                let full = gen_path.join(dir);
                if full.exists() {
                    let full_str = full.to_string_lossy().to_string();
                    if !paths.contains(&full_str) {
                        paths.push(full_str);
                    }
                }
            }
        }
    }

    Ok(())
}

fn find_and_load_shell_environment(
    explicit_config_path: Option<&str>,
    shell_name: Option<&str>,
) -> Result<Option<ResolvedShellEnv>> {
    let config_paths = find_configuration_paths(explicit_config_path);

    for config_path in config_paths {
        if !config_path.exists() {
            continue;
        }

        if let Ok(config) = load_configuration_from_lua(&config_path) {
            if !config.shells.is_empty() {
                return resolve_shell_from_config(&config, shell_name, &config_path);
            }
        }

        if config_path
            .file_name()
            .map_or(false, |n| n == "rscm-shell.lua")
        {
            return Ok(None);
        }
    }

    Ok(None)
}

fn find_configuration_paths(explicit_path: Option<&str>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(p) = explicit_path {
        paths.push(PathBuf::from(p));
        return paths;
    }

    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("configuration.lua"));
        paths.push(cwd.join("rscm-shell.lua"));
    }

    if let Some(home) = dirs::home_dir() {
        let user_dir = home.join(".config/rscm");
        paths.push(user_dir.join("configuration.lua"));
        paths.push(user_dir.join("rscm-shell.lua"));
    }

    paths.push(PathBuf::from("/etc/rscm/configuration.lua"));
    paths.push(PathBuf::from("/etc/rscm/rscm-shell.lua"));

    paths
}

fn load_configuration_from_lua(path: &Path) -> Result<Configuration> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("Cannot read {}: {}", path.display(), e))?;

    let engine = crate::lua::LuaEngine::new()?;
    let config = engine.load_config(&content, path)?;

    Ok(config)
}

fn resolve_shell_from_config(
    config: &Configuration,
    shell_name: Option<&str>,
    config_path: &Path,
) -> Result<Option<ResolvedShellEnv>> {
    if config.shells.is_empty() {
        return Ok(None);
    }

    if let Some(name) = shell_name {
        if let Some(shell_cfg) = config.shells.get(name) {
            println!(
                "Using shell environment '{}' from: {}",
                name,
                config_path.display()
            );
            return Ok(Some(ResolvedShellEnv {
                name: name.to_string(),
                packages: shell_cfg.packages.clone(),
                shell_hook: shell_cfg.shell_hook.clone(),
                variables: shell_cfg.variables.clone(),
            }));
        } else {
            return Err(anyhow!(
                "Shell environment '{}' not found in {}",
                name,
                config_path.display()
            ));
        }
    }

    let mut default_shell: Option<(&String, &ShellEnvironmentConfig)> = None;
    let mut first_shell: Option<(&String, &ShellEnvironmentConfig)> = None;

    for (name, shell_cfg) in &config.shells {
        if first_shell.is_none() {
            first_shell = Some((name, shell_cfg));
        }
        if shell_cfg.default {
            default_shell = Some((name, shell_cfg));
            break;
        }
    }

    if let Some((name, shell_cfg)) = default_shell {
        println!(
            "Using default shell environment '{}' from: {}",
            name,
            config_path.display()
        );
        return Ok(Some(ResolvedShellEnv {
            name: name.clone(),
            packages: shell_cfg.packages.clone(),
            shell_hook: shell_cfg.shell_hook.clone(),
            variables: shell_cfg.variables.clone(),
        }));
    }

    if config.shells.len() == 1 {
        if let Some((name, shell_cfg)) = first_shell {
            println!(
                "Using shell environment '{}' from: {}",
                name,
                config_path.display()
            );
            return Ok(Some(ResolvedShellEnv {
                name: name.clone(),
                packages: shell_cfg.packages.clone(),
                shell_hook: shell_cfg.shell_hook.clone(),
                variables: shell_cfg.variables.clone(),
            }));
        }
    }

    let shell_names: Vec<&String> = config.shells.keys().collect();
    Err(anyhow!(
        "Multiple shell environments defined in {} ({}), but no default specified.\n\
         Use --shell-name to specify one of: {}",
        config_path.display(),
        shell_names.len(),
        shell_names
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

pub fn list_shell_environments() -> Result<()> {
    let config_paths = find_configuration_paths(None);
    let mut found_any = false;

    for config_path in config_paths {
        if !config_path.exists() {
            continue;
        }

        if let Ok(config) = load_configuration_from_lua(&config_path) {
            if !config.shells.is_empty() {
                if !found_any {
                    println!("Available shell environments:");
                    println!();
                }
                found_any = true;

                println!("From: {}", config_path.display());
                for (name, shell_cfg) in &config.shells {
                    let default_marker = if shell_cfg.default { " (default)" } else { "" };
                    let description = shell_cfg.description.as_deref().unwrap_or("No description");
                    let pkg_count = shell_cfg.packages.len();

                    println!(
                        "  {}{} - {} ({} packages)",
                        name, default_marker, description, pkg_count
                    );
                }
                println!();
            }
        }
    }

    if !found_any {
        println!("No shell environments defined in configuration files.");
        println!();
        println!("To define shell environments, add a 'shells' section to your configuration.lua:");
        println!();
        println!("  shells = {{");
        println!("    default = {{");
        println!("      description = \"Default development shell\",");
        println!("      default = true,");
        println!("      packages = {{ \"vim\", \"git\", \"htop\" }},");
        println!("      shell_hook = \"echo 'Welcome to dev shell'\",");
        println!("    }},");
        println!("    minimal = {{");
        println!("      description = \"Minimal shell with basic tools\",");
        println!("      packages = {{ \"vim\" }},");
        println!("    }},");
        println!("  }}");
    }

    Ok(())
}

fn shell_config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("rscm-shell.lua"));
    }

    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".config/rscm/rscm-shell.lua"));
    }

    paths.push(PathBuf::from("/etc/rscm/rscm-shell.lua"));

    paths
}

fn parse_cli_vars(vars: &[String]) -> Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    for var in vars {
        if let Some((key, value)) = var.split_once('=') {
            if key.is_empty() {
                return Err(anyhow!("Invalid variable (empty key): {}", var));
            }
            result.push((key.to_string(), value.to_string()));
        } else {
            return Err(anyhow!(
                "Invalid variable '{}', expected KEY=VALUE format",
                var
            ));
        }
    }
    Ok(result)
}

fn load_shell_config(explicit_path: Option<&str>, cli_vars: &[String]) -> Result<ShellConfig> {
    let parsed_vars = parse_cli_vars(cli_vars)?;

    for (k, v) in &parsed_vars {
        unsafe {
            std::env::set_var(k, v);
        }
    }

    let config_path = if let Some(p) = explicit_path {
        let p = PathBuf::from(p);
        if !p.exists() {
            return Err(anyhow!("Shell config not found: {}", p.display()));
        }
        p
    } else {
        match shell_config_search_paths().into_iter().find(|p| p.exists()) {
            Some(p) => p,
            None => return Ok(ShellConfig::default()),
        }
    };

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| anyhow!("Cannot read {}: {}", config_path.display(), e))?;

    let lua = Lua::new();

    for (k, v) in &parsed_vars {
        lua.globals().set(k.as_str(), v.as_str())?;
    }

    let captured_packages: Table = lua.create_table()?;
    lua.globals()
        .set("__rscm_shell_packages", &captured_packages)?;

    let captured_env: Table = lua.create_table()?;
    lua.globals().set("__rscm_shell_env", &captured_env)?;

    let pkg_fn = lua.create_function(|lua, table: Table| {
        let captured: Table = lua.globals().get("__rscm_shell_packages")?;
        for pair in table.pairs::<mlua::Value, mlua::Value>() {
            let (key, value) = pair?;
            captured.set(key, value)?;
        }
        Ok(())
    })?;
    lua.globals().set("packages", pkg_fn)?;

    let env_fn = lua.create_function(|lua, table: Table| {
        let captured: Table = lua.globals().get("__rscm_shell_env")?;
        for pair in table.pairs::<mlua::Value, mlua::Value>() {
            let (key, value) = pair?;
            captured.set(key, value)?;
        }
        Ok(())
    })?;
    lua.globals().set("environment", env_fn)?;

    lua.load(&content).exec()?;

    let mut packages = Vec::new();
    for pair in captured_packages.pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;
        match key {
            mlua::Value::Integer(_) => {
                if let mlua::Value::String(s) = value {
                    packages.push(s.to_str()?.to_string());
                }
            }
            mlua::Value::String(s) => {
                packages.push(s.to_str()?.to_string());
            }
            _ => {}
        }
    }

    let mut env_vars: Vec<(String, String)> = Vec::new();
    let mut shell_hook: Option<String> = None;

    // 从 __rscm_shell_env 中读取 environment 块定义的配置
    if let Ok(env_table) = lua.globals().get::<Table>("__rscm_shell_env") {
        if let Ok(hook) = env_table.get::<String>("shell_hook") {
            shell_hook = Some(hook);
        }
        if shell_hook.is_none() {
            if let Ok(hook) = env_table.get::<String>("shellHook") {
                shell_hook = Some(hook);
            }
        }

        if let Ok(vars) = env_table.get::<Table>("variables") {
            for pair in vars.pairs::<String, String>() {
                if let Ok((k, v)) = pair {
                    env_vars.push((k, v));
                }
            }
        }
    }
    if shell_hook.is_none() {
        if let Ok(hook) = lua.globals().get::<String>("shellHook") {
            shell_hook = Some(hook);
        }
    }
    if shell_hook.is_none() {
        if let Ok(hook) = lua.globals().get::<String>("shell_hook") {
            shell_hook = Some(hook);
        }
    }

    println!("Using shell config: {}", config_path.display());
    Ok(ShellConfig {
        packages,
        shell_hook,
        env: env_vars,
    })
}

fn execute_command(cmd: &str, env_vars: &[(String, String)]) -> Result<()> {
    let mut parts = cmd.split_whitespace();
    let program = parts.next().ok_or_else(|| anyhow!("Empty command"))?;
    let args: Vec<&str> = parts.collect();

    let mut command = Command::new(program);
    command.args(&args);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    for (key, value) in env_vars {
        command.env(key, value);
    }

    let status = command
        .status()
        .map_err(|e| anyhow!("Failed to execute '{}': {}", cmd, e))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn get_user_shell() -> String {
    use std::ffi::CStr;

    unsafe {
        let pw_ptr = libc::getpwuid(libc::getuid());
        if !pw_ptr.is_null() {
            let pw = *pw_ptr;
            if !pw.pw_shell.is_null() {
                let shell_cstr = CStr::from_ptr(pw.pw_shell);
                if let Ok(shell_str) = shell_cstr.to_str() {
                    if !shell_str.is_empty() {
                        return shell_str.to_string();
                    }
                }
            }
        }
    }

    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

fn run_hook(hook: &str, env_vars: &[(String, String)]) -> Result<()> {
    let shell = get_user_shell();

    let mut command = Command::new(&shell);
    command.arg("-c").arg(hook);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    for (key, value) in env_vars {
        command.env(key, value);
    }

    let status = command
        .status()
        .map_err(|e| anyhow!("Failed to run shell_hook: {}", e))?;

    if !status.success() {
        return Err(anyhow!(
            "shell_hook exited with non-zero status: {}",
            status.code().unwrap_or(-1)
        ));
    }

    Ok(())
}

fn start_shell(
    env_vars: &[(String, String)],
    shell_hook: Option<&str>,
    shell_args: &[String],
    pure: bool,
) -> Result<()> {
    let shell = get_user_shell();

    if let Some(hook) = shell_hook {
        run_hook(hook, env_vars)?;
    }

    let mut command = Command::new(&shell);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let current_dir = std::env::current_dir()?;
    command.current_dir(&current_dir);

    for (key, value) in env_vars {
        command.env(key, value);
    }

    command.env("RSCM_SHELL", "1");
    if pure {
        command.env("RSCM_PURE_SHELL", "1");
    }

    command.args(shell_args);

    let status = command
        .status()
        .map_err(|e| anyhow!("Failed to start shell '{}': {}", shell, e))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn ensure_packages_in_store(store_root: &Path, packages: &[String]) -> Result<Vec<String>> {
    let pkg_factory = crate::pkg::PackageManagerFactory::new(store_root.to_path_buf());

    let mut result = Vec::new();

    for pkg_name in packages {
        let mut found_in_store = false;

        if let Ok(entries) = std::fs::read_dir(store_root.join("packages")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{}-", pkg_name)) {
                    found_in_store = true;
                    break;
                }
            }
        }

        if found_in_store {
            result.push(pkg_name.clone());
            continue;
        }

        println!("📦 Installing '{}' to rscm store...", pkg_name);

        let config = crate::pkg::PackageConfig {
            name: pkg_name.clone(),
            version: None,
            build_type: crate::pkg::BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let manager = pkg_factory.for_package(&config)?;
        let info = manager.install(&config, false)?;
        println!(
            "✓ Installed {}-{}-{}",
            info.name, info.version, info.release
        );
        result.push(pkg_name.clone());
    }

    Ok(result)
}

fn link_packages_to_shell_env(
    shell_path: &Path,
    packages: &[String],
    store_root: &Path,
) -> Result<()> {
    let packages_dir = store_root.join("packages");
    if !packages_dir.exists() {
        return Ok(());
    }

    for pkg_name in packages {
        for entry in std::fs::read_dir(&packages_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{}-", pkg_name)) {
                let pkg_path = entry.path();

                for dir in &[
                    "bin",
                    "usr/bin",
                    "sbin",
                    "usr/sbin",
                    "lib",
                    "usr/lib",
                    "lib64",
                    "usr/lib64",
                ] {
                    let src_dir = pkg_path.join(dir);
                    if src_dir.exists() {
                        let dest_dir = shell_path.join(dir);
                        fs::create_dir_all(&dest_dir)?;

                        for file_entry in std::fs::read_dir(&src_dir)? {
                            let file_entry = file_entry?;
                            let src_file = file_entry.path();
                            let dest_file = dest_dir.join(file_entry.file_name());

                            if !dest_file.exists() {
                                if src_file.is_symlink() {
                                    let target = std::fs::read_link(&src_file)?;
                                    std::os::unix::fs::symlink(target, &dest_file)?;
                                } else if src_file.is_dir() {
                                    fs::create_dir_all(&dest_file)?;
                                } else {
                                    let _ = fs::hard_link(&src_file, &dest_file);
                                }
                            }
                        }
                    }
                }
                break;
            }
        }
    }

    Ok(())
}

fn collect_shell_env_paths(shell_path: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let mut bin_paths = Vec::new();
    let mut lib_paths = Vec::new();

    for dir in &["bin", "usr/bin", "sbin", "usr/sbin"] {
        let full = shell_path.join(dir);
        if full.exists() {
            let full_str = full.to_string_lossy().to_string();
            if !bin_paths.contains(&full_str) {
                bin_paths.push(full_str);
            }
        }
    }

    for dir in &["lib", "usr/lib", "lib64", "usr/lib64"] {
        let full = shell_path.join(dir);
        if full.exists() {
            let full_str = full.to_string_lossy().to_string();
            if !lib_paths.contains(&full_str) {
                lib_paths.push(full_str);
            }
        }
    }

    Ok((bin_paths, lib_paths))
}

fn collect_package_paths(
    store_root: &Path,
    packages: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    let mut bin_paths = Vec::new();
    let mut lib_paths = Vec::new();

    let packages_dir = store_root.join("packages");
    if !packages_dir.exists() {
        return Ok((bin_paths, lib_paths));
    }

    for pkg_name in packages {
        for entry in std::fs::read_dir(&packages_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{}-", pkg_name)) {
                let pkg_path = entry.path();
                for dir in &["bin", "usr/bin", "sbin", "usr/sbin"] {
                    let full = pkg_path.join(dir);
                    if full.exists() {
                        let full_str = full.to_string_lossy().to_string();
                        if !bin_paths.contains(&full_str) {
                            bin_paths.push(full_str);
                        }
                    }
                }
                for dir in &["lib", "usr/lib", "lib64", "usr/lib64"] {
                    let full = pkg_path.join(dir);
                    if full.exists() {
                        let full_str = full.to_string_lossy().to_string();
                        if !lib_paths.contains(&full_str) {
                            lib_paths.push(full_str);
                        }
                    }
                }
                break;
            }
        }
    }

    Ok((bin_paths, lib_paths))
}
