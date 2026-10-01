//! The official Rust SDK for the Telmoni platform.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

/// Default Telmoni API endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://telmoni.com";

/// Client configuration for connecting to Telmoni.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Telmoni API endpoint URL (default: `https://telmoni.com`).
    pub endpoint: String,
    /// Tenant or organization ID.
    pub tenant_id: Option<String>,
    /// Authentication token or API key (`TELMONI_API_KEY`).
    pub auth_token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            tenant_id: None,
            auth_token: None,
        }
    }
}

impl Config {
    /// Constructs a new configuration with custom endpoint and tenant.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        tenant_id: Option<String>,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            tenant_id,
            auth_token,
        }
    }

    /// Loads configuration from standard environment variables:
    /// - `TELMONI_ENDPOINT` (or defaults to `https://telmoni.com`)
    /// - `TELMONI_TENANT_ID`
    /// - `TELMONI_API_KEY`
    #[must_use]
    pub fn from_env() -> Self {
        let endpoint = std::env::var("TELMONI_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
            .trim_end_matches('/')
            .to_string();

        let tenant_id = std::env::var("TELMONI_TENANT_ID").ok();
        let auth_token = std::env::var("TELMONI_API_KEY").ok();

        Self {
            endpoint,
            tenant_id,
            auth_token,
        }
    }
}

/// The official Telmoni client instance.
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
