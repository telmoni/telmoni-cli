//! `telmoni config` command.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::config::{get_config_value, load_config, set_config_value};

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
        /// The setting key (endpoint, output_format).
        key: String,
    },
    /// Set a configuration parameter.
    Set {
        /// The setting key (endpoint, output_format).
        key: String,
        /// The value to assign.
        value: String,
    },
    /// List all configured settings.
    List,
}

/// Executes the `telmoni config` command.
pub fn execute(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigSubcommand::Get { key } => match get_config_value(&key)? {
            Some(val) => println!("{val}"),
            None => println!("(not set)"),
        },
        ConfigSubcommand::Set { key, value } => {
            set_config_value(&key, &value)?;
            println!("✓ Set {key} = {value}");
        }
        ConfigSubcommand::List => {
            let config = load_config()?;
            println!("Telmoni CLI Configuration");
            println!("-------------------------");
            println!(
                "  endpoint:      {}",
                config
                    .endpoint
                    .as_deref()
                    .unwrap_or("(default: https://telmoni.com)")
            );
            println!(
                "  output_format: {}",
                config.output_format.as_deref().unwrap_or("(default: text)")
            );
        }
    }
    Ok(())
}
