use telmoni_sdk::{Config, DEFAULT_ENDPOINT, Telmoni};

#[test]
fn default_config_initializes() {
    let config = Config::default();
    assert_eq!(config.endpoint, DEFAULT_ENDPOINT);
    assert_eq!(config.endpoint, "https://telmoni.com");
    let client = Telmoni::new(config);
    assert_eq!(client.config().endpoint, "https://telmoni.com");
}

#[test]
fn custom_config_strips_trailing_slashes() {
    let config = Config {
        endpoint: "https://custom.endpoint///".to_string(),
        organization_id: None,
        auth_token: None,
    };
    let client = Telmoni::new(config);
    assert_eq!(client.config().endpoint, "https://custom.endpoint");
}

#[test]
fn from_env_initializes() {
    let client = Telmoni::from_env();
    assert!(!client.config().endpoint.is_empty());
}
