//! `telmoni run --monitor <slug> -- <command>`: a command run under one of
//! the project's monitors, its start and its end each a ping on the lane a
//! crontab's `curl` uses. The command keeps its standard streams and its
//! exit code, which becomes the CLI's; nothing of its output is sent. A
//! Telmoni that does not answer, or refuses a ping, is a note on stderr and
//! never the reason a job did not run.

use std::ffi::OsString;
use std::future::Future;
use std::io::Read as _;
use std::process::ExitStatus;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Args;
use tracing::debug;

use crate::transport::{LaneRequest, REDACTED, Transport, parse_lane_error};

/// The longest one ping may take each time it is tried: the lane answers
/// in milliseconds, and a job must not wait long on a Telmoni that does not.
pub const PING_WAIT: Duration = Duration::from_secs(5);

/// The wait before a ping that got no answer, or a 5xx, is tried once more:
/// an end ping lost to a blip is a run Telmoni counts as never ended.
pub const PING_RETRY_AFTER: Duration = Duration::from_secs(1);

/// The exit codes for a command that could not be started, the shell's own:
/// one not found, and one that cannot run.
pub const EXIT_NOT_FOUND: i32 = 127;
pub const EXIT_CANNOT_RUN: i32 = 126;

/// A slug's longest, as the platform spells one (`crates/shared/src/slug.rs`).
const SLUG_MAX_LEN: usize = 48;

/// Arguments for `telmoni run`.
#[derive(Args)]
pub struct RunArgs {
    /// The monitor the command runs under, by its slug.
    #[arg(long, value_name = "SLUG")]
    pub monitor: String,

    /// The project's ping key; else `TELMONI_PING_KEY`.
    // On the command line as on a crontab line, where a ping key lives by
    // design: it can fake a ping and nothing more.
    #[arg(long, value_name = "KEY")]
    pub ping_key: Option<String>,

    /// The command to run, after `--`.
    #[arg(last = true, required = true, num_args = 1.., value_name = "COMMAND")]
    pub command: Vec<OsString>,
}

/// ⚠ By hand: `ping_key` is a credential.
impl std::fmt::Debug for RunArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            monitor,
            ping_key,
            command,
        } = self;
        f.debug_struct("RunArgs")
            .field("monitor", monitor)
            .field("ping_key", &ping_key.as_ref().map(|_| REDACTED))
            .field("command", command)
            .finish()
    }
}

/// Runs the command under the monitor and answers the exit code to end
/// with: the command's, or [`EXIT_NOT_FOUND`] or [`EXIT_CANNOT_RUN`] where
/// it could not start. `endpoint` is the one a key asks at, `ping_key_env`
/// is `TELMONI_PING_KEY` as `main` read it, and `sleep` is the wait before
/// a ping is tried again, passed in so a test waits for nothing.
pub async fn execute<S, SF>(
    args: RunArgs,
    transport: &impl Transport,
    endpoint: &str,
    ping_key_env: Option<String>,
    sleep: S,
) -> Result<i32>
where
    S: Fn(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let Some((program, arguments)) = args.command.split_first() else {
        bail!("telmoni run: a command to run goes after --");
    };
    let pings = match Pings::of(
        endpoint,
        args.ping_key.as_deref(),
        ping_key_env.as_deref(),
        &args.monitor,
    ) {
        Ok(pings) => Some(pings),
        Err(why) => {
            eprintln!("note: telmoni run: {why}; the command runs unmonitored");
            None
        }
    };
    debug!(monitor = %args.monitor, rid = pings.as_ref().and_then(|p| p.rid.as_deref()), "run");

    if let Some(pings) = &pings
        && let Err(why) = ping(transport, pings.start(), &sleep).await
    {
        eprintln!("note: telmoni run: the start ping {why}; the command runs all the same");
    }

    let code = match tokio::process::Command::new(program)
        .args(arguments)
        .spawn()
    {
        Ok(mut child) => match child.wait().await {
            Ok(status) => exit_code_of(status),
            Err(err) => {
                eprintln!(
                    "telmoni run: waiting for {} failed: {err}",
                    program.to_string_lossy()
                );
                EXIT_CANNOT_RUN
            }
        },
        Err(err) => {
            eprintln!(
                "telmoni run: cannot run {}: {err}",
                program.to_string_lossy()
            );
            if err.kind() == std::io::ErrorKind::NotFound {
                EXIT_NOT_FOUND
            } else {
                EXIT_CANNOT_RUN
            }
        }
    };
    debug!(code, "command ended");

    if let Some(pings) = &pings
        && let Err(why) = ping(transport, pings.end(code), &sleep).await
    {
        eprintln!("note: telmoni run: the end ping {why}");
    }
    Ok(code)
}

/// The pings of one run: the monitor's address on the lane, and the id the
/// run goes by on each.
struct Pings {
    /// `{endpoint}/ping/v1/{ping_key}/{slug}`.
    base: reqwest::Url,
    rid: Option<String>,
}

impl Pings {
    /// The address, from the key the flag gives, else the variable, and the
    /// slug; or why no ping can be sent, said without the key.
    fn of(
        endpoint: &str,
        flag: Option<&str>,
        variable: Option<&str>,
        slug: &str,
    ) -> Result<Self, String> {
        let key = flag
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .or_else(|| variable.map(str::trim).filter(|key| !key.is_empty()));
        let Some(key) = key else {
            return Err("no ping key: pass --ping-key or set TELMONI_PING_KEY".to_string());
        };
        if !is_slug(slug) {
            return Err(format!(
                "a monitor's slug is lowercase letters and digits joined by single hyphens, \
                 up to {SLUG_MAX_LEN} characters"
            ));
        }
        let mut base = reqwest::Url::parse(endpoint)
            .map_err(|_| format!("{endpoint} is not an address a ping can go to"))?;
        base.path_segments_mut()
            .map_err(|()| format!("{endpoint} is not an address a ping can go to"))?
            .pop_if_empty()
            .extend(["ping", "v1", key, slug]);
        Ok(Self {
            base,
            rid: fresh_rid(),
        })
    }

    /// `/start`: the run opens, and its stall timeout starts.
    fn start(&self) -> String {
        let mut url = self.base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.push("start");
        }
        self.named(url).into()
    }

    /// A bare ping for success, else `/fail` with the command's exit code.
    fn end(&self, code: i32) -> String {
        let mut url = self.base.clone();
        if code != 0 {
            if let Ok(mut path) = url.path_segments_mut() {
                path.push("fail");
            }
            let mut url = self.named(url);
            url.query_pairs_mut()
                .append_pair("exit_code", &code.to_string());
            return url.into();
        }
        self.named(url).into()
    }

    /// `url` naming the run, where the pings have an id for it.
    fn named(&self, mut url: reqwest::Url) -> reqwest::Url {
        if let Some(rid) = &self.rid {
            url.query_pairs_mut().append_pair("rid", rid);
        }
        url
    }
}

/// One ping, tried twice at most. A 2xx is heard. A 4xx is final, since the
/// lane refuses nothing it would take a moment later. No answer, or a 5xx,
/// is tried once more after [`PING_RETRY_AFTER`]. The error is the note's
/// tail: what happened, with the key cut out of any address in it.
async fn ping<S, SF>(transport: &impl Transport, url: String, sleep: &S) -> Result<(), String>
where
    S: Fn(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let mut again = true;
    loop {
        let request = LaneRequest {
            method: reqwest::Method::POST,
            url: url.clone(),
            bearer: None,
            organization: None,
            json: None,
        };
        let why = match tokio::time::timeout(PING_WAIT, transport.send(request)).await {
            Ok(Ok(answer)) if (200..300).contains(&answer.status) => return Ok(()),
            Ok(Ok(answer)) if answer.status < 500 => {
                return Err(format!("was refused: {}", parse_lane_error(&answer)));
            }
            Ok(Ok(answer)) => format!("was answered {}", answer.status),
            Ok(Err(err)) => format!("got no answer ({err})"),
            Err(_) => format!("got no answer within {} seconds", PING_WAIT.as_secs()),
        };
        if !again {
            return Err(why);
        }
        again = false;
        debug!(why, "ping: trying again");
        sleep(PING_RETRY_AFTER).await;
    }
}

/// The exit code a command's end gives the CLI: its own, or 128 and the
/// signal's number where a signal ended it, as a shell reports one.
fn exit_code_of(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    // Nothing else `wait` answers with: the shell's general failure.
    1
}

/// Whether `slug` is spelled as the platform spells a monitor's: lowercase
/// letters and digits in runs joined by single hyphens, up to
/// [`SLUG_MAX_LEN`]. Anything else could not be a monitor, and a `..` or a
/// `/` would make the lane's address another address.
fn is_slug(slug: &str) -> bool {
    slug.len() <= SLUG_MAX_LEN
        && slug.split('-').all(|run| {
            !run.is_empty()
                && run
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// A run id for the pings, a v4 UUID from the system's randomness: a ping
/// tried again is heard as the ping it repeats, and two runs of one monitor
/// at once are told apart. None where the system gives none, and the pings
/// name no run.
fn fresh_rid() -> Option<String> {
    use std::fmt::Write as _;

    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .ok()?
        .read_exact(&mut bytes)
        .ok()?;
    if let Some(byte) = bytes.get_mut(6) {
        *byte = (*byte & 0x0f) | 0x40;
    }
    if let Some(byte) = bytes.get_mut(8) {
        *byte = (*byte & 0x3f) | 0x80;
    }
    let mut rid = String::with_capacity(36);
    for (at, byte) in bytes.iter().enumerate() {
        if matches!(at, 4 | 6 | 8 | 10) {
            rid.push('-');
        }
        let _ = write!(rid, "{byte:02x}");
    }
    Some(rid)
}
