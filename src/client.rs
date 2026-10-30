//! Public `/v1` Telmoni API types and operations.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::transport::{LaneRequest, Transport, expect_json};

/// Owner information returned by `/v1/organization`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V1Owner {
    /// Owner email.
    pub email: String,
    /// Owner display name.
    pub display_name: Option<String>,
}

/// Organization details returned by `/v1/organization`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V1Organization {
    /// Organization identifier (`org_...`).
    pub organization_id: String,
    /// The slug the console's paths name it by (`/{slug}`): set once at birth,
    /// to the URL its founder chose or else derived from the name it is born
    /// with (a placeholder, `org-` and ten random characters, when that name
    /// gives none), then moved only by a change to its URL on Settings, never
    /// by a rename. Only the id names the organization.
    pub slug: String,
    /// Organization name, never empty.
    pub name: String,
    /// Organization owner details.
    pub owner: Option<V1Owner>,
}

impl V1Organization {
    /// The organization's label: its name, as the console shows it. Never the
    /// owner's address, which `/v1/organization` carries beside it: an
    /// address is a person's.
    pub fn label(&self) -> &str {
        self.name.trim()
    }
}

/// Retrieves organization details for a static API key via `GET {endpoint}/v1/organization`.
pub async fn fetch_v1_organization(
    transport: &impl Transport,
    endpoint: &str,
    api_key: &str,
) -> Result<V1Organization> {
    let req = LaneRequest {
        bearer: Some(api_key.to_string()),
        ..LaneRequest::to(reqwest::Method::GET, endpoint, "/v1/organization")
    };
    expect_json(
        &transport.send(req).await?,
        200,
        "the /v1/organization answer",
    )
}
