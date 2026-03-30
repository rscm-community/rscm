use crate::cache::CacheManager;
use crate::config::Configuration;
use crate::lock::LockManager;
use crate::lua::LuaEngine;
use crate::store::Store;
use crate::toolchain::ToolchainManager;
use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use nix::unistd::geteuid;
use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

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
    Build {
        #[arg(long, short)]
        sync: bool,
        #[arg(long)]
        system: Option<String>,
    },
    Switch {
        id: Option<u64>,
        #[arg(long, short)]
        sync: bool,
        #[arg(long)]
        system: Option<String>,
    },
    Generations {
        #[command(subcommand)]
        action: GenerationsAction,
    },
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
pub enum GenerationsAction {
    List,
    Delete {
        id: Option<u64>,
        #[arg(long, short)]
        keep: Option<u64>,
        #[arg(long, short)]
        remove_oldest: Option<u64>,
        #[arg(long, short)]
        all: bool,
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

fn check_root() {
    let euid = geteuid();
    if !euid.is_root() {
        println!("Hint: This operation requires root privileges.\nRun with: sudo rscm <command>");
        process::exit(-1);
    }
}

fn lock_config(update: bool, force: bool, config: Option<String>) -> Result<()> {
    let config_path = find_config_file(config.as_deref())?;
    let store_root = get_store_root()?;
    let manager = LockManager::new(config_path.clone(), store_root);
    manager.lock(update, force)?;
    Ok(())
}

fn load_config(config_path: PathBuf) -> Result<Configuration> {
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| anyhow!("Cannot read {}: {}", config_path.display(), e))?;

    println!("Using configuration: {}", config_path.display());

    let engine = LuaEngine::new()?;

    engine.load_config(&content)
}

fn build_system(system: Option<String>) -> Result<u64> {
    println!("Building new generation...",);
    let config_path = find_config_file(Some(SYSTEM_CONFIG_PATH))?;
    let config = load_config(config_path)?;
    let store_root = get_store_root()?;
    let mut store = Store::new(store_root)?;
    let id = store.create_generation(config)?;
    println!("New generation id: {}", id);
    Ok(id)
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
        Commands::Build { sync, system } => {
            check_root();
            if sync {
                lock_config(true, false, Some(String::from(SYSTEM_CONFIG_PATH)))?;
            }
            build_system(system)?;
            Ok(())
        }
        Commands::Switch { id, sync, system } => {
            check_root();
            let store_root = get_store_root()?;
            let mut store = Store::new(store_root)?;
            if let Some(id) = id {
                store.activate_generation(id)?
            } else if let Some(system) = system {
            } else {
                if sync {
                    lock_config(true, false, Some(String::from(SYSTEM_CONFIG_PATH)))?;
                }
                let id = build_system(None)?;
                store.activate_generation(id)?
            }
            Ok(())
        }
        Commands::Generations { action } => {
            let store_root = get_store_root()?;
            let store = Store::new(store_root)?;
            match action {
                GenerationsAction::List => {
                    let generations = store.list_generations()?;
                    if generations.is_empty() {
                        println!("No generations found.");
                    } else {
                        let current_link = Path::new("/rscm/current-system");
                        let current_name = if current_link.exists() {
                            fs::read_link(current_link).ok().and_then(|p| {
                                p.file_name()
                                    .map(|n| n.to_os_string())
                                    .and_then(|s| s.into_string().ok())
                            })
                        } else {
                            None
                        };
                        for g in &generations {
                            let is_current = current_name
                                .as_ref()
                                .map(|s| s == &g.id.to_string())
                                .unwrap_or(false);
                            let marker = if is_current { " (current)" } else { "" };
                            println!("Generation {}{}", g.id, marker);
                        }
                    }
                    Ok(())
                }
                GenerationsAction::Delete {
                    id,
                    keep,
                    remove_oldest,
                    all,
                } => {
                    check_root();
                    let generations = store.list_generations()?;
                    let current_link = Path::new("/rscm/current-system");
                    let current_id = if current_link.exists() {
                        fs::read_link(current_link)
                            .ok()
                            .and_then(|p| {
                                p.file_name()
                                    .map(|n| n.to_os_string())
                                    .and_then(|s| s.into_string().ok())
                            })
                            .and_then(|s| s.parse::<u64>().ok())
                    } else {
                        None
                    };

                    if let Some(id) = id {
                        store.delete_generation(id)
                    } else if let Some(n) = keep {
                        let mut ids: Vec<u64> = generations.iter().map(|g| g.id).collect();
                        ids.sort();
                        let keep_count = n as usize;
                        let ids_to_keep: Vec<u64> =
                            ids.iter().rev().take(keep_count).cloned().collect();
                        let ids_to_delete: Vec<u64> = ids
                            .iter()
                            .filter(|id| !ids_to_keep.contains(id))
                            .cloned()
                            .collect();
                        for id in ids_to_delete {
                            if Some(id) != current_id {
                                store.delete_generation(id)?;
                            }
                        }
                        println!(
                            "Deleted {} generations, kept {} most recent.",
                            ids.len() - keep_count,
                            keep_count
                        );
                        Ok(())
                    } else if let Some(n) = remove_oldest {
                        let mut ids: Vec<u64> = generations.iter().map(|g| g.id).collect();
                        ids.sort();
                        let remove_count = n as usize;
                        let ids_to_delete: Vec<u64> =
                            ids.iter().take(remove_count).cloned().collect();
                        for id in ids_to_delete {
                            if Some(id) != current_id {
                                store.delete_generation(id)?;
                            }
                        }
                        println!("Deleted {} oldest generations.", remove_count);
                        Ok(())
                    } else if all {
                        println!(
                            "WARNING: This will delete all generations except the current one."
                        );
                        print!("Are you sure? Type 'yes' to confirm: ");
                        std::io::Write::flush(&mut std::io::stdout())?;
                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input)?;
                        if input.trim() == "yes" {
                            let mut deleted = 0;
                            for g in &generations {
                                if Some(g.id) != current_id {
                                    store.delete_generation(g.id)?;
                                    deleted += 1;
                                }
                            }
                            println!("Deleted {} generations.", deleted);
                            Ok(())
                        } else {
                            println!("Aborted.");
                            Ok(())
                        }
                    } else {
                        Err(anyhow!(
                            "Missing argument: 'id', 'keep', 'remove_oldest', 'all'\nPlease provide one of these arguments or use -h to get help."
                        ))
                    }
                }
            }
        }
        Commands::Shell => todo!(),
        Commands::Lock {
            update,
            force,
            config,
        } => lock_config(update, force, config),
        Commands::Check { path } => {
            let config_path = if path.is_empty() {
                find_config_file(None)?
            } else {
                PathBuf::from(&path)
            };
            match load_config(config_path) {
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
