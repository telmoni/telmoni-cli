//! `telmoni login` command implementation.

use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Args;

use crate::auth::device::{fetch_me, poll_once, poll_until_granted, start_device_auth};
use crate::auth::storage::{Credentials, CredentialsStore, print_active_organization};
use crate::config::{Config, resolve_endpoint};
use crate::transport::{REDACTED, Transport};

/// Arguments for `telmoni login`.
#[derive(Args)]
pub struct LoginArgs {
    /// Authenticate non-interactively using an API key.
    // clap's help prints an `env` argument's current value beside its name;
    // hidden, so `telmoni login --help` on a host that exports the key does
    // not write the key to the terminal or a CI log.
    #[arg(long, env = "TELMONI_API_KEY", hide_env_values = true)]
    pub key: Option<String>,

    /// Target Telmoni endpoint URL (e.g. `https://telmoni.com`).
    #[arg(long, env = "TELMONI_ENDPOINT")]
    pub endpoint: Option<String>,

    /// Do not automatically open the browser.
    #[arg(long)]
    pub no_browser: bool,
}

/// ⚠ By hand: `key` is the API key.
impl std::fmt::Debug for LoginArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            key,
            endpoint,
            no_browser,
        } = self;
        f.debug_struct("LoginArgs")
            .field("key", &key.as_ref().map(|_| REDACTED))
            .field("endpoint", endpoint)
            .field("no_browser", no_browser)
            .finish()
    }
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

/// Whether `candidate` is on `endpoint`'s origin: the same scheme, host and
/// port, the port read with its scheme's default filled in.
pub fn same_origin(candidate: &str, endpoint: &str) -> bool {
    let (Ok(candidate), Ok(endpoint)) = (
        reqwest::Url::parse(candidate),
        reqwest::Url::parse(endpoint),
    ) else {
        return false;
    };
    candidate.scheme() == endpoint.scheme()
        && candidate.host_str() == endpoint.host_str()
        && candidate.port_or_known_default() == endpoint.port_or_known_default()
}

/// Executes the `telmoni login` flow. `endpoint_env` is `TELMONI_ENDPOINT`
/// as `main` read it. The browser and the wait between polls are passed in,
/// like the transport, so a test runs the whole flow without either.
pub async fn execute<B, S, SF>(
    args: LoginArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    config: &Config,
    endpoint_env: Option<String>,
    open_browser: B,
    sleep: S,
) -> Result<()>
where
    B: FnOnce(&str) -> std::io::Result<()>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let endpoint = resolve_endpoint(args.endpoint.as_deref(), endpoint_env.as_deref(), config);

    if let Some(key) = args.key {
        return login_with_api_key(&key, endpoint, store);
    }

    let browser = (!args.no_browser).then_some(open_browser);
    login_with_device_flow(endpoint, transport, store, browser, sleep).await
}

fn login_with_api_key(key: &str, endpoint: String, store: &CredentialsStore) -> Result<()> {
    validate_api_key(key)?;
    let creds = Credentials::for_api_key(endpoint.clone(), key.to_string());
    store.save(&creds)?;

    println!("Signed in with API key");
    println!("Endpoint: {endpoint}");
    Ok(())
}

async fn login_with_device_flow<B, S, SF>(
    endpoint: String,
    transport: &impl Transport,
    store: &CredentialsStore,
    open_browser: Option<B>,
    sleep: S,
) -> Result<()>
where
    B: FnOnce(&str) -> std::io::Result<()>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let start = start_device_auth(transport, &endpoint).await?;

    println!("First copy your one-time code: {}", start.user_code);
    println!("Then open {} and enter it.", start.verification_uri);

    if let Some(open_browser) = open_browser {
        let browser_url = start
            .verification_uri_complete
            .as_deref()
            .unwrap_or(&start.verification_uri);
        // Opened only on the endpoint's own origin: the answer is the
        // server's, and one naming another host or scheme would hand the
        // one-time code — and the machine's URL handler — to whatever it
        // named. The address is printed above either way.
        if same_origin(browser_url, &endpoint) {
            if let Err(e) = open_browser(browser_url) {
                eprintln!("note: could not open browser: {e}");
            }
        } else {
            eprintln!("note: the verification page is not on {endpoint}; open it yourself");
        }
    }

    eprintln!("Waiting for approval…");

    let interval = Duration::from_secs(start.interval.max(1));
    let expires_in = Duration::from_secs(start.expires_in);

    let authn = poll_until_granted(
        interval,
        expires_in,
        || poll_once(transport, &endpoint, &start.device_code),
        sleep,
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
