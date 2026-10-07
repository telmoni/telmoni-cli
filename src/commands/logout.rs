//! `telmoni logout` command implementation.

use anyhow::{Context, Result, bail};
use clap::Args;

use crate::auth::device::{
    SessionEnded, bearer_expired, fetch_me, refresh_session, refused_credentials, revoke_session,
};
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

    // An API key has no session on the server to end.
    if creds.auth_type == AuthType::Device {
        validate_env_org(&creds, telmoni_org_env.as_deref())?;
        if let Err(err) =
            end_session(transport, store, &mut creds, telmoni_org_env.as_deref()).await
        {
            eprintln!(
                "note: the server did not confirm the sign-out ({err}); \
                 end the session from Active sessions in the console"
            );
        }
    }

    // A file that stays signs in still, so "Signed out" would be false.
    store
        .clear()
        .map_err(|err| anyhow::anyhow!("{err}; remove it yourself"))?;
    println!("Signed out");
    Ok(())
}

/// Ends the session on the server: `Ok` once the server confirmed it, or said
/// it had already ended; the error is why that could not be confirmed. What
/// it refreshes on the way it saves, as every refresh does: the file goes
/// afterwards whatever the answer.
pub async fn end_session(
    transport: &impl Transport,
    store: &CredentialsStore,
    creds: &mut Credentials,
    env_org: Option<&str>,
) -> Result<()> {
    let row_id = creds
        .session_row_id
        .clone()
        .context("no session id was saved at sign-in")?;
    let org_header = resolve_revoke_org_header(creds, env_org);

    let now_ts = chrono::Utc::now().timestamp();
    let expiring = creds.expires_at.is_some_and(|exp| now_ts >= exp - 60);
    let mut refreshed = false;
    if expiring && creds.refresh_token.is_some() {
        match refresh_session(transport, store, creds).await {
            Ok(()) => refreshed = true,
            Err(err) if err.is::<SessionEnded>() => return Ok(()),
            // The bearer may still be inside the platform's leeway: the
            // revoke decides.
            Err(_) => {}
        }
    }

    let mut ended =
        revoke_with_org_fallback(transport, creds, &row_id, org_header.as_deref()).await;

    // A clock that runs slow here skips the early refresh, and the revoke is
    // then refused as an expired bearer, or as an unknown one once the
    // platform has swept it. One refresh decides whether the session lives.
    if !expiring && creds.refresh_token.is_some() && ended.as_ref().is_err_and(bearer_expired) {
        match refresh_session(transport, store, creds).await {
            Ok(()) => {
                refreshed = true;
                ended = revoke_with_org_fallback(transport, creds, &row_id, org_header.as_deref())
                    .await;
            }
            Err(err) if err.is::<SessionEnded>() => return Ok(()),
            Err(err) => return Err(err),
        }
    }

    match ended {
        // Refused as a session that is over, or refused again with a bearer
        // just renewed: it had already ended. A bearer refused as expired
        // that could not be renewed proves nothing.
        Err(err) if refused_credentials(&err).is_some() && (refreshed || !bearer_expired(&err)) => {
            Ok(())
        }
        ended => ended,
    }
}

fn validate_env_org(creds: &Credentials, env_org: Option<&str>) -> Result<()> {
    if let Some(target) = env_org
        && creds.organization_named(target).is_none()
    {
        bail!(
            "TELMONI_ORG names no organization in the cached list; run telmoni status \
             without it to refresh the list"
        );
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

async fn revoke_with_org_fallback(
    transport: &impl Transport,
    creds: &Credentials,
    row_id: &str,
    initial_org_header: Option<&str>,
) -> Result<()> {
    let token = creds
        .access_token
        .as_deref()
        .context("missing access token")?;
    let endpoint = &creds.endpoint;
    let mut ended = revoke_session(transport, endpoint, token, row_id, initial_org_header).await;

    // The cached organization list can be stale. The server refuses a
    // revoke that names an organization the person has left (403), and one
    // that names none while they belong to one (400), but takes one naming
    // none from somebody in no organization. So a refused name is dropped
    // first, and only a 400 asks `/me` which to name.
    // ⚠ Not `/me` first: it makes an organization for somebody in none, and
    // signing out would found one.
    if initial_org_header.is_some() && lane_status(&ended) == Some(403) {
        ended = revoke_session(transport, endpoint, token, row_id, None).await;
    }
    if lane_status(&ended) == Some(400)
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
    ended
}

fn lane_status(res: &Result<()>) -> Option<u16> {
    res.as_ref()
        .err()?
        .downcast_ref::<LaneError>()
        .map(|e| e.status)
}
