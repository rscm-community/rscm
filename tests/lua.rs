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
