//! `telmoni team` commands for managing team contexts.

use anyhow::{Result, bail};
use clap::Subcommand;

use crate::auth::device::{fetch_me, refresh_if_needed};
use crate::auth::storage::{AuthType, CredentialsStore, StoredPerson, StoredTeam};
use crate::transport::Transport;

/// Subcommands for `telmoni team`.
#[derive(Debug, Subcommand)]
pub enum TeamCommand {
    /// List teams you belong to.
    List,
    /// Switch active team context.
    Switch {
        /// Team ID (e.g. `team_...`).
        team_id: String,
    },
}

/// Executes `telmoni team` subcommands.
pub async fn execute(
    cmd: TeamCommand,
    transport: &impl Transport,
    store: &CredentialsStore,
    profile: &str,
) -> Result<()> {
    let mut creds = match store.load(profile)? {
        Some(c) => c,
        None => bail!("team commands need a browser session; run telmoni login"),
    };

    if creds.auth_type != AuthType::Device {
        bail!("team commands need a browser session; run telmoni login");
    }

    match cmd {
        TeamCommand::List => {
            if creds.teams.is_empty() {
                println!("No teams");
                return Ok(());
            }

            for team in &creds.teams {
                let marker = if creds.active_team_id.as_deref() == Some(&team.team_id) {
                    '*'
                } else {
                    ' '
                };
                println!("{marker} {}  {}  {}", team.team_id, team.label, team.role);
            }
            Ok(())
        }
        TeamCommand::Switch { team_id } => {
            if !creds.teams.iter().any(|o| o.team_id == team_id) {
                bail!("unknown team {team_id}; run telmoni team list");
            }

            refresh_if_needed(transport, store, profile, &mut creds).await?;

            let access_token = creds.access_token.as_deref().unwrap_or_default();

            let me = fetch_me(transport, &creds.endpoint, access_token, Some(&team_id)).await?;

            if me.active_team_id.as_deref() != Some(&team_id) {
                bail!("you are no longer in {team_id}");
            }

            creds.person = Some(StoredPerson {
                user_id: me.person.user_id,
                email: me.person.email,
                display_name: me.person.display_name,
            });
            creds.teams = me
                .teams
                .iter()
                .map(|o| StoredTeam {
                    team_id: o.team_id.clone(),
                    label: o.label().to_string(),
                    role: o.role.clone(),
                })
                .collect();
            creds.active_team_id = me.active_team_id;
            creds.session_row_id = me.session_row_id;
            creds.updated_at = chrono::Utc::now().timestamp();

            store.save(profile, &creds)?;

            if let Some(team) = creds.teams.iter().find(|o| o.team_id == team_id) {
                println!(
                    "Active team: {} ({}, {})",
                    team.label, team.team_id, team.role
                );
            }

            Ok(())
        }
    }
}
