//! RFC 8628 Device Authorization Grant implementation for Telmoni CLI.

use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::auth::storage::{Credentials, CredentialsStore};
use crate::transport::{LaneAnswer, LaneError, LaneRequest, REDACTED, Transport, parse_lane_error};

/// Response from `POST /cli/auth/device`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    /// Secret device code to poll with.
    pub device_code: String,
    /// User-facing short code, consonants only (e.g. "BCDF-GHJK").
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

/// ⚠ By hand: the device code is the secret the grant is polled with. The
/// user code is shown anyway.
impl std::fmt::Debug for DeviceStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Destructured, so a field added later has to be placed here: shown,
        // or redacted.
        let Self {
            device_code: _,
            user_code,
            verification_uri,
            verification_uri_complete,
            expires_in,
            interval,
        } = self;
        f.debug_struct("DeviceStart")
            .field("device_code", &REDACTED)
            .field("user_code", user_code)
            .field("verification_uri", verification_uri)
            .field("verification_uri_complete", verification_uri_complete)
            .field("expires_in", expires_in)
            .field("interval", interval)
            .finish()
    }
}

/// Authentication result returned on successful authorization or refresh.
#[derive(Clone, PartialEq, Eq, Deserialize)]
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
    /// Access token: an opaque secret the platform minted, not a JWT.
    pub access_token: String,
    /// Refresh token, rotated on every grant.
    pub refresh_token: Option<String>,
    /// Access token validity in seconds.
    pub expires_in: i64,
    /// Authentication method used.
    pub auth_method: Option<String>,
}

/// ⚠ By hand: a derived `Debug` would print both tokens.
impl std::fmt::Debug for AuthnResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Destructured, so a field added later has to be placed here: shown,
        // or redacted.
        let Self {
            user_id,
            email,
            email_verified,
            first_name,
            last_name,
            access_token: _,
            refresh_token,
            expires_in,
            auth_method,
        } = self;
        f.debug_struct("AuthnResult")
            .field("user_id", user_id)
            .field("email", email)
            .field("email_verified", email_verified)
            .field("first_name", first_name)
            .field("last_name", last_name)
            .field("access_token", &REDACTED)
            .field("refresh_token", &refresh_token.as_ref().map(|_| REDACTED))
            .field("expires_in", expires_in)
            .field("auth_method", auth_method)
            .finish()
    }
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

/// Organization membership details returned by `/cli/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    /// Organization ID (`org_...`).
    pub organization_id: String,
    /// The slug the console's paths name it by (`/{slug}`): set once at birth,
    /// to the URL its founder chose or else derived from the name it is born
    /// with (a placeholder, `org-` and ten random characters, when that name
    /// gives none), then moved only by a change to its URL on Settings, never
    /// by a rename. Only the id names the organization.
    pub slug: String,
    /// Organization name, never empty: a provisioned one is born named after
    /// its holder ("Ada's organization", else "My organization"), and one
    /// created from the console's switcher under the name its founder gave.
    pub name: String,
    /// Owner email address.
    pub owner_email: Option<String>,
    /// Owner display name.
    pub owner_display_name: Option<String>,
    /// User's role (`owner`, `admin`, `member`).
    pub role: String,
}

impl Organization {
    /// The organization's label: its name, as the console shows it. Never the
    /// owner's address, which `/cli/me` carries beside it: an address is a
    /// person's.
    pub fn label(&self) -> &str {
        self.name.trim()
    }
}

/// Response returned by `POST /cli/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    /// Person identity.
    pub person: Person,
    /// Organizations the person belongs to.
    #[serde(default)]
    pub organizations: Vec<Organization>,
    /// The organization this answer acts in: the one the request named, while
    /// the person belongs to it, else their default.
    #[serde(default)]
    pub active_organization_id: Option<String>,
    /// The person's default organization: the one they chose in Account
    /// Settings, else the oldest they own, else the oldest they belong to.
    /// `None` exactly when `active_organization_id` is.
    #[serde(default)]
    pub default_organization_id: Option<String>,
    /// Session row ID for Active Sessions tracking.
    pub session_row_id: Option<String>,
    /// True only on the answer that provisioned an organization for them: not
    /// on a first sign-in while sign-ups are closed, and true again once their
    /// last organization is gone and a fresh one is made.
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
        organization: None,
        json: None,
    };

    let answer = transport.send(req).await?;
    if answer.status == 200 {
        let start: DeviceStart =
            serde_json::from_str(&answer.body).context("decoding device authorization response")?;
        debug!(
            expires_in = start.expires_in,
            interval = start.interval,
            "device code issued"
        );
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
        organization: None,
        json: Some(serde_json::json!({ "deviceCode": device_code })),
    };

    let answer = transport.send(req).await?;
    parse_poll_answer(&answer)
}

/// Decodes the HTTP status code and response payload of a device poll answer.
fn parse_poll_answer(answer: &LaneAnswer) -> Result<PollOutcome> {
    match answer.status {
        200 => {
            let authn: AuthnResult = serde_json::from_str(&answer.body)
                .context("decoding authorization result from poll")?;
            Ok(PollOutcome::Granted(authn))
        }
        202 => {
            let is_slow_down = serde_json::from_str::<serde_json::Value>(&answer.body)
                .ok()
                .and_then(|v| {
                    v.get("status")
                        .and_then(|s| s.as_str())
                        .map(|s| s == "slow_down")
                })
                .unwrap_or(false);
            if is_slow_down {
                Ok(PollOutcome::SlowDown)
            } else {
                Ok(PollOutcome::Pending)
            }
        }
        401 if parse_lane_error(answer).problem_type.as_deref() == Some(INVALID_TOKEN) => {
            Ok(PollOutcome::Failed(
                "the device code is no longer valid; run telmoni login again".to_string(),
            ))
        }
        429 => {
            let lane_err = parse_lane_error(answer);
            Ok(PollOutcome::RateLimited {
                retry_after_secs: lane_err.retry_after_secs,
            })
        }
        _ => {
            let lane_err = parse_lane_error(answer);
            Ok(PollOutcome::Failed(lane_err.message))
        }
    }
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
            PollOutcome::Granted(authn) => {
                debug!("approved");
                return Ok(authn);
            }
            PollOutcome::Pending => debug!("not approved yet"),
            PollOutcome::SlowDown => {
                current_interval += Duration::from_secs(5);
                debug!(
                    interval_secs = current_interval.as_secs(),
                    "asked to slow down"
                );
            }
            PollOutcome::RateLimited { retry_after_secs } => {
                let dur = retry_after_secs.map_or(current_interval, |s| {
                    Duration::from_secs(s).max(current_interval)
                });
                debug!(wait_secs = dur.as_secs(), "rate limited");
                next_sleep = Some(dur);
            }
            PollOutcome::Failed(msg) => bail!("{msg}"),
        }
    }
}

/// Calls `POST {endpoint}/cli/me` to retrieve session identity and organizations.
pub async fn fetch_me(
    transport: &impl Transport,
    endpoint: &str,
    access_token: &str,
    organization_id: Option<&str>,
) -> Result<Me> {
    let url = format!("{}/cli/me", endpoint.trim_end_matches('/'));
    let req = LaneRequest {
        method: reqwest::Method::POST,
        url,
        bearer: Some(access_token.to_string()),
        organization: organization_id.map(ToString::to_string),
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
        organization: None,
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

/// A bearer that expired. The platform keeps its row a while.
const TOKEN_EXPIRED: &str = "/errors/auth/token-expired";

/// A token the platform does not know. On a bearer lane that includes an
/// expired bearer once the retention sweep has deleted its row, so it is not
/// proof the session ended; on the refresh lane the refresh token is gone,
/// and the session with it.
const INVALID_TOKEN: &str = "/errors/auth/invalid-token";

/// A revoked session, or an account being deleted.
const UNAUTHENTICATED: &str = "/errors/auth/unauthenticated";

/// The session is over: the platform said so, or nothing is left to renew it
/// with. The credentials file is gone by the time this is returned.
#[derive(Debug)]
pub struct SessionEnded;

impl std::fmt::Display for SessionEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("session ended; run telmoni login")
    }
}

impl std::error::Error for SessionEnded {}

/// The problem type of a 401 that speaks about the caller's own credentials.
/// ⚠ Any other 401 ends nothing: it is not auth judging this session (the
/// door answers its own hop failing as a 503, and a 401 of another shape
/// comes from whatever sits in front of the platform), and taken as the end
/// it would sign out every CLI that met it.
pub(crate) fn refused_credentials(err: &anyhow::Error) -> Option<&'static str> {
    let lane_err = err
        .downcast_ref::<LaneError>()
        .filter(|e| e.status == 401)?;
    [TOKEN_EXPIRED, INVALID_TOKEN, UNAUTHENTICATED]
        .into_iter()
        .find(|t| lane_err.problem_type.as_deref() == Some(*t))
}

/// Whether a bearer lane's refusal is one a refresh may cure. The bearer
/// lives minutes, and a clock that runs slow here skips the early refresh.
pub(crate) fn bearer_expired(err: &anyhow::Error) -> bool {
    matches!(
        refused_credentials(err),
        Some(TOKEN_EXPIRED | INVALID_TOKEN)
    )
}

/// Refreshes access tokens if expired or expiring within 60 seconds.
pub async fn refresh_if_needed(
    transport: &impl Transport,
    store: &CredentialsStore,
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

    debug!("refreshing: the access token expires within 60 seconds, or its expiry is unknown");
    refresh_session(transport, store, creds).await
}

/// Refreshes the session's tokens and saves them: the platform rotates the
/// refresh token on every use, so the new one has to reach the file.
pub(crate) async fn refresh_session(
    transport: &impl Transport,
    store: &CredentialsStore,
    creds: &mut Credentials,
) -> Result<()> {
    let Some(rt) = creds.refresh_token.clone() else {
        debug!("no refresh token held");
        return Err(session_ended(store));
    };
    let row_id = creds.session_row_id.as_deref();
    let mut answer = refresh_tokens(transport, &creds.endpoint, &rt, row_id).await;
    // ⚠ The platform may have spent the token without its answer arriving:
    // the door gives up on auth after ten seconds and answers a 503 of its
    // own. A spent token presented again within the platform's grace (thirty
    // seconds) earns the next pair; presented later, by the next command, it
    // ends the session. So the refresh is asked for again at once.
    if answer.as_ref().is_err_and(unanswered) {
        debug!("the refresh got no answer to read; asking again within the platform's grace");
        answer = refresh_tokens(transport, &creds.endpoint, &rt, row_id).await;
    }
    let authn = answer.map_err(|err| ended_or_surfaced(store, err))?;
    creds.apply_refresh(&authn);
    store.save(creds)
}

/// A request the platform may have acted on without the CLI reading its
/// answer: a 5xx, or no answer it could read.
fn unanswered(err: &anyhow::Error) -> bool {
    err.downcast_ref::<LaneError>()
        .is_none_or(|lane_err| lane_err.status >= 500)
}

/// `POST /cli/me` for the stored session, held to the platform's 401 rules: a
/// bearer refused as expired or unknown gets one refresh, which decides
/// whether the session lives, and one retry; a refusal that speaks about the
/// session means it was ended elsewhere, so the credentials file goes.
/// Nothing else deletes it: a 5xx or a dropped connection says nothing about
/// whether the session is still live.
pub async fn fetch_me_for_session(
    transport: &impl Transport,
    store: &CredentialsStore,
    creds: &mut Credentials,
    organization_id: Option<&str>,
) -> Result<Me> {
    let access_token = creds.access_token.clone().context("missing access token")?;
    let err = match fetch_me(transport, &creds.endpoint, &access_token, organization_id).await {
        Ok(me) => return Ok(me),
        Err(err) => err,
    };
    if !bearer_expired(&err) {
        return Err(ended_or_surfaced(store, err));
    }

    debug!(
        problem_type = refused_credentials(&err),
        "the bearer was refused; refreshing once"
    );
    refresh_session(transport, store, creds).await?;
    let fresh = creds.access_token.clone().context("missing access token")?;
    fetch_me(transport, &creds.endpoint, &fresh, organization_id)
        .await
        .map_err(|err| ended_or_surfaced(store, err))
}

/// A refusal that speaks about the caller's credentials ends the session, and
/// the credentials file goes. Any other failure leaves the file alone and
/// surfaces the server's message.
fn ended_or_surfaced(store: &CredentialsStore, err: anyhow::Error) -> anyhow::Error {
    if let Some(problem_type) = refused_credentials(&err) {
        debug!(problem_type, "the session has ended");
        return session_ended(store);
    }
    match err.downcast_ref::<LaneError>() {
        Some(lane_err) => anyhow::anyhow!("{}", lane_err.message),
        None => err,
    }
}

fn session_ended(store: &CredentialsStore) -> anyhow::Error {
    if let Err(err) = store.clear() {
        tracing::warn!(error = %err, "the ended session's credentials file stays; remove it yourself");
    }
    anyhow::Error::new(SessionEnded)
}

/// Revokes a CLI session via `POST {endpoint}/cli/sessions/{sessionRowId}/revoke`.
pub async fn revoke_session(
    transport: &impl Transport,
    endpoint: &str,
    access_token: &str,
    session_row_id: &str,
    organization_id: Option<&str>,
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
        organization: organization_id.map(ToString::to_string),
        json: None,
    };

    let answer = transport.send(req).await?;
    if answer.status == 204 {
        return Ok(());
    }

    // ⚠ A 404 is "already gone" only as auth types it. The door's own 404,
    // for a path no lane matches (a session id that is not a UUID), leaves
    // the session as live as it was.
    let lane_err = parse_lane_error(&answer);
    if answer.status == 404 && lane_err.problem_type.as_deref() == Some("/errors/auth/not-found") {
        return Ok(());
    }
    bail!(lane_err)
}
