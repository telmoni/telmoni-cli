//! `telmoni profile` commands for managing connection profiles.

use anyhow::Result;
use clap::Subcommand;

use crate::config::{Config, save_config};

/// Subcommands for `telmoni profile`.
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List configured profiles configured in config.json.
    List,
    /// Set the active profile.
    Use {
        /// Name of the profile.
        name: String,
    },
    /// Show the active profile.
    Show,
}

/// Executes `telmoni profile` subcommands.
pub fn execute(cmd: ProfileCommand, mut config: Config) -> Result<()> {
    match cmd {
        ProfileCommand::List => {
            let active = crate::config::active_profile(None, &config);
            println!("Active: {active}");
            println!("Profiles:");
            for name in config.profiles.keys() {
                let marker = if name == &active { "*" } else { " " };
                println!("{marker} {name}");
            }
            if !config.profiles.contains_key("default") {
                let marker = if "default" == active { "*" } else { " " };
                println!("{marker} default");
            }
        }
        ProfileCommand::Use { name } => {
            config.active_profile = Some(name.clone());
            save_config(&config)?;
            println!("Active profile set to '{name}'");
        }
        ProfileCommand::Show => {
            let active = crate::config::active_profile(None, &config);
            println!("{active}");
        }
    }
    Ok(())
}
