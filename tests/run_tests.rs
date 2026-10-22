//! `telmoni run`: the pings it sends around a command, and how it fails
//! open. ⚠ No request leaves this suite: the transport is a mock, and each
//! command wrapped is `sh` with a line of its own.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use telmoni_cli::commands::run::{
    EXIT_CANNOT_RUN, EXIT_NOT_FOUND, PING_RETRY_AFTER, RunArgs, execute,
};
use telmoni_cli::transport::{LaneAnswer, LaneRequest, Transport, loggable_url};

const KEY: &str = "telmoniping_k3yk3yk3yk3yk3yk3yk3yk3yk3yk3yk3";
const ENDPOINT: &str = "https://api.telmoni.com";

/// A transport that records every request and answers from a queue, `200
/// OK` once the queue is empty, as the lane answers a ping.
#[derive(Default, Clone)]
struct MockTransport {
    answers: Arc<Mutex<VecDeque<anyhow::Result<LaneAnswer>>>>,
    requests: Arc<Mutex<Vec<LaneRequest>>>,
}

impl MockTransport {
    fn push_answer(&self, status: u16, body: &str) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.push_back(Ok(LaneAnswer {
                status,
                content_type: Some("application/problem+json".to_string()),
                body: body.to_string(),
            }));
        }
    }

    fn push_error(&self, message: &str) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.push_back(Err(anyhow::anyhow!("{message}")));
        }
    }

    fn requests(&self) -> Vec<LaneRequest> {
        self.requests
            .lock()
            .map(|requests| requests.clone())
            .unwrap_or_default()
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
        next.unwrap_or_else(|| {
            Ok(LaneAnswer {
                status: 200,
                content_type: Some("text/plain; charset=utf-8".to_string()),
                body: "OK".to_string(),
            })
        })
    }
}

/// The waits a run asked for, recorded in place of sleeping.
#[derive(Default, Clone)]
struct Slept(Arc<Mutex<Vec<Duration>>>);

impl Slept {
    fn sleeper(&self) -> impl Fn(Duration) -> std::future::Ready<()> {
        let slept = Arc::clone(&self.0);
        move |wait| {
            if let Ok(mut slept) = slept.lock() {
                slept.push(wait);
            }
            std::future::ready(())
        }
    }

    fn waits(&self) -> Vec<Duration> {
        self.0.lock().map(|waits| waits.clone()).unwrap_or_default()
    }
}

fn args(monitor: &str, ping_key: Option<&str>, command: &[&str]) -> RunArgs {
    RunArgs {
        monitor: monitor.to_string(),
        ping_key: ping_key.map(str::to_string),
        command: command.iter().map(OsString::from).collect(),
    }
}

fn sh(line: &str) -> Vec<&str> {
    vec!["sh", "-c", line]
}

/// The address's path, and its query as a map.
fn parts(url: &str) -> (String, HashMap<String, String>) {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return (format!("not an address: {url}"), HashMap::new());
    };
    let query = parsed
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    (parsed.path().to_string(), query)
}

/// Whether `rid` is spelled as a v4 UUID.
fn is_v4_uuid(rid: &str) -> bool {
    let bytes = rid.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(at, b)| match at {
            8 | 13 | 18 | 23 => *b == b'-',
            14 => *b == b'4',
            19 => matches!(b, b'8' | b'9' | b'a' | b'b'),
            _ => b.is_ascii_hexdigit() && !b.is_ascii_uppercase(),
        })
}

// A run is a `/start` ping, the command, and a bare ping for success, both
// `POST` with no credential header, naming the same run; the command's exit
// code is the answer.
#[tokio::test]
async fn a_run_is_a_start_and_an_end_ping_around_the_command() {
    let transport = MockTransport::default();
    let slept = Slept::default();

    let code = execute(
        args("nightly-sync", Some(KEY), &sh("exit 0")),
        &transport,
        ENDPOINT,
        None,
        slept.sleeper(),
    )
    .await
    .unwrap();
    assert_eq!(code, 0);

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.method, reqwest::Method::POST);
        assert!(request.bearer.is_none());
        assert!(request.organization.is_none());
        assert!(request.json.is_none());
    }
    let (start_path, start_query) = parts(&requests[0].url);
    let (end_path, end_query) = parts(&requests[1].url);
    assert_eq!(start_path, format!("/ping/v1/{KEY}/nightly-sync/start"));
    assert_eq!(end_path, format!("/ping/v1/{KEY}/nightly-sync"));
    let rid = &start_query["rid"];
    assert!(is_v4_uuid(rid), "{rid}");
    assert_eq!(end_query["rid"], *rid, "one run on both pings");
    assert_eq!(start_query.len(), 1);
    assert_eq!(end_query.len(), 1);
    assert!(slept.waits().is_empty());
}

// A command that fails ends its run with `/fail` and its exit code, which
// the CLI exits with too; one a signal ends is 128 and the signal, as a
// shell reports it.
#[tokio::test]
async fn a_failed_command_is_a_fail_ping_with_its_exit_code() {
    let transport = MockTransport::default();
    let code = execute(
        args("nightly", Some(KEY), &sh("exit 3")),
        &transport,
        ENDPOINT,
        None,
        Slept::default().sleeper(),
    )
    .await
    .unwrap();
    assert_eq!(code, 3);
    let (path, query) = parts(&transport.requests()[1].url);
    assert_eq!(path, format!("/ping/v1/{KEY}/nightly/fail"));
    assert_eq!(query["exit_code"], "3");
    assert!(is_v4_uuid(&query["rid"]));

    let transport = MockTransport::default();
    let code = execute(
        args("nightly", Some(KEY), &sh("kill -TERM $$")),
        &transport,
        ENDPOINT,
        None,
        Slept::default().sleeper(),
    )
    .await
    .unwrap();
    assert_eq!(code, 128 + 15);
    let (path, query) = parts(&transport.requests()[1].url);
    assert_eq!(path, format!("/ping/v1/{KEY}/nightly/fail"));
    assert_eq!(query["exit_code"], "143");
}

// A command that cannot start is a `/fail` too, with the shell's code for
// it, so the monitor hears that its job did not run.
#[tokio::test]
async fn a_command_that_cannot_start_is_a_fail_ping_and_the_shells_code() {
    let transport = MockTransport::default();
    let code = execute(
        args(
            "nightly",
            Some(KEY),
            &["telmoni-run-test-no-such-program-7f3a", "--flag"],
        ),
        &transport,
        ENDPOINT,
        None,
        Slept::default().sleeper(),
    )
    .await
    .unwrap();
    assert_eq!(code, EXIT_NOT_FOUND);
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    let (path, query) = parts(&requests[1].url);
    assert_eq!(path, format!("/ping/v1/{KEY}/nightly/fail"));
    assert_eq!(query["exit_code"], EXIT_NOT_FOUND.to_string());

    // A directory is found and cannot run.
    let transport = MockTransport::default();
    let code = execute(
        args("nightly", Some(KEY), &["/"]),
        &transport,
        ENDPOINT,
        None,
        Slept::default().sleeper(),
    )
    .await
    .unwrap();
    assert_eq!(code, EXIT_CANNOT_RUN);
    let (_, query) = parts(&transport.requests()[1].url);
    assert_eq!(query["exit_code"], EXIT_CANNOT_RUN.to_string());
}

// Telmoni failing to answer never fails the command: each ping is tried
// once more after the wait, then given up, and the command's exit code
// stands.
#[tokio::test]
async fn the_command_runs_whatever_telmoni_answers() {
    let transport = MockTransport::default();
    let slept = Slept::default();
    for _ in 0..4 {
        transport.push_error("request to https://api.telmoni.com/ping/v1/… failed: refused");
    }
    let code = execute(
        args("nightly", Some(KEY), &sh("exit 5")),
        &transport,
        ENDPOINT,
        None,
        slept.sleeper(),
    )
    .await
    .unwrap();
    assert_eq!(code, 5);
    let requests = transport.requests();
    assert_eq!(requests.len(), 4, "two tries a ping");
    assert_eq!(requests[0].url, requests[1].url);
    assert_eq!(requests[2].url, requests[3].url);
    assert_ne!(requests[0].url, requests[2].url);
    assert_eq!(slept.waits(), vec![PING_RETRY_AFTER, PING_RETRY_AFTER]);
}

// A refusal is final, since the lane refuses nothing it would take a moment
// later; a 5xx is tried again once.
#[tokio::test]
async fn a_refusal_is_final_and_a_5xx_is_tried_again() {
    let transport = MockTransport::default();
    let slept = Slept::default();
    transport.push_answer(
        404,
        r#"{"type":"/errors/auth/not-found","title":"Not found","detail":"no ping key by that value","status":404}"#,
    );
    transport.push_answer(503, "");
    let code = execute(
        args("nightly", Some(KEY), &sh("exit 0")),
        &transport,
        ENDPOINT,
        None,
        slept.sleeper(),
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    let requests = transport.requests();
    assert_eq!(
        requests.len(),
        3,
        "one try for the refusal, two for the 503"
    );
    assert!(requests[0].url.ends_with("/start") || requests[0].url.contains("/start?"));
    assert_eq!(requests[1].url, requests[2].url);
    assert_eq!(slept.waits(), vec![PING_RETRY_AFTER]);
}

// Without a key, or with a slug the platform could not hold, nothing is
// sent and the command runs: a `..` or a `/` would make the lane's address
// another address. The flag's key comes before the variable's, and a blank
// one is none.
#[tokio::test]
async fn without_a_key_or_a_slug_nothing_is_sent_and_the_command_runs() {
    for (monitor, flag, variable) in [
        ("nightly", None, None),
        ("nightly", Some("  "), Some("")),
        ("..", Some(KEY), None),
        ("a/b", Some(KEY), None),
        ("Nightly", Some(KEY), None),
        ("", Some(KEY), None),
        ("two--hyphens", Some(KEY), None),
        (
            "a-slug-of-forty-nine-characters-is-one-too-many-xx",
            Some(KEY),
            None,
        ),
    ] {
        let transport = MockTransport::default();
        let code = execute(
            args(monitor, flag, &sh("exit 7")),
            &transport,
            ENDPOINT,
            variable.map(str::to_string),
            Slept::default().sleeper(),
        )
        .await
        .unwrap();
        assert_eq!(code, 7, "{monitor:?} {flag:?} {variable:?}");
        assert!(
            transport.requests().is_empty(),
            "{monitor:?} {flag:?} {variable:?}"
        );
    }

    let transport = MockTransport::default();
    execute(
        args("nightly", Some(" telmoniping_flag "), &sh("exit 0")),
        &transport,
        ENDPOINT,
        Some("telmoniping_variable".to_string()),
        Slept::default().sleeper(),
    )
    .await
    .unwrap();
    let (path, _) = parts(&transport.requests()[0].url);
    assert_eq!(path, "/ping/v1/telmoniping_flag/nightly/start");

    let transport = MockTransport::default();
    execute(
        args("nightly", None, &sh("exit 0")),
        &transport,
        "https://telmoni.example/base/",
        Some("telmoniping_variable".to_string()),
        Slept::default().sleeper(),
    )
    .await
    .unwrap();
    let (path, _) = parts(&transport.requests()[0].url);
    assert_eq!(path, "/base/ping/v1/telmoniping_variable/nightly/start");
}

// A log line or an error shows an address with its key cut out, by the
// key's place on the lane and by its shape anywhere, as the platform's own
// log does; everything else in it stands.
#[test]
fn the_key_is_cut_out_of_an_address_a_log_line_shows() {
    for (url, shown) in [
        (
            "https://api.telmoni.com/ping/v1/telmoniping_k3y/nightly/start?rid=1",
            "https://api.telmoni.com/ping/v1/…/nightly/start?rid=1",
        ),
        (
            "http://localhost:8082/ping/v1/notakey/nightly",
            "http://localhost:8082/ping/v1/…/nightly",
        ),
        (
            "https://api.telmoni.com/ping/notakey/nightly",
            "https://api.telmoni.com/ping/…/nightly",
        ),
        (
            "https://api.telmoni.com/v1/organization",
            "https://api.telmoni.com/v1/organization",
        ),
        (
            "https://api.telmoni.com/nightly/telmoni_4p1k3y",
            "https://api.telmoni.com/nightly/…",
        ),
        (
            "https://api.telmoni.com/ping/v1/'telmoniping_k3y'/nightly",
            "https://api.telmoni.com/ping/v1/…/nightly",
        ),
        ("not a url/telmoniping_k3y/x", "not a url/…/x"),
    ] {
        assert_eq!(loggable_url(url), shown, "{url}");
    }
}
