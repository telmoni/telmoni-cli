//! Transport seam and lane error parsing for Telmoni CLI.

use std::future::Future;
use std::time::Duration;

use anyhow::Context;
use tracing::debug;

/// What `Debug` shows in place of a secret.
pub(crate) const REDACTED: &str = "<redacted>";

/// The longest a request may take, from connecting to the last byte of the
/// answer: without one a stalled connection waits forever. Well past the
/// console's own wait on the server (10 s: the `/cli` door's
/// `UPSTREAM_TIMEOUT_MS`, the `/v1` relay's `READ_TIMEOUT_MS`), so its 503
/// problem arrives first.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest connecting may take, within `REQUEST_TIMEOUT`: a host that
/// never answers fails in seconds rather than at the end of the whole budget.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Request sent to a Telmoni lane.
#[derive(Clone)]
pub struct LaneRequest {
    /// HTTP method.
    pub method: reqwest::Method,
    /// Absolute URL.
    pub url: String,
    /// Bearer token for `Authorization: Bearer <token>`.
    pub bearer: Option<String>,
    /// Organization context for `x-organization-id: <org>`.
    pub organization: Option<String>,
    /// JSON payload, if any.
    pub json: Option<serde_json::Value>,
}

/// ⚠ By hand: the bearer is the access token or the API key, and a body
/// carries the refresh token or the device code.
impl std::fmt::Debug for LaneRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Destructured, so a field added later has to be placed here: shown,
        // or redacted.
        let Self {
            method,
            url,
            bearer,
            organization,
            json,
        } = self;
        f.debug_struct("LaneRequest")
            .field("method", method)
            .field("url", url)
            .field("bearer", &bearer.as_ref().map(|_| REDACTED))
            .field("organization", organization)
            .field("json", &json.as_ref().map(|_| REDACTED))
            .finish()
    }
}

/// Raw response from a Telmoni lane.
#[derive(Clone)]
pub struct LaneAnswer {
    /// HTTP status code.
    pub status: u16,
    /// Response Content-Type header value.
    pub content_type: Option<String>,
    /// Raw body string.
    pub body: String,
}

/// ⚠ By hand: the body of a granted poll or a refresh carries the tokens, and
/// the device start's the device code.
impl std::fmt::Debug for LaneAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            status,
            content_type,
            body: _,
        } = self;
        f.debug_struct("LaneAnswer")
            .field("status", status)
            .field("content_type", content_type)
            .field("body", &REDACTED)
            .finish()
    }
}

/// Abstract transport trait allowing deterministic unit testing without a network server.
pub trait Transport {
    /// Sends a lane request and returns the lane answer.
    fn send(&self, req: LaneRequest) -> impl Future<Output = anyhow::Result<LaneAnswer>>;
}

/// Constructs the standardized User-Agent string:
/// `telmoni-cli/<version> (<os>; <arch>)`
pub fn build_user_agent() -> String {
    format!(
        "telmoni-cli/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Standard error representation returned by Telmoni API lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneError {
    /// HTTP status code.
    pub status: u16,
    /// The problem document's `type`, when it was one.
    pub problem_type: Option<String>,
    /// The one-line message to show.
    pub message: String,
    /// Retry-After duration in seconds, when present.
    pub retry_after_secs: Option<u64>,
}

impl std::fmt::Display for LaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LaneError {}

/// Parses non-2xx lane answers into a structured `LaneError`.
pub fn parse_lane_error(answer: &LaneAnswer) -> LaneError {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&answer.body) {
        if let Some(err) = parse_problem_json(&val, answer.status) {
            return err;
        }
        if let Some(err) = parse_simple_error_json(&val, answer.status) {
            return err;
        }
    }

    parse_fallback_body(&answer.body, answer.status)
}

/// Shape 1: `application/problem+json`: `{ "type": string, "title": string, "detail"?: string }`
fn parse_problem_json(val: &serde_json::Value, status: u16) -> Option<LaneError> {
    let ptype = val.get("type").and_then(|v| v.as_str())?;
    let title = val.get("title").and_then(|v| v.as_str())?;

    let detail = val.get("detail").and_then(|v| v.as_str());
    let message = match detail {
        Some(d) if !d.trim().is_empty() => format!("{title}: {d}"),
        _ => title.to_string(),
    };
    let retry_after_secs = val.get("retry_after_secs").and_then(|v| v.as_u64());

    Some(LaneError {
        status,
        problem_type: Some(ptype.to_string()),
        message,
        retry_after_secs,
    })
}

/// Shape 2: `{ "error": string }`
fn parse_simple_error_json(val: &serde_json::Value, status: u16) -> Option<LaneError> {
    let err_msg = val.get("error").and_then(|v| v.as_str())?;
    Some(LaneError {
        status,
        problem_type: None,
        message: err_msg.to_string(),
        retry_after_secs: None,
    })
}

/// Shape 3: Text snippet fallback for unstructured responses.
fn parse_fallback_body(body: &str, status: u16) -> LaneError {
    let snippet: String = body.chars().take(200).collect();
    let trimmed = snippet.trim();
    let message = if trimmed.is_empty() {
        format!("request failed ({status})")
    } else if snippet.starts_with(' ') || snippet.starts_with(':') {
        format!("request failed ({status}){snippet}")
    } else {
        format!("request failed ({status}) {snippet}")
    };

    LaneError {
        status,
        problem_type: None,
        message,
        retry_after_secs: None,
    }
}

/// Whether `host`, as `Url::host_str` spells it, is this machine: `localhost`,
/// a name under it, or a loopback address. An IPv6 address comes in brackets.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => host == "localhost" || host.ends_with(".localhost"),
    }
}

/// ⚠ Plain HTTP reaches only this machine, whatever the request carries.
/// Every lane either sends a credential or is answered one: the device
/// start sends none and is answered the device code. Judged by its own
/// contents, that request would cross in the clear, and `login` would print
/// a code and open a browser before its first poll was refused.
pub fn refuse_cleartext(url: &str) -> anyhow::Result<()> {
    if let Ok(parsed) = reqwest::Url::parse(url)
        && parsed.scheme() == "http"
    {
        let host = parsed.host_str().unwrap_or_default();
        if !is_loopback_host(host) {
            anyhow::bail!(
                "refusing to send credentials over unencrypted HTTP to '{host}'; use HTTPS"
            );
        }
    }
    Ok(())
}

/// ⚠ Whether `url` goes straight to this machine, past any proxy the
/// environment names. reqwest exempts no loopback host from the system's
/// proxy variables, so a proxy would take a plain-HTTP request to this
/// machine, bearer and all, off it in the clear: the one thing
/// `refuse_cleartext` allows plain HTTP for. A remote proxy cannot reach this
/// machine's ports anyway.
pub fn bypasses_proxy(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|parsed| parsed.host_str().is_some_and(is_loopback_host))
}

/// Production implementation of `Transport` backed by `reqwest`.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    /// Every other host, through any proxy the environment names.
    client: reqwest::Client,
    /// This machine, through none (`bypasses_proxy`).
    direct: reqwest::Client,
}

impl ReqwestTransport {
    /// Creates a new `ReqwestTransport` with the standardized user agent.
    pub fn new() -> anyhow::Result<Self> {
        let client = client_builder().build().context("building HTTP client")?;
        let direct = client_builder()
            .no_proxy()
            .build()
            .context("building HTTP client")?;
        Ok(Self { client, direct })
    }
}

/// One builder for both clients, so they cannot drift apart on anything but
/// the proxy.
fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(build_user_agent())
        // No lane answers a redirect, and following one would resend the
        // bearer to any same-host, same-port target whatever its scheme:
        // reqwest strips `Authorization` only when the host or the known
        // default port changes. A 3xx is answered as the error it is.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
}

/// The one line `main` prints for a request that got no answer. The cause at
/// the bottom of the chain is what tells a refused connection, a name that
/// does not resolve and a certificate that does not verify apart.
fn request_failed(url: &str, err: &reqwest::Error) -> anyhow::Error {
    if err.is_timeout() {
        return anyhow::anyhow!("timed out waiting for {url}");
    }
    let mut cause: &dyn std::error::Error = err;
    while let Some(source) = cause.source() {
        cause = source;
    }
    anyhow::anyhow!("request to {url} failed: {cause}")
}

impl Transport for ReqwestTransport {
    async fn send(&self, req: LaneRequest) -> anyhow::Result<LaneAnswer> {
        refuse_cleartext(&req.url)?;
        debug!(
            method = %req.method,
            url = %req.url,
            organization = req.organization.as_deref(),
            "request"
        );

        let client = if bypasses_proxy(&req.url) {
            &self.direct
        } else {
            &self.client
        };
        let mut builder = client.request(req.method, &req.url);
        if let Some(token) = req.bearer {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(org) = req.organization {
            builder = builder.header("x-organization-id", &org);
        }
        if let Some(json_val) = req.json {
            builder = builder.json(&json_val);
        }

        let resp = builder
            .send()
            .await
            .map_err(|err| request_failed(&req.url, &err))?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = resp
            .text()
            .await
            .map_err(|err| request_failed(&req.url, &err))?;
        debug!(status, content_type = content_type.as_deref(), "answer");

        Ok(LaneAnswer {
            status,
            content_type,
            body,
        })
    }
}
