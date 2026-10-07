//! `telmoni status` and `telmoni whoami` commands.

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::json;

use crate::auth::device::{fetch_me_for_session, refresh_if_needed};
use crate::auth::storage::{AuthType, Credentials, CredentialsStore, print_active_organization};
use crate::client::fetch_v1_organization;
use crate::config::Config;
use crate::transport::Transport;

/// Arguments for `telmoni status` and `telmoni whoami`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Format output as machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

/// Executes the `status` / `whoami` check. `telmoni_org_env` is `TELMONI_ORG`
/// and `endpoint_env` is `TELMONI_ENDPOINT`, both as `main` read them.
pub async fn execute(
    args: StatusArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    config: &Config,
    telmoni_org_env: Option<String>,
    endpoint_env: Option<String>,
) -> Result<()> {
    let Some(mut creds) = store.load()? else {
        // A script that asked for JSON reads stdout as JSON: it gets that or
        // nothing, and the refusal on stderr.
        if !args.json {
            let endpoint = crate::config::resolve_endpoint(None, endpoint_env.as_deref(), config);
            println!("Endpoint: {endpoint}");
        }
        bail!("Not signed in. Run telmoni login, or telmoni login --key <API key>.");
    };

    match creds.auth_type {
        AuthType::ApiKey => execute_api_key(args, transport, &creds).await,
        AuthType::Device => {
            execute_device(args, transport, store, &mut creds, telmoni_org_env).await
        }
    }
}

async fn execute_api_key(
    args: StatusArgs,
    transport: &impl Transport,
    creds: &Credentials,
) -> Result<()> {
    let api_key = creds
        .api_key
        .as_deref()
        .context("missing api key in credentials")?;

    let org = fetch_v1_organization(transport, &creds.endpoint, api_key).await?;

    if args.json {
        let out = json!({
            "authType": "api_key",
            "endpoint": creds.endpoint,
            "organization": {
                "organizationId": org.organization_id,
                "slug": org.slug,
                "label": org.label(),
            }
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("API key for {} ({})", org.organization_id, org.label());
        println!("Endpoint: {}", creds.endpoint);
    }
    Ok(())
}

async fn execute_device(
    args: StatusArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    creds: &mut Credentials,
    telmoni_org_env: Option<String>,
) -> Result<()> {
    let active_org_id = resolve_device_active_org(creds, telmoni_org_env.as_deref())?;

    // Refresh first if access token is within 60 seconds of expiry
    refresh_if_needed(transport, store, creds).await?;

    let me = fetch_me_for_session(transport, store, creds, active_org_id.as_deref()).await?;
    creds.update_from_me(&me, telmoni_org_env.is_some());
    // Only the cached list is at stake: the tokens a refresh rotated were
    // saved by the refresh itself, or it failed.
    if let Err(err) = store.save(creds) {
        eprintln!(
            "note: the organization list could not be saved ({err}); the next command fetches it again"
        );
    }

    // `/me` falls back to the person's default organization — the one they
    // chose in Account Settings, else the oldest they own, else the oldest
    // they belong to — when the one asked for is no longer theirs, so the
    // cached list's say-so is not enough.
    if let Some(env_org) = telmoni_org_env.as_deref()
        && me.active_organization_id != active_org_id
    {
        bail!("you are no longer in {env_org}");
    }

    // ⚠ Nor is its say-so on a slug. A URL change moves one and another
    // organization may take it since, so the list just answered has to
    // give the organization the same name; else this would report on one
    // the console no longer shows at that URL.
    if let Some(env_org) = telmoni_org_env.as_deref() {
        let named_now = creds.organization_named(env_org);
        if named_now.map(|o| &o.organization_id) != active_org_id.as_ref() {
            bail!("{env_org} no longer names the organization it did; run telmoni org list");
        }
    }

    let effective_active_id = if telmoni_org_env.is_some() {
        active_org_id.as_deref()
    } else {
        creds.active_organization_id.as_deref()
    };

    let remaining_secs = creds.expires_at.unwrap_or(0) - chrono::Utc::now().timestamp();
    let token_expires_in_secs = remaining_secs.max(0);

    if args.json {
        print_device_json(creds, effective_active_id, token_expires_in_secs)?;
    } else {
        print_device_text(creds, effective_active_id, token_expires_in_secs)?;
    }
    Ok(())
}

/// The id of the organization to act in: the one the environment names, by
/// id or slug, else the stored active one.
fn resolve_device_active_org(
    creds: &Credentials,
    telmoni_org_env: Option<&str>,
) -> Result<Option<String>> {
    if let Some(env_org) = telmoni_org_env {
        // The cache, not the truth: a slug the organization took since, by a
        // URL change, is unknown here until `/cli/me` is read again.
        let Some(named) = creds.organization_named(env_org) else {
            bail!(
                "TELMONI_ORG names no organization in the cached list; run telmoni status \
                 without it to refresh the list"
            );
        };
        Ok(Some(named.organization_id.clone()))
    } else {
        Ok(creds.active_organization_id.clone())
    }
}

fn print_device_json(
    creds: &Credentials,
    effective_active_id: Option<&str>,
    token_expires_in_secs: i64,
) -> Result<()> {
    let active_org = effective_active_id
        .and_then(|id| creds.find_organization(id))
        .map(|o| {
            json!({
                "organizationId": o.organization_id,
                "slug": o.slug,
                "label": o.label,
                "role": o.role,
            })
        });

    let person_json = json!({
        "userId": creds.person.as_ref().map(|p| &p.user_id),
        "email": creds.person.as_ref().map(|p| &p.email),
        "displayName": creds.person.as_ref().and_then(|p| p.display_name.as_ref()),
    });

    let out = json!({
        "authType": "device",
        "endpoint": creds.endpoint,
        "person": person_json,
        "activeOrganization": active_org,
        "organizationCount": creds.organizations.len(),
        "tokenExpiresInSecs": token_expires_in_secs,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn print_device_text(
    creds: &Credentials,
    effective_active_id: Option<&str>,
    token_expires_in_secs: i64,
) -> Result<()> {
    let person = creds
        .person
        .as_ref()
        .context("missing person in credentials")?;

    if let Some(ref name) = person.display_name {
        println!("Signed in as {} ({})", person.email, name);
    } else {
        println!("Signed in as {}", person.email);
    }

    let active_org = effective_active_id.and_then(|id| creds.find_organization(id));
    print_active_organization(active_org, effective_active_id);

    println!("Organizations: {}", creds.organizations.len());
    println!("Endpoint: {}", creds.endpoint);
    let minutes = token_expires_in_secs / 60;
    println!("Token: expires in {minutes} minutes");
    Ok(())
}
