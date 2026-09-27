//! Public `/v1` Telmoni API types and operations.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::transport::{LaneRequest, Transport, parse_lane_error};

/// Owner information returned by `/v1/team`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V1Owner {
    /// Owner email.
    pub email: String,
    /// Owner display name.
    pub display_name: Option<String>,
}

/// Team details returned by `/v1/team`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V1Team {
    /// Team identifier (`team_...` or `team_...`).
    pub team_id: String,
    /// Team name.
    pub name: Option<String>,
    /// Team owner details.
    pub owner: Option<V1Owner>,
}

impl V1Team {
    /// Returns the resolved label for the team:
    /// `name` when non-empty after trimming, else `owner.email`, else "Team".
    pub fn label(&self) -> &str {
        if let Some(ref n) = self.name {
            let trimmed = n.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        if let Some(ref o) = self.owner {
            return o.email.as_str();
        }
        "Team"
    }
}

/// Retrieves team details for a static API key via `GET {endpoint}/v1/team`.
pub async fn fetch_v1_team(
    transport: &impl Transport,
    endpoint: &str,
    api_key: &str,
) -> Result<V1Team> {
    let url = format!("{}/v1/team", endpoint.trim_end_matches('/'));
    let req = LaneRequest {
        method: reqwest::Method::GET,
        url,
        bearer: Some(api_key.to_string()),
        team: None,
        json: None,
    };

    let answer = transport.send(req).await?;
    if answer.status == 200 {
        let team: V1Team =
            serde_json::from_str(&answer.body).context("decoding /v1/team response")?;
        return Ok(team);
    }

    let lane_err = parse_lane_error(&answer);
    bail!(lane_err)
}
