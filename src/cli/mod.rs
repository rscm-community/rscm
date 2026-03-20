use crate::lua::LuaEngine;
use crate::toolchain::ToolchainManager;
use anyhow::Result;
use clap::{Parser, Subcommand};

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
    Lock,
    Check {
        path: String,
    },
    Toolchain {
        #[command(subcommand)]
        action: ToolchainAction,
    },
}

#[derive(Subcommand)]
pub enum ToolchainAction {
    Status,
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Edit => todo!(),
        Commands::Build => todo!(),
        Commands::Switch => todo!(),
        Commands::Generations => todo!(),
        Commands::Shell => todo!(),
        Commands::Lock => todo!(),
        Commands::Check { path } => {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path, e))?;

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
    }
}
