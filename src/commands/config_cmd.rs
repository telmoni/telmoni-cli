//! `telmoni config` command.

use std::path::Path;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::config::{DEFAULT_TELMONI_ENDPOINT, get_config_value, load_config, set_config_value};

/// Arguments for `telmoni config`.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigSubcommand {
    /// Get a configuration parameter.
    Get {
        /// The setting key (endpoint, output_format, api_key_helper).
        key: String,
    },
    /// Set a configuration parameter.
    Set {
        /// The setting key (endpoint, output_format, api_key_helper).
        key: String,
        /// The value to assign.
        value: String,
    },
    /// List all configured settings.
    List,
}

/// Executes the `telmoni config` command against the configuration file at
/// `path`.
pub fn execute(args: ConfigArgs, path: &Path) -> Result<()> {
    match args.command {
        ConfigSubcommand::Get { key } => match get_config_value(path, &key)? {
            Some(val) => println!("{val}"),
            None => println!("(not set)"),
        },
        ConfigSubcommand::Set { key, value } => {
            set_config_value(path, &key, &value)?;
            println!("✓ Set {key} = {value}");
        }
        ConfigSubcommand::List => {
            let config = load_config(path)?;
            println!("Telmoni CLI Configuration");
            println!("-------------------------");
            println!(
                "  endpoint:       {}",
                config
                    .endpoint
                    .unwrap_or_else(|| format!("(default: {DEFAULT_TELMONI_ENDPOINT})"))
            );
            println!(
                "  output_format:  {}",
                config.output_format.as_deref().unwrap_or("(default: text)")
            );
            println!(
                "  api_key_helper: {}",
                config.api_key_helper.as_deref().unwrap_or("(not set)")
            );
        }
    }
    Ok(())
}
