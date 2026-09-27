//! `telmoni login` command implementation.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Args;

use crate::auth::device::{Team, fetch_me, poll_once, poll_until_granted, start_device_auth};
use crate::auth::storage::{AuthType, Credentials, CredentialsStore, StoredPerson, StoredTeam};
use crate::config::{Config, read_dotenv_var, resolve_endpoint};
use crate::transport::Transport;

/// Arguments for `telmoni login`.
#[derive(Debug, Args)]
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

/// Validates that an API key starts with `telmoni_` and contains no whitespace.
pub fn validate_api_key(key: &str) -> Result<()> {
    if !key.starts_with("telmoni_") || key.chars().any(|c| c.is_whitespace()) {
        bail!("an API key starts with telmoni_ and has no spaces");
    }
    Ok(())
}

/// Executes the `telmoni login` flow. `endpoint_env` is `TELMONI_ENDPOINT`
/// as `main` read it (environment, else `.env`).
pub async fn execute(
    args: LoginArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    config: &Config,
    endpoint_env: Option<String>,
    profile: &str,
) -> Result<()> {
    let endpoint = resolve_endpoint(
        args.endpoint.as_deref(),
        endpoint_env.as_deref(),
        config,
        profile,
    );

    // 1. Programmatic API key login
    let explicit_key = args.key.or_else(|| read_dotenv_var("TELMONI_API_KEY"));
    if let Some(key) = explicit_key {
        validate_api_key(&key)?;

        let creds = Credentials {
            auth_type: AuthType::ApiKey,
            endpoint: endpoint.clone(),
            access_token: None,
            refresh_token: None,
            expires_at: None,
            session_row_id: None,
            person: None,
            teams: Vec::new(),
            active_team_id: None,
            api_key: Some(key),
            updated_at: chrono::Utc::now().timestamp(),
        };

        store.save(profile, &creds)?;
        println!("Signed in with API key");
        println!("Endpoint: {endpoint}");
        return Ok(());
    }

    // 2. Interactive device authorization grant (RFC 8628)
    let start = start_device_auth(transport, &endpoint).await?;

    println!("First copy your one-time code: {}", start.user_code);
    println!("Then open {} and enter it.", start.verification_uri);

    if !args.no_browser
        && let Some(ref complete_url) = start.verification_uri_complete
        && let Err(e) = open::that(complete_url)
    {
        eprintln!("note: could not open browser: {e}");
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

    // Fetch user profile and team list
    let me = fetch_me(transport, &endpoint, &authn.access_token, None).await?;

    let stored_teams: Vec<StoredTeam> = me
        .teams
        .iter()
        .map(|team: &Team| StoredTeam {
            team_id: team.team_id.clone(),
            label: team.label().to_string(),
            role: team.role.clone(),
        })
        .collect();

    let creds = Credentials {
        auth_type: AuthType::Device,
        endpoint,
        access_token: Some(authn.access_token),
        refresh_token: authn.refresh_token,
        expires_at: Some(chrono::Utc::now().timestamp() + authn.expires_in),
        session_row_id: me.session_row_id,
        person: Some(StoredPerson {
            user_id: me.person.user_id,
            email: me.person.email.clone(),
            display_name: me.person.display_name,
        }),
        teams: stored_teams,
        active_team_id: me.active_team_id.clone(),
        api_key: None,
        updated_at: chrono::Utc::now().timestamp(),
    };

    store.save(profile, &creds)?;

    println!("Signed in as {}", me.person.email);
    if let Some(active_id) = &me.active_team_id {
        if let Some(team) = creds.teams.iter().find(|o| &o.team_id == active_id) {
            println!(
                "Active team: {} ({}, {})",
                team.label, team.team_id, team.role
            );
        } else {
            println!("Active team: {active_id}");
        }
    } else {
        println!("No team");
    }

    Ok(())
}
