//! The `telmoni` binary itself: what `main.rs` reads, where its files land,
//! and what reaches stdout and stderr.
//!
//! ⚠ Each run gets a home and a working directory of its own under the system
//! temp directory, and an environment holding nothing but that home: the
//! person's real configuration, credentials, `.env` and `TELMONI_*`
//! variables never reach it. No command run here makes a request.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static RUN: AtomicU64 = AtomicU64::new(1);

/// A home directory and a working directory of the test's own.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    #[expect(
        clippy::unwrap_used,
        reason = "test scaffolding: a sandbox that cannot be made fails the test"
    )]
    fn new() -> Self {
        let n = RUN.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("telmoni-cli-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("work")).unwrap();
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    /// Where `dirs::config_dir()` puts the CLI's files under this home.
    fn telmoni_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home()
                .join("Library")
                .join("Application Support")
                .join("telmoni")
        } else {
            self.home().join(".config").join("telmoni")
        }
    }

    /// `telmoni <args>` with `HOME` and `env` as its whole environment.
    #[expect(
        clippy::unwrap_used,
        reason = "test scaffolding: a binary that cannot be run fails the test"
    )]
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_telmoni"))
            .args(args)
            .current_dir(self.work())
            .env_clear()
            .env("HOME", self.home())
            .envs(env.iter().copied())
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// Signed out, status prints the endpoint it would sign in against on stdout
// and the refusal on stderr, and exits 1; with --json stdout stays empty.
// TELMONI_ENDPOINT is read, and a blank one is no value.
#[test]
fn signed_out_status_names_the_endpoint() {
    let sandbox = Sandbox::new();

    let out = sandbox.run(&["status"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "Endpoint: https://telmoni.com\n");
    assert!(stderr(&out).contains("Not signed in"), "{}", stderr(&out));

    let out = sandbox.run(&["status"], &[("TELMONI_ENDPOINT", "localhost:3000")]);
    assert_eq!(stdout(&out), "Endpoint: http://localhost:3000\n");

    let out = sandbox.run(&["status"], &[("TELMONI_ENDPOINT", "  ")]);
    assert_eq!(stdout(&out), "Endpoint: https://telmoni.com\n");

    let out = sandbox.run(&["whoami", "--json"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
}

// The configuration file lives in the configuration directory, never the
// working directory; the environment wins over it; a malformed one is a note
// and the defaults.
#[test]
fn configuration_file_through_the_binary() {
    let sandbox = Sandbox::new();
    let path = sandbox.telmoni_dir().join("config.json");

    let out = sandbox.run(
        &["config", "set", "endpoint", "https://config.example"],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(path.exists());
    assert_eq!(std::fs::read_dir(sandbox.work()).unwrap().count(), 0);

    let out = sandbox.run(&["config", "get", "endpoint"], &[]);
    assert_eq!(stdout(&out), "https://config.example\n");

    let out = sandbox.run(&["config", "list"], &[]);
    assert!(
        stdout(&out).contains("endpoint:      https://config.example"),
        "{}",
        stdout(&out)
    );

    let out = sandbox.run(&["status"], &[]);
    assert_eq!(stdout(&out), "Endpoint: https://config.example\n");

    let out = sandbox.run(&["status"], &[("TELMONI_ENDPOINT", "https://env.example")]);
    assert_eq!(stdout(&out), "Endpoint: https://env.example\n");

    let out = sandbox.run(&["config", "get", "colour"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("unknown configuration key 'colour'"),
        "{}",
        stderr(&out)
    );

    std::fs::write(&path, "{ not json").unwrap();
    let out = sandbox.run(&["status"], &[]);
    assert!(
        stderr(&out).contains("note: could not load config file"),
        "{}",
        stderr(&out)
    );
    assert_eq!(stdout(&out), "Endpoint: https://telmoni.com\n");
}

// A debug build reads `.env` from the working directory, after the
// environment; a release build never does, so a checkout it runs in cannot
// redirect it.
#[test]
fn dotenv_only_in_a_debug_build() {
    let sandbox = Sandbox::new();
    std::fs::write(
        sandbox.work().join(".env"),
        "TELMONI_ENDPOINT='https://dotenv.example'\n",
    )
    .unwrap();

    let out = sandbox.run(&["status"], &[]);
    let read = if cfg!(debug_assertions) {
        "https://dotenv.example"
    } else {
        "https://telmoni.com"
    };
    assert_eq!(stdout(&out), format!("Endpoint: {read}\n"));

    let out = sandbox.run(&["status"], &[("TELMONI_ENDPOINT", "https://env.example")]);
    assert_eq!(stdout(&out), "Endpoint: https://env.example\n");
}

// An API key signs in without a request and is kept in the configuration
// directory, private. `org` refuses it, and logout deletes it, again without
// a request. -v logs to stderr, and nothing anywhere prints the key.
#[test]
fn api_key_sign_in_and_out() {
    let sandbox = Sandbox::new();
    // Were a request made after all, this endpoint refuses it on this machine.
    let env = [("TELMONI_ENDPOINT", "http://127.0.0.1:9")];
    let creds = sandbox.telmoni_dir().join("credentials.json");
    let mut outputs = Vec::new();

    let out = sandbox.run(&["login", "--key", "not-a-key"], &env);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("an API key starts with telmoni_"),
        "{}",
        stderr(&out)
    );
    assert!(!creds.exists());

    let out = sandbox.run(&["-v", "login", "--key", "telmoni_secret_key"], &env);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "Signed in with API key\nEndpoint: http://127.0.0.1:9\n"
    );
    assert!(stderr(&out).contains("DEBUG"), "{}", stderr(&out));
    assert!(creds.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&creds).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    outputs.push(out);

    let out = sandbox.run(&["-v", "org", "list"], &env);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("org commands need a browser session"),
        "{}",
        stderr(&out)
    );
    outputs.push(out);

    let out = sandbox.run(&["-v", "logout"], &env);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Signed out\n");
    assert!(!creds.exists());
    outputs.push(out);

    for out in &outputs {
        assert!(!stdout(out).contains("telmoni_secret_key"));
        assert!(!stderr(out).contains("telmoni_secret_key"));
    }
}
