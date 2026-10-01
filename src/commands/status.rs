//! `telmoni status` and `telmoni whoami` commands.

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::json;

use crate::auth::device::{fetch_me, refresh_if_needed, refresh_tokens};
use crate::auth::storage::{AuthType, CredentialsStore, StoredOrganization, StoredPerson};
use crate::client::fetch_v1_organization;
use crate::config::Config;
use crate::transport::{LaneError, Transport};

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
        let endpoint = crate::config::resolve_endpoint(None, endpoint_env.as_deref(), config);
        println!("Endpoint: {endpoint}");
        bail!("Not signed in. Run telmoni login, or telmoni login --key <API key>.");
    };

    match creds.auth_type {
        AuthType::ApiKey => {
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
        AuthType::Device => {
            let active_org_id = if let Some(env_org) = telmoni_org_env.as_deref() {
                if !creds
                    .organizations
                    .iter()
                    .any(|o| o.organization_id == env_org)
                {
                    bail!("TELMONI_ORG names an organization you are not in");
                }
                Some(env_org.to_string())
            } else {
                creds.active_organization_id.clone()
            };

            // Refresh first if access token is within 60 seconds of expiry
            refresh_if_needed(transport, store, &mut creds).await?;

            let access_token = creds
                .access_token
                .as_deref()
                .context("missing access token")?;

            // Fetch /cli/me with token expired retry handling
            let me_res = fetch_me(
                transport,
                &creds.endpoint,
                access_token,
                active_org_id.as_deref(),
            )
            .await;

            let me = match me_res {
                Ok(m) => m,
                Err(err) => {
                    if let Some(lane_err) = err.downcast_ref::<LaneError>() {
                        if lane_err.status == 401 {
                            if lane_err.problem_type.as_deref()
                                == Some("/errors/auth/token-expired")
                            {
                                // Refresh once and retry once
                                let rt = creds
                                    .refresh_token
                                    .as_ref()
                                    .context("missing refresh token")?;
                                let authn = refresh_tokens(
                                    transport,
                                    &creds.endpoint,
                                    rt,
                                    creds.session_row_id.as_deref(),
                                )
                                .await
                                .map_err(|_e| {
                                    let _ = store.clear();
                                    anyhow::anyhow!("session ended; run telmoni login")
                                })?;

                                creds.access_token = Some(authn.access_token.clone());
                                if let Some(new_rt) = authn.refresh_token {
                                    creds.refresh_token = Some(new_rt);
                                }
                                creds.expires_at =
                                    Some(chrono::Utc::now().timestamp() + authn.expires_in);
                                creds.updated_at = chrono::Utc::now().timestamp();
                                store.save(&creds)?;

                                fetch_me(
                                    transport,
                                    &creds.endpoint,
                                    &authn.access_token,
                                    active_org_id.as_deref(),
                                )
                                .await
                                .map_err(|_| {
                                    let _ = store.clear();
                                    anyhow::anyhow!("session ended; run telmoni login")
                                })?
                            } else {
                                let _ = store.clear();
                                bail!("session ended; run telmoni login");
                            }
                        } else {
                            bail!("{}", lane_err.message);
                        }
                    } else {
                        return Err(err);
                    }
                }
            };

            creds.person = Some(StoredPerson {
                user_id: me.person.user_id,
                email: me.person.email,
                display_name: me.person.display_name,
            });
            creds.organizations = me
                .organizations
                .iter()
                .map(|o| StoredOrganization {
                    organization_id: o.organization_id.clone(),
                    label: o.label().to_string(),
                    role: o.role.clone(),
                })
                .collect();
            if telmoni_org_env.is_none() {
                creds.active_organization_id = me.active_organization_id.clone();
            }
            if me.session_row_id.is_some() {
                creds.session_row_id = me.session_row_id;
            }
            creds.updated_at = chrono::Utc::now().timestamp();
            let _ = store.save(&creds);

            let effective_active_org_id = if telmoni_org_env.is_some() {
                active_org_id
            } else {
                creds.active_organization_id.clone()
            };

            let remaining_secs = creds.expires_at.unwrap_or(0) - chrono::Utc::now().timestamp();
            let token_expires_in_secs = remaining_secs.max(0);

            if args.json {
                let active_org = if let Some(ref active_id) = effective_active_org_id {
                    creds
                        .organizations
                        .iter()
                        .find(|o| &o.organization_id == active_id)
                        .map(|o| {
                            json!({
                                "organizationId": o.organization_id,
                                "label": o.label,
                                "role": o.role,
                            })
                        })
                } else {
                    None
                };

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
            } else {
                let person = creds
                    .person
                    .as_ref()
                    .context("missing person in credentials")?;
                if let Some(ref name) = person.display_name {
                    println!("Signed in as {} ({})", person.email, name);
                } else {
                    println!("Signed in as {}", person.email);
                }

                if let Some(ref active_id) = effective_active_org_id {
                    if let Some(org) = creds
                        .organizations
                        .iter()
                        .find(|o| &o.organization_id == active_id)
                    {
                        println!(
                            "Active organization: {} ({}, {})",
                            org.label, org.organization_id, org.role
                        );
                    } else {
                        println!("Active organization: {active_id}");
                    }
                } else {
                    println!("No organization");
                }

                println!("Organizations: {}", creds.organizations.len());
                println!("Endpoint: {}", creds.endpoint);
                let minutes = token_expires_in_secs / 60;
                println!("Token: expires in {minutes} minutes");
            }
            Ok(())
        }
    }
}
