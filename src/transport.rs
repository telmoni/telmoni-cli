//! Transport seam and lane error parsing for Telmoni CLI.

use std::future::Future;

use anyhow::Context;

/// Request sent to a Telmoni lane.
#[derive(Debug, Clone)]
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

/// Raw response from a Telmoni lane.
#[derive(Debug, Clone)]
pub struct LaneAnswer {
    /// HTTP status code.
    pub status: u16,
    /// Response Content-Type header value.
    pub content_type: Option<String>,
    /// Raw body string.
    pub body: String,
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
    if answer.status == 426 {
        return LaneError {
            status: answer.status,
            problem_type: None,
            message: "this CLI is too old; upgrade it".to_string(),
            retry_after_secs: None,
        };
    }

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&answer.body) {
        // Shape 1: application/problem+json: { "type": string, "title": string, "status": number, "detail"?: string }
        if let (Some(ptype), Some(title)) = (
            val.get("type").and_then(|v| v.as_str()),
            val.get("title").and_then(|v| v.as_str()),
        ) {
            let detail = val.get("detail").and_then(|v| v.as_str());
            let message = match detail {
                Some(d) if !d.trim().is_empty() => format!("{title}: {d}"),
                _ => title.to_string(),
            };
            let retry_after_secs = val.get("retry_after_secs").and_then(|v| v.as_u64());
            return LaneError {
                status: answer.status,
                problem_type: Some(ptype.to_string()),
                message,
                retry_after_secs,
            };
        }

        // Shape 2: { "error": string }
        if let Some(err_msg) = val.get("error").and_then(|v| v.as_str()) {
            return LaneError {
                status: answer.status,
                problem_type: None,
                message: err_msg.to_string(),
                retry_after_secs: None,
            };
        }
    }

    // Shape 3: Anything else: message is `request failed (<status>)` plus the first 200 characters of the body.
    let snippet: String = answer.body.chars().take(200).collect();
    let trimmed = snippet.trim();
    let message = if trimmed.is_empty() {
        format!("request failed ({})", answer.status)
    } else if snippet.starts_with(' ') || snippet.starts_with(':') {
        format!("request failed ({}){}", answer.status, snippet)
    } else {
        format!("request failed ({}) {}", answer.status, snippet)
    };

    LaneError {
        status: answer.status,
        problem_type: None,
        message,
        retry_after_secs: None,
    }
}

/// Production implementation of `Transport` backed by `reqwest`.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// Creates a new `ReqwestTransport` with the standardized user agent.
    pub fn new() -> anyhow::Result<Self> {
        let ua = build_user_agent();
        let client = reqwest::Client::builder()
            .user_agent(ua)
            .build()
            .context("building HTTP client")?;
        Ok(Self { client })
    }
}

impl Transport for ReqwestTransport {
    async fn send(&self, req: LaneRequest) -> anyhow::Result<LaneAnswer> {
        let mut builder = self.client.request(req.method, &req.url);
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
            .with_context(|| format!("transport send failed for {}", req.url))?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = resp.text().await.context("reading response body")?;

        Ok(LaneAnswer {
            status,
            content_type,
            body,
        })
    }
}
