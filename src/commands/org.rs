//! `telmoni org` commands for managing organization contexts.

use anyhow::{Result, bail};
use clap::Subcommand;

use crate::auth::device::{fetch_me, refresh_if_needed};
use crate::auth::storage::{AuthType, CredentialsStore, StoredOrganization, StoredPerson};
use crate::transport::Transport;

/// Subcommands for `telmoni org`.
#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    /// List organizations you belong to.
    List,
    /// Switch active organization context.
    Switch {
        /// Organization ID (e.g. `org_...`).
        org_id: String,
    },
}

/// Executes `telmoni org` subcommands.
pub async fn execute(
    cmd: OrgCommand,
    transport: &impl Transport,
    store: &CredentialsStore,
) -> Result<()> {
    let Some(mut creds) = store.load()? else {
        bail!("org commands need a browser session; run telmoni login");
    };

    if creds.auth_type != AuthType::Device {
        bail!("org commands need a browser session; run telmoni login");
    }

    match cmd {
        OrgCommand::List => {
            if creds.organizations.is_empty() {
                println!("No organizations");
                return Ok(());
            }

            for org in &creds.organizations {
                let marker =
                    if creds.active_organization_id.as_deref() == Some(&org.organization_id) {
                        '*'
                    } else {
                        ' '
                    };
                println!(
                    "{marker} {}  {}  {}",
                    org.organization_id, org.label, org.role
                );
            }
            Ok(())
        }
        OrgCommand::Switch { org_id } => {
            if !creds
                .organizations
                .iter()
                .any(|o| o.organization_id == org_id)
            {
                bail!("unknown organization {org_id}; run telmoni org list");
            }

            refresh_if_needed(transport, store, &mut creds).await?;

            let access_token = creds.access_token.as_deref().unwrap_or_default();

            let me = fetch_me(transport, &creds.endpoint, access_token, Some(&org_id)).await?;

            if me.active_organization_id.as_deref() != Some(&org_id) {
                bail!("you are no longer in {org_id}");
            }

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
            creds.active_organization_id = me.active_organization_id;
            creds.session_row_id = me.session_row_id;
            creds.updated_at = chrono::Utc::now().timestamp();

            store.save(&creds)?;

            if let Some(org) = creds
                .organizations
                .iter()
                .find(|o| o.organization_id == org_id)
            {
                println!(
                    "Active organization: {} ({}, {})",
                    org.label, org.organization_id, org.role
                );
            }

            Ok(())
        }
    }
}
