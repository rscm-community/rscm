use crate::cache::CacheManager;
use crate::lock::LockManager;
use crate::lua::LuaEngine;
use crate::toolchain::ToolchainManager;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

const SYSTEM_CONFIG_PATH: &str = "/etc/rscm/configuration.lua";
const LOCAL_CONFIG_NAME: &str = "configuration.lua";
const USER_CONFIG_SUBDIR: &str = ".config/rscm";

#[derive(Parser)]
#[command(name = "rscm")]
#[command(about = "Reproducible System Configuration Manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Edit,
    Build,
    Switch,
    Generations,
    Shell,
    Lock {
        #[arg(long, short)]
        update: bool,
        #[arg(long, short)]
        force: bool,
        #[arg(long, short)]
        config: Option<String>,
    },
    Check {
        #[arg(default_value = "")]
        path: String,
    },
    Toolchain {
        #[command(subcommand)]
        action: ToolchainAction,
    },
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}

#[derive(Subcommand)]
pub enum ToolchainAction {
    Status,
}

#[derive(Subcommand)]
pub enum CacheAction {
    Status,
    Clean {
        #[arg(long)]
        archive: bool,
        #[arg(long)]
        aur: bool,
        #[arg(long)]
        all: bool,
    },
}

pub fn find_config_file(explicit_path: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = explicit_path {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
        return Err(anyhow!("Configuration file not found: {}", path.display()));
    }

    let local = PathBuf::from(LOCAL_CONFIG_NAME);
    if local.exists() {
        return Ok(local);
    }

    if let Some(home) = dirs::home_dir() {
        let user_config = home.join(USER_CONFIG_SUBDIR).join(LOCAL_CONFIG_NAME);
        if user_config.exists() {
            return Ok(user_config);
        }
    }

    let system = PathBuf::from(SYSTEM_CONFIG_PATH);
    if system.exists() {
        return Ok(system);
    }

    Err(anyhow!(
        "No configuration file found. Looked for: ./{}, ~/.config/rscm/{}, {}",
        LOCAL_CONFIG_NAME,
        LOCAL_CONFIG_NAME,
        SYSTEM_CONFIG_PATH
    ))
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Edit => todo!(),
        Commands::Build => todo!(),
        Commands::Switch => todo!(),
        Commands::Generations => todo!(),
        Commands::Shell => todo!(),
        Commands::Lock {
            update,
            force,
            config,
        } => {
            let config_path = find_config_file(config.as_deref())?;

            let store_root = dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("/var/lib"))
                .join("rscm")
                .join("store");

            let manager = LockManager::new(config_path.clone(), store_root);
            manager.lock(update, force)?;

            Ok(())
        }
        Commands::Check { path } => {
            let config_path = if path.is_empty() {
                find_config_file(None)?
            } else {
                PathBuf::from(&path)
            };

            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| anyhow!("Cannot read {}: {}", config_path.display(), e))?;

            println!("Using configuration: {}", config_path.display());

            let engine = LuaEngine::new()?;

            match engine.load_config(&content) {
                Ok(config) => {
                    println!("✓ Valid Lua syntax");
                    let sections: Vec<&str> = [
                        config.system.as_ref().map(|_| "system"),
                        if !config.packages.list.is_empty() || !config.packages.map.is_empty() {
                            Some("packages")
                        } else {
                            None
                        },
                        if !config.services.is_empty() {
                            Some("services")
                        } else {
                            None
                        },
                        if !config.users.is_empty() {
                            Some("users")
                        } else {
                            None
                        },
                        config.network.as_ref().map(|_| "network"),
                        if !config.systems.is_empty() {
                            Some("systems")
                        } else {
                            None
                        },
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    println!("✓ Found sections: {}", sections.join(", "));
                }
                Err(e) => {
                    println!("✗ Error: {}", e);
                }
            }

            Ok(())
        }
        Commands::Toolchain { action } => {
            let mut manager = ToolchainManager::new();
            match action {
                ToolchainAction::Status => {
                    manager.check_status()?;
                    let report = manager.get_report();
                    println!("{}", report);
                    Ok(())
                }
            }
        }
        Commands::Cache { action } => {
            let store_root = dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("/var/lib"))
                .join("rscm")
                .join("store");

            let cache_manager = CacheManager::new(store_root);

            match action {
                CacheAction::Status => {
                    let stats = cache_manager.status();
                    println!("Cache Status:");
                    println!(
                        "  archive: {} ({} files)",
                        CacheManager::format_size(stats.archive_size),
                        stats.archive_files
                    );
                    println!(
                        "  aur: {} ({} files)",
                        CacheManager::format_size(stats.aur_size),
                        stats.aur_files
                    );
                    println!("  total: {}", CacheManager::format_size(stats.total_size));
                    Ok(())
                }
                CacheAction::Clean { archive, aur, all } => {
                    let clean_all = all || (!archive && !aur);
                    let mut freed: u64 = 0;

                    if clean_all || archive {
                        freed += cache_manager.clean_archive()?;
                    }
                    if clean_all || aur {
                        freed += cache_manager.clean_aur()?;
                    }

                    println!("Total freed: {}", CacheManager::format_size(freed));
                    Ok(())
                }
            }
        }
    }
}
