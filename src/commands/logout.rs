//! `telmoni logout` command implementation.

use anyhow::{Result, bail};
use clap::Args;

use crate::auth::device::{refresh_tokens, revoke_session};
use crate::auth::storage::{AuthType, CredentialsStore};
use crate::transport::{LaneError, Transport};

/// Arguments for `telmoni logout`.
#[derive(Debug, Args)]
pub struct LogoutArgs {}

/// Executes the `telmoni logout` flow.
pub async fn execute(
    _args: LogoutArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    profile: &str,
    telmoni_team_env: Option<String>,
) -> Result<()> {
    let mut creds = match store.load(profile)? {
        Some(c) => c,
        None => {
            if store.path.exists() {
                let _ = store.clear(profile);
            }
            println!("Not signed in");
            return Ok(());
        }
    };

    match creds.auth_type {
        AuthType::ApiKey => {
            let _ = store.clear(profile);
            println!("Signed out");
            Ok(())
        }
        AuthType::Device => {
            if let Some(ref env_team) = telmoni_team_env
                && !creds.teams.iter().any(|o| &o.team_id == env_team)
            {
                bail!("TELMONI_TEAM names an team you are not in");
            }

            // The server requires the header of anyone in an team,
            // and `/me` names an active one whenever the list is non-empty,
            // so the stored active team is always the right value.
            let active_team_for_revoke = if let Some(ref env_team) = telmoni_team_env {
                Some(env_team.as_str())
            } else {
                creds.active_team_id.as_deref()
            };

            let team_header = if !creds.teams.is_empty() {
                active_team_for_revoke.or_else(|| creds.teams.first().map(|o| o.team_id.as_str()))
            } else {
                None
            };

            let mut skip_revoke = false;
            let now_ts = chrono::Utc::now().timestamp();
            if let Some(exp) = creds.expires_at
                && now_ts >= exp - 60
                && let Some(ref rt) = creds.refresh_token
            {
                match refresh_tokens(
                    transport,
                    &creds.endpoint,
                    rt,
                    creds.session_row_id.as_deref(),
                )
                .await
                {
                    Ok(authn) => {
                        creds.access_token = Some(authn.access_token);
                    }
                    Err(err) => {
                        if let Some(lane_err) = err.downcast_ref::<LaneError>()
                            && lane_err.status == 401
                        {
                            skip_revoke = true;
                        }
                    }
                }
            }

            if !skip_revoke
                && let (Some(token), Some(row_id)) = (&creds.access_token, &creds.session_row_id)
            {
                let _ =
                    revoke_session(transport, &creds.endpoint, token, row_id, team_header).await;
            }

            let _ = store.clear(profile);
            println!("Signed out");
            Ok(())
        }
    }
}
