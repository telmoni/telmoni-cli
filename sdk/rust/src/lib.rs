//! The Rust SDK scaffold for the Telmoni platform: configuration only, no HTTP yet.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

/// Default Telmoni API endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://telmoni.com";

/// Client configuration for connecting to Telmoni.
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    /// Telmoni API endpoint URL (default: `https://telmoni.com`).
    pub endpoint: String,
    /// Organization ID (`org_…`).
    pub organization_id: Option<String>,
    /// The API key (`telmoni_…`, `TELMONI_API_KEY`); `/v1` takes no other bearer.
    /// Never serialized, and redacted in `Debug`: a configuration written out
    /// or logged would otherwise carry the key.
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Destructured, so a field added later has to be placed here: shown,
        // or redacted.
        let Self {
            endpoint,
            organization_id,
            api_key,
        } = self;
        f.debug_struct("Config")
            .field("endpoint", endpoint)
            .field("organization_id", organization_id)
            .field("api_key", &api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            organization_id: None,
            api_key: None,
        }
    }
}

impl Config {
    /// Constructs a new configuration with custom endpoint and organization.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        organization_id: Option<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            organization_id,
            api_key,
        }
    }

    /// Loads configuration from standard environment variables, an empty or
    /// whitespace value counting as unset, as the CLI reads them:
    /// - `TELMONI_ENDPOINT` (or defaults to `https://telmoni.com`)
    /// - `TELMONI_ORG`
    /// - `TELMONI_API_KEY`
    #[must_use]
    pub fn from_env() -> Self {
        let endpoint = env_var("TELMONI_ENDPOINT")
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
            .trim_end_matches('/')
            .to_string();

        Self {
            endpoint,
            organization_id: env_var("TELMONI_ORG"),
            api_key: env_var("TELMONI_API_KEY"),
        }
    }
}

/// A variable's value, `None` when it is unset, empty or whitespace.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// The Telmoni client instance: holds configuration until the SDK's contract exists.
#[derive(Debug, Clone)]
pub struct Client {
    config: Config,
}

/// Type alias for the client matching other SDK conventions.
pub type Telmoni = Client;

impl Client {
    /// Creates a new `Client` with the given configuration, sanitizing the endpoint URL.
    #[must_use]
    pub fn new(mut config: Config) -> Self {
        config.endpoint = config.endpoint.trim_end_matches('/').to_string();
        Self { config }
    }

    /// Creates a new `Client` with the given configuration, sanitizing the endpoint URL.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self::new(config)
    }

    /// Creates a new `Client` initialized from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            config: Config::from_env(),
        }
    }

    /// Returns a reference to the client's current configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new(Config::default())
    }
}
