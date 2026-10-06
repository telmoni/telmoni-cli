//! `telmoni org` commands for managing organization contexts.

use anyhow::{Result, anyhow, bail};
use clap::Subcommand;

use crate::auth::device::{fetch_me_for_session, refresh_if_needed};
use crate::auth::storage::{AuthType, Credentials, CredentialsStore};
use crate::transport::Transport;

/// Subcommands for `telmoni org`.
#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    /// List organizations you belong to.
    List,
    /// Switch active organization context.
    Switch {
        /// Organization ID (e.g. `org_...`) or slug (as in the console's URL).
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
        bail!("org commands need a browser session; run telmoni login");
    };

    if creds.auth_type != AuthType::Device {
        bail!("org commands need a browser session; run telmoni login");
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
            "{marker} {}  {}  {}  {}",
            org.organization_id, org.slug, org.label, org.role
        );
    }
}

async fn execute_switch(
    creds: &mut Credentials,
    organization: &str,
    transport: &impl Transport,
    store: &CredentialsStore,
) -> Result<()> {
    // By id or slug, against the cache alone: an unknown one costs no request.
    // `org list` reads the same cache, so it is `status` that refreshes it.
    let Some(target) = creds.organization_named(organization) else {
        bail!(
            "unknown organization {organization}: not in the cached list; run telmoni status \
             to refresh it, then telmoni org list"
        );
    };
    let target_id = target.organization_id.clone();

    refresh_if_needed(transport, store, creds).await?;

    let me = fetch_me_for_session(transport, store, creds, Some(&target_id)).await?;

    // ⚠ The cache named the organization, and two things may have moved since:
    // `/me` acts in the person's default organization when the one asked for
    // is no longer theirs, and a URL change in the console takes a slug
    // along, after which another organization may hold it. So the answer has
    // to act in the organization, and its list has to give it the same name.
    // The list is kept either way, which is what lets `org list` and the next
    // run read the organizations as the console does; the active organization
    // moves to the one asked for only when both hold.
    creds.update_from_me(&me, true);
    let holds = if me.active_organization_id.as_deref() != Some(&target_id) {
        Err(anyhow!("you are no longer in {organization}"))
    } else {
        match creds.organization_named(organization) {
            Some(named) if named.organization_id == target_id => Ok(()),
            Some(_) => Err(anyhow!(
                "{organization} now names another organization; run telmoni org list"
            )),
            None => Err(anyhow!(
                "unknown organization {organization}; run telmoni org list"
            )),
        }
    };
    if holds.is_ok() {
        creds.active_organization_id = Some(target_id.clone());
    }
    store.save(creds)?;
    holds?;

    if let Some(org) = creds.find_organization(&target_id) {
        println!("Active organization: {}", org.display_summary());
    }

    Ok(())
}
