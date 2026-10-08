//! Comprehensive unit tests verifying device authorization grant, transport seam,
//! credentials store, error parser, and organization switching logic.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use telmoni_cli::auth::device::{
    AuthnResult, Organization, PollOutcome, poll_once, poll_until_granted, refresh_if_needed,
};
use telmoni_cli::auth::storage::{
    AuthType, Credentials, CredentialsStore, StoredOrganization, StoredPerson,
};
use telmoni_cli::commands::{logout, org};
use telmoni_cli::transport::{
    LaneAnswer, LaneRequest, Transport, build_user_agent, parse_lane_error,
};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_store() -> CredentialsStore {
    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("telmoni-test-{}-{}", std::process::id(), n));
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    CredentialsStore::new(path)
}

#[derive(Default, Clone)]
struct MockTransport {
    answers: Arc<Mutex<VecDeque<anyhow::Result<LaneAnswer>>>>,
    pub requests: Arc<Mutex<Vec<LaneRequest>>>,
}

impl MockTransport {
    fn new() -> Self {
        Self::default()
    }

    fn push_answer(&self, status: u16, body: impl Into<String>) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.push_back(Ok(LaneAnswer {
                status,
                content_type: Some("application/json".to_string()),
                body: body.into(),
            }));
        }
    }

    fn push_error(&self, err: anyhow::Error) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.push_back(Err(err));
        }
    }
}

impl Transport for MockTransport {
    async fn send(&self, req: LaneRequest) -> anyhow::Result<LaneAnswer> {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(req);
        }
        let next = self
            .answers
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {e}"))?
            .pop_front();
        match next {
            Some(res) => res,
            None => anyhow::bail!("unexpected request to MockTransport"),
        }
    }
}

// 1. User-Agent string shape
#[test]
fn test_user_agent_shape() {
    let ua = build_user_agent();
    assert!(
        ua.starts_with("telmoni-cli/"),
        "User-Agent should start with telmoni-cli/: {ua}"
    );
    assert!(
        ua.contains(&format!("telmoni-cli/{}", env!("CARGO_PKG_VERSION"))),
        "User-Agent should contain pkg version: {ua}"
    );
    assert!(
        ua.contains(std::env::consts::OS),
        "User-Agent should contain OS: {ua}"
    );
    assert!(
        ua.contains(std::env::consts::ARCH),
        "User-Agent should contain ARCH: {ua}"
    );
}

// 2. Credentials round-trip through CredentialsStore with a temp path:
//    both auth types, nulls preserved as None, and an unparseable file reading as not signed in
#[test]
fn test_credentials_round_trip() {
    let store = temp_store();

    // Not signed in initially
    assert!(store.load().unwrap().is_none());

    // Device credentials with some nulls
    let device_creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("token_123".to_string()),
        refresh_token: None,
        expires_at: Some(1790000000),
        session_row_id: None,
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
            slug: "acme-corp".to_string(),
            label: "Acme Corp".to_string(),
            role: "owner".to_string(),
        }],
        active_organization_id: None,
        api_key: None,
        updated_at: 1790000000,
    };

    store.save(&device_creds).unwrap();
    let loaded = store.load().unwrap().expect("should load credentials");
    assert_eq!(loaded, device_creds);
    assert!(loaded.refresh_token.is_none());
    assert!(loaded.session_row_id.is_none());
    assert!(loaded.active_organization_id.is_none());
    assert!(loaded.api_key.is_none());

    // API Key credentials
    let api_creds = Credentials {
        auth_type: AuthType::ApiKey,
        endpoint: "https://custom.telmoni.com".to_string(),
        access_token: None,
        refresh_token: None,
        expires_at: None,
        session_row_id: None,
        person: None,
        organizations: Vec::new(),
        active_organization_id: None,
        api_key: Some("telmoni_secret_key".to_string()),
        updated_at: 1790000050,
    };

    store.save(&api_creds).unwrap();
    let loaded_api = store.load().unwrap().expect("should load api credentials");
    assert_eq!(loaded_api, api_creds);

    // Unparseable file reads as not signed in
    std::fs::write(&store.path, "corrupted { invalid json").unwrap();
    assert!(store.load().unwrap().is_none());

    // Device file missing access token reads as not signed in
    std::fs::write(
        &store.path,
        r#"{"auth_type":"device","endpoint":"https://telmoni.com","updated_at":100}"#,
    )
    .unwrap();
    assert!(store.load().unwrap().is_none());

    // Clear deletes the file
    store.save(&api_creds).unwrap();
    assert!(store.path.exists());
    store.clear().unwrap();
    assert!(!store.path.exists());

    // Clear says so when the file cannot be deleted: a directory in its place
    std::fs::create_dir(&store.path).unwrap();
    assert!(store.clear().is_err());
    std::fs::remove_dir(&store.path).unwrap();
}

// 3. The LaneError parser: a problem with and without detail, a 429 with retry_after_secs,
//    { "error": … }, and a plain-text body
#[test]
fn test_lane_error_parser() {
    // Problem with detail
    let answer1 = LaneAnswer {
        status: 400,
        content_type: Some("application/problem+json".to_string()),
        body: r#"{"type":"/errors/invalid-code","title":"invalid code","status":400,"detail":"the code has expired"}"#.to_string(),
    };
    let err1 = parse_lane_error(&answer1);
    assert_eq!(err1.status, 400);
    assert_eq!(err1.problem_type.as_deref(), Some("/errors/invalid-code"));
    assert_eq!(err1.message, "invalid code: the code has expired");
    assert_eq!(err1.retry_after_secs, None);

    // Problem without detail
    let answer2 = LaneAnswer {
        status: 401,
        content_type: Some("application/problem+json".to_string()),
        body: r#"{"type":"/errors/auth/token-expired","title":"token expired","status":401}"#
            .to_string(),
    };
    let err2 = parse_lane_error(&answer2);
    assert_eq!(err2.status, 401);
    assert_eq!(
        err2.problem_type.as_deref(),
        Some("/errors/auth/token-expired")
    );
    assert_eq!(err2.message, "token expired");

    // 429 with retry_after_secs
    let answer3 = LaneAnswer {
        status: 429,
        content_type: Some("application/problem+json".to_string()),
        body: r#"{"type":"/errors/tenant/rate-limited","title":"rate limited","status":429,"detail":"retry after 45s","retry_after_secs":45}"#.to_string(),
    };
    let err3 = parse_lane_error(&answer3);
    assert_eq!(err3.status, 429);
    assert_eq!(err3.retry_after_secs, Some(45));
    assert_eq!(err3.message, "rate limited: retry after 45s");

    // { "error": ... }
    let answer4 = LaneAnswer {
        status: 503,
        content_type: Some("application/json".to_string()),
        body: r#"{"error":"upstream unavailable"}"#.to_string(),
    };
    let err4 = parse_lane_error(&answer4);
    assert_eq!(err4.status, 503);
    assert_eq!(err4.message, "upstream unavailable");

    // Plain-text body
    let answer5 = LaneAnswer {
        status: 502,
        content_type: Some("text/plain".to_string()),
        body: "bad gateway from proxy".to_string(),
    };
    let err5 = parse_lane_error(&answer5);
    assert_eq!(err5.status, 502);
    assert_eq!(err5.message, "request failed (502) bad gateway from proxy");

    // 426, as the platform defines it: its detail names both versions, so it
    // reads like any other problem rather than as a message of the CLI's own
    let answer6 = LaneAnswer {
        status: 426,
        content_type: Some("application/problem+json".to_string()),
        body: r#"{"type":"/errors/incompatible-client","title":"incompatible client","status":426,"detail":"Telmoni CLI 0.0.1 is older than the minimum supported 0.1.0; upgrade to continue"}"#.to_string(),
    };
    let err6 = parse_lane_error(&answer6);
    assert_eq!(err6.status, 426);
    assert_eq!(
        err6.message,
        "incompatible client: Telmoni CLI 0.0.1 is older than the minimum supported 0.1.0; upgrade to continue"
    );
}

// 4. poll_once: 200, each 202, 429, 401, 403
#[tokio::test]
async fn test_poll_once_outcomes() {
    let transport = MockTransport::new();

    // 200 Granted
    transport.push_answer(
        200,
        r#"{
            "userId": "usr_1",
            "accessToken": "tok_xyz",
            "expiresIn": 3600
        }"#,
    );
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert!(
        matches!(outcome, PollOutcome::Granted(_)),
        "expected Granted"
    );
    if let PollOutcome::Granted(authn) = outcome {
        assert_eq!(authn.user_id, "usr_1");
        assert_eq!(authn.access_token, "tok_xyz");
    }

    // 202 Pending
    transport.push_answer(202, r#"{"status":"authorization_pending"}"#);
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert_eq!(outcome, PollOutcome::Pending);

    // 202 SlowDown
    transport.push_answer(202, r#"{"status":"slow_down"}"#);
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert_eq!(outcome, PollOutcome::SlowDown);

    // 429 RateLimited
    transport.push_answer(
        429,
        r#"{"type":"/errors/tenant/rate-limited","title":"rate limited","status":429,"retry_after_secs":12}"#,
    );
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PollOutcome::RateLimited {
            retry_after_secs: Some(12)
        }
    );

    // 401 Dead device code, as auth answers one it does not know
    transport.push_answer(
        401,
        r#"{"type":"/errors/auth/invalid-token","title":"invalid token","status":401}"#,
    );
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PollOutcome::Failed(
            "the device code is no longer valid; run telmoni login again".to_string()
        )
    );

    // 403 Authorization denied, as auth answers it: a problem document, read
    // as `title: detail`
    transport.push_answer(
        403,
        r#"{"type":"/errors/authz/forbidden","title":"forbidden","status":403,"detail":"the sign-in was denied from the console"}"#,
    );
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PollOutcome::Failed("forbidden: the sign-in was denied from the console".to_string())
    );

    // 400 Expired, as auth answers a code approved too late
    transport.push_answer(
        400,
        r#"{"type":"/errors/auth/bad-request","title":"bad request","status":400,"detail":"the device code expired before it was approved; start again"}"#,
    );
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PollOutcome::Failed(
            "bad request: the device code expired before it was approved; start again".to_string()
        )
    );
}

// 5. poll_until_granted: pending then granted; two slow_downs (+5s each);
//    a 429 sleeping for retry_after_secs; a 403 ending the loop;
//    local expiry ending the loop without an extra poll;
//    each test asserts the recorded sequence of sleeps
#[tokio::test]
async fn test_poll_until_granted_scenarios() {
    let dummy_authn = AuthnResult {
        user_id: "usr_1".to_string(),
        email: Some("usr@example.com".to_string()),
        email_verified: true,
        first_name: None,
        last_name: None,
        access_token: "at_1".to_string(),
        refresh_token: None,
        expires_in: 3600,
        auth_method: None,
    };

    // Scenario 1: pending then granted
    {
        let outcomes = Arc::new(Mutex::new(VecDeque::from([
            PollOutcome::Pending,
            PollOutcome::Granted(dummy_authn.clone()),
        ])));
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let s_clone = sleeps.clone();
        let o_clone = outcomes.clone();

        let authn = poll_until_granted(
            Duration::from_secs(5),
            Duration::from_secs(60),
            move || {
                let o = o_clone.clone();
                async move { Ok(o.lock().unwrap().pop_front().unwrap()) }
            },
            |dur| {
                let s = s_clone.clone();
                async move {
                    s.lock().unwrap().push(dur);
                }
            },
            Instant::now,
        )
        .await
        .unwrap();

        assert_eq!(authn.access_token, "at_1");
        assert_eq!(
            *sleeps.lock().unwrap(),
            vec![Duration::from_secs(5), Duration::from_secs(5)]
        );
    }

    // Scenario 2: two slow_downs (+5s each)
    {
        let outcomes = Arc::new(Mutex::new(VecDeque::from([
            PollOutcome::SlowDown,
            PollOutcome::SlowDown,
            PollOutcome::Granted(dummy_authn.clone()),
        ])));
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let s_clone = sleeps.clone();
        let o_clone = outcomes.clone();

        let _ = poll_until_granted(
            Duration::from_secs(5),
            Duration::from_secs(120),
            move || {
                let o = o_clone.clone();
                async move { Ok(o.lock().unwrap().pop_front().unwrap()) }
            },
            |dur| {
                let s = s_clone.clone();
                async move {
                    s.lock().unwrap().push(dur);
                }
            },
            Instant::now,
        )
        .await
        .unwrap();

        assert_eq!(
            *sleeps.lock().unwrap(),
            vec![
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(15)
            ]
        );
    }

    // Scenario 3: a 429 sleeping for retry_after_secs
    {
        let outcomes = Arc::new(Mutex::new(VecDeque::from([
            PollOutcome::RateLimited {
                retry_after_secs: Some(30),
            },
            PollOutcome::Granted(dummy_authn.clone()),
        ])));
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let s_clone = sleeps.clone();
        let o_clone = outcomes.clone();

        let _ = poll_until_granted(
            Duration::from_secs(5),
            Duration::from_secs(120),
            move || {
                let o = o_clone.clone();
                async move { Ok(o.lock().unwrap().pop_front().unwrap()) }
            },
            |dur| {
                let s = s_clone.clone();
                async move {
                    s.lock().unwrap().push(dur);
                }
            },
            Instant::now,
        )
        .await
        .unwrap();

        assert_eq!(
            *sleeps.lock().unwrap(),
            vec![Duration::from_secs(5), Duration::from_secs(30)]
        );
    }

    // Scenario 3b: a 429 asking for less than the interval still waits the
    // interval, which is the least the server allows between polls
    {
        let outcomes = Arc::new(Mutex::new(VecDeque::from([
            PollOutcome::RateLimited {
                retry_after_secs: Some(2),
            },
            PollOutcome::Granted(dummy_authn.clone()),
        ])));
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let s_clone = sleeps.clone();
        let o_clone = outcomes.clone();

        let _ = poll_until_granted(
            Duration::from_secs(5),
            Duration::from_secs(120),
            move || {
                let o = o_clone.clone();
                async move { Ok(o.lock().unwrap().pop_front().unwrap()) }
            },
            |dur| {
                let s = s_clone.clone();
                async move {
                    s.lock().unwrap().push(dur);
                }
            },
            Instant::now,
        )
        .await
        .unwrap();

        assert_eq!(
            *sleeps.lock().unwrap(),
            vec![Duration::from_secs(5), Duration::from_secs(5)]
        );
    }

    // Scenario 4: a 403 ending the loop
    {
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let s_clone = sleeps.clone();

        let err = poll_until_granted(
            Duration::from_secs(5),
            Duration::from_secs(60),
            || async { Ok(PollOutcome::Failed("access denied".to_string())) },
            |dur| {
                let s = s_clone.clone();
                async move {
                    s.lock().unwrap().push(dur);
                }
            },
            Instant::now,
        )
        .await
        .unwrap_err();

        assert_eq!(err.to_string(), "access denied");
        assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_secs(5)]);
    }

    // Scenario 5: local expiry ending the loop without an extra poll
    {
        let poll_count = Arc::new(Mutex::new(0));
        let poll_count_clone = poll_count.clone();
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let s_clone = sleeps.clone();
        let sim_now = Arc::new(Mutex::new(Instant::now()));
        let sim_now_clone = sim_now.clone();
        let sim_now_poll = sim_now.clone();

        let err = poll_until_granted(
            Duration::from_secs(5),
            Duration::from_secs(10),
            move || {
                let pc = poll_count_clone.clone();
                async move {
                    *pc.lock().unwrap() += 1;
                    Ok(PollOutcome::Pending)
                }
            },
            |dur| {
                let s = s_clone.clone();
                let clock = sim_now_clone.clone();
                async move {
                    s.lock().unwrap().push(dur);
                    let mut clk = clock.lock().unwrap();
                    *clk += dur;
                }
            },
            move || *sim_now_poll.lock().unwrap(),
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("the code expired before it was approved")
        );
        assert_eq!(
            *poll_count.lock().unwrap(),
            1,
            "expiry must stop before second poll"
        );
        assert_eq!(
            *sleeps.lock().unwrap(),
            vec![Duration::from_secs(5), Duration::from_secs(5)]
        );
    }
}

// 6. refresh_if_needed: 401 clearing the file; 200 saving the new tokens;
//    a null refreshToken in the 200 leaving none: the one presented is spent
#[tokio::test]
async fn test_refresh_if_needed() {
    let store = temp_store();
    let transport = MockTransport::new();

    // 200 saving new tokens
    let mut creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("old_at".to_string()),
        refresh_token: Some("old_rt".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() - 10), // expired
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "bob@example.com".to_string(),
            display_name: None,
        }),
        organizations: Vec::new(),
        active_organization_id: None,
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    transport.push_answer(
        200,
        r#"{
            "userId": "usr_1",
            "accessToken": "new_at",
            "refreshToken": "new_rt",
            "expiresIn": 3600
        }"#,
    );

    refresh_if_needed(&transport, &store, &mut creds)
        .await
        .unwrap();
    assert_eq!(creds.access_token.as_deref(), Some("new_at"));
    assert_eq!(creds.refresh_token.as_deref(), Some("new_rt"));

    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.access_token.as_deref(), Some("new_at"));
    assert_eq!(loaded.refresh_token.as_deref(), Some("new_rt"));

    // null refreshToken: the grant spent the one presented, and keeping it
    // would present a spent token next time, which after a short grace ends
    // the whole session
    creds.expires_at = Some(chrono::Utc::now().timestamp() - 10);
    store.save(&creds).unwrap();

    transport.push_answer(
        200,
        r#"{
            "userId": "usr_1",
            "accessToken": "new_at_2",
            "refreshToken": null,
            "expiresIn": 3600
        }"#,
    );

    refresh_if_needed(&transport, &store, &mut creds)
        .await
        .unwrap();
    assert_eq!(creds.access_token.as_deref(), Some("new_at_2"));
    assert_eq!(creds.refresh_token, None);
    assert_eq!(store.load().unwrap().unwrap().refresh_token, None);

    // 401 clearing the file
    creds.refresh_token = Some("rt_again".to_string());
    creds.expires_at = Some(chrono::Utc::now().timestamp() - 10);
    store.save(&creds).unwrap();

    transport.push_answer(
        401,
        r#"{"type":"/errors/auth/unauthenticated","title":"unauthenticated","status":401}"#,
    );

    let err = refresh_if_needed(&transport, &store, &mut creds)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("session ended"));
    assert!(
        store.load().unwrap().is_none(),
        "store should be cleared after 401"
    );
    // One refresh request per case so far: the 401 was answered, not skipped.
    assert_eq!(transport.requests.lock().unwrap().len(), 3);

    // no refresh token held — what a null-token grant leaves behind: nothing
    // to present, so no request, and the file goes as it does on a 401
    creds.refresh_token = None;
    creds.expires_at = Some(chrono::Utc::now().timestamp() - 10);
    store.save(&creds).unwrap();

    let err = refresh_if_needed(&transport, &store, &mut creds)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("session ended"));
    assert!(store.load().unwrap().is_none());
    assert_eq!(transport.requests.lock().unwrap().len(), 3);
}

// 6b. A refresh that got no answer to read, a 5xx or none at all, is asked for
//     again at once with the same token: the platform may have spent it, and
//     takes it again only within a short grace. One it answered is not.
#[tokio::test]
async fn test_refresh_asked_again_when_unanswered() {
    let store = temp_store();
    let transport = MockTransport::new();
    let expiring = || {
        let mut creds = device_creds(&["org_1"], "org_1");
        creds.expires_at = Some(chrono::Utc::now().timestamp() - 10);
        creds
    };
    let take = || std::mem::take(&mut *transport.requests.lock().unwrap());

    // a 503, then the pair
    let mut creds = expiring();
    store.save(&creds).unwrap();
    transport.push_answer(503, OWN_SIDE_FAILED);
    transport.push_answer(200, FRESH_TOKENS);
    refresh_if_needed(&transport, &store, &mut creds)
        .await
        .unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().refresh_token.as_deref(),
        Some("rt2")
    );
    let reqs = take();
    assert_eq!(reqs.len(), 2);
    assert_eq!(
        reqs[0].json, reqs[1].json,
        "the same token, presented again"
    );

    // no answer at all, then the pair
    let mut creds = expiring();
    store.save(&creds).unwrap();
    transport.push_error(anyhow::anyhow!(
        "timed out waiting for https://telmoni.com/cli/auth/refresh"
    ));
    transport.push_answer(200, FRESH_TOKENS);
    refresh_if_needed(&transport, &store, &mut creds)
        .await
        .unwrap();
    assert_eq!(creds.access_token.as_deref(), Some("fresh"));
    assert_eq!(take().len(), 2);

    // a refusal is an answer, and is not asked for again
    let mut creds = expiring();
    store.save(&creds).unwrap();
    transport.push_answer(
        429,
        r#"{"type":"/errors/tenant/rate-limited","title":"rate limited","status":429,"detail":"retry after 3s","retry_after_secs":3}"#,
    );
    let err = refresh_if_needed(&transport, &store, &mut creds)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "rate limited: retry after 3s");
    assert!(store.path.exists());
    assert_eq!(take().len(), 1);
}

// 7. organization label: its name, as the console shows it, whether its owner
//    renamed it or it still has the one it was born with — never the owner's
//    address, which /cli/me carries beside it
#[test]
fn test_organization_label() {
    let renamed = Organization {
        organization_id: "org_1".to_string(),
        slug: "acme-corp".to_string(),
        name: "Acme Corp".to_string(),
        owner_email: Some("owner@example.com".to_string()),
        owner_display_name: Some("Owner".to_string()),
        role: "owner".to_string(),
    };
    assert_eq!(renamed.label(), "Acme Corp");

    let as_born = Organization {
        organization_id: "org_2".to_string(),
        slug: "adas-organization".to_string(),
        name: "Ada's organization".to_string(),
        owner_email: Some("ada@example.com".to_string()),
        owner_display_name: Some("Ada Lovelace".to_string()),
        role: "member".to_string(),
    };
    assert_eq!(as_born.label(), "Ada's organization");
}

// 7b. the same label under an API key: `/v1/organization` carries the owner's
//     address beside the name, and it labels nothing
#[test]
fn test_v1_organization_label_never_uses_the_owners_address() {
    use telmoni_cli::client::{V1Organization, V1Owner};
    let named = V1Organization {
        organization_id: "org_1".to_string(),
        slug: "acme".to_string(),
        name: "  Acme Corp ".to_string(),
        owner: Some(V1Owner {
            email: "owner@example.com".to_string(),
            display_name: None,
        }),
    };
    assert_eq!(named.label(), "Acme Corp");
}

// 8. org switch: unknown organization rejected without network; active switch verified
//    against /me answer and sent as x-organization-id; mismatch bailing with
//    you are no longer in <id>
#[tokio::test]
async fn test_org_switch_logic() {
    let store = temp_store();
    let transport = MockTransport::new();

    let creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("token_123".to_string()),
        refresh_token: Some("rt_123".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![
            StoredOrganization {
                organization_id: "org_1".to_string(),
                slug: "org-one".to_string(),
                label: "Org One".to_string(),
                role: "owner".to_string(),
            },
            StoredOrganization {
                organization_id: "org_2".to_string(),
                slug: "org-two".to_string(),
                label: "Org Two".to_string(),
                role: "member".to_string(),
            },
        ],
        active_organization_id: Some("org_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    // 8a. Unknown organization rejected without network
    let err = org::execute(
        org::OrgCommand::Switch {
            organization: "org_unknown".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unknown organization org_unknown"));
    assert_eq!(
        transport.requests.lock().unwrap().len(),
        0,
        "no network on unknown organization"
    );

    // 8b. Active switch verified against /me answer
    transport.push_answer(
        200,
        r#"{
            "person": {
                "userId": "usr_1",
                "email": "alice@example.com"
            },
            "organizations": [
                {
                    "organizationId": "org_1",
                    "slug": "org-one",
                    "name": "Org One",
                    "role": "owner"
                },
                {
                    "organizationId": "org_2",
                    "slug": "org-two",
                    "name": "Org Two",
                    "role": "member"
                }
            ],
            "activeOrganizationId": "org_2",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );

    org::execute(
        org::OrgCommand::Switch {
            organization: "org_2".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap();

    let updated = store.load().unwrap().unwrap();
    assert_eq!(updated.active_organization_id.as_deref(), Some("org_2"));
    {
        let reqs = transport.requests.lock().unwrap();
        assert_eq!(reqs[0].url, "https://telmoni.com/cli/me");
        assert_eq!(reqs[0].organization.as_deref(), Some("org_2"));
    }

    // 8c. Mismatch bailing with you are no longer in <id>
    transport.push_answer(
        200,
        r#"{
            "person": {
                "userId": "usr_1",
                "email": "alice@example.com"
            },
            "organizations": [
                {
                    "organizationId": "org_1",
                    "slug": "org-one",
                    "name": "Org One",
                    "role": "owner"
                }
            ],
            "activeOrganizationId": "org_1",
            "defaultOrganizationId": "org_1",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );

    let mismatch_err = org::execute(
        org::OrgCommand::Switch {
            organization: "org_2".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap_err();

    assert_eq!(mismatch_err.to_string(), "you are no longer in org_2");
    // The answer's list is kept, so `org list` stops showing the organization
    // the person left; and since that was the active one, it gives way to
    // their default.
    let kept = store.load().unwrap().unwrap();
    assert_eq!(kept.organizations.len(), 1, "the fresh list is kept");
    assert_eq!(kept.organizations[0].organization_id, "org_1");
    assert_eq!(kept.active_organization_id.as_deref(), Some("org_1"));
}

// 9. logout: deletes the file on a 401 at refresh, and on a 404 at revoke
#[tokio::test]
async fn test_logout_outcomes() {
    let store = temp_store();
    let transport = MockTransport::new();

    // 9a. Deletes file on 401 at refresh
    let creds_refresh_401 = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("at_1".to_string()),
        refresh_token: Some("rt_1".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() - 10), // expired, needs refresh
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "bob@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
            slug: "acme".to_string(),
            label: "Acme".to_string(),
            role: "owner".to_string(),
        }],
        active_organization_id: Some("org_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds_refresh_401).unwrap();
    assert!(store.path.exists());

    // Refresh answers 401
    transport.push_answer(
        401,
        r#"{"type":"/errors/auth/unauthenticated","title":"unauthenticated","status":401}"#,
    );

    logout::execute(logout::LogoutArgs {}, &transport, &store, None)
        .await
        .unwrap();

    assert!(
        !store.path.exists(),
        "file must be deleted on 401 at refresh"
    );

    // 9b. Deletes file on 404 at revoke
    let creds_revoke_404 = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("at_valid".to_string()),
        refresh_token: Some("rt_valid".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600), // valid, no refresh
        session_row_id: Some("0192a3b4-2222-7000-8000-000000000002".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "bob@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
            slug: "acme".to_string(),
            label: "Acme".to_string(),
            role: "owner".to_string(),
        }],
        active_organization_id: Some("org_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds_revoke_404).unwrap();
    assert!(store.path.exists());

    // Revoke answers 404, as auth does for a session already gone
    transport.push_answer(
        404,
        r#"{"type":"/errors/auth/not-found","title":"not found","status":404,"detail":"session not found"}"#,
    );

    logout::execute(logout::LogoutArgs {}, &transport, &store, None)
        .await
        .unwrap();

    assert!(
        !store.path.exists(),
        "file must be deleted on 404 at revoke"
    );
}

// 9b. Endpoint precedence: flag, then TELMONI_ENDPOINT as main read it,
//     then the config file, then the default; blanks are skipped and a
//     trailing slash is dropped.
#[test]
fn test_endpoint_precedence() {
    use telmoni_cli::config::{Config, resolve_endpoint};

    let config = Config {
        endpoint: Some("https://from-config.example/".to_string()),
        output_format: None,
    };
    assert_eq!(
        resolve_endpoint(
            Some("https://flag.example/"),
            Some("https://env.example"),
            &config
        ),
        "https://flag.example"
    );
    assert_eq!(
        resolve_endpoint(Some("  "), Some("https://env.example"), &config),
        "https://env.example"
    );
    assert_eq!(
        resolve_endpoint(None, None, &config),
        "https://from-config.example"
    );
    assert_eq!(
        resolve_endpoint(None, Some(""), &Config::default()),
        "https://telmoni.com"
    );
    assert_eq!(
        resolve_endpoint(Some("api.example.com/"), None, &Config::default()),
        "https://api.example.com"
    );
    assert_eq!(
        resolve_endpoint(Some("localhost:3000/"), None, &Config::default()),
        "http://localhost:3000"
    );
    assert_eq!(
        resolve_endpoint(Some("127.0.0.1:8080"), None, &Config::default()),
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        resolve_endpoint(Some("[::1]:3000"), None, &Config::default()),
        "http://[::1]:3000"
    );
    // A bare host gets plain HTTP only when it is this machine, not when its
    // name merely starts like one that is.
    assert_eq!(
        resolve_endpoint(Some("localhost.example.com"), None, &Config::default()),
        "https://localhost.example.com"
    );
    assert_eq!(
        resolve_endpoint(Some("127.0.0.1.example.com"), None, &Config::default()),
        "https://127.0.0.1.example.com"
    );
    assert_eq!(
        resolve_endpoint(Some(" / "), Some("https://env.example"), &Config::default()),
        "https://env.example"
    );
}

// 10. API key validation
#[test]
fn test_api_key_validation() {
    use telmoni_cli::commands::login::validate_api_key;

    assert!(validate_api_key("telmoni_abc123_xyz").is_ok());
    assert!(validate_api_key("sk_live_123").is_err());
    assert!(validate_api_key("telmoni_abc 123").is_err());
    assert!(validate_api_key("telmoni_abc\t123").is_err());
    assert!(validate_api_key("telmoni_abc\n123").is_err());
    assert!(validate_api_key("telmoni_").is_err());
}

// 11. API key login and status
#[tokio::test]
async fn test_api_key_login_and_status() {
    use telmoni_cli::commands::{login, status};
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let config = Config::default();

    // Login with API key - no network call, no browser, no waiting
    login::execute(
        login::LoginArgs {
            key: Some("telmoni_test_api_key".to_string()),
            endpoint: Some("https://telmoni.com".to_string()),
            no_browser: false,
        },
        &transport,
        &store,
        &config,
        None,
        |_: &str| -> std::io::Result<()> { panic!("an API key login opens no browser") },
        |_| async { panic!("an API key login does not poll") },
    )
    .await
    .unwrap();

    assert_eq!(transport.requests.lock().unwrap().len(), 0);

    let creds = store.load().unwrap().unwrap();
    assert_eq!(creds.auth_type, AuthType::ApiKey);
    assert_eq!(creds.api_key.as_deref(), Some("telmoni_test_api_key"));

    // Status calls /v1/organization
    transport.push_answer(
        200,
        r#"{
            "organization_id": "org_api_1",
            "slug": "api-org",
            "name": "API Org",
            "owner": { "email": "owner@api.com", "display_name": "API Owner" }
        }"#,
    );

    status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://telmoni.com/v1/organization");
    assert_eq!(reqs[0].bearer.as_deref(), Some("telmoni_test_api_key"));
}

// 12. TELMONI_ORG env validation
#[tokio::test]
async fn test_telmoni_org_env_validation() {
    use telmoni_cli::commands::{logout, status};
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let config = Config::default();

    let creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("token_valid".to_string()),
        refresh_token: Some("rt_valid".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_allowed".to_string(),
            slug: "allowed-org".to_string(),
            label: "Allowed Org".to_string(),
            role: "owner".to_string(),
        }],
        active_organization_id: Some("org_allowed".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    // Invalid TELMONI_ORG on status fails before network
    let status_err = status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &config,
        Some("org_forbidden".to_string()),
        None,
    )
    .await
    .unwrap_err();
    // The cache may simply be stale — a URL change moved the slug — so the
    // message blames the cache, not the person's membership.
    assert_eq!(
        status_err.to_string(),
        "TELMONI_ORG names no organization in the cached list; run telmoni status without it \
         to refresh the list"
    );

    // Invalid TELMONI_ORG on logout fails before network and preserves file
    let logout_err = logout::execute(
        logout::LogoutArgs {},
        &transport,
        &store,
        Some("org_forbidden".to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(
        logout_err.to_string(),
        "TELMONI_ORG names no organization in the cached list; run telmoni status without it \
         to refresh the list"
    );
    assert!(store.path.exists());
    assert_eq!(transport.requests.lock().unwrap().len(), 0);
}

// 13. Status updates cached credentials from /cli/me answer
#[tokio::test]
async fn test_status_updates_cached_credentials() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let config = Config::default();

    let creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("token_original".to_string()),
        refresh_token: Some("rt_original".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
            slug: "org-old".to_string(),
            label: "Org Old".to_string(),
            role: "member".to_string(),
        }],
        active_organization_id: Some("org_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    // Server answers with updated display name and newly added organization
    transport.push_answer(
        200,
        r#"{
            "person": {
                "userId": "usr_1",
                "email": "alice@example.com",
                "displayName": "Alice Smith"
            },
            "organizations": [
                {
                    "organizationId": "org_1",
                    "slug": "acme-corp",
                    "name": "Acme Corp",
                    "role": "admin"
                },
                {
                    "organizationId": "org_2",
                    "slug": "beta-labs",
                    "name": "Beta Labs",
                    "role": "owner"
                }
            ],
            "activeOrganizationId": "org_2",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );

    status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    let updated = store.load().unwrap().unwrap();
    assert_eq!(
        updated.person.unwrap().display_name.as_deref(),
        Some("Alice Smith")
    );
    assert_eq!(updated.organizations.len(), 2);
    assert_eq!(updated.organizations[0].label, "Acme Corp");
    // A URL change moves the slug; the id stays.
    assert_eq!(updated.organizations[0].organization_id, "org_1");
    assert_eq!(updated.organizations[0].slug, "acme-corp");
    assert_eq!(updated.organizations[0].role, "admin");
    assert_eq!(updated.organizations[1].label, "Beta Labs");
    assert_eq!(updated.active_organization_id.as_deref(), Some("org_2"));
}

// 13b. Status on an organization as it was born: `/cli/me` answers it named
//      after its holder, at the slug that name reads as. The label is that
//      name, never the owner's address, and the slug is kept as answered.
#[tokio::test]
async fn test_status_labels_an_organization_as_born() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let config = Config::default();

    let creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("token_original".to_string()),
        refresh_token: Some("rt_original".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: Vec::new(),
        active_organization_id: None,
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "alice@example.com", "displayName": "Alice Smith" },
            "organizations": [
                {
                    "organizationId": "org_1",
                    "slug": "alices-organization",
                    "name": "Alice's organization",
                    "ownerEmail": "alice@example.com",
                    "role": "owner"
                }
            ],
            "activeOrganizationId": "org_1",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );

    status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    let updated = store.load().unwrap().unwrap();
    assert_eq!(updated.organizations.len(), 1);
    assert_eq!(updated.organizations[0].label, "Alice's organization");
    assert_eq!(updated.organizations[0].slug, "alices-organization");
    assert_eq!(updated.active_organization_id.as_deref(), Some("org_1"));
}

// 13c. Status for somebody in no organization — sign-ups closed, or their last
//      one gone: nothing to label and nothing active, and the command succeeds
#[tokio::test]
async fn test_status_in_no_organization() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let config = Config::default();

    let creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("token_original".to_string()),
        refresh_token: Some("rt_original".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: Vec::new(),
        active_organization_id: None,
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "alice@example.com" },
            "organizations": [],
            "activeOrganizationId": null,
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );

    status::execute(
        status::StatusArgs { json: true },
        &transport,
        &store,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    let updated = store.load().unwrap().unwrap();
    assert!(updated.organizations.is_empty());
    assert_eq!(updated.active_organization_id, None);
}

// 14. Status retries on token expired
#[tokio::test]
async fn test_status_retries_on_token_expired() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let config = Config::default();

    let creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("stale_token".to_string()),
        refresh_token: Some("valid_refresh".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
            slug: "org-one".to_string(),
            label: "Org One".to_string(),
            role: "owner".to_string(),
        }],
        active_organization_id: Some("org_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    // 1. First /cli/me call fails with 401 token-expired
    transport.push_answer(
        401,
        r#"{"type":"/errors/auth/token-expired","title":"token expired","status":401}"#,
    );

    // 2. Refresh call succeeds
    transport.push_answer(
        200,
        r#"{
            "userId": "usr_1",
            "accessToken": "fresh_access_token",
            "refreshToken": "new_refresh_token",
            "expiresIn": 1800
        }"#,
    );

    // 3. Retry /cli/me succeeds with fresh token
    transport.push_answer(
        200,
        r#"{
            "person": {
                "userId": "usr_1",
                "email": "alice@example.com"
            },
            "organizations": [
                {
                    "organizationId": "org_1",
                    "slug": "org-one",
                    "name": "Org One",
                    "role": "owner"
                }
            ],
            "activeOrganizationId": "org_1",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );

    status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &config,
        None,
        None,
    )
    .await
    .unwrap();

    let updated = store.load().unwrap().unwrap();
    assert_eq!(updated.access_token.as_deref(), Some("fresh_access_token"));
    assert_eq!(updated.refresh_token.as_deref(), Some("new_refresh_token"));
    assert!(store.path.exists());
}

// 15. Organization wire shape: /cli/me as the platform answers it (extra fields
//     ignored), and an API key reading /v1/organization
#[tokio::test]
async fn test_organization_wire_shape() {
    use telmoni_cli::auth::device::Me;
    use telmoni_cli::client::fetch_v1_organization;

    // A. Deserializing Me with "organizations", "organizationId" and "activeOrganizationId"
    let json_me = r#"{
        "person": {
            "userId": "usr_org_1",
            "email": "user@org.test",
            "displayName": null,
            "analyticsOptIn": false,
            "emailVerified": true
        },
        "organizations": [
            {
                "organizationId": "org_alpha",
                "slug": "org-alpha",
                "name": "Org Alpha",
                "ownerEmail": "user@org.test",
                "ownerDisplayName": null,
                "role": "owner",
                "ownershipOfferExpiresAt": null
            }
        ],
        "activeOrganizationId": "org_alpha",
        "defaultOrganizationId": "org_alpha",
        "memberships": [],
        "incomingInvites": [],
        "projectOffers": [],
        "flags": {
            "api_tokens": true,
            "connectors": true,
            "members": true,
            "public_api": true,
            "signup": true
        },
        "firstLogin": false,
        "sessionRowId": "0192a3b4-0000-7000-8000-000000000001"
    }"#;

    let me: Me = serde_json::from_str(json_me).unwrap();
    assert_eq!(me.person.user_id, "usr_org_1");
    assert_eq!(me.organizations.len(), 1);
    assert_eq!(me.organizations[0].organization_id, "org_alpha");
    assert_eq!(me.organizations[0].slug, "org-alpha");
    assert_eq!(me.organizations[0].label(), "Org Alpha");
    assert_eq!(me.active_organization_id.as_deref(), Some("org_alpha"));
    assert_eq!(me.default_organization_id.as_deref(), Some("org_alpha"));

    // B. fetch_v1_organization reads /v1/organization
    let transport = MockTransport::new();
    transport.push_answer(
        200,
        r#"{
        "organization_id": "org_public",
        "slug": "public-org",
        "name": "Public Org",
        "owner": {
            "email": "lead@org.test",
            "display_name": "Org Lead"
        }
    }"#,
    );

    let org = fetch_v1_organization(&transport, "https://telmoni.com", "telmoni_api_token")
        .await
        .unwrap();
    assert_eq!(org.organization_id, "org_public");
    assert_eq!(org.slug, "public-org");
    assert_eq!(org.label(), "Public Org");

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://telmoni.com/v1/organization");
}

/// The slug a fixture organization goes by: its id, as a slug would spell it.
fn slug_of(id: &str) -> String {
    id.replace('_', "-")
}

fn device_creds(orgs: &[&str], active: &str) -> Credentials {
    Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("at".to_string()),
        refresh_token: Some("rt".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: orgs
            .iter()
            .map(|id| StoredOrganization {
                organization_id: (*id).to_string(),
                slug: slug_of(id),
                label: (*id).to_string(),
                role: "member".to_string(),
            })
            .collect(),
        active_organization_id: Some(active.to_string()),
        api_key: None,
        updated_at: 100,
    }
}

const TOKEN_EXPIRED: &str =
    r#"{"type":"/errors/auth/token-expired","title":"token expired","status":401}"#;

/// What auth answers for a token it does not know, an expired bearer the
/// retention sweep has deleted among them.
const INVALID_TOKEN: &str =
    r#"{"type":"/errors/auth/invalid-token","title":"invalid token","status":401}"#;

/// What auth answers for a revoked session.
const UNAUTHENTICATED: &str =
    r#"{"type":"/errors/auth/unauthenticated","title":"unauthenticated","status":401}"#;

/// What the door answers when auth refuses the console's own service secret:
/// the platform failing on its own side, which says nothing about the session.
const OWN_SIDE_FAILED: &str = r#"{"type":"/errors/upstream-unavailable","title":"upstream unavailable","status":503,"detail":"the platform failed on its own side; try again shortly"}"#;

/// A 401 that is not auth's, with no problem type: what something in front of
/// the platform might answer.
const FOREIGN_401: &str = r#"{"error":"unauthorized"}"#;

/// A refresh as the platform grants one.
const FRESH_TOKENS: &str =
    r#"{"userId":"usr_1","accessToken":"fresh","refreshToken":"rt2","expiresIn":900}"#;

/// `/cli/me` for `device_creds(&["org_1"], "org_1")`'s person.
const ME_IN_ORG_1: &str = r#"{
    "person": { "userId": "usr_1", "email": "alice@example.com" },
    "organizations": [ { "organizationId": "org_1", "slug": "org-1", "name": "Org 1", "role": "member" } ],
    "activeOrganizationId": "org_1",
    "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
}"#;

// 16. A server failure while curing an expired token never deletes credentials
#[tokio::test]
async fn test_status_keeps_credentials_on_refresh_server_error() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    store.save(&device_creds(&["org_1"], "org_1")).unwrap();

    transport.push_answer(401, TOKEN_EXPIRED);
    // The door's own answer when the server does not: a problem document. The
    // refresh is asked for twice, since the first may have spent the token.
    let unavailable = r#"{"type":"/errors/upstream-unavailable","title":"upstream unavailable","status":503,"detail":"the server did not answer; try again shortly"}"#;
    transport.push_answer(503, unavailable);
    transport.push_answer(503, unavailable);

    let err = status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &Config::default(),
        None,
        None,
    )
    .await
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        "upstream unavailable: the server did not answer; try again shortly"
    );
    assert!(store.path.exists(), "a 503 must never delete credentials");
    assert_eq!(transport.requests.lock().unwrap().len(), 3);
}

// 17. org switch obeys the session's 401 rules: ended deletes, expired is cured
#[tokio::test]
async fn test_org_switch_session_401s() {
    let store = temp_store();
    let transport = MockTransport::new();
    store
        .save(&device_creds(&["org_1", "org_2"], "org_1"))
        .unwrap();

    transport.push_answer(
        401,
        r#"{"type":"/errors/auth/unauthenticated","title":"unauthenticated","status":401}"#,
    );
    let err = org::execute(
        org::OrgCommand::Switch {
            organization: "org_2".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "session ended; run telmoni login");
    assert!(!store.path.exists(), "an ended session deletes credentials");

    store
        .save(&device_creds(&["org_1", "org_2"], "org_1"))
        .unwrap();
    transport.push_answer(401, TOKEN_EXPIRED);
    transport.push_answer(
        200,
        r#"{"userId":"usr_1","accessToken":"fresh","refreshToken":"rt2","expiresIn":1800}"#,
    );
    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "alice@example.com" },
            "organizations": [
                { "organizationId": "org_1", "slug": "one", "name": "One", "role": "owner" },
                { "organizationId": "org_2", "slug": "two", "name": "Two", "role": "member" }
            ],
            "activeOrganizationId": "org_2",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );
    org::execute(
        org::OrgCommand::Switch {
            organization: "org_2".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap();

    let updated = store.load().unwrap().unwrap();
    assert_eq!(updated.access_token.as_deref(), Some("fresh"));
    assert_eq!(updated.active_organization_id.as_deref(), Some("org_2"));
}

// 18. TELMONI_ORG on status is held to the server's answer, not the cache
#[tokio::test]
async fn test_status_telmoni_org_left_since_cached() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    store
        .save(&device_creds(&["org_1", "org_2"], "org_1"))
        .unwrap();

    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "alice@example.com" },
            "organizations": [ { "organizationId": "org_1", "slug": "one", "name": "One", "role": "owner" } ],
            "activeOrganizationId": "org_1",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );

    let err = status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &Config::default(),
        Some("org_2".to_string()),
        None,
    )
    .await
    .unwrap_err();

    assert_eq!(err.to_string(), "you are no longer in org_2");
    let updated = store.load().unwrap().unwrap();
    assert_eq!(updated.organizations.len(), 1, "the fresh list is kept");
}

// 18b. An active organization the person has left gives way to their default,
//      never to the one a command named: `TELMONI_ORG` acts for one command,
//      and a switch that is refused moves nothing to its target
#[tokio::test]
async fn test_active_organization_left_gives_way_to_the_default() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    // As cached: org_1 at /org-1, org_2 at /org-2, org_3 at /org-3, org_3
    // active. Since then the person left org_3 and joined org_4, which they
    // chose as their default, and org_2 took the slug org_1's URL change left.
    let stale = device_creds(&["org_1", "org_2", "org_3"], "org_3");
    let me_acting_in = |active: &str| {
        format!(
            r#"{{
                "person": {{ "userId": "usr_1", "email": "alice@example.com" }},
                "organizations": [
                    {{ "organizationId": "org_1", "slug": "one-labs", "name": "One Labs", "role": "member" }},
                    {{ "organizationId": "org_2", "slug": "org-1", "name": "Org 1", "role": "member" }},
                    {{ "organizationId": "org_4", "slug": "org-4", "name": "Org 4", "role": "member" }}
                ],
                "activeOrganizationId": "{active}",
                "defaultOrganizationId": "org_4",
                "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
            }}"#
        )
    };
    let cached = || store.load().unwrap().unwrap();

    // status under TELMONI_ORG reports on org_2, and leaves it inactive
    store.save(&stale).unwrap();
    transport.push_answer(200, me_acting_in("org_2"));
    status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &Config::default(),
        Some("org_2".to_string()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(cached().active_organization_id.as_deref(), Some("org_4"));

    // a switch refused because the slug moved leaves org_1 inactive
    store.save(&stale).unwrap();
    transport.push_answer(200, me_acting_in("org_1"));
    let err = org::execute(
        org::OrgCommand::Switch {
            organization: "org-1".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "org-1 now names another organization; run telmoni org list"
    );
    assert_eq!(cached().active_organization_id.as_deref(), Some("org_4"));
}

// 19. logout, when the cached organization is refused: the revoke goes again
//     naming none, which the platform takes from somebody in no organization,
//     and only when they are in one does `/me` say which to name. Asked first,
//     `/me` would make an organization for somebody in none.
#[tokio::test]
async fn test_logout_retries_revoke_with_current_organization() {
    let store = temp_store();
    let transport = MockTransport::new();
    let forbidden = r#"{"type":"/errors/authz/forbidden","title":"forbidden","status":403}"#;

    // 19a. in another organization since
    store
        .save(&device_creds(&["org_gone"], "org_gone"))
        .unwrap();
    transport.push_answer(403, forbidden);
    transport.push_answer(
        400,
        r#"{"type":"/errors/auth/bad-request","title":"bad request","status":400,"detail":"missing x-organization-id header: name the organization this is recorded on"}"#,
    );
    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "alice@example.com" },
            "organizations": [ { "organizationId": "org_now", "slug": "now", "name": "Now", "role": "member" } ],
            "activeOrganizationId": "org_now",
            "defaultOrganizationId": "org_now",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );
    transport.push_answer(204, "");

    logout::execute(logout::LogoutArgs {}, &transport, &store, None)
        .await
        .unwrap();

    let reqs = std::mem::take(&mut *transport.requests.lock().unwrap());
    assert_eq!(reqs.len(), 4);
    assert_eq!(reqs[0].organization.as_deref(), Some("org_gone"));
    assert_eq!(reqs[1].organization, None);
    assert_eq!(reqs[2].url, "https://telmoni.com/cli/me");
    assert_eq!(reqs[2].organization, None);
    assert_eq!(reqs[3].organization.as_deref(), Some("org_now"));
    assert!(!store.path.exists());

    // 19b. in no organization since: the revoke naming none ends it, and
    //      `/me` is never asked
    store
        .save(&device_creds(&["org_gone"], "org_gone"))
        .unwrap();
    transport.push_answer(403, forbidden);
    transport.push_answer(204, "");

    logout::execute(logout::LogoutArgs {}, &transport, &store, None)
        .await
        .unwrap();

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(reqs.iter().all(|r| r.url.ends_with("/revoke")));
    assert_eq!(reqs[1].organization, None);
    assert!(!store.path.exists());
}

// 20. The credentials file is private from creation: a directory made for it
//     is 0700, an older 0644 file comes out 0600, and no temporary is left
#[cfg(unix)]
#[test]
fn test_credentials_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("telmoni-perm-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    let store = CredentialsStore::new(dir.join("telmoni").join("credentials.json"));
    let creds = device_creds(&["org_1"], "org_1");
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

    store.save(&creds).unwrap();
    assert_eq!(mode(&store.path), 0o600);
    assert_eq!(mode(store.path.parent().unwrap()), 0o700);

    std::fs::set_permissions(&store.path, std::fs::Permissions::from_mode(0o644)).unwrap();
    store.save(&creds).unwrap();
    assert_eq!(
        mode(&store.path),
        0o600,
        "an older, looser file is replaced"
    );
    assert_eq!(store.load().unwrap(), Some(creds));

    let leftovers: Vec<_> = std::fs::read_dir(store.path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        vec![std::ffi::OsString::from("credentials.json")]
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn test_reqwest_transport_refuses_cleartext_http_credentials() {
    use telmoni_cli::transport::{LaneRequest, ReqwestTransport, Transport};

    let transport = ReqwestTransport::new().unwrap();
    let req = LaneRequest {
        method: reqwest::Method::GET,
        url: "http://remote-insecure.example.com/cli/me".to_string(),
        bearer: Some("secret-token-123".to_string()),
        organization: None,
        json: None,
    };

    let err = transport.send(req).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("refusing to send credentials over unencrypted HTTP"),
        "expected cleartext refusal, got: {err}"
    );
}

#[tokio::test]
async fn test_reqwest_transport_refuses_cleartext_http_tokens_in_json() {
    use telmoni_cli::transport::{LaneRequest, ReqwestTransport, Transport};

    let transport = ReqwestTransport::new().unwrap();
    let req = LaneRequest {
        method: reqwest::Method::POST,
        url: "http://remote-insecure.example.com/cli/auth/refresh".to_string(),
        bearer: None,
        organization: None,
        json: Some(serde_json::json!({ "refreshToken": "rt_secret" })),
    };

    let err = transport.send(req).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("refusing to send credentials over unencrypted HTTP"),
        "expected cleartext refusal for refresh token in json, got: {err}"
    );
}

// The device start sends no credential and is answered one, the device code.
// Refused like the rest, a login stops before it prints a code or opens a
// browser, not at its first poll.
#[tokio::test]
async fn test_reqwest_transport_refuses_cleartext_http_device_start() {
    use telmoni_cli::transport::{LaneRequest, ReqwestTransport, Transport};

    let transport = ReqwestTransport::new().unwrap();
    let req = LaneRequest {
        method: reqwest::Method::POST,
        url: "http://remote-insecure.example.com/cli/auth/device".to_string(),
        bearer: None,
        organization: None,
        json: None,
    };

    let err = transport.send(req).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("refusing to send credentials over unencrypted HTTP"),
        "expected cleartext refusal for the device start, got: {err}"
    );
}

// Plain HTTP reaches this machine and nothing else. Which hosts are this
// machine is read from the parsed URL: an IPv6 address comes in brackets, and
// a name that starts with `localhost` need not be it.
#[test]
fn test_cleartext_http_only_to_this_machine() {
    use telmoni_cli::transport::refuse_cleartext;

    for allowed in [
        "https://telmoni.com/cli/me",
        "http://localhost:3000/cli/me",
        "http://app.localhost:3000/cli/me",
        "http://127.0.0.1:3000/cli/me",
        "http://[::1]:3000/cli/me",
    ] {
        assert!(refuse_cleartext(allowed).is_ok(), "{allowed}");
    }
    for refused in [
        "http://telmoni.com/cli/me",
        "http://localhost.example.com/cli/me",
        "http://127.0.0.1.example.com/cli/me",
        "http://192.168.1.10:3000/cli/me",
    ] {
        assert!(refuse_cleartext(refused).is_err(), "{refused}");
    }
}

// 21. A label is not a name: `org switch` takes an id or a slug, and a label
//     is unknown even when one organization alone carries it. Two called
//     Acme, one at /acme: "Acme" is refused before any request, "acme" is
//     the slug and names the one at /acme, and the id names either.
#[tokio::test]
async fn test_org_switch_takes_no_label() {
    use telmoni_cli::auth::storage::{AuthType, Credentials, StoredOrganization, StoredPerson};
    use telmoni_cli::commands::org;

    let store = temp_store();
    let transport = MockTransport::new();

    let creds = Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("token_1".to_string()),
        refresh_token: Some("rt_1".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111-7000-8000-000000000001".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "bob@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![
            StoredOrganization {
                organization_id: "org_alpha".to_string(),
                slug: "acme".to_string(),
                label: "Acme".to_string(),
                role: "owner".to_string(),
            },
            StoredOrganization {
                organization_id: "org_beta".to_string(),
                slug: "acme-2".to_string(),
                label: "Acme".to_string(),
                role: "member".to_string(),
            },
        ],
        active_organization_id: Some("org_alpha".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    let err = org::execute(
        org::OrgCommand::Switch {
            organization: "Acme".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown organization Acme: not in the cached list; run telmoni status to refresh it, \
         then telmoni org list"
    );
    assert_eq!(transport.requests.lock().unwrap().len(), 0);

    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "bob@example.com" },
            "organizations": [
                { "organizationId": "org_alpha", "slug": "acme", "name": "Acme", "role": "owner" },
                { "organizationId": "org_beta", "slug": "acme-2", "name": "Acme", "role": "member" }
            ],
            "activeOrganizationId": "org_beta"
        }"#,
    );

    org::execute(
        org::OrgCommand::Switch {
            organization: "org_beta".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap();
    let updated = store.load().unwrap().unwrap();
    assert_eq!(updated.active_organization_id.as_deref(), Some("org_beta"));

    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "bob@example.com" },
            "organizations": [
                { "organizationId": "org_alpha", "slug": "acme", "name": "Acme", "role": "owner" },
                { "organizationId": "org_beta", "slug": "acme-2", "name": "Acme", "role": "member" }
            ],
            "activeOrganizationId": "org_alpha"
        }"#,
    );
    org::execute(
        org::OrgCommand::Switch {
            organization: "acme".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap();
    let updated = store.load().unwrap().unwrap();
    assert_eq!(updated.active_organization_id.as_deref(), Some("org_alpha"));
}

// 22. A slug names an organization wherever an id does: `org switch`, and
//     TELMONI_ORG on status and logout. It is what the console's URL shows;
//     the wire still carries the id.
#[tokio::test]
async fn test_organization_named_by_slug() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    // The helper's organizations go by `org-1` and `org-2`.
    store
        .save(&device_creds(&["org_1", "org_2"], "org_1"))
        .unwrap();

    let me_acting_in = |active: &str| {
        format!(
            r#"{{
                "person": {{ "userId": "usr_1", "email": "alice@example.com" }},
                "organizations": [
                    {{ "organizationId": "org_1", "slug": "org-1", "name": "One", "role": "owner" }},
                    {{ "organizationId": "org_2", "slug": "org-2", "name": "Two", "role": "member" }}
                ],
                "activeOrganizationId": "{active}",
                "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
            }}"#
        )
    };

    // 22a. org switch by slug
    transport.push_answer(200, me_acting_in("org_2"));
    org::execute(
        org::OrgCommand::Switch {
            organization: "org-2".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap();
    assert_eq!(
        store
            .load()
            .unwrap()
            .unwrap()
            .active_organization_id
            .as_deref(),
        Some("org_2")
    );

    // 22b. status under a slug acts in it for the one command, and keeps the
    //      stored active organization
    transport.push_answer(200, me_acting_in("org_1"));
    status::execute(
        status::StatusArgs { json: true },
        &transport,
        &store,
        &Config::default(),
        Some("org-1".to_string()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        store
            .load()
            .unwrap()
            .unwrap()
            .active_organization_id
            .as_deref(),
        Some("org_2")
    );

    // 22c. the server's answer is held to the id the slug named, not to the
    //      slug's text: `/me` fell back, so the person has left it since
    transport.push_answer(200, me_acting_in("org_2"));
    let left = status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &Config::default(),
        Some("org-1".to_string()),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(left.to_string(), "you are no longer in org-1");

    // 22d. logout under a slug names its id on the revoke
    transport.push_answer(204, "");
    logout::execute(
        logout::LogoutArgs {},
        &transport,
        &store,
        Some("org-1".to_string()),
    )
    .await
    .unwrap();

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    for (request, organization) in reqs.iter().zip(["org_2", "org_1", "org_1", "org_1"]) {
        assert_eq!(request.organization.as_deref(), Some(organization));
    }
    assert!(reqs[3].url.ends_with("/revoke"));
    assert!(!store.path.exists());
}

// 23. A URL change moves a slug, and another organization may take it since. The
//     cache's say-so on a name is held to the answer, like its say-so on
//     membership: a name that has moved is refused, the fresh list is kept,
//     and the next run reads the name as the console does.
#[tokio::test]
async fn test_slug_moved_since_cached() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    // As cached: org_1 at /org-1, org_2 at /org-2, org_3 at /org-3.
    let stale = device_creds(&["org_1", "org_2", "org_3"], "org_3");

    // Since then org_1's URL was changed, and org_2 took the slug it left.
    let me_acting_in = |active: &str| {
        format!(
            r#"{{
                "person": {{ "userId": "usr_1", "email": "alice@example.com" }},
                "organizations": [
                    {{ "organizationId": "org_1", "slug": "one-labs", "name": "One Labs", "role": "owner" }},
                    {{ "organizationId": "org_2", "slug": "org-1", "name": "Org 1", "role": "member" }},
                    {{ "organizationId": "org_3", "slug": "org-3", "name": "Three", "role": "member" }}
                ],
                "activeOrganizationId": "{active}",
                "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
            }}"#
        )
    };
    let config = Config::default();
    let status_under = |name: &'static str| {
        status::execute(
            status::StatusArgs { json: false },
            &transport,
            &store,
            &config,
            Some(name.to_string()),
            None,
        )
    };
    let switch_to = |name: &'static str| {
        org::execute(
            org::OrgCommand::Switch {
                organization: name.to_string(),
            },
            &transport,
            &store,
        )
    };
    let cached = || store.load().unwrap().unwrap();

    // 23a. status: the server acts in org_1, which the name no longer names
    store.save(&stale).unwrap();
    transport.push_answer(200, me_acting_in("org_1"));
    let moved = status_under("org-1").await.unwrap_err();
    assert_eq!(
        moved.to_string(),
        "org-1 no longer names the organization it did; run telmoni org list"
    );
    assert_eq!(cached().organizations[0].slug, "one-labs");
    assert_eq!(cached().active_organization_id.as_deref(), Some("org_3"));

    // 23b. the next run acts in the organization the console shows at /org-1
    transport.push_answer(200, me_acting_in("org_2"));
    status_under("org-1").await.unwrap();

    // 23c. org switch: refused too, saying where the name went, and the
    //      active organization stays
    store.save(&stale).unwrap();
    transport.push_answer(200, me_acting_in("org_1"));
    let moved = switch_to("org-1").await.unwrap_err();
    assert_eq!(
        moved.to_string(),
        "org-1 now names another organization; run telmoni org list"
    );
    assert_eq!(cached().organizations[1].slug, "org-1");
    assert_eq!(cached().active_organization_id.as_deref(), Some("org_3"));

    // 23d. and the next switch goes where the console's URL does
    transport.push_answer(200, me_acting_in("org_2"));
    switch_to("org-1").await.unwrap();
    assert_eq!(cached().active_organization_id.as_deref(), Some("org_2"));

    // 23e. an id never moves: under one, the organization whose URL changed is still itself
    transport.push_answer(200, me_acting_in("org_1"));
    status_under("org_1").await.unwrap();

    // 23f. a slug that moved and was taken by nobody is unknown once the
    //      answer is in, as it would have been with a fresh cache
    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "alice@example.com" },
            "organizations": [
                { "organizationId": "org_1", "slug": "one-labs", "name": "One Labs", "role": "owner" },
                { "organizationId": "org_2", "slug": "org-1", "name": "Org 1", "role": "member" },
                { "organizationId": "org_3", "slug": "three-labs", "name": "Three Labs", "role": "member" }
            ],
            "activeOrganizationId": "org_3",
            "sessionRowId": "0192a3b4-1111-7000-8000-000000000001"
        }"#,
    );
    let gone = switch_to("org-3").await.unwrap_err();
    assert_eq!(
        gone.to_string(),
        "unknown organization org-3; run telmoni org list"
    );
    assert_eq!(cached().organizations[2].slug, "three-labs");
    assert_eq!(cached().active_organization_id.as_deref(), Some("org_2"));

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 6);
    for (request, organization) in reqs
        .iter()
        .zip(["org_1", "org_2", "org_1", "org_2", "org_1", "org_3"])
    {
        assert_eq!(request.organization.as_deref(), Some(organization));
    }
}

// 24. Debug never prints a secret, whichever struct holds it: no token,
//     refresh token, device code or API key. What is not secret still shows.
#[test]
fn test_debug_never_prints_a_secret() {
    use telmoni_cli::auth::device::DeviceStart;
    use telmoni_cli::commands::login::LoginArgs;

    let mut device = device_creds(&["org_1"], "org_1");
    device.access_token = Some("secret_access".to_string());
    device.refresh_token = Some("secret_refresh".to_string());
    let api_key = Credentials::for_api_key(
        "https://telmoni.com".to_string(),
        "telmoni_secret_key".to_string(),
    );
    let request = LaneRequest {
        method: reqwest::Method::POST,
        url: "https://telmoni.com/cli/auth/refresh".to_string(),
        bearer: Some("secret_access".to_string()),
        organization: Some("org_1".to_string()),
        json: Some(serde_json::json!({ "refreshToken": "secret_refresh" })),
    };
    let answer = LaneAnswer {
        status: 200,
        content_type: Some("application/json".to_string()),
        body: r#"{"accessToken":"secret_access","refreshToken":"secret_refresh"}"#.to_string(),
    };
    let start: DeviceStart = serde_json::from_str(DEVICE_START).unwrap();
    let granted = PollOutcome::Granted(
        serde_json::from_str::<AuthnResult>(
            r#"{"userId":"usr_1","accessToken":"secret_access","refreshToken":"secret_refresh","expiresIn":900}"#,
        )
        .unwrap(),
    );
    let args = LoginArgs {
        key: Some("telmoni_secret_key".to_string()),
        endpoint: None,
        no_browser: true,
    };

    let printed = [
        format!("{device:?}"),
        format!("{api_key:#?}"),
        format!("{request:?}"),
        format!("{answer:?}"),
        format!("{start:?}"),
        format!("{granted:?}"),
        format!("{args:?}"),
    ];
    for out in &printed {
        for secret in [
            "secret_access",
            "secret_refresh",
            "secret_device_code",
            "telmoni_secret_key",
        ] {
            assert!(!out.contains(secret), "{secret} printed in {out}");
        }
        assert!(out.contains("<redacted>"), "{out}");
    }
    assert!(printed[0].contains("org_1"), "{}", printed[0]);
    assert!(
        printed[2].contains("https://telmoni.com/cli/auth/refresh"),
        "{}",
        printed[2]
    );
    assert!(printed[4].contains("BCDF-GHJK"), "the user code is shown");
}

// 25. A 401 on /cli/me is read by its type. The platform failing on its own
//     side, which the door answers as a 503, says nothing of the session, and
//     nor does a 401 that is not auth's: it is kept. One for an unknown
//     bearer, which an expired one is once swept, gets the refresh that
//     decides; one for a session that is over ends it at once.
#[tokio::test]
async fn test_status_reads_a_401_by_its_type() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let config = Config::default();
    let status = || {
        status::execute(
            status::StatusArgs { json: false },
            &transport,
            &store,
            &config,
            None,
            None,
        )
    };

    // 25a. the platform failing on its own side, on /cli/me: kept, and said as
    //      the door says it
    store.save(&device_creds(&["org_1"], "org_1")).unwrap();
    transport.push_answer(503, OWN_SIDE_FAILED);
    let err = status().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "upstream unavailable: the platform failed on its own side; try again shortly"
    );
    assert!(store.path.exists());

    // 25b. a 401 that is not auth's: kept, with no refresh
    transport.push_answer(401, FOREIGN_401);
    let err = status().await.unwrap_err();
    assert_eq!(err.to_string(), "unauthorized");
    assert!(store.path.exists());

    // 25c. and on the refresh an expiring bearer asks for first, which is
    //      asked for twice
    let mut expiring = device_creds(&["org_1"], "org_1");
    expiring.expires_at = Some(chrono::Utc::now().timestamp() - 10);
    store.save(&expiring).unwrap();
    transport.push_answer(503, OWN_SIDE_FAILED);
    transport.push_answer(503, OWN_SIDE_FAILED);
    status().await.unwrap_err();
    assert!(store.path.exists());

    // 25d. invalid-token on /cli/me for a session that lives: one refresh, one retry
    store.save(&device_creds(&["org_1"], "org_1")).unwrap();
    transport.push_answer(401, INVALID_TOKEN);
    transport.push_answer(200, FRESH_TOKENS);
    transport.push_answer(200, ME_IN_ORG_1);
    status().await.unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().access_token.as_deref(),
        Some("fresh")
    );

    // 25e. invalid-token on /cli/me, and the refresh refused too: it has ended
    transport.push_answer(401, INVALID_TOKEN);
    transport.push_answer(401, INVALID_TOKEN);
    let err = status().await.unwrap_err();
    assert_eq!(err.to_string(), "session ended; run telmoni login");
    assert!(!store.path.exists());

    // 25f. unauthenticated on /cli/me: ended, with no refresh
    store.save(&device_creds(&["org_1"], "org_1")).unwrap();
    transport.push_answer(401, UNAUTHENTICATED);
    let err = status().await.unwrap_err();
    assert_eq!(err.to_string(), "session ended; run telmoni login");
    assert!(!store.path.exists());

    let reqs = transport.requests.lock().unwrap();
    let paths: Vec<&str> = reqs
        .iter()
        .map(|r| r.url.trim_start_matches("https://telmoni.com"))
        .collect();
    assert_eq!(
        paths,
        [
            "/cli/me",
            "/cli/me",
            "/cli/auth/refresh",
            "/cli/auth/refresh",
            "/cli/me",
            "/cli/auth/refresh",
            "/cli/me",
            "/cli/me",
            "/cli/auth/refresh",
            "/cli/me",
        ]
    );
    assert_eq!(reqs[6].bearer.as_deref(), Some("fresh"));
}

// 26. Signing out under the same rules. A revoke refused as an expired bearer
//     (a clock that runs slow here skipped the early refresh) is refreshed and
//     sent again; one refused as unknown, with the refresh refused too, had
//     already ended, as had one refused as a session that is over. The
//     platform failing on its own side, a 401 that is not auth's, a 404 that
//     is not auth's "session not found", an expired bearer with nothing to
//     renew it, and no answer at all are not confirmed; logout says so and
//     deletes the file regardless.
#[tokio::test]
async fn test_end_session_reads_a_401_by_its_type() {
    use telmoni_cli::commands::logout::end_session;

    let store = temp_store();
    let transport = MockTransport::new();
    let take = || std::mem::take(&mut *transport.requests.lock().unwrap());

    // 26a. an expired bearer: refreshed, and the revoke sent again with the new one
    let mut creds = device_creds(&["org_1"], "org_1");
    store.save(&creds).unwrap();
    transport.push_answer(401, TOKEN_EXPIRED);
    transport.push_answer(200, FRESH_TOKENS);
    transport.push_answer(204, "");
    end_session(&transport, &store, &mut creds, None)
        .await
        .unwrap();
    let reqs = take();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[0].bearer.as_deref(), Some("at"));
    assert!(reqs[1].url.ends_with("/cli/auth/refresh"));
    assert_eq!(
        reqs[2].url,
        "https://telmoni.com/cli/sessions/0192a3b4-1111-7000-8000-000000000001/revoke"
    );
    assert_eq!(reqs[2].bearer.as_deref(), Some("fresh"));
    assert_eq!(reqs[2].organization.as_deref(), Some("org_1"));

    // 26b. an unknown bearer, and the refresh refused too: already ended
    let mut creds = device_creds(&["org_1"], "org_1");
    transport.push_answer(401, INVALID_TOKEN);
    transport.push_answer(401, INVALID_TOKEN);
    end_session(&transport, &store, &mut creds, None)
        .await
        .unwrap();
    assert_eq!(take().len(), 2);

    // 26c. a session that is over: already ended, with no refresh
    let mut creds = device_creds(&["org_1"], "org_1");
    transport.push_answer(401, UNAUTHENTICATED);
    end_session(&transport, &store, &mut creds, None)
        .await
        .unwrap();
    assert_eq!(take().len(), 1);

    // 26d. the platform failing on its own side, and a 401 that is not auth's:
    //      not confirmed, and no refresh
    transport.push_answer(503, OWN_SIDE_FAILED);
    let err = end_session(&transport, &store, &mut creds, None)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "upstream unavailable: the platform failed on its own side; try again shortly"
    );
    assert_eq!(take().len(), 1);
    transport.push_answer(401, FOREIGN_401);
    let err = end_session(&transport, &store, &mut creds, None)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "unauthorized");
    assert_eq!(take().len(), 1);

    // 26e. the door's own 404, for a path no lane matches: not confirmed
    transport.push_answer(
        404,
        r#"{"type":"/errors/not-found","title":"not found","status":404,"detail":"no such lane"}"#,
    );
    let err = end_session(&transport, &store, &mut creds, None)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "not found: no such lane");
    take();

    // 26f. an expired bearer and no refresh token to renew it: proves nothing
    creds.refresh_token = None;
    transport.push_answer(401, TOKEN_EXPIRED);
    let err = end_session(&transport, &store, &mut creds, None)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "token expired");
    assert_eq!(take().len(), 1);

    // 26g. no answer at all
    transport.push_error(anyhow::anyhow!(
        "timed out waiting for https://telmoni.com/cli/sessions/0192a3b4-1111-7000-8000-000000000001/revoke"
    ));
    let err = end_session(&transport, &store, &mut creds, None)
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("timed out waiting for"));
    take();

    // 26h. logout deletes the file whatever the answer
    store.save(&device_creds(&["org_1"], "org_1")).unwrap();
    transport.push_answer(503, OWN_SIDE_FAILED);
    logout::execute(logout::LogoutArgs {}, &transport, &store, None)
        .await
        .unwrap();
    assert!(!store.path.exists());
}

// 27. logout under an API key ends nothing on the server: no request, and
//     the file goes
#[tokio::test]
async fn test_logout_with_an_api_key() {
    let store = temp_store();
    let transport = MockTransport::new();
    store
        .save(&Credentials::for_api_key(
            "https://telmoni.com".to_string(),
            "telmoni_key".to_string(),
        ))
        .unwrap();

    logout::execute(logout::LogoutArgs {}, &transport, &store, None)
        .await
        .unwrap();

    assert!(!store.path.exists());
    assert!(transport.requests.lock().unwrap().is_empty());
}

// 28. A request that got no answer, a refused connection or a timeout, says
//     nothing about the session: the credentials file stays, and the failure
//     is what the person reads
#[tokio::test]
async fn test_network_failures_keep_credentials() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let config = Config::default();
    let status = || {
        status::execute(
            status::StatusArgs { json: false },
            &transport,
            &store,
            &config,
            None,
            None,
        )
    };
    let no_answer = || anyhow::anyhow!("timed out waiting for https://telmoni.com/cli/me");

    // 28a. /cli/me
    store.save(&device_creds(&["org_1"], "org_1")).unwrap();
    transport.push_error(no_answer());
    let err = status().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "timed out waiting for https://telmoni.com/cli/me"
    );
    assert!(store.path.exists());

    // 28b. the refresh an expiring bearer asks for, which is asked for twice
    let mut expiring = device_creds(&["org_1"], "org_1");
    expiring.expires_at = Some(chrono::Utc::now().timestamp() - 10);
    store.save(&expiring).unwrap();
    transport.push_error(no_answer());
    transport.push_error(no_answer());
    refresh_if_needed(&transport, &store, &mut expiring)
        .await
        .unwrap_err();
    assert_eq!(
        store.load().unwrap().unwrap().refresh_token.as_deref(),
        Some("rt")
    );

    // 28c. the refresh that would cure an expired bearer
    store.save(&device_creds(&["org_1"], "org_1")).unwrap();
    transport.push_answer(401, TOKEN_EXPIRED);
    transport.push_error(no_answer());
    transport.push_error(no_answer());
    status().await.unwrap_err();
    assert!(store.path.exists());

    // 28d. /v1 under an API key
    store
        .save(&Credentials::for_api_key(
            "https://telmoni.com".to_string(),
            "telmoni_key".to_string(),
        ))
        .unwrap();
    transport.push_error(anyhow::anyhow!(
        "request to https://telmoni.com/v1/organization failed: Connection refused (os error 111)"
    ));
    status().await.unwrap_err();
    assert!(store.path.exists());
}

/// A device start as the platform answers one: five seconds, ten minutes.
const DEVICE_START: &str = r#"{
    "deviceCode": "secret_device_code",
    "userCode": "BCDF-GHJK",
    "verificationUri": "https://telmoni.com/auth/device",
    "verificationUriComplete": "https://telmoni.com/auth/device?code=BCDF-GHJK",
    "expiresIn": 600,
    "interval": 5
}"#;

/// A granted poll.
const GRANTED: &str =
    r#"{"userId":"usr_1","accessToken":"at_new","refreshToken":"rt_new","expiresIn":900}"#;

/// `telmoni login` through the device flow, with a browser that opens nothing
/// and waits that take no time.
async fn device_login(transport: &MockTransport, store: &CredentialsStore) -> anyhow::Result<()> {
    use telmoni_cli::commands::login;
    use telmoni_cli::config::Config;

    login::execute(
        login::LoginArgs {
            key: None,
            endpoint: Some("https://telmoni.com".to_string()),
            no_browser: true,
        },
        transport,
        store,
        &Config::default(),
        None,
        |_: &str| Ok(()),
        |_| async {},
    )
    .await
}

// 29. The interactive login end to end, with the browser and the wait between
//     polls passed in: the browser opens the URL that carries the code, the
//     grant is polled at its interval and 5 s more after a slow_down, and the
//     session is saved once /cli/me has answered
#[tokio::test]
async fn test_interactive_login() {
    use telmoni_cli::commands::login;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let slept = Arc::new(Mutex::new(Vec::<Duration>::new()));

    transport.push_answer(200, DEVICE_START);
    transport.push_answer(202, r#"{"status":"authorization_pending"}"#);
    transport.push_answer(202, r#"{"status":"slow_down"}"#);
    transport.push_answer(200, GRANTED);
    transport.push_answer(200, ME_IN_ORG_1);

    login::execute(
        login::LoginArgs {
            key: None,
            endpoint: Some("https://telmoni.com".to_string()),
            no_browser: false,
        },
        &transport,
        &store,
        &Config::default(),
        None,
        |url: &str| {
            opened.lock().unwrap().push(url.to_string());
            Ok(())
        },
        |dur| {
            let slept = slept.clone();
            async move { slept.lock().unwrap().push(dur) }
        },
    )
    .await
    .unwrap();

    assert_eq!(
        *opened.lock().unwrap(),
        ["https://telmoni.com/auth/device?code=BCDF-GHJK"]
    );
    assert_eq!(
        *slept.lock().unwrap(),
        [
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_secs(10)
        ]
    );

    let reqs = transport.requests.lock().unwrap();
    let paths: Vec<&str> = reqs
        .iter()
        .map(|r| r.url.trim_start_matches("https://telmoni.com"))
        .collect();
    assert_eq!(
        paths,
        [
            "/cli/auth/device",
            "/cli/auth/device/poll",
            "/cli/auth/device/poll",
            "/cli/auth/device/poll",
            "/cli/me",
        ]
    );
    assert_eq!(reqs[0].bearer, None);
    assert_eq!(reqs[0].json, None);
    assert_eq!(
        reqs[1].json,
        Some(serde_json::json!({ "deviceCode": "secret_device_code" }))
    );
    assert_eq!(reqs[4].bearer.as_deref(), Some("at_new"));
    assert_eq!(reqs[4].organization, None);

    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.auth_type, AuthType::Device);
    assert_eq!(saved.endpoint, "https://telmoni.com");
    assert_eq!(saved.access_token.as_deref(), Some("at_new"));
    assert_eq!(saved.refresh_token.as_deref(), Some("rt_new"));
    assert_eq!(
        saved.session_row_id.as_deref(),
        Some("0192a3b4-1111-7000-8000-000000000001")
    );
    assert_eq!(saved.active_organization_id.as_deref(), Some("org_1"));
}

// 30. The browser is a convenience: one that will not open is a note and the
//     login goes on, and --no-browser opens none. The interval never drops
//     below a second, whatever the start says.
#[tokio::test]
async fn test_interactive_login_without_a_browser() {
    use telmoni_cli::commands::login;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    let slept = Arc::new(Mutex::new(Vec::<Duration>::new()));

    transport.push_answer(
        200,
        DEVICE_START.replace(r#""interval": 5"#, r#""interval": 0"#),
    );
    transport.push_answer(200, GRANTED);
    transport.push_answer(200, ME_IN_ORG_1);
    login::execute(
        login::LoginArgs {
            key: None,
            endpoint: Some("https://telmoni.com".to_string()),
            no_browser: false,
        },
        &transport,
        &store,
        &Config::default(),
        None,
        |_: &str| Err(std::io::Error::other("no display")),
        |dur| {
            let slept = slept.clone();
            async move { slept.lock().unwrap().push(dur) }
        },
    )
    .await
    .unwrap();
    assert_eq!(*slept.lock().unwrap(), [Duration::from_secs(1)]);
    assert!(store.path.exists());

    transport.push_answer(200, DEVICE_START);
    transport.push_answer(200, GRANTED);
    transport.push_answer(200, ME_IN_ORG_1);
    login::execute(
        login::LoginArgs {
            key: None,
            endpoint: Some("https://telmoni.com".to_string()),
            no_browser: true,
        },
        &transport,
        &store,
        &Config::default(),
        None,
        |_: &str| -> std::io::Result<()> { panic!("--no-browser opens no browser") },
        |_| async {},
    )
    .await
    .unwrap();
}

// 31. A login that does not end in a grant saves nothing: denied in the
//     console, a 401 that is not about the device code (said as it is), or a
//     request that got no answer, at the start or mid-poll
#[tokio::test]
async fn test_interactive_login_that_fails_saves_nothing() {
    let store = temp_store();
    let transport = MockTransport::new();

    transport.push_answer(200, DEVICE_START);
    transport.push_answer(
        403,
        r#"{"type":"/errors/authz/forbidden","title":"forbidden","status":403,"detail":"the sign-in was denied from the console"}"#,
    );
    let err = device_login(&transport, &store).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "forbidden: the sign-in was denied from the console"
    );

    transport.push_answer(200, DEVICE_START);
    transport.push_answer(401, FOREIGN_401);
    let err = device_login(&transport, &store).await.unwrap_err();
    assert_eq!(err.to_string(), "unauthorized");

    transport.push_error(anyhow::anyhow!(
        "request to https://telmoni.com/cli/auth/device failed: Connection refused (os error 111)"
    ));
    let err = device_login(&transport, &store).await.unwrap_err();
    assert!(err.to_string().contains("Connection refused"), "{err}");

    transport.push_answer(200, DEVICE_START);
    transport.push_answer(202, r#"{"status":"authorization_pending"}"#);
    transport.push_error(anyhow::anyhow!(
        "timed out waiting for https://telmoni.com/cli/auth/device/poll"
    ));
    let err = device_login(&transport, &store).await.unwrap_err();
    assert!(
        err.to_string().starts_with("timed out waiting for"),
        "{err}"
    );

    assert!(!store.path.exists());
}

// 32. The configuration file, at the path main hands down: a missing one is
//     the defaults, keys round-trip, an unknown key is refused and leaves the
//     file as it was, and a malformed one is an error main turns into a note
#[test]
fn test_configuration_file() {
    use telmoni_cli::config::{get_config_value, load_config, set_config_value};

    let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("telmoni-config-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("telmoni").join("config.json");

    assert_eq!(load_config(&path).unwrap().endpoint, None);
    assert_eq!(get_config_value(&path, "endpoint").unwrap(), None);

    set_config_value(&path, "endpoint", "https://config.example").unwrap();
    set_config_value(&path, "output_format", "json").unwrap();
    assert_eq!(
        get_config_value(&path, "endpoint").unwrap().as_deref(),
        Some("https://config.example")
    );
    assert_eq!(
        load_config(&path).unwrap().output_format.as_deref(),
        Some("json")
    );

    let before = std::fs::read_to_string(&path).unwrap();
    let err = set_config_value(&path, "colour", "blue").unwrap_err();
    assert!(
        err.to_string()
            .contains("unknown configuration key 'colour'"),
        "{err}"
    );
    assert!(get_config_value(&path, "colour").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

    std::fs::write(&path, "{ not json").unwrap();
    let err = load_config(&path).unwrap_err();
    assert!(err.to_string().starts_with("parsing config file"), "{err}");

    std::fs::remove_dir_all(&dir).unwrap();
}
