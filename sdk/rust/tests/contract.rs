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
        api_key: None,
    };
    let client = Telmoni::new(config);
    assert_eq!(client.config().endpoint, "https://custom.endpoint");
}

#[test]
fn debug_never_prints_the_key() {
    let config = Config::new(
        "https://telmoni.com",
        Some("org_1".to_string()),
        Some("telmoni_secret_key".to_string()),
    );
    let client = Telmoni::new(config.clone());

    for printed in [format!("{config:?}"), format!("{client:#?}")] {
        assert!(!printed.contains("telmoni_secret_key"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(printed.contains("org_1"), "{printed}");
    }
    assert_eq!(
        client.config().api_key.as_deref(),
        Some("telmoni_secret_key")
    );
}

#[test]
fn from_env_initializes() {
    let client = Telmoni::from_env();
    assert!(!client.config().endpoint.is_empty());
}
