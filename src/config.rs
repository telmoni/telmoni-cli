//! Configuration management for the Telmoni CLI.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::auth::key::env_key_digest;
use crate::transport::is_loopback_host;

/// Default Telmoni API endpoint: the machine host, where the server serves
/// the lanes a machine calls straight; the console's host answers people.
pub const DEFAULT_TELMONI_ENDPOINT: &str = "https://api.telmoni.com";

/// CLI configuration file structure (`~/.config/telmoni/config.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub endpoint: Option<String>,
    pub output_format: Option<String>,
    /// A shell command that prints an API key, run for each command that
    /// acts with one when `TELMONI_API_KEY` is unset: a secret manager's
    /// read, so the key is fetched fresh and never saved. The command, not
    /// the key, is what this file holds.
    pub api_key_helper: Option<String>,
    /// Whether to act with a key in `TELMONI_API_KEY` ahead of the saved
    /// login, as a person at a terminal answered, by the key's digest
    /// (`env_key_digest`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub api_key_answers: BTreeMap<String, bool>,
}

/// The keys `config get` and `config set` take, for the refusal of any other.
const KEYS: &str = "endpoint, output_format, api_key_helper, use_api_key_env";

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
/// `use_api_key_env` is the answer kept for `env_key`, the key now in
/// `TELMONI_API_KEY`.
pub fn get_config_value(path: &Path, key: &str, env_key: Option<&str>) -> Result<Option<String>> {
    let config = load_config(path)?;
    match key {
        "endpoint" => Ok(config.endpoint),
        "output_format" => Ok(config.output_format),
        "api_key_helper" => Ok(config.api_key_helper),
        "use_api_key_env" => Ok(config
            .api_key_answers
            .get(&answer_slot(env_key)?)
            .map(bool::to_string)),
        _ => bail!("unknown configuration key '{key}'. Valid keys: {KEYS}"),
    }
}

/// Sets a specific config key's value in the file at `path`.
/// `use_api_key_env` answers for `env_key`, the key now in `TELMONI_API_KEY`.
pub fn set_config_value(path: &Path, key: &str, value: &str, env_key: Option<&str>) -> Result<()> {
    let mut config = load_config(path)?;
    match key {
        "endpoint" => config.endpoint = Some(value.to_string()),
        "output_format" => config.output_format = Some(value.to_string()),
        "api_key_helper" => config.api_key_helper = Some(value.to_string()),
        "use_api_key_env" => {
            let answer = match value {
                "true" => true,
                "false" => false,
                _ => bail!("use_api_key_env is true or false"),
            };
            config.api_key_answers.insert(answer_slot(env_key)?, answer);
        }
        _ => bail!("unknown configuration key '{key}'. Valid keys: {KEYS}"),
    }
    save_config(path, &config)
}

/// Keeps `answer`, whether to act with `env_key` ahead of the saved login,
/// in the file at `path`.
pub fn set_api_key_answer(path: &Path, env_key: &str, answer: bool) -> Result<()> {
    set_config_value(path, "use_api_key_env", &answer.to_string(), Some(env_key))
}

/// The digest the answer for `env_key` is kept under; an error, naming the
/// variable and never its value, when it holds no API key.
fn answer_slot(env_key: Option<&str>) -> Result<String> {
    let Some(value) = env_key else {
        bail!("use_api_key_env answers for the key in TELMONI_API_KEY, which is not set");
    };
    env_key_digest(value)
        .context("TELMONI_API_KEY gives no API key: an API key starts with tm_ and has no spaces")
}
