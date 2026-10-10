//! An API key supplied for one command: `TELMONI_API_KEY`, else what the
//! configured `api_key_helper` prints. Either is used in place of the saved
//! login and never saved, so a CI job or a secret manager needs no
//! `telmoni login` and leaves no key on disk.

use std::time::Duration;

use anyhow::{Result, bail};

use crate::commands::login::validate_api_key;

/// Where a supplied key came from, which `status` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// `TELMONI_API_KEY`.
    Environment,
    /// The configuration file's `api_key_helper`.
    Helper,
}

impl KeySource {
    /// The name a person set it by.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Environment => "TELMONI_API_KEY",
            Self::Helper => "api_key_helper",
        }
    }

    /// The word `status --json` gives it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Helper => "helper",
        }
    }
}

/// An API key for this command alone.
#[derive(Clone, PartialEq, Eq)]
pub struct SuppliedKey {
    /// The key.
    pub key: String,
    /// Where it came from.
    pub source: KeySource,
}

/// ⚠ By hand: `key` is the API key.
impl std::fmt::Debug for SuppliedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SuppliedKey")
            .field("key", &crate::transport::REDACTED)
            .field("source", &self.source)
            .finish()
    }
}

/// The key this command runs as, when one is supplied for it: `env_key`
/// (`TELMONI_API_KEY`, blank already read as unset), else the output of
/// `helper` run by `run_helper`; `None` leaves the saved login. A key either
/// gives that is not an API key is an error naming where it came from,
/// never the key.
pub async fn supplied_key<R, F>(
    env_key: Option<String>,
    helper: Option<&str>,
    run_helper: R,
) -> Result<Option<SuppliedKey>>
where
    R: FnOnce(String) -> F,
    F: Future<Output = Result<String>>,
{
    let (key, source) = if let Some(key) = env_key {
        (key, KeySource::Environment)
    } else if let Some(command) = helper.map(str::trim).filter(|c| !c.is_empty()) {
        (run_helper(command.to_owned()).await?, KeySource::Helper)
    } else {
        return Ok(None);
    };
    let key = key.trim().to_owned();
    if validate_api_key(&key).is_err() {
        bail!(
            "{} gives no API key: an API key starts with telmoni_ and has no spaces",
            source.name()
        );
    }
    Ok(Some(SuppliedKey { key, source }))
}

/// How long the helper may take: a secret manager that has to ask for a
/// fingerprint or a password answers in seconds, one that hangs never does.
pub const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs `command` through the shell and returns what it printed. Its stderr
/// is the person's to see, so a prompt or a refusal reaches them; its stdout
/// is the key and is never shown.
pub async fn run_helper(command: String) -> Result<String> {
    let mut shell = tokio::process::Command::new("sh");
    shell
        .arg("-c")
        .arg(&command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(HELPER_TIMEOUT, shell.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => bail!("api_key_helper could not be run: {err}"),
        Err(_) => bail!(
            "api_key_helper did not answer within {} seconds",
            HELPER_TIMEOUT.as_secs()
        ),
    };
    if !output.status.success() {
        bail!("api_key_helper failed ({})", output.status);
    }
    let Ok(printed) = String::from_utf8(output.stdout) else {
        bail!("api_key_helper printed something that is not text");
    };
    if printed.trim().is_empty() {
        bail!("api_key_helper printed nothing");
    }
    Ok(printed)
}
