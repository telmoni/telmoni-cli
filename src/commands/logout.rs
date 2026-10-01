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
            if let Some(ref env_org) = telmoni_org_env
                && !creds
                    .organizations
                    .iter()
                    .any(|o| &o.organization_id == env_org)
            {
                bail!("TELMONI_ORG names an organization you are not in");
            }

            // The server requires the header of anyone in an organization,
            // and `/me` names an active one whenever the list is non-empty,
            // so the stored active organization is always the right value.
            let active_org_for_revoke = if let Some(ref env_org) = telmoni_org_env {
                Some(env_org.as_str())
            } else {
                creds.active_organization_id.as_deref()
            };

            let org_header = if !creds.organizations.is_empty() {
                active_org_for_revoke.or_else(|| {
                    creds
                        .organizations
                        .first()
                        .map(|o| o.organization_id.as_str())
                })
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
                let _ = revoke_session(transport, &creds.endpoint, token, row_id, org_header).await;
            }

            let _ = store.clear();
            println!("Signed out");
            Ok(())
        }
    }
}
