//! `telmoni org` commands for managing organization contexts.

use anyhow::{Result, bail};
use clap::Subcommand;

use crate::auth::device::{fetch_me_for_session, refresh_if_needed};
use crate::auth::storage::{AuthType, Credentials, CredentialsStore, StoredOrganization};
use crate::transport::Transport;

/// Subcommands for `telmoni org`.
#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    /// List organizations you belong to.
    List,
    /// Switch active organization context.
    Switch {
        /// Organization ID (e.g. `org_...`) or name.
        #[arg(value_name = "ORGANIZATION")]
        organization: String,
    },
}

/// Executes `telmoni org` subcommands.
pub async fn execute(
    cmd: OrgCommand,
    transport: &impl Transport,
    store: &CredentialsStore,
) -> Result<()> {
    let mut creds = require_device_session(store)?;

    match cmd {
        OrgCommand::List => {
            execute_list(&creds);
            Ok(())
        }
        OrgCommand::Switch { organization } => {
            execute_switch(&mut creds, &organization, transport, store).await
        }
    }
}

fn require_device_session(store: &CredentialsStore) -> Result<Credentials> {
    let Some(creds) = store.load()? else {
        bail!("organization commands need a browser session; run telmoni login");
    };

    if creds.auth_type != AuthType::Device {
        bail!("organization commands need a browser session; run telmoni login");
    }

    Ok(creds)
}

fn execute_list(creds: &Credentials) {
    if creds.organizations.is_empty() {
        println!("No organizations");
        return;
    }

    for org in &creds.organizations {
        let marker = if creds.active_organization_id.as_deref() == Some(&org.organization_id) {
            '*'
        } else {
            ' '
        };
        println!(
            "{marker} {}  {}  {}",
            org.organization_id, org.label, org.role
        );
    }
}

async fn execute_switch(
    creds: &mut Credentials,
    organization: &str,
    transport: &impl Transport,
    store: &CredentialsStore,
) -> Result<()> {
    let target = resolve_target_org(&creds.organizations, organization)?;
    let target_id = target.organization_id.clone();

    refresh_if_needed(transport, store, creds).await?;

    let me = fetch_me_for_session(transport, store, creds, Some(&target_id)).await?;

    if me.active_organization_id.as_deref() != Some(&target_id) {
        bail!("you are no longer in {organization}");
    }

    creds.update_from_me(&me, false);
    store.save(creds)?;

    if let Some(org) = creds.find_organization(&target_id) {
        println!("Active organization: {}", org.display_summary());
    }

    Ok(())
}

fn resolve_target_org<'a>(
    orgs: &'a [StoredOrganization],
    identifier: &str,
) -> Result<&'a StoredOrganization> {
    if let Some(by_id) = orgs.iter().find(|o| o.organization_id == identifier) {
        return Ok(by_id);
    }

    let matching: Vec<_> = orgs
        .iter()
        .filter(|o| o.label.eq_ignore_ascii_case(identifier))
        .collect();

    match matching.as_slice() {
        [] => bail!("unknown organization {identifier}; run telmoni organization list"),
        [single] => Ok(*single),
        multiple => {
            let ids: Vec<String> = multiple
                .iter()
                .map(|o| format!("{} ({})", o.organization_id, o.role))
                .collect();
            bail!(
                "multiple organizations named '{identifier}': {}; specify by organization ID",
                ids.join(", ")
            );
        }
    }
}
