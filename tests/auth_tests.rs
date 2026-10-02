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

    #[expect(dead_code, reason = "helper for failure-injection test cases")]
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
        body: r#"{"type":"/errors/rate-limit","title":"Too Many Requests","status":429,"detail":"slow down","retry_after_secs":45}"#.to_string(),
    };
    let err3 = parse_lane_error(&answer3);
    assert_eq!(err3.status, 429);
    assert_eq!(err3.retry_after_secs, Some(45));
    assert_eq!(err3.message, "Too Many Requests: slow down");

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

    // 426 upgrade
    let answer6 = LaneAnswer {
        status: 426,
        content_type: None,
        body: "".to_string(),
    };
    let err6 = parse_lane_error(&answer6);
    assert_eq!(err6.status, 426);
    assert_eq!(err6.message, "this CLI is too old; upgrade it");
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
        r#"{"type":"/errors/rate-limit","title":"rate limited","status":429,"retry_after_secs":12}"#,
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

    // 401 Dead device code
    transport.push_answer(401, r#"{"error":"invalid code"}"#);
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PollOutcome::Failed(
            "the device code is no longer valid; run telmoni login again".to_string()
        )
    );

    // 403 Authorization denied
    transport.push_answer(403, r#"{"error":"access denied by user"}"#);
    let outcome = poll_once(&transport, "https://telmoni.com", "code123")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        PollOutcome::Failed("access denied by user".to_string())
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
        access_token: "jwt_token".to_string(),
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

        assert_eq!(authn.access_token, "jwt_token");
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
//    a null refreshToken in the 200 leaving the old refresh token in place
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
        session_row_id: Some("0192a3b4-1111".to_string()),
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

    // null refreshToken leaving old refresh token in place
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
    assert_eq!(creds.refresh_token.as_deref(), Some("new_rt")); // preserved

    // 401 clearing the file
    creds.expires_at = Some(chrono::Utc::now().timestamp() - 10);
    store.save(&creds).unwrap();

    transport.push_answer(
        401,
        r#"{"type":"/errors/auth/session-ended","title":"session ended","status":401}"#,
    );

    let err = refresh_if_needed(&transport, &store, &mut creds)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("session ended"));
    assert!(
        store.load().unwrap().is_none(),
        "store should be cleared after 401"
    );
}

// 7. organization label: name when present, else owner email, else Organization
#[test]
fn test_organization_label_fallback() {
    let org_with_name = Organization {
        organization_id: "org_1".to_string(),
        name: Some("Acme Corp".to_string()),
        owner_email: Some("owner@example.com".to_string()),
        owner_display_name: None,
        role: "owner".to_string(),
    };
    assert_eq!(org_with_name.label(), "Acme Corp");

    let org_with_empty_name = Organization {
        organization_id: "org_2".to_string(),
        name: Some("   ".to_string()),
        owner_email: Some("owner@example.com".to_string()),
        owner_display_name: None,
        role: "admin".to_string(),
    };
    assert_eq!(org_with_empty_name.label(), "owner@example.com");

    let org_without_name = Organization {
        organization_id: "org_3".to_string(),
        name: None,
        owner_email: Some("owner@example.com".to_string()),
        owner_display_name: None,
        role: "member".to_string(),
    };
    assert_eq!(org_without_name.label(), "owner@example.com");

    let org_without_owner = Organization {
        organization_id: "org_4".to_string(),
        name: None,
        owner_email: None,
        owner_display_name: None,
        role: "member".to_string(),
    };
    assert_eq!(org_without_owner.label(), "Organization");
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
        session_row_id: Some("0192a3b4-1111".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![
            StoredOrganization {
                organization_id: "org_1".to_string(),
                label: "Org One".to_string(),
                role: "owner".to_string(),
            },
            StoredOrganization {
                organization_id: "org_2".to_string(),
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
                    "name": "Org One",
                    "role": "owner"
                },
                {
                    "organizationId": "org_2",
                    "name": "Org Two",
                    "role": "member"
                }
            ],
            "activeOrganizationId": "org_2",
            "sessionRowId": "0192a3b4-1111"
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
                    "name": "Org One",
                    "role": "owner"
                }
            ],
            "activeOrganizationId": "org_1",
            "sessionRowId": "0192a3b4-1111"
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
        session_row_id: Some("0192a3b4-1111".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "bob@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
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
        r#"{"type":"/errors/auth/session-ended","title":"session ended","status":401}"#,
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
        session_row_id: Some("0192a3b4-2222".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "bob@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
            label: "Acme".to_string(),
            role: "owner".to_string(),
        }],
        active_organization_id: Some("org_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds_revoke_404).unwrap();
    assert!(store.path.exists());

    // Revoke answers 404
    transport.push_answer(404, r#"{"error":"session not found"}"#);

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

    // Login with API key - no network call
    login::execute(
        login::LoginArgs {
            key: Some("telmoni_test_api_key".to_string()),
            endpoint: Some("https://telmoni.com".to_string()),
            no_browser: true,
        },
        &transport,
        &store,
        &config,
        None,
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
        session_row_id: Some("0192a3b4-1111".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_allowed".to_string(),
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
    assert_eq!(
        status_err.to_string(),
        "TELMONI_ORG names an organization you are not in"
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
        "TELMONI_ORG names an organization you are not in"
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
        session_row_id: Some("0192a3b4-1111".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
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
                    "name": "Acme Corp",
                    "role": "admin"
                },
                {
                    "organizationId": "org_2",
                    "name": "Beta Labs",
                    "role": "owner"
                }
            ],
            "activeOrganizationId": "org_2",
            "sessionRowId": "0192a3b4-1111"
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
    assert_eq!(updated.organizations[0].role, "admin");
    assert_eq!(updated.organizations[1].label, "Beta Labs");
    assert_eq!(updated.active_organization_id.as_deref(), Some("org_2"));
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
        session_row_id: Some("0192a3b4-1111".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![StoredOrganization {
            organization_id: "org_1".to_string(),
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
                    "name": "Org One",
                    "role": "owner"
                }
            ],
            "activeOrganizationId": "org_1",
            "sessionRowId": "0192a3b4-1111"
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
            "analyticsOptIn": false
        },
        "organizations": [
            {
                "organizationId": "org_alpha",
                "name": "Org Alpha",
                "ownerEmail": "user@org.test",
                "ownerDisplayName": null,
                "role": "owner",
                "ownershipOfferExpiresAt": null
            }
        ],
        "deletedOrganizations": [],
        "activeOrganizationId": "org_alpha",
        "memberships": [],
        "incomingInvites": [],
        "projectOffers": [],
        "flags": [],
        "firstLogin": false,
        "sessionRowId": "0192a3b4-0000-7000-8000-000000000001"
    }"#;

    let me: Me = serde_json::from_str(json_me).unwrap();
    assert_eq!(me.person.user_id, "usr_org_1");
    assert_eq!(me.organizations.len(), 1);
    assert_eq!(me.organizations[0].organization_id, "org_alpha");
    assert_eq!(me.organizations[0].label(), "Org Alpha");
    assert_eq!(me.active_organization_id.as_deref(), Some("org_alpha"));

    // B. fetch_v1_organization reads /v1/organization
    let transport = MockTransport::new();
    transport.push_answer(
        200,
        r#"{
        "organization_id": "org_public",
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
    assert_eq!(org.label(), "Public Org");

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://telmoni.com/v1/organization");
}

fn device_creds(orgs: &[&str], active: &str) -> Credentials {
    Credentials {
        auth_type: AuthType::Device,
        endpoint: "https://telmoni.com".to_string(),
        access_token: Some("at".to_string()),
        refresh_token: Some("rt".to_string()),
        expires_at: Some(chrono::Utc::now().timestamp() + 3600),
        session_row_id: Some("0192a3b4-1111".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        }),
        organizations: orgs
            .iter()
            .map(|id| StoredOrganization {
                organization_id: (*id).to_string(),
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

// 16. A server failure while curing an expired token never deletes credentials
#[tokio::test]
async fn test_status_keeps_credentials_on_refresh_server_error() {
    use telmoni_cli::commands::status;
    use telmoni_cli::config::Config;

    let store = temp_store();
    let transport = MockTransport::new();
    store.save(&device_creds(&["org_1"], "org_1")).unwrap();

    transport.push_answer(401, TOKEN_EXPIRED);
    transport.push_answer(503, r#"{"error":"upstream unavailable"}"#);

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

    assert_eq!(err.to_string(), "upstream unavailable");
    assert!(store.path.exists(), "a 503 must never delete credentials");
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
                { "organizationId": "org_1", "name": "One", "role": "owner" },
                { "organizationId": "org_2", "name": "Two", "role": "member" }
            ],
            "activeOrganizationId": "org_2",
            "sessionRowId": "0192a3b4-1111"
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
            "organizations": [ { "organizationId": "org_1", "name": "One", "role": "owner" } ],
            "activeOrganizationId": "org_1",
            "sessionRowId": "0192a3b4-1111"
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

// 19. logout names the organization `/me` answers when the cached one is refused
#[tokio::test]
async fn test_logout_retries_revoke_with_current_organization() {
    let store = temp_store();
    let transport = MockTransport::new();
    store
        .save(&device_creds(&["org_gone"], "org_gone"))
        .unwrap();

    transport.push_answer(
        403,
        r#"{"type":"/errors/authz/forbidden","title":"forbidden","status":403}"#,
    );
    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "alice@example.com" },
            "organizations": [ { "organizationId": "org_now", "name": "Now", "role": "member" } ],
            "activeOrganizationId": "org_now",
            "sessionRowId": "0192a3b4-1111"
        }"#,
    );
    transport.push_answer(204, "");

    logout::execute(logout::LogoutArgs {}, &transport, &store, None)
        .await
        .unwrap();

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[0].organization.as_deref(), Some("org_gone"));
    assert_eq!(reqs[1].url, "https://telmoni.com/cli/me");
    assert_eq!(reqs[1].organization, None);
    assert_eq!(reqs[2].organization.as_deref(), Some("org_now"));
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

#[tokio::test]
async fn test_org_switch_duplicate_label_disambiguation() {
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
        session_row_id: Some("0192a3b4-1111".to_string()),
        person: Some(StoredPerson {
            user_id: "usr_1".to_string(),
            email: "bob@example.com".to_string(),
            display_name: None,
        }),
        organizations: vec![
            StoredOrganization {
                organization_id: "org_alpha".to_string(),
                label: "Acme".to_string(),
                role: "owner".to_string(),
            },
            StoredOrganization {
                organization_id: "org_beta".to_string(),
                label: "Acme".to_string(),
                role: "member".to_string(),
            },
        ],
        active_organization_id: Some("org_alpha".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save(&creds).unwrap();

    // Switching by label "Acme" when two orgs share it should fail and prompt to use ID
    let err = org::execute(
        org::OrgCommand::Switch {
            organization: "Acme".to_string(),
        },
        &transport,
        &store,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("multiple organizations named 'Acme'"),
        "expected disambiguation error, got: {err}"
    );
    assert!(err.to_string().contains("org_alpha (owner)"));
    assert!(err.to_string().contains("org_beta (member)"));

    // Switching by explicit ID works without ambiguity
    transport.push_answer(
        200,
        r#"{
            "person": { "userId": "usr_1", "email": "bob@example.com" },
            "organizations": [
                { "organizationId": "org_alpha", "name": "Acme", "role": "owner" },
                { "organizationId": "org_beta", "name": "Acme", "role": "member" }
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
}

#[test]
fn test_base_config_dir_fallback() {
    use telmoni_cli::config::{base_config_dir, config_path};

    let base = base_config_dir();
    assert!(!base.as_os_str().is_empty());

    let path = config_path().unwrap();
    assert!(path.ends_with("telmoni/config.json"));
}
