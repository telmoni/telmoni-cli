//! Configuration management for the Telmoni CLI.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Default Telmoni API endpoint.
pub const DEFAULT_TELMONI_ENDPOINT: &str = "https://telmoni.com";

/// Reads a key value from a local `.env` file without requiring unsafe environment mutations.
pub fn read_dotenv_var(key: &str) -> Option<String> {
    let content = std::fs::read_to_string(".env").ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=')
            && k.trim() == key
        {
            let mut val = v.trim();
            if (val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\''))
            {
                val = &val[1..val.len() - 1];
            }
            let val_str = val.trim().to_string();
            if !val_str.is_empty() {
                return Some(val_str);
            }
        }
    }
    None
}

/// CLI configuration file structure (`~/.config/telmoni/config.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub endpoint: Option<String>,
    pub output_format: Option<String>,
    pub active_profile: Option<String>,
    #[serde(default)]
    pub profiles: std::collections::HashMap<String, ProfileConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileConfig {
    pub endpoint: Option<String>,
    pub output_format: Option<String>,
}

/// The endpoint a command that holds no stored credentials talks to, in
/// precedence order: the `--endpoint` flag, then `TELMONI_ENDPOINT` (the
/// environment, else `.env`, both read by `main` and passed in as
/// `env_value`), then the config file, then the default. Pure, so it is
/// testable; the environment is read only at the command entry point.
pub fn resolve_endpoint(
    flag: Option<&str>,
    env_value: Option<&str>,
    config: &Config,
    profile: &str,
) -> String {
    let non_blank = |s: &str| {
        let trimmed = s.trim();
        (!trimmed.is_empty()).then(|| trimmed.trim_end_matches('/').to_string())
    };
    if let Some(ep) = flag.and_then(non_blank) {
        return ep;
    }
    if let Some(ep) = env_value.and_then(non_blank) {
        return ep;
    }

    if let Some(pconf) = config.profiles.get(profile)
        && let Some(ep) = pconf.endpoint.as_deref().and_then(non_blank)
    {
        return ep;
    }

    if profile == "default"
        && let Some(ep) = config.endpoint.as_deref().and_then(non_blank)
    {
        return ep;
    }

    DEFAULT_TELMONI_ENDPOINT.to_string()
}

pub fn active_profile(cli_profile: Option<&str>, config: &Config) -> String {
    if let Some(p) = cli_profile
        && !p.trim().is_empty()
    {
        return p.trim().to_string();
    }
    config
        .active_profile
        .clone()
        .unwrap_or_else(|| "default".to_string())
}

/// Returns the path to `~/.config/telmoni/config.json`.
pub fn config_path() -> Result<PathBuf> {
    let base_dir = dirs::config_dir().context("resolving user config directory")?;
    Ok(base_dir.join("telmoni").join("config.json"))
}

/// Loads configuration from disk, returning default if file does not exist.
pub fn load_config() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("reading config file from {}", path.display()))?;

    let config: Config = serde_json::from_str(&contents)
        .with_context(|| format!("parsing config file from {}", path.display()))?;

    Ok(config)
}

/// Saves configuration to disk.
pub fn save_config(config: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(config).context("serializing config")?;
    std::fs::write(&path, json)
        .with_context(|| format!("writing config file to {}", path.display()))?;

    Ok(())
}

/// Gets a specific config key's value.
pub fn get_config_value(key: &str) -> Result<Option<String>> {
    let config = load_config()?;
    match key {
        "endpoint" => Ok(config.endpoint),
        "output_format" => Ok(config.output_format),
        _ => bail!("unknown configuration key '{key}'. Valid keys: endpoint, output_format"),
    }
}

/// Sets a specific config key's value and saves to disk.
pub fn set_config_value(key: &str, value: &str) -> Result<()> {
    let mut config = load_config()?;
    match key {
        "endpoint" => config.endpoint = Some(value.to_string()),
        "output_format" => config.output_format = Some(value.to_string()),
        _ => bail!("unknown configuration key '{key}'. Valid keys: endpoint, output_format"),
    }
    save_config(&config)
}
