use crate::cache::CacheManager;
use crate::config::Configuration;
use crate::lock::{LockManager, LockTracker};
use crate::lua::LuaEngine;
use crate::store::Store;
use crate::toolchain::ToolchainManager;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use nix::unistd::geteuid;
use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

const SYSTEM_CONFIG_DIR: &str = "/etc/rscm";
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
    #[command(about = "Initialize the rscm storage")]
    Init {
        #[arg(
            long,
            short,
            help = "Force initialization even if store already exists"
        )]
        force: bool,
    },
    #[command(about = "Edit the configuration file")]
    Edit,
    #[command(about = "Build a new system generation")]
    Build {
        #[arg(long, short, help = "Sync lock file before building")]
        sync: bool,
        #[arg(long, help = "Specify the target system name")]
        system: Option<String>,
    },
    #[command(about = "Switch to a different system generation")]
    Switch {
        #[arg(help = "Generation ID to switch to")]
        id: Option<u64>,
        #[arg(long, short, help = "Sync lock file before switching")]
        sync: bool,
        #[arg(long, help = "Specify the target system name")]
        system: Option<String>,
    },
    #[command(about = "Manage system generations")]
    Generations {
        #[command(subcommand)]
        action: GenerationsAction,
    },
    #[command(about = "Start a shell session")]
    Shell,
    #[command(about = "Lock the configuration")]
    Lock {
        #[arg(long, short, help = "Update existing lock file")]
        update: bool,
        #[arg(long, short, help = "Force lock creation")]
        force: bool,
        #[arg(long, short, help = "Path to configuration file")]
        config: Option<String>,
    },
    #[command(about = "Check configuration syntax")]
    Check {
        #[arg(default_value = "", help = "Path to configuration file to check")]
        path: String,
    },
    #[command(about = "Manage toolchain")]
    Toolchain {
        #[command(subcommand)]
        action: ToolchainAction,
    },
    #[command(about = "Manage cache")]
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    #[command(about = "Garbage collect unreferenced store contents")]
    Gc {
        #[arg(
            long,
            short,
            help = "Show what would be deleted without actually deleting"
        )]
        dry_run: bool,
        #[arg(
            long,
            help = "Delete unreferenced generations before running GC (keeps current generation)"
        )]
        generations: bool,
        #[arg(
            long,
            requires = "generations",
            help = "When used with --generations, keep the most recent N generations"
        )]
        keep: Option<u64>,
        #[arg(
            long,
            requires = "generations",
            help = "When used with --generations, remove the oldest N generations"
        )]
        remove_oldest: Option<u64>,
        #[arg(long, help = "Delete the specified generation before running GC")]
        delete_generation: Option<u64>,
        #[arg(
            long,
            value_delimiter = ',',
            help = "Delete the specified generations before running GC (comma-separated IDs)"
        )]
        delete_generations: Option<Vec<u64>>,
    },
}

#[derive(Subcommand)]
pub enum GenerationsAction {
    #[command(about = "List all system generations")]
    List,
    #[command(about = "Delete system generations")]
    Delete {
        #[arg(help = "Generation ID to delete")]
        id: Option<u64>,
        #[arg(long, short, help = "Keep the most recent N generations")]
        keep: Option<u64>,
        #[arg(long, short, help = "Remove the oldest N generations")]
        remove_oldest: Option<u64>,
        #[arg(long, short, help = "Delete all generations except the current one")]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum ToolchainAction {
    #[command(about = "Show toolchain status")]
    Status,
}

#[derive(Subcommand)]
pub enum CacheAction {
    #[command(about = "Show cache status")]
    Status,
    #[command(about = "Clean cache")]
    Clean {
        #[arg(long, help = "Clean archive cache")]
        archive: bool,
        #[arg(long, help = "Clean AUR cache")]
        aur: bool,
        #[arg(long, help = "Clean all caches")]
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
        println!("Creating {}...", SYSTEM_STORE_ROOT);

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
    }

    create_store_subdirs(&system_store)?;

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
        "scripts",
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

fn init_config_dir() -> Result<()> {
    let config_dir = Path::new(SYSTEM_CONFIG_DIR);
    let config_file = config_dir.join("configuration.lua");

    if config_dir.exists() {
        println!(
            "✓ Configuration directory {} already exists",
            SYSTEM_CONFIG_DIR
        );
    } else {
        println!("Creating {}...", SYSTEM_CONFIG_DIR);
        std::fs::create_dir_all(config_dir).map_err(|e| {
            anyhow!(
                "Failed to create {}: {}. Run with sudo.",
                SYSTEM_CONFIG_DIR,
                e
            )
        })?;
        println!("✓ Created {}", SYSTEM_CONFIG_DIR);
    }

    if config_file.exists() {
        println!("✓ Configuration file {} already exists", SYSTEM_CONFIG_PATH);
    } else {
        println!("Creating {}...", SYSTEM_CONFIG_PATH);
         let default_config = r#"-- rscm configuration file
system {
    hostname = "my-host",
    timezone = "Asia/Shanghai",
    locale = "zh_CN.UTF-8",
    locales = {
        "zh_CN.UTF-8 UTF-8",
        "en_US.UTF-8 UTF-8",
    },
    keymap = "us",
    cleanup = {
        generations = { keep = 10 },
    },
}

packages {
    -- List packages to install
    "vim",
    "git",
    "curl",
}

services {
    -- Define systemd services
    -- sshd = {
    --     enable = true,
    --     start_now = true,
    -- },
}

users {
    -- Define user accounts
    -- alice = {
    --     password = "password",
    -- },
}

boot {
    -- kernel = {
    --     package = "linux",
    --     params = {
    --         "quiet",
    --         "splash",
    --     },
    -- },
    -- loader = {
    --     systemdBoot = {
    --         enable = true,
    --     },
    },
}
"#;
        std::fs::write(&config_file, default_config).map_err(|e| {
            anyhow!(
                "Failed to create {}: {}. Run with sudo.",
                SYSTEM_CONFIG_PATH,
                e
            )
        })?;
        println!("✓ Created {}", SYSTEM_CONFIG_PATH);
    }

    Ok(())
}

fn check_root() {
    let euid = geteuid();
    if !euid.is_root() {
        println!("Hint: This operation requires root privileges.\nRun with: sudo rscm <command>");
        process::exit(1);
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

    engine.load_config(&content, &config_path)
}

fn build_system(system: Option<String>) -> Result<u64> {
    println!("Building new generation...",);
    let config_path = find_config_file(Some(SYSTEM_CONFIG_PATH))?;
    let config = load_config(config_path.clone())?;
    let store_root = get_store_root()?;
    let mut store = Store::new(store_root)?;

    let config_dir = config_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid config path"))?;
    let tracker = LockTracker::new(config_dir);
    let lock_file = tracker.load()?;
    let Some(lock_file) = lock_file else {
        return Err(anyhow!(
            "No lock file found.\nRun 'rscm lock' first or use '-s' parameter."
        ));
    };
    let id = store.create_generation(config, &lock_file)?;
    println!("New generation id: {}", id);
    Ok(id)
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Init { force } => {
            println!("Initializing rscm...");
            init_config_dir()?;
            let store_root = init_store(force)?;
            println!("\nStore root: {}", store_root.display());
            println!("Configuration: {}", SYSTEM_CONFIG_PATH);
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
            let mut store = Store::new(store_root)?;
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
        Commands::Gc {
            dry_run,
            generations,
            keep,
            remove_oldest,
            delete_generation,
            delete_generations,
        } => {
            check_root();
            let store_root = get_store_root()?;
            let mut store = Store::new(store_root)?;

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

            let mut generations_deleted = 0u64;

            if let Some(id) = delete_generation {
                if Some(id) == current_id {
                    return Err(anyhow!(
                        "Cannot delete generation {}: it is currently active",
                        id
                    ));
                }
                store.delete_generation(id)?;
                generations_deleted += 1;
            }

            if let Some(ids) = delete_generations {
                for id in &ids {
                    if Some(*id) == current_id {
                        return Err(anyhow!(
                            "Cannot delete generation {}: it is currently active",
                            id
                        ));
                    }
                    store.delete_generation(*id)?;
                    generations_deleted += 1;
                }
            }

            if generations {
                let all_gens = store.list_generations()?;
                let mut ids: Vec<u64> = all_gens.iter().map(|g| g.id).collect();
                ids.sort();

                if let Some(n) = keep {
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
                            generations_deleted += 1;
                        }
                    }
                } else if let Some(n) = remove_oldest {
                    let remove_count = n as usize;
                    let ids_to_delete: Vec<u64> = ids.iter().take(remove_count).cloned().collect();
                    for id in ids_to_delete {
                        if Some(id) != current_id {
                            store.delete_generation(id)?;
                            generations_deleted += 1;
                        }
                    }
                } else {
                    for g in &all_gens {
                        if Some(g.id) != current_id {
                            store.delete_generation(g.id)?;
                            generations_deleted += 1;
                        }
                    }
                }
            }

            if generations_deleted > 0 {
                if dry_run {
                    println!("Would delete {} generation(s).", generations_deleted);
                } else {
                    println!("Deleted {} generation(s).", generations_deleted);
                }
            }

            let result = store.gc(dry_run)?;
            if dry_run {
                println!("Dry run - would collect:");
            } else {
                println!("Collected:");
            }
            println!("  {} content files", result.collected_contents);
            println!("  {} packages", result.collected_packages);
            println!("  freed: {}", CacheManager::format_size(result.freed_space));
            Ok(())
        }
    }
}
