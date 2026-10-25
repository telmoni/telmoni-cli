//! `telmoni login` command implementation.

use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Args;

use crate::auth::device::{fetch_me, poll_once, poll_until_granted, start_device_auth};
use crate::auth::storage::{Credentials, CredentialsStore, print_active_organization};
use crate::config::{Config, resolve_endpoint};
use crate::transport::Transport;

/// Arguments for `telmoni login`.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Save an API key read from standard input, for scripts and servers:
    /// `telmoni login --with-key < key.txt`.
    // Never the key itself as an argument: that reaches `ps`, shell history
    // and CI logs. A run that needs a key without saving one sets
    // `TELMONI_API_KEY` instead.
    #[arg(long, conflicts_with = "no_browser")]
    pub with_key: bool,

    /// Target Telmoni endpoint URL (e.g. `https://api.telmoni.com`).
    #[arg(long, env = "TELMONI_ENDPOINT")]
    pub endpoint: Option<String>,

    /// Do not automatically open the browser.
    #[arg(long)]
    pub no_browser: bool,
}

/// Validates that an API key starts with `tm_`, has a non-empty payload, and contains no whitespace.
pub fn validate_api_key(key: &str) -> Result<()> {
    if !key.starts_with("tm_") || key.len() <= "tm_".len() || key.chars().any(|c| c.is_whitespace())
    {
        bail!("an API key starts with tm_ and has no spaces");
    }
    Ok(())
}

/// Whether `candidate` is a page of the deployment `endpoint` names: on
/// `endpoint`'s own origin — the same scheme, host and port, the port read
/// with its scheme's default filled in — or on the console beside a machine
/// host, the host `endpoint` has with its leading `api.` taken off, over the
/// same scheme and port. The device page is the console's, and a platform
/// keeps its console and its machine host apart (`telmoni.com` beside
/// `api.telmoni.com`), so the one page the CLI opens unasked is the one the
/// platform it signs in to named, and never an address a stranger's answer
/// could point it at.
pub fn same_deployment(candidate: &str, endpoint: &str) -> bool {
    let (Ok(candidate), Ok(endpoint)) = (
        reqwest::Url::parse(candidate),
        reqwest::Url::parse(endpoint),
    ) else {
        return false;
    };
    if candidate.scheme() != endpoint.scheme()
        || candidate.port_or_known_default() != endpoint.port_or_known_default()
    {
        return false;
    }
    let (Some(page), Some(machine)) = (candidate.host_str(), endpoint.host_str()) else {
        return false;
    };
    page == machine || machine.strip_prefix("api.") == Some(page)
}

/// Executes the `telmoni login` flow. `endpoint_env` is `TELMONI_ENDPOINT`
/// as `main` read it. Standard input, the browser and the wait between polls
/// are passed in, like the transport, so a test runs the whole flow without
/// any of them.
#[expect(
    clippy::too_many_arguments,
    reason = "each seam a test replaces is a parameter, as the transport is"
)]
pub async fn execute<K, B, S, SF>(
    args: LoginArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    config: &Config,
    endpoint_env: Option<String>,
    read_key: K,
    open_browser: B,
    sleep: S,
) -> Result<()>
where
    K: FnOnce() -> Result<String>,
    B: FnOnce(&str) -> std::io::Result<()>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let endpoint = resolve_endpoint(args.endpoint.as_deref(), endpoint_env.as_deref(), config);

    if args.with_key {
        return login_with_api_key(read_key()?.trim(), endpoint, store);
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
        // Opened only on a page of the deployment signed in to — the
        // endpoint's own origin, or the console beside its machine host
        // (`same_deployment`): the answer is the server's, and one naming
        // another host or scheme would hand the one-time code — and the
        // machine's URL handler — to whatever it named. The address is
        // printed above either way.
        if same_deployment(browser_url, &endpoint) {
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

#[cfg(test)]
mod tests {
    use super::same_deployment;

    /// The platform's own page opens: on the endpoint itself, or on the
    /// console beside its machine host.
    #[test]
    fn the_console_beside_a_machine_host_is_the_same_deployment() {
        assert!(same_deployment(
            "https://telmoni.com/auth/device?code=ABCD",
            "https://api.telmoni.com"
        ));
        assert!(same_deployment(
            "https://dev.telmoni.com/auth/device",
            "https://api.dev.telmoni.com"
        ));
        assert!(same_deployment(
            "https://telmoni.example.com/auth/device",
            "https://telmoni.example.com"
        ));
        assert!(same_deployment(
            "http://localhost:3000/auth/device",
            "http://localhost:3000"
        ));
    }

    /// Anything else stays shut: another host, the label the other way
    /// round, another scheme, another port, or no address at all.
    #[test]
    fn any_other_address_is_not() {
        assert!(!same_deployment(
            "https://evil.example/auth/device",
            "https://api.telmoni.com"
        ));
        assert!(!same_deployment(
            "https://api.telmoni.com/auth/device",
            "https://telmoni.com"
        ));
        assert!(!same_deployment(
            "https://notapi.telmoni.com/auth/device",
            "https://api.telmoni.com"
        ));
        assert!(!same_deployment(
            "http://telmoni.com/auth/device",
            "https://api.telmoni.com"
        ));
        assert!(!same_deployment(
            "https://telmoni.com:8443/auth/device",
            "https://api.telmoni.com"
        ));
        assert!(!same_deployment("not a url", "https://api.telmoni.com"));
    }
}
