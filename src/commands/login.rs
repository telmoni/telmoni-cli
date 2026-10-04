//! `telmoni login` command implementation.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Args;

use crate::auth::device::{fetch_me, poll_once, poll_until_granted, start_device_auth};
use crate::auth::storage::{Credentials, CredentialsStore, print_active_organization};
use crate::config::{Config, resolve_endpoint};
use crate::transport::Transport;

/// Arguments for `telmoni login`. No `Debug`: `key` is the API key.
#[derive(Args)]
pub struct LoginArgs {
    /// Authenticate non-interactively using an API key.
    #[arg(long, env = "TELMONI_API_KEY")]
    pub key: Option<String>,

    /// Target Telmoni endpoint URL (e.g. `https://telmoni.com`).
    #[arg(long, env = "TELMONI_ENDPOINT")]
    pub endpoint: Option<String>,

    /// Do not automatically open the browser.
    #[arg(long)]
    pub no_browser: bool,
}

/// Validates that an API key starts with `telmoni_`, has a non-empty payload, and contains no whitespace.
pub fn validate_api_key(key: &str) -> Result<()> {
    if !key.starts_with("telmoni_")
        || key.len() <= "telmoni_".len()
        || key.chars().any(|c| c.is_whitespace())
    {
        bail!("an API key starts with telmoni_ and has no spaces");
    }
    Ok(())
}

/// Executes the `telmoni login` flow. `endpoint_env` is `TELMONI_ENDPOINT`
/// as `main` read it.
pub async fn execute(
    args: LoginArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    config: &Config,
    endpoint_env: Option<String>,
) -> Result<()> {
    let endpoint = resolve_endpoint(args.endpoint.as_deref(), endpoint_env.as_deref(), config);

    if let Some(key) = args.key {
        return login_with_api_key(&key, endpoint, store);
    }

    login_with_device_flow(args, endpoint, transport, store).await
}

fn login_with_api_key(key: &str, endpoint: String, store: &CredentialsStore) -> Result<()> {
    validate_api_key(key)?;
    let creds = Credentials::for_api_key(endpoint.clone(), key.to_string());
    store.save(&creds)?;

    println!("Signed in with API key");
    println!("Endpoint: {endpoint}");
    Ok(())
}

async fn login_with_device_flow(
    args: LoginArgs,
    endpoint: String,
    transport: &impl Transport,
    store: &CredentialsStore,
) -> Result<()> {
    let start = start_device_auth(transport, &endpoint).await?;

    println!("First copy your one-time code: {}", start.user_code);
    println!("Then open {} and enter it.", start.verification_uri);

    if !args.no_browser {
        let browser_url = start
            .verification_uri_complete
            .as_deref()
            .unwrap_or(&start.verification_uri);
        if let Err(e) = open::that(browser_url) {
            eprintln!("note: could not open browser: {e}");
        }
    }

    eprintln!("Waiting for approval…");

    let interval = Duration::from_secs(start.interval.max(1));
    let expires_in = Duration::from_secs(start.expires_in);

    let authn = poll_until_granted(
        interval,
        expires_in,
        || poll_once(transport, &endpoint, &start.device_code),
        tokio::time::sleep,
        Instant::now,
    )
    .await?;

    let me = fetch_me(transport, &endpoint, &authn.access_token, None).await?;
    let email = me.person.email.clone();
    let raw_active_id = me.active_organization_id.clone();

    let creds = Credentials::from_device_auth(endpoint, authn, me);
    store.save(&creds)?;

    println!("Signed in as {email}");
    print_active_organization(creds.active_organization(), raw_active_id.as_deref());

    Ok(())
}
