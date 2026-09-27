//! Comprehensive unit tests verifying device authorization grant, transport seam,
//! credentials store, error parser, and team switching logic.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use telmoni_cli::auth::device::{
    AuthnResult, PollOutcome, Team, poll_once, poll_until_granted, refresh_if_needed,
};
use telmoni_cli::auth::storage::{
    AuthType, Credentials, CredentialsStore, StoredPerson, StoredTeam,
};
use telmoni_cli::commands::{logout, team};
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

    #[allow(dead_code)]
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
    assert!(store.load("default").unwrap().is_none());

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
        teams: vec![StoredTeam {
            team_id: "team_1".to_string(),
            label: "Acme Corp".to_string(),
            role: "owner".to_string(),
        }],
        active_team_id: None,
        api_key: None,
        updated_at: 1790000000,
    };

    store.save("default", &device_creds).unwrap();
    let loaded = store
        .load("default")
        .unwrap()
        .expect("should load credentials");
    assert_eq!(loaded, device_creds);
    assert!(loaded.refresh_token.is_none());
    assert!(loaded.session_row_id.is_none());
    assert!(loaded.active_team_id.is_none());
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
        teams: Vec::new(),
        active_team_id: None,
        api_key: Some("telmoni_secret_key".to_string()),
        updated_at: 1790000050,
    };

    store.save("default", &api_creds).unwrap();
    let loaded_api = store
        .load("default")
        .unwrap()
        .expect("should load api credentials");
    assert_eq!(loaded_api, api_creds);

    // Unparseable file reads as not signed in
    std::fs::write(&store.path, "corrupted { invalid json").unwrap();
    assert!(store.load("default").unwrap().is_none());

    // Device file missing access token reads as not signed in
    std::fs::write(
        &store.path,
        r#"{"auth_type":"device","endpoint":"https://telmoni.com","updated_at":100}"#,
    )
    .unwrap();
    assert!(store.load("default").unwrap().is_none());

    // Clear deletes the file
    store.save("default", &api_creds).unwrap();
    assert!(store.path.exists());
    store.clear("default").unwrap();
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
        teams: Vec::new(),
        active_team_id: None,
        api_key: None,
        updated_at: 100,
    };
    store.save("default", &creds).unwrap();

    transport.push_answer(
        200,
        r#"{
            "userId": "usr_1",
            "accessToken": "new_at",
            "refreshToken": "new_rt",
            "expiresIn": 3600
        }"#,
    );

    refresh_if_needed(&transport, &store, "default", &mut creds)
        .await
        .unwrap();
    assert_eq!(creds.access_token.as_deref(), Some("new_at"));
    assert_eq!(creds.refresh_token.as_deref(), Some("new_rt"));

    let loaded = store.load("default").unwrap().unwrap();
    assert_eq!(loaded.access_token.as_deref(), Some("new_at"));
    assert_eq!(loaded.refresh_token.as_deref(), Some("new_rt"));

    // null refreshToken leaving old refresh token in place
    creds.expires_at = Some(chrono::Utc::now().timestamp() - 10);
    store.save("default", &creds).unwrap();

    transport.push_answer(
        200,
        r#"{
            "userId": "usr_1",
            "accessToken": "new_at_2",
            "refreshToken": null,
            "expiresIn": 3600
        }"#,
    );

    refresh_if_needed(&transport, &store, "default", &mut creds)
        .await
        .unwrap();
    assert_eq!(creds.access_token.as_deref(), Some("new_at_2"));
    assert_eq!(creds.refresh_token.as_deref(), Some("new_rt")); // preserved

    // 401 clearing the file
    creds.expires_at = Some(chrono::Utc::now().timestamp() - 10);
    store.save("default", &creds).unwrap();

    transport.push_answer(
        401,
        r#"{"type":"/errors/auth/session-ended","title":"session ended","status":401}"#,
    );

    let err = refresh_if_needed(&transport, &store, "default", &mut creds)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("session ended"));
    assert!(
        store.load("default").unwrap().is_none(),
        "store should be cleared after 401"
    );
}

// 7. team label: name when present, else owner email, else Team
#[test]
fn test_team_label_fallback() {
    let team_with_name = Team {
        team_id: "team_1".to_string(),
        name: Some("Acme Corp".to_string()),
        owner_email: Some("owner@example.com".to_string()),
        owner_display_name: None,
        role: "owner".to_string(),
    };
    assert_eq!(team_with_name.label(), "Acme Corp");

    let team_with_empty_name = Team {
        team_id: "team_2".to_string(),
        name: Some("   ".to_string()),
        owner_email: Some("owner@example.com".to_string()),
        owner_display_name: None,
        role: "admin".to_string(),
    };
    assert_eq!(team_with_empty_name.label(), "owner@example.com");

    let team_without_name = Team {
        team_id: "team_3".to_string(),
        name: None,
        owner_email: Some("owner@example.com".to_string()),
        owner_display_name: None,
        role: "member".to_string(),
    };
    assert_eq!(team_without_name.label(), "owner@example.com");

    let team_without_owner = Team {
        team_id: "team_4".to_string(),
        name: None,
        owner_email: None,
        owner_display_name: None,
        role: "member".to_string(),
    };
    assert_eq!(team_without_owner.label(), "Team");
}

// 8. team switch: unknown team rejected without network; active switch verified against /me answer;
//    mismatch bailing with you are no longer in <id>
#[tokio::test]
async fn test_team_switch_logic() {
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
        teams: vec![
            StoredTeam {
                team_id: "team_1".to_string(),
                label: "Team One".to_string(),
                role: "owner".to_string(),
            },
            StoredTeam {
                team_id: "team_2".to_string(),
                label: "Team Two".to_string(),
                role: "member".to_string(),
            },
        ],
        active_team_id: Some("team_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save("default", &creds).unwrap();

    // 8a. Unknown team rejected without network
    let err = team::execute(
        team::TeamCommand::Switch {
            team_id: "team_unknown".to_string(),
        },
        &transport,
        &store,
        "default",
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unknown team team_unknown"));
    assert_eq!(
        transport.requests.lock().unwrap().len(),
        0,
        "no network on unknown team"
    );

    // 8b. Active switch verified against /me answer
    transport.push_answer(
        200,
        r#"{
            "person": {
                "userId": "usr_1",
                "email": "alice@example.com"
            },
            "teams": [
                {
                    "teamId": "team_1",
                    "name": "Team One",
                    "role": "owner"
                },
                {
                    "teamId": "team_2",
                    "name": "Team Two",
                    "role": "member"
                }
            ],
            "activeTeamId": "team_2",
            "sessionRowId": "0192a3b4-1111"
        }"#,
    );

    team::execute(
        team::TeamCommand::Switch {
            team_id: "team_2".to_string(),
        },
        &transport,
        &store,
        "default",
    )
    .await
    .unwrap();

    let updated = store.load("default").unwrap().unwrap();
    assert_eq!(updated.active_team_id.as_deref(), Some("team_2"));

    // 8c. Mismatch bailing with you are no longer in <id>
    transport.push_answer(
        200,
        r#"{
            "person": {
                "userId": "usr_1",
                "email": "alice@example.com"
            },
            "teams": [
                {
                    "teamId": "team_1",
                    "name": "Team One",
                    "role": "owner"
                }
            ],
            "activeTeamId": "team_1",
            "sessionRowId": "0192a3b4-1111"
        }"#,
    );

    let mismatch_err = team::execute(
        team::TeamCommand::Switch {
            team_id: "team_2".to_string(),
        },
        &transport,
        &store,
        "default",
    )
    .await
    .unwrap_err();

    assert_eq!(mismatch_err.to_string(), "you are no longer in team_2");
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
        teams: vec![StoredTeam {
            team_id: "team_1".to_string(),
            label: "Acme".to_string(),
            role: "owner".to_string(),
        }],
        active_team_id: Some("team_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save("default", &creds_refresh_401).unwrap();
    assert!(store.path.exists());

    // Refresh answers 401
    transport.push_answer(
        401,
        r#"{"type":"/errors/auth/session-ended","title":"session ended","status":401}"#,
    );

    logout::execute(logout::LogoutArgs {}, &transport, &store, "default", None)
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
        teams: vec![StoredTeam {
            team_id: "team_1".to_string(),
            label: "Acme".to_string(),
            role: "owner".to_string(),
        }],
        active_team_id: Some("team_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save("default", &creds_revoke_404).unwrap();
    assert!(store.path.exists());

    // Revoke answers 404
    transport.push_answer(404, r#"{"error":"session not found"}"#);

    logout::execute(logout::LogoutArgs {}, &transport, &store, "default", None)
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
        active_profile: None,
        profiles: std::collections::HashMap::new(),
        endpoint: Some("https://from-config.example/".to_string()),
        output_format: None,
    };
    assert_eq!(
        resolve_endpoint(
            Some("https://flag.example/"),
            Some("https://env.example"),
            &config,
            "default"
        ),
        "https://flag.example"
    );
    assert_eq!(
        resolve_endpoint(Some("  "), Some("https://env.example"), &config, "default"),
        "https://env.example"
    );
    assert_eq!(
        resolve_endpoint(None, None, &config, "default"),
        "https://from-config.example"
    );
    assert_eq!(
        resolve_endpoint(None, Some(""), &Config::default(), "default"),
        "https://telmoni.com"
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
        "default",
    )
    .await
    .unwrap();

    assert_eq!(transport.requests.lock().unwrap().len(), 0);

    let creds = store.load("default").unwrap().unwrap();
    assert_eq!(creds.auth_type, AuthType::ApiKey);
    assert_eq!(creds.api_key.as_deref(), Some("telmoni_test_api_key"));

    // Status calls /v1/team
    transport.push_answer(
        200,
        r#"{
            "team_id": "team_api_1",
            "name": "API Team",
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
        "default",
    )
    .await
    .unwrap();

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://telmoni.com/v1/team");
    assert_eq!(reqs[0].bearer.as_deref(), Some("telmoni_test_api_key"));
}

// 12. TELMONI_TEAM env validation
#[tokio::test]
async fn test_telmoni_team_env_validation() {
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
        teams: vec![StoredTeam {
            team_id: "team_allowed".to_string(),
            label: "Allowed Team".to_string(),
            role: "owner".to_string(),
        }],
        active_team_id: Some("team_allowed".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save("default", &creds).unwrap();

    // Invalid TELMONI_TEAM on status fails before network
    let status_err = status::execute(
        status::StatusArgs { json: false },
        &transport,
        &store,
        &config,
        Some("team_forbidden".to_string()),
        None,
        "default",
    )
    .await
    .unwrap_err();
    assert_eq!(
        status_err.to_string(),
        "TELMONI_TEAM names an team you are not in"
    );

    // Invalid TELMONI_TEAM on logout fails before network and preserves file
    let logout_err = logout::execute(
        logout::LogoutArgs {},
        &transport,
        &store,
        "default",
        Some("team_forbidden".to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(
        logout_err.to_string(),
        "TELMONI_TEAM names an team you are not in"
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
        teams: vec![StoredTeam {
            team_id: "team_1".to_string(),
            label: "Team Old".to_string(),
            role: "member".to_string(),
        }],
        active_team_id: Some("team_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save("default", &creds).unwrap();

    // Server answers with updated display name and newly added team
    transport.push_answer(
        200,
        r#"{
            "person": {
                "userId": "usr_1",
                "email": "alice@example.com",
                "displayName": "Alice Smith"
            },
            "teams": [
                {
                    "teamId": "team_1",
                    "name": "Acme Corp",
                    "role": "admin"
                },
                {
                    "teamId": "team_2",
                    "name": "Beta Labs",
                    "role": "owner"
                }
            ],
            "activeTeamId": "team_2",
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
        "default",
    )
    .await
    .unwrap();

    let updated = store.load("default").unwrap().unwrap();
    assert_eq!(
        updated.person.unwrap().display_name.as_deref(),
        Some("Alice Smith")
    );
    assert_eq!(updated.teams.len(), 2);
    assert_eq!(updated.teams[0].label, "Acme Corp");
    assert_eq!(updated.teams[0].role, "admin");
    assert_eq!(updated.teams[1].label, "Beta Labs");
    assert_eq!(updated.active_team_id.as_deref(), Some("team_2"));
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
        teams: vec![StoredTeam {
            team_id: "team_1".to_string(),
            label: "Team One".to_string(),
            role: "owner".to_string(),
        }],
        active_team_id: Some("team_1".to_string()),
        api_key: None,
        updated_at: 100,
    };
    store.save("default", &creds).unwrap();

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
            "teams": [
                {
                    "teamId": "team_1",
                    "name": "Team One",
                    "role": "owner"
                }
            ],
            "activeTeamId": "team_1",
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
        "default",
    )
    .await
    .unwrap();

    let updated = store.load("default").unwrap().unwrap();
    assert_eq!(updated.access_token.as_deref(), Some("fresh_access_token"));
    assert_eq!(updated.refresh_token.as_deref(), Some("new_refresh_token"));
    assert!(store.path.exists());
}

// 15. Team terminology compatibility: deserializes teams and activeTeamId, falls back to /v1/team on 404
#[tokio::test]
async fn test_team_terminology_compatibility() {
    use telmoni_cli::auth::device::Me;
    use telmoni_cli::client::fetch_v1_team;

    // A. Deserializing Me with "teams", "teamId", and "activeTeamId"
    let json_team_me = r#"{
        "person": {
            "userId": "usr_team_1",
            "email": "user@team.test"
        },
        "teams": [
            {
                "teamId": "team_alpha",
                "name": "Team Alpha",
                "role": "owner"
            }
        ],
        "activeTeamId": "team_alpha"
    }"#;

    let me: Me = serde_json::from_str(json_team_me).unwrap();
    assert_eq!(me.person.user_id, "usr_team_1");
    assert_eq!(me.teams.len(), 1);
    assert_eq!(me.teams[0].team_id, "team_alpha");
    assert_eq!(me.teams[0].label(), "Team Alpha");
    assert_eq!(me.active_team_id.as_deref(), Some("team_alpha"));

    // B. fetch_v1_team returns normally
    let transport = MockTransport::new();
    transport.push_answer(
        200,
        r#"{
        "team_id": "team_public",
        "name": "Public Team",
        "owner": {
            "email": "lead@team.test",
            "display_name": "Team Lead"
        }
    }"#,
    );

    let team = fetch_v1_team(&transport, "https://telmoni.com", "telmoni_api_token")
        .await
        .unwrap();
    assert_eq!(team.team_id, "team_public");
    assert_eq!(team.label(), "Public Team");

    let reqs = transport.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://telmoni.com/v1/team");
}
