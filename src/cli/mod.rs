use crate::cache::CacheManager;
use crate::lock::LockManager;
use crate::lua::LuaEngine;
use crate::toolchain::ToolchainManager;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

const SYSTEM_CONFIG_PATH: &str = "/etc/rscm/configuration.lua";
const LOCAL_CONFIG_NAME: &str = "configuration.lua";
const USER_CONFIG_SUBDIR: &str = ".config/rscm";
const SYSTEM_STORE_ROOT: &str = "/rscm/store";

#[derive(Parser)]
#[command(name = "rscm")]
#[command(about = "Reproducible System Configuration Manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Init {
        #[arg(long, short)]
        force: bool,
    },
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

pub fn get_store_root() -> Result<PathBuf> {
    let store = PathBuf::from(SYSTEM_STORE_ROOT);
    if !store.exists() {
        return Err(anyhow!(
            "Store directory {} does not exist. Run 'sudo rscm init' first.",
            SYSTEM_STORE_ROOT
        ));
    }
    Ok(store)
}

pub fn init_store(force: bool) -> Result<PathBuf> {
    let system_store = PathBuf::from(SYSTEM_STORE_ROOT);
    let system_root = Path::new("/rscm");

    if !force && system_store.exists() {
        println!("✓ Store already exists at {}", SYSTEM_STORE_ROOT);
    } else {
        println!("Creating {} (requires root)...", SYSTEM_STORE_ROOT);

        std::fs::create_dir_all(&system_store).map_err(|e| {
            anyhow!(
                "Failed to create {}: {}. Run with sudo.",
                SYSTEM_STORE_ROOT,
                e
            )
        })?;

        println!("✓ Created {}", SYSTEM_STORE_ROOT);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let perms = std::fs::Permissions::from_mode(0o775);
            let _ = std::fs::set_permissions(system_root, perms);
            let perms2 = std::fs::Permissions::from_mode(0o775);
            let _ = std::fs::set_permissions(&system_store, perms2);

            println!("✓ Set permissions to 775 (owner:group can read/write)");
        }

        create_store_subdirs(&system_store)?;
    }

    println!("\nNote: To allow regular users to write to /rscm:");
    println!("  sudo groupadd -f rscm");
    println!("  sudo chown -R root:rscm /rscm");
    println!("  sudo chmod -R 775 /rscm");
    println!("  sudo usermod -aG rscm $USER");

    Ok(system_store)
}

fn create_store_subdirs(store_root: &Path) -> Result<()> {
    let subdirs = [
        "content",
        "packages",
        "generations/generations",
        "generations/profiles",
        "cache/archive",
        "cache/aur",
        "sources",
        "locks/history",
        "locks/tags",
        "log",
    ];

    for subdir in &subdirs {
        let path = store_root.join(subdir);
        std::fs::create_dir_all(&path)
            .map_err(|e| anyhow!("Failed to create {}: {}", path.display(), e))?;
    }

    let refs_file = store_root.join("references.json");
    if !refs_file.exists() {
        std::fs::write(&refs_file, "{}")
            .map_err(|e| anyhow!("Failed to create references.json: {}", e))?;
    }

    println!("✓ Created store subdirectories");
    Ok(())
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Init { force } => {
            println!("Initializing rscm storage...");
            let store_root = init_store(force)?;
            println!("\nStore root: {}", store_root.display());
            println!("You can now run 'rscm lock' to create a lock file.");
            Ok(())
        }
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

            let store_root = get_store_root()?;

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
            let store_root = get_store_root()?;

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
