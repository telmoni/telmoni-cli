//! Configuration management for the Telmoni CLI.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::transport::is_loopback_host;

/// Default Telmoni API endpoint.
pub const DEFAULT_TELMONI_ENDPOINT: &str = "https://telmoni.com";

/// CLI configuration file structure (`~/.config/telmoni/config.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub endpoint: Option<String>,
    pub output_format: Option<String>,
}

/// Normalizes an endpoint string by trimming whitespace, stripping trailing slashes,
/// and ensuring an `http://` (for this machine) or `https://` scheme.
fn normalize_endpoint_url(s: &str) -> Option<String> {
    let trimmed = s.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }
    // The host is parsed out, not read off the front of the string:
    // `localhost.example.com` starts with `localhost` and is not this machine.
    let local = reqwest::Url::parse(&format!("http://{trimmed}"))
        .is_ok_and(|url| url.host_str().is_some_and(is_loopback_host));
    let scheme = if local { "http" } else { "https" };
    Some(format!("{scheme}://{trimmed}"))
}

/// The endpoint a command that holds no stored credentials talks to, in
/// precedence order: the `--endpoint` flag, then `TELMONI_ENDPOINT` (read by
/// `main` and passed in as `env_value`), then the config file, then the
/// default. Pure, so it is testable; the environment is read only at the
/// command entry point.
pub fn resolve_endpoint(flag: Option<&str>, env_value: Option<&str>, config: &Config) -> String {
    // clap fills `--endpoint` from `TELMONI_ENDPOINT` too, so the first
    // source cannot tell the two apart.
    let (endpoint, source) = flag
        .and_then(normalize_endpoint_url)
        .map(|e| (e, "--endpoint or TELMONI_ENDPOINT"))
        .or_else(|| {
            env_value
                .and_then(normalize_endpoint_url)
                .map(|e| (e, "TELMONI_ENDPOINT"))
        })
        .or_else(|| {
            config
                .endpoint
                .as_deref()
                .and_then(normalize_endpoint_url)
                .map(|e| (e, "the configuration file"))
        })
        .unwrap_or_else(|| (DEFAULT_TELMONI_ENDPOINT.to_string(), "the default"));
    debug!(%endpoint, source, "endpoint");
    endpoint
}

/// Loads the configuration file at `path`; a missing one is the default.
pub fn load_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        debug!(path = %path.display(), "no configuration file");
        return Ok(Config::default());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file from {}", path.display()))?;

    let config: Config = serde_json::from_str(&contents)
        .with_context(|| format!("parsing config file from {}", path.display()))?;

    Ok(config)
}

/// Saves the configuration to the file at `path`.
pub fn save_config(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(config).context("serializing config")?;
    std::fs::write(path, json)
        .with_context(|| format!("writing config file to {}", path.display()))?;

    Ok(())
}

/// Gets a specific config key's value from the file at `path`.
pub fn get_config_value(path: &Path, key: &str) -> Result<Option<String>> {
    let config = load_config(path)?;
    match key {
        "endpoint" => Ok(config.endpoint),
        "output_format" => Ok(config.output_format),
        _ => bail!("unknown configuration key '{key}'. Valid keys: endpoint, output_format"),
    }
}

/// Sets a specific config key's value in the file at `path`.
pub fn set_config_value(path: &Path, key: &str, value: &str) -> Result<()> {
    let mut config = load_config(path)?;
    match key {
        "endpoint" => config.endpoint = Some(value.to_string()),
        "output_format" => config.output_format = Some(value.to_string()),
        _ => bail!("unknown configuration key '{key}'. Valid keys: endpoint, output_format"),
    }
    save_config(path, &config)
}
