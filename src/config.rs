//! Configuration management for the Telmoni CLI.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Default Telmoni API endpoint.
pub const DEFAULT_TELMONI_ENDPOINT: &str = "https://telmoni.com";

/// CLI configuration file structure (`~/.config/telmoni/config.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub endpoint: Option<String>,
    pub output_format: Option<String>,
}

/// Normalizes an endpoint string by trimming whitespace, stripping trailing slashes,
/// and ensuring an `http://` (for localhost) or `https://` scheme.
pub fn normalize_endpoint_url(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_trailing = trimmed.trim_end_matches('/');
    if without_trailing.starts_with("http://") || without_trailing.starts_with("https://") {
        Some(without_trailing.to_string())
    } else if without_trailing.starts_with("localhost")
        || without_trailing.starts_with("127.0.0.1")
        || without_trailing.starts_with("::1")
    {
        Some(format!("http://{without_trailing}"))
    } else {
        Some(format!("https://{without_trailing}"))
    }
}

/// The endpoint a command that holds no stored credentials talks to, in
/// precedence order: the `--endpoint` flag, then `TELMONI_ENDPOINT` (read by
/// `main` and passed in as `env_value`), then the config file, then the
/// default. Pure, so it is testable; the environment is read only at the
/// command entry point.
pub fn resolve_endpoint(flag: Option<&str>, env_value: Option<&str>, config: &Config) -> String {
    flag.and_then(normalize_endpoint_url)
        .or_else(|| env_value.and_then(normalize_endpoint_url))
        .or_else(|| config.endpoint.as_deref().and_then(normalize_endpoint_url))
        .unwrap_or_else(|| DEFAULT_TELMONI_ENDPOINT.to_string())
}

/// Returns the base directory for Telmoni configuration and state.
/// Defaults to `dirs::config_dir()`, falling back to the current directory (`.`)
/// in headless or minimal container environments where `$HOME` is not set.
pub fn base_config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Returns the path to `~/.config/telmoni/config.json`.
pub fn config_path() -> Result<PathBuf> {
    Ok(base_config_dir().join("telmoni").join("config.json"))
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
