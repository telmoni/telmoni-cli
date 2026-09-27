//! RFC 8628 Device Authorization Grant implementation for Telmoni CLI.

use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::auth::storage::{Credentials, CredentialsStore};
use crate::transport::{LaneError, LaneRequest, Transport, parse_lane_error};

/// Response from `POST /cli/auth/device`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    /// Secret device code to poll with.
    pub device_code: String,
    /// User-facing short code (e.g. "ABCD-EFGH").
    pub user_code: String,
    /// Verification URL where user enters the code.
    pub verification_uri: String,
    /// Complete verification URL with code embedded, if supported.
    pub verification_uri_complete: Option<String>,
    /// Lifetime of device code in seconds.
    pub expires_in: u64,
    /// Minimal polling interval in seconds.
    pub interval: u64,
}

/// Authentication result returned on successful authorization or refresh.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthnResult {
    /// User ID.
    pub user_id: String,
    /// User email address.
    pub email: Option<String>,
    /// Whether user email is verified.
    #[serde(default)]
    pub email_verified: bool,
    /// First name.
    pub first_name: Option<String>,
    /// Last name.
    pub last_name: Option<String>,
    /// Access token (JWT).
    pub access_token: String,
    /// Refresh token (null if unchanged).
    pub refresh_token: Option<String>,
    /// Access token validity in seconds.
    pub expires_in: i64,
    /// Authentication method used.
    pub auth_method: Option<String>,
}

/// Person identity returned by `/cli/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    /// User ID.
    pub user_id: String,
    /// Email address.
    pub email: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Whether user opted into analytics.
    #[serde(default)]
    pub analytics_opt_in: bool,
}

/// Team membership details returned by `/cli/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    /// Team ID.
    pub team_id: String,
    /// Team name.
    pub name: Option<String>,
    /// Owner email address.
    pub owner_email: Option<String>,
    /// Owner display name.
    pub owner_display_name: Option<String>,
    /// User's role (`owner`, `admin`, `member`).
    pub role: String,
}

impl Team {
    /// Computes the team label:
    /// `name` when non-empty after trimming, else `owner_email` when non-null, else "Team".
    pub fn label(&self) -> &str {
        if let Some(ref n) = self.name {
            let trimmed = n.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        if let Some(ref email) = self.owner_email {
            return email.as_str();
        }
        "Team"
    }
}

/// Response returned by `POST /cli/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    /// Person identity.
    pub person: Person,
    /// Teams list.
    #[serde(default)]
    pub teams: Vec<Team>,
    /// Active team ID.
    #[serde(default)]
    pub active_team_id: Option<String>,
    /// Session row ID for Active Sessions tracking.
    pub session_row_id: Option<String>,
    /// Whether this is the person's first login.
    #[serde(default)]
    pub first_login: bool,
}

/// Result of a single device code poll attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// Authorization was approved and tokens were issued.
    Granted(AuthnResult),
    /// Authorization is still pending user approval.
    Pending,
    /// Authorization pending, but polling was too fast; add 5 seconds to interval.
    SlowDown,
    /// Client was rate limited by server.
    RateLimited {
        /// Retry-After duration in seconds, when provided.
        retry_after_secs: Option<u64>,
    },
    /// Polling failed definitively with an error message to display.
    Failed(String),
}

/// Initiates device authorization by calling `POST {endpoint}/cli/auth/device`.
pub async fn start_device_auth(transport: &impl Transport, endpoint: &str) -> Result<DeviceStart> {
    let url = format!("{}/cli/auth/device", endpoint.trim_end_matches('/'));
    let req = LaneRequest {
        method: reqwest::Method::POST,
        url,
        bearer: None,
        team: None,
        json: None,
    };

    let answer = transport.send(req).await?;
    if answer.status == 200 {
        let start: DeviceStart =
            serde_json::from_str(&answer.body).context("decoding device authorization response")?;
        return Ok(start);
    }

    let lane_err = parse_lane_error(&answer);
    bail!("{}", lane_err.message);
}

/// Performs a single poll iteration against `POST {endpoint}/cli/auth/device/poll`.
pub async fn poll_once(
    transport: &impl Transport,
    endpoint: &str,
    device_code: &str,
) -> Result<PollOutcome> {
    let url = format!("{}/cli/auth/device/poll", endpoint.trim_end_matches('/'));
    let req = LaneRequest {
        method: reqwest::Method::POST,
        url,
        bearer: None,
        team: None,
        json: Some(serde_json::json!({ "deviceCode": device_code })),
    };

    let answer = transport.send(req).await?;
    if answer.status == 200 {
        let authn: AuthnResult = serde_json::from_str(&answer.body)
            .context("decoding authorization result from poll")?;
        return Ok(PollOutcome::Granted(authn));
    }

    if answer.status == 202 {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&answer.body)
            && val.get("status").and_then(|v| v.as_str()) == Some("slow_down")
        {
            return Ok(PollOutcome::SlowDown);
        }
        return Ok(PollOutcome::Pending);
    }

    let lane_err = parse_lane_error(&answer);
    if answer.status == 401 {
        return Ok(PollOutcome::Failed(
            "the device code is no longer valid; run telmoni login again".to_string(),
        ));
    }
    if answer.status == 426 {
        return Ok(PollOutcome::Failed(
            "this CLI is too old; upgrade it".to_string(),
        ));
    }
    if answer.status == 429 {
        return Ok(PollOutcome::RateLimited {
            retry_after_secs: lane_err.retry_after_secs,
        });
    }

    Ok(PollOutcome::Failed(lane_err.message))
}

/// Injectable poll loop implementing exact RFC 8628 backoff and error handling.
pub async fn poll_until_granted<P, PF, S, SF, C>(
    interval: Duration,
    expires_in: Duration,
    mut poll: P,
    mut sleep: S,
    mut now: C,
) -> Result<AuthnResult>
where
    P: FnMut() -> PF,
    PF: Future<Output = Result<PollOutcome>>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
    C: FnMut() -> Instant,
{
    let started = now();
    let mut current_interval = interval;
    let mut next_sleep: Option<Duration> = None;

    loop {
        let sleep_dur = next_sleep.take().unwrap_or(current_interval);
        sleep(sleep_dur).await;

        if now().duration_since(started) >= expires_in {
            bail!("the code expired before it was approved; run telmoni login again");
        }

        match poll().await? {
            PollOutcome::Granted(authn) => return Ok(authn),
            PollOutcome::Pending => {}
            PollOutcome::SlowDown => {
                current_interval += Duration::from_secs(5);
            }
            PollOutcome::RateLimited { retry_after_secs } => {
                let dur = retry_after_secs
                    .map(Duration::from_secs)
                    .unwrap_or(current_interval);
                next_sleep = Some(dur);
            }
            PollOutcome::Failed(msg) => bail!("{msg}"),
        }
    }
}

/// Calls `POST {endpoint}/cli/me` to retrieve session identity and teams.
pub async fn fetch_me(
    transport: &impl Transport,
    endpoint: &str,
    access_token: &str,
    team_id: Option<&str>,
) -> Result<Me> {
    let url = format!("{}/cli/me", endpoint.trim_end_matches('/'));
    let req = LaneRequest {
        method: reqwest::Method::POST,
        url,
        bearer: Some(access_token.to_string()),
        team: team_id.map(ToString::to_string),
        json: None,
    };

    let answer = transport.send(req).await?;
    if answer.status == 200 {
        let me: Me = serde_json::from_str(&answer.body).context("decoding /cli/me response")?;
        return Ok(me);
    }

    let lane_err = parse_lane_error(&answer);
    bail!(lane_err)
}

/// Refreshes session tokens via `POST {endpoint}/cli/auth/refresh`.
pub async fn refresh_tokens(
    transport: &impl Transport,
    endpoint: &str,
    refresh_token: &str,
    session_row_id: Option<&str>,
) -> Result<AuthnResult> {
    let url = format!("{}/cli/auth/refresh", endpoint.trim_end_matches('/'));
    let mut body_map = serde_json::Map::new();
    body_map.insert(
        "refreshToken".to_string(),
        serde_json::Value::String(refresh_token.to_string()),
    );
    if let Some(row_id) = session_row_id {
        body_map.insert(
            "sessionRowId".to_string(),
            serde_json::Value::String(row_id.to_string()),
        );
    }

    let req = LaneRequest {
        method: reqwest::Method::POST,
        url,
        bearer: None,
        team: None,
        json: Some(serde_json::Value::Object(body_map)),
    };

    let answer = transport.send(req).await?;
    if answer.status == 200 {
        let authn: AuthnResult = serde_json::from_str(&answer.body)
            .context("decoding refresh response from /cli/auth/refresh")?;
        return Ok(authn);
    }

    let lane_err = parse_lane_error(&answer);
    bail!(lane_err)
}

/// Refreshes access tokens if expired or expiring within 60 seconds.
pub async fn refresh_if_needed(
    transport: &impl Transport,
    store: &CredentialsStore,
    profile: &str,
    creds: &mut Credentials,
) -> Result<()> {
    if creds.auth_type != crate::auth::storage::AuthType::Device {
        return Ok(());
    }

    let now_ts = chrono::Utc::now().timestamp();
    if let Some(exp) = creds.expires_at
        && now_ts < exp - 60
    {
        return Ok(());
    }

    let rt = match creds.refresh_token.as_ref() {
        Some(rt) => rt.clone(),
        None => {
            let _ = store.clear(profile);
            bail!("session ended; run telmoni login");
        }
    };

    let res = refresh_tokens(
        transport,
        &creds.endpoint,
        &rt,
        creds.session_row_id.as_deref(),
    )
    .await;

    match res {
        Ok(authn) => {
            creds.access_token = Some(authn.access_token);
            if let Some(new_rt) = authn.refresh_token {
                creds.refresh_token = Some(new_rt);
            }
            creds.expires_at = Some(chrono::Utc::now().timestamp() + authn.expires_in);
            creds.updated_at = chrono::Utc::now().timestamp();
            store.save(profile, creds)?;
            Ok(())
        }
        Err(err) => {
            if let Some(lane_err) = err.downcast_ref::<LaneError>() {
                if lane_err.status == 401 {
                    let _ = store.clear(profile);
                    bail!("session ended; run telmoni login");
                }
                bail!("{}", lane_err.message);
            }
            Err(err)
        }
    }
}

/// Revokes a CLI session via `POST {endpoint}/cli/sessions/{sessionRowId}/revoke`.
pub async fn revoke_session(
    transport: &impl Transport,
    endpoint: &str,
    access_token: &str,
    session_row_id: &str,
    team_id: Option<&str>,
) -> Result<()> {
    let url = format!(
        "{}/cli/sessions/{}/revoke",
        endpoint.trim_end_matches('/'),
        session_row_id
    );
    let req = LaneRequest {
        method: reqwest::Method::POST,
        url,
        bearer: Some(access_token.to_string()),
        team: team_id.map(ToString::to_string),
        json: None,
    };

    let answer = transport.send(req).await?;
    if answer.status == 204 || answer.status == 404 {
        return Ok(());
    }

    let lane_err = parse_lane_error(&answer);
    bail!(lane_err)
}
