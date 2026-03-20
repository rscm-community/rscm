use anyhow::Result;
use rscm::lua::LuaEngine;

#[test]
fn test_extend_inheritance() -> Result<()> {
    let engine = LuaEngine::new()?;
    let content = r#"
systems {
    base = {
        description = "Base system",
        packages = { "vim", "git" },
        services = { test = { enable = true } },
    },
    
    derived = extend("base", {
        description = "Derived system",
        packages = { "htop", "curl" },
    }),
}
"#;
    let config = engine.load_config(content)?;
    assert_eq!(config.systems.len(), 2);

    let derived = config
        .systems
        .get("derived")
        .expect("derived system not found");
    assert_eq!(derived.description, Some("Derived system".to_string()));
    assert!(derived.inherits.contains(&"base".to_string()));

    let base = config.systems.get("base").expect("base system not found");
    assert_eq!(base.description, Some("Base system".to_string()));

    Ok(())
}

#[test]
fn test_system_config_parsing() -> Result<()> {
    let engine = LuaEngine::new()?;
    let content = r#"
local cfg = {
    hostname = "test-host",
    timezone = "UTC",
    locale = "en_US.UTF-8",
    keymap = "us"
}
system(cfg)
"#;
    let config = engine.load_config(content)?;
    let system = config.system.expect("system config not found");
    assert_eq!(system.hostname, Some("test-host".to_string()));
    assert_eq!(system.timezone, Some("UTC".to_string()));
    Ok(())
}

#[test]
fn test_packages_parsing() -> Result<()> {
    let engine = LuaEngine::new()?;
    let content = r#"
local cfg = { "vim", "git", "curl" }
packages(cfg)
"#;
    let config = engine.load_config(content)?;
    assert_eq!(config.packages.list.len(), 3);
    assert!(config.packages.list.contains(&"vim".to_string()));
    Ok(())
}

#[test]
fn test_full_config_loading() -> Result<()> {
    use rscm::config::loader::ConfigLoader;

    let loader = ConfigLoader::new()?;
    let config = loader.load("test_config.lua")?;

    assert!(
        !config.sources.is_empty(),
        "sources section should not be empty"
    );
    assert!(
        config.sources.contains_key("dotfiles"),
        "should have dotfiles source"
    );
    assert!(
        config.sources.contains_key("local_modules"),
        "should have local_modules source"
    );

    assert!(config.system.is_some(), "system section should exist");
    let system = config.system.as_ref().unwrap();
    assert_eq!(system.hostname, Some("workstation".to_string()));

    assert!(config.boot.is_some(), "boot section should exist");
    let boot = config.boot.as_ref().unwrap();
    assert!(boot.kernel.is_some(), "kernel config should exist");

    assert!(
        !config.packages.list.is_empty(),
        "packages list should not be empty"
    );

    assert!(
        config.programs.contains_key("git"),
        "should have git program"
    );

    assert!(
        config.hardware.graphics.is_some(),
        "graphics config should exist"
    );

    assert!(config.security.sudo.is_some(), "sudo config should exist");

    assert!(
        config.filesystems.len() > 0,
        "filesystems section should not be empty"
    );

    assert!(
        !config.swapdevices.is_empty(),
        "swapdevices should not be empty"
    );

    assert!(
        config.environment.variables.is_some()
            || config.environment.session_variables.is_some()
            || config.environment.shell_init.is_some()
            || config.environment.paths_to_link.is_some(),
        "environment section should have at least one field"
    );

    assert!(
        config.plugins.contains_key("docker"),
        "should have docker plugin"
    );

    assert!(
        config.systems.contains_key("workstation"),
        "should have workstation system"
    );
    assert!(
        config.systems.contains_key("devserver"),
        "should have devserver system"
    );

    assert!(config.outputs.is_some(), "outputs section should exist");

    Ok(())
}
