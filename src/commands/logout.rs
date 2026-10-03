//! `telmoni logout` command implementation.

use anyhow::{Result, bail};
use clap::Args;

use crate::auth::device::{fetch_me, refresh_tokens, revoke_session};
use crate::auth::storage::{AuthType, Credentials, CredentialsStore};
use crate::transport::{LaneError, Transport};

/// Arguments for `telmoni logout`.
#[derive(Debug, Args)]
pub struct LogoutArgs {}

/// Executes the `telmoni logout` flow.
pub async fn execute(
    _args: LogoutArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    telmoni_org_env: Option<String>,
) -> Result<()> {
    let Some(mut creds) = store.load()? else {
        let _ = store.clear();
        println!("Not signed in");
        return Ok(());
    };

    match creds.auth_type {
        AuthType::ApiKey => {
            let _ = store.clear();
            println!("Signed out");
            Ok(())
        }
        AuthType::Device => {
            execute_device_logout(transport, store, &mut creds, telmoni_org_env).await
        }
    }
}

async fn execute_device_logout(
    transport: &impl Transport,
    store: &CredentialsStore,
    creds: &mut Credentials,
    telmoni_org_env: Option<String>,
) -> Result<()> {
    validate_env_org(creds, telmoni_org_env.as_deref())?;

    let skip_revoke = refresh_token_if_expiring(transport, creds).await;

    if !skip_revoke
        && let (Some(token), Some(row_id)) = (&creds.access_token, &creds.session_row_id)
    {
        let org_header = resolve_revoke_org_header(creds, telmoni_org_env.as_deref());
        revoke_with_org_fallback(
            transport,
            &creds.endpoint,
            token,
            row_id,
            org_header.as_deref(),
        )
        .await;
    }

    let _ = store.clear();
    println!("Signed out");
    Ok(())
}

fn validate_env_org(creds: &Credentials, env_org: Option<&str>) -> Result<()> {
    if let Some(target) = env_org
        && creds.organization_named(target).is_none()
    {
        bail!("TELMONI_ORG names an organization you are not in");
    }
    Ok(())
}

/// The server requires the header of anyone in an organization, and `/me`
/// names an active one whenever the list is non-empty, so the stored active
/// organization is always the right value when the environment names none.
fn resolve_revoke_org_header(creds: &Credentials, env_org: Option<&str>) -> Option<String> {
    if creds.organizations.is_empty() {
        return None;
    }

    // The header takes the id, whichever of the two the environment gave.
    env_org
        .and_then(|target| creds.organization_named(target))
        .map(|named| named.organization_id.clone())
        .or_else(|| creds.active_organization_id.clone())
        .or_else(|| {
            creds
                .organizations
                .first()
                .map(|o| o.organization_id.clone())
        })
}

async fn refresh_token_if_expiring(transport: &impl Transport, creds: &mut Credentials) -> bool {
    let now_ts = chrono::Utc::now().timestamp();
    let is_expiring = creds.expires_at.is_some_and(|exp| now_ts >= exp - 60);

    if !is_expiring {
        return false;
    }

    let Some(ref rt) = creds.refresh_token else {
        return false;
    };

    match refresh_tokens(
        transport,
        &creds.endpoint,
        rt,
        creds.session_row_id.as_deref(),
    )
    .await
    {
        Ok(authn) => {
            creds.apply_refresh(&authn);
            false
        }
        Err(err) => err
            .downcast_ref::<LaneError>()
            .is_some_and(|lane_err| lane_err.status == 401),
    }
}

async fn revoke_with_org_fallback(
    transport: &impl Transport,
    endpoint: &str,
    token: &str,
    row_id: &str,
    initial_org_header: Option<&str>,
) {
    let mut ended = revoke_session(transport, endpoint, token, row_id, initial_org_header).await;

    // The cached organization list can be stale, and the server
    // refuses a revoke that names an organization the person has
    // left (403) or names none while they belong to one (400).
    // `/me` says which one to name now.
    if lane_status(&ended).is_some_and(|s| s == 400 || s == 403)
        && let Ok(me) = fetch_me(transport, endpoint, token, None).await
    {
        ended = revoke_session(
            transport,
            endpoint,
            token,
            row_id,
            me.active_organization_id.as_deref(),
        )
        .await;
    }

    // A 401 means the session was already ended elsewhere.
    if lane_status(&ended) != Some(401)
        && let Err(err) = ended
    {
        eprintln!(
            "note: the server did not confirm the sign-out ({err}); \
             end the session from Active sessions in the console"
        );
    }
}

fn lane_status(res: &Result<()>) -> Option<u16> {
    res.as_ref()
        .err()?
        .downcast_ref::<LaneError>()
        .map(|e| e.status)
}
