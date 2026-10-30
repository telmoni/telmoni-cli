//! The saved login: in the macOS Keychain on a Mac, else in a credentials
//! file only its owner can read.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::transport::REDACTED;

/// Person identity stored in credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPerson {
    /// User ID.
    pub user_id: String,
    /// User email address.
    pub email: String,
    /// Optional display name.
    pub display_name: Option<String>,
}

impl From<&crate::auth::device::Person> for StoredPerson {
    fn from(person: &crate::auth::device::Person) -> Self {
        Self {
            user_id: person.user_id.clone(),
            email: person.email.clone(),
            display_name: person.display_name.clone(),
        }
    }
}

/// Organization summary stored in credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOrganization {
    /// Organization ID (`org_...`).
    pub organization_id: String,
    /// The slug the console's paths name it by, as `/cli/v1/me` last answered.
    pub slug: String,
    /// Organization human-readable label.
    pub label: String,
    /// User's role in the organization (`owner`, `admin`, `member`).
    pub role: String,
}

impl StoredOrganization {
    /// Formats the organization summary as `<label> (<organization_id>, <role>)`.
    pub fn display_summary(&self) -> String {
        format!("{} ({}, {})", self.label, self.organization_id, self.role)
    }
}

impl From<&crate::auth::device::Organization> for StoredOrganization {
    fn from(org: &crate::auth::device::Organization) -> Self {
        Self {
            organization_id: org.organization_id.clone(),
            slug: org.slug.clone(),
            label: org.label().to_string(),
            role: org.role.clone(),
        }
    }
}

/// The saved login: a device sign-in or a saved API key, told apart by the
/// file's `auth_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "auth_type", rename_all = "snake_case")]
pub enum Credentials {
    /// Interactive device authorization grant (RFC 8628).
    Device(DeviceSession),
    /// Static API key bearer authentication, from `login --with-key`.
    ApiKey(SavedKey),
}

/// A borrowed [`Credentials`], so a session is saved without a copy.
#[derive(Serialize)]
#[serde(tag = "auth_type", rename_all = "snake_case")]
pub enum CredentialsRef<'a> {
    /// A device sign-in.
    Device(&'a DeviceSession),
    /// A saved API key.
    ApiKey(&'a SavedKey),
}

impl<'a> From<&'a Credentials> for CredentialsRef<'a> {
    fn from(creds: &'a Credentials) -> Self {
        match creds {
            Credentials::Device(session) => Self::Device(session),
            Credentials::ApiKey(key) => Self::ApiKey(key),
        }
    }
}

impl<'a> From<&'a DeviceSession> for CredentialsRef<'a> {
    fn from(session: &'a DeviceSession) -> Self {
        Self::Device(session)
    }
}

impl<'a> From<&'a SavedKey> for CredentialsRef<'a> {
    fn from(key: &'a SavedKey) -> Self {
        Self::ApiKey(key)
    }
}

/// How long before its expiry a bearer is renewed rather than sent.
pub const REFRESH_SKEW_SECS: i64 = 60;

/// Seconds since the Unix epoch, as the credentials file keeps time.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// A device sign-in: the tokens, the session row and the person's cached
/// organizations.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSession {
    /// Endpoint associated with this session.
    pub endpoint: String,
    /// Access token: an opaque secret the platform minted.
    pub access_token: String,
    /// Refresh token; nullable on the wire (`AuthnResult.refreshToken`).
    pub refresh_token: Option<String>,
    /// Access token expiration timestamp in Unix seconds.
    pub expires_at: i64,
    /// Session row ID (UUID), what refresh and revoke name the session by.
    pub session_row_id: String,
    /// Person information.
    pub person: StoredPerson,
    /// Organizations the person belongs to.
    #[serde(default)]
    pub organizations: Vec<StoredOrganization>,
    /// ID of the active organization context.
    pub active_organization_id: Option<String>,
    /// Last updated timestamp in Unix seconds.
    pub updated_at: i64,
}

/// ⚠ By hand, not derived: a derived `Debug` would print the tokens into any
/// log line or test failure that formats the session.
impl std::fmt::Debug for DeviceSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Destructured, so a field added later has to be placed here: shown,
        // or redacted.
        let Self {
            endpoint,
            access_token: _,
            refresh_token,
            expires_at,
            session_row_id,
            person,
            organizations,
            active_organization_id,
            updated_at,
        } = self;
        f.debug_struct("DeviceSession")
            .field("endpoint", endpoint)
            .field("access_token", &REDACTED)
            .field("refresh_token", &refresh_token.as_ref().map(|_| REDACTED))
            .field("expires_at", expires_at)
            .field("session_row_id", session_row_id)
            .field("person", person)
            .field("organizations", organizations)
            .field("active_organization_id", active_organization_id)
            .field("updated_at", updated_at)
            .finish()
    }
}

/// An API key saved by `login --with-key`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedKey {
    /// Endpoint the key is used against.
    pub endpoint: String,
    /// The API key.
    pub api_key: String,
    /// Last updated timestamp in Unix seconds.
    pub updated_at: i64,
}

/// ⚠ By hand, not derived: a derived `Debug` would print the key.
impl std::fmt::Debug for SavedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            endpoint,
            api_key: _,
            updated_at,
        } = self;
        f.debug_struct("SavedKey")
            .field("endpoint", endpoint)
            .field("api_key", &REDACTED)
            .field("updated_at", updated_at)
            .finish()
    }
}

impl Credentials {
    /// Constructs a new credentials object for API key authentication.
    pub fn for_api_key(endpoint: String, key: String) -> Self {
        Self::ApiKey(SavedKey {
            endpoint,
            api_key: key,
            updated_at: unix_now(),
        })
    }

    /// When the login was last written, which decides between two copies.
    pub const fn updated_at(&self) -> i64 {
        match self {
            Self::Device(session) => session.updated_at,
            Self::ApiKey(key) => key.updated_at,
        }
    }
}

impl DeviceSession {
    /// Constructs a session from a successful device authorization.
    pub fn from_device_auth(
        endpoint: String,
        authn: crate::auth::device::AuthnResult,
        me: crate::auth::device::Me,
    ) -> Self {
        let now = unix_now();
        Self {
            endpoint,
            access_token: authn.access_token,
            refresh_token: authn.refresh_token,
            expires_at: now + authn.expires_in,
            session_row_id: me.session_row_id,
            person: StoredPerson::from(&me.person),
            organizations: me
                .organizations
                .iter()
                .map(StoredOrganization::from)
                .collect(),
            active_organization_id: me.active_organization_id,
            updated_at: now,
        }
    }

    /// Updates stored person, organizations, and session row from `/cli/v1/me`.
    /// Without `preserve_active_org` the active organization becomes the one
    /// `/me` acted in. With it, for a request that named an organization of
    /// its own, it is kept while it is still one of the person's; one they
    /// have left would be sent on every request after, so it gives way to
    /// their default, never to the one the request named.
    pub fn update_from_me(&mut self, me: &crate::auth::device::Me, preserve_active_org: bool) {
        self.person = StoredPerson::from(&me.person);
        self.organizations = me
            .organizations
            .iter()
            .map(StoredOrganization::from)
            .collect();
        let still_theirs = self
            .active_organization_id
            .as_deref()
            .is_some_and(|id| self.find_organization(id).is_some());
        if !preserve_active_org {
            self.active_organization_id = me.active_organization_id.clone();
        } else if !still_theirs {
            self.active_organization_id = me.default_organization_id.clone();
        }
        self.session_row_id.clone_from(&me.session_row_id);
        self.updated_at = unix_now();
    }

    /// Applies refreshed authentication tokens. The grant spent the refresh
    /// token it was handed whatever it answers, and a spent one presented
    /// again after a short grace ends the whole session, so an answer without
    /// one leaves none to keep.
    pub fn apply_refresh(&mut self, authn: &crate::auth::device::AuthnResult) {
        let now = unix_now();
        self.access_token.clone_from(&authn.access_token);
        self.refresh_token.clone_from(&authn.refresh_token);
        self.expires_at = now + authn.expires_in;
        self.updated_at = now;
    }

    /// Whether the access token is past, or within `REFRESH_SKEW_SECS` of,
    /// its expiry at `now`.
    pub const fn expiring(&self, now: i64) -> bool {
        now >= self.expires_at - REFRESH_SKEW_SECS
    }

    /// Finds an organization by its ID.
    pub fn find_organization(&self, org_id: &str) -> Option<&StoredOrganization> {
        self.organizations
            .iter()
            .find(|o| o.organization_id == org_id)
    }

    /// The cached organization an id or a slug names. The two never look
    /// alike: a slug has no underscore, and an id always has one. A slug is
    /// the one `/cli/v1/me` last answered, so a URL change since is not known
    /// here.
    ///
    /// ⚠ Exactly, like an id. A slug is lowercase, and read loosely `Acme`
    /// would pick the organization at `/acme` out of two that are both
    /// called Acme, where a name must be refused as ambiguous.
    pub fn organization_named(&self, identifier: &str) -> Option<&StoredOrganization> {
        self.organizations
            .iter()
            .find(|o| o.organization_id == identifier || o.slug == identifier)
    }

    /// The organization `TELMONI_ORG` names, by id or slug, from the cached
    /// list; `None` when it is unset.
    pub fn organization_from_env(
        &self,
        env_org: Option<&str>,
    ) -> Result<Option<&StoredOrganization>> {
        let Some(name) = env_org else {
            return Ok(None);
        };
        self.organization_named(name).map(Some).context(
            "TELMONI_ORG names no organization in the cached list; run telmoni status without \
             it to refresh the list",
        )
    }

    /// Returns the currently active organization record, if found.
    pub fn active_organization(&self) -> Option<&StoredOrganization> {
        self.active_organization_id
            .as_deref()
            .and_then(|id| self.find_organization(id))
    }
}

/// Where the saved login is kept: on a Mac, the macOS Keychain, with the
/// credentials file for when the Keychain refuses; elsewhere the file alone.
#[derive(Debug, Clone)]
pub struct CredentialsStore {
    /// Path to credentials file, and on a Mac the name of its Keychain item.
    pub path: PathBuf,
    /// Whether the Keychain is tried before the file. Only the binary's own
    /// store tries it, so no suite reads or writes a person's Keychain.
    keychain: bool,
    /// Whether the Keychain may put up a dialog.
    may_ask: bool,
}

impl CredentialsStore {
    /// A store that keeps the credentials in the file at `path` alone.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            keychain: false,
            may_ask: false,
        }
    }

    /// The store the binary runs with. On macOS the credentials go to the
    /// Keychain, in an item named for `path`, so a run under another home
    /// keeps an item of its own; the file at `path` holds them only while
    /// the Keychain refuses, as a locked one over SSH does. Elsewhere the
    /// file alone: the CLI runs in containers and on CI runners, which have
    /// no keychain to ask.
    ///
    /// `may_ask` says a person is there to answer the Keychain's dialogs:
    /// the one that unlocks it, and the one that lets a rebuilt binary read
    /// the item an older build saved. ⚠ Without one, a dialog never returns
    /// and the command hangs (a macOS CI runner has a screen and no one at
    /// it), so a dialog the Keychain would need is a refusal instead, as
    /// over SSH, and the file stands in.
    pub fn system(path: PathBuf, may_ask: bool) -> Self {
        Self {
            path,
            keychain: cfg!(target_os = "macos"),
            may_ask,
        }
    }

    /// Loads the saved credentials, if any. Nothing saved reads as not
    /// signed in; a copy that is there but cannot be read says so on stderr
    /// first, since the person did sign in once and has to again. With a
    /// copy in the Keychain and one in the file, which a save the Keychain
    /// refused left, the newer is read.
    pub fn load(&self) -> Result<Option<Credentials>> {
        Ok(self.read(true))
    }

    /// The saved login as it stands, for a command that passes it over: no
    /// note on stderr, since nobody is signing in with it. A copy that cannot
    /// be read is none.
    pub fn peek(&self) -> Option<Credentials> {
        self.read(false)
    }

    /// The saved login from the Keychain or the file, the newer where both
    /// hold one; `notes` says whether a copy that cannot be read, or a
    /// Keychain that did not answer, is said so.
    fn read(&self, notes: bool) -> Option<Credentials> {
        let mut unanswered = None;
        let in_keychain = if self.keychain {
            match keychain::read(&self.path, self.may_ask) {
                Ok(Some(content)) => parse(&content, "the Keychain", notes),
                Ok(None) => {
                    debug!("no credentials in the Keychain");
                    None
                }
                Err(err) => {
                    debug!(%err, "the Keychain could not be read");
                    unanswered = Some(err.to_string());
                    None
                }
            }
        } else {
            None
        };
        let in_file = self.load_file(notes);
        if notes
            && in_keychain.is_none()
            && in_file.is_none()
            && let Some(err) = unanswered
        {
            eprintln!(
                "note: the Keychain did not answer ({err}), so a login saved there is not read"
            );
        }
        match (in_keychain, in_file) {
            (Some(keychain), Some(file)) if file.updated_at() > keychain.updated_at() => Some(file),
            (Some(keychain), _) => Some(keychain),
            (None, file) => file,
        }
    }

    fn load_file(&self, notes: bool) -> Option<Credentials> {
        let path = self.path.display();
        if !self.path.exists() {
            debug!(%path, "no credentials file");
            return None;
        }
        match std::fs::read_to_string(&self.path) {
            Ok(content) => parse(&content, "the credentials file", notes),
            Err(err) => {
                debug!(%path, %err, "the credentials file cannot be read");
                if notes {
                    eprintln!("note: the saved credentials cannot be read; sign in again");
                }
                None
            }
        }
    }

    /// Saves the credentials: to the Keychain where there is one that
    /// answers, deleting the file a refused save left; else to the file.
    pub fn save<'a>(&self, creds: impl Into<CredentialsRef<'a>>) -> Result<()> {
        let json =
            serde_json::to_string_pretty(&creds.into()).context("serializing credentials")?;
        if self.keychain {
            match keychain::write(&self.path, &json, self.may_ask) {
                Ok(()) => {
                    debug!("credentials saved to the Keychain");
                    // An older copy, as secret as this one, would otherwise
                    // stay readable on disk.
                    if let Err(err) = remove_if_present(&self.path) {
                        eprintln!(
                            "note: an older copy of the credentials is still at {} ({err}); \
                             delete it",
                            self.path.display()
                        );
                    }
                    return Ok(());
                }
                Err(err) => {
                    debug!(%err, "the Keychain refused the credentials");
                    if !self.path.exists() {
                        eprintln!(
                            "note: the Keychain refused the credentials ({err}); they are kept \
                             in {} instead",
                            self.path.display()
                        );
                    }
                }
            }
        }
        write_replacing(&self.path, json.as_bytes(), 0o600)?;
        debug!(path = %self.path.display(), "credentials saved");
        Ok(())
    }

    /// Deletes the saved credentials, from the Keychain and the file both.
    /// One already gone is fine; one that cannot be deleted is an error,
    /// since the token or key it holds still signs in.
    pub fn clear(&self) -> Result<()> {
        let mut kept = Vec::new();
        if self.keychain {
            match keychain::delete(&self.path, self.may_ask) {
                Ok(()) => debug!("credentials deleted from the Keychain"),
                Err(err) => kept.push(format!("the Keychain item: {err}")),
            }
        }
        match remove_if_present(&self.path) {
            Ok(()) => debug!(path = %self.path.display(), "credentials file deleted"),
            Err(err) => kept.push(format!(
                "the credentials file {}: {err}",
                self.path.display()
            )),
        }
        if kept.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("deleting {}", kept.join("; ")))
        }
    }
}

/// The credentials `content`, from `origin`, holds; none for a copy that
/// does not parse or lacks what it signs in with, said on stderr when `notes`.
fn parse(content: &str, origin: &str, notes: bool) -> Option<Credentials> {
    let creds = match serde_json::from_str::<Credentials>(content) {
        Ok(creds) => creds,
        Err(err) => {
            // ⚠ Where, never the error's own text: serde quotes the value it
            // choked on, and in these credentials that can be a token.
            debug!(
                origin,
                category = ?err.classify(),
                line = err.line(),
                column = err.column(),
                "the saved credentials do not parse"
            );
            if notes {
                eprintln!("note: the saved credentials cannot be read; sign in again");
            }
            return None;
        }
    };
    debug!(origin, "credentials read");
    Some(creds)
}

/// A filesystem step that failed, with its reason: `main` prints only the
/// outermost message, and an `io::Error` names no secret.
pub(crate) fn io_step(step: &str, path: &Path, err: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!("{step} {}: {err}", path.display())
}

/// Writes `bytes` to `path` whole or not at all, the file created with `mode`
/// (before the umask) and any directory made for it `0700`.
///
/// ⚠ Never in place: the bytes go to a sibling created with `mode` from the
/// start, which is then renamed over the old file. So a token never sits in a
/// file anyone else can read, a crash mid-write leaves the previous file
/// whole rather than a torn one, and an older file's looser mode goes with it.
pub(crate) fn write_replacing(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Only directories created here are made private; one that already
    // exists (the system temp directory, a config root) is not ours to
    // tighten.
    let mut dirs = std::fs::DirBuilder::new();
    dirs.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut dirs, 0o700);
    dirs.create(parent)
        .map_err(|err| io_step("creating directory", parent, err))?;

    let file_name = path
        .file_name()
        .context("the path has no file name")?
        .to_string_lossy();
    let tmp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    // A sibling left by a crashed run of this same process id would make
    // `create_new` fail; it holds nothing worth keeping.
    let _ = std::fs::remove_file(&tmp);

    let written = (|| -> Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
        #[cfg(not(unix))]
        let _ = mode;
        let mut file = options
            .open(&tmp)
            .map_err(|err| io_step("creating", &tmp, err))?;
        file.write_all(bytes)
            .map_err(|err| io_step("writing", &tmp, err))?;
        file.sync_all()
            .map_err(|err| io_step("flushing", &tmp, err))?;
        std::fs::rename(&tmp, path).map_err(|err| io_step("writing", path, err))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return written;
    }

    // The rename is durable only once the directory entry is on disk.
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Deletes the file at `path`; one already gone is no error.
fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        done => done,
    }
}

/// The Keychain's half of the store: one generic password, under the service
/// `telmoni`, its account the credentials file's path.
#[cfg(target_os = "macos")]
mod keychain {
    use std::path::Path;

    use security_framework::base::Error;
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, generic_password, set_generic_password,
    };

    const SERVICE: &str = "telmoni";

    /// `errSecItemNotFound`: no item under the service and account.
    const ITEM_NOT_FOUND: i32 = -25300;

    fn account(path: &Path) -> String {
        path.display().to_string()
    }

    /// Runs `call` with the Keychain's dialogs off unless `may_ask`, so that
    /// one it would need fails at once, `errSecInteractionNotAllowed`, rather
    /// than wait for a person who is not there. The switch is the process's
    /// own, and back on once `call` returns.
    fn with_dialogs<T>(may_ask: bool, call: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        let _quiet = if may_ask {
            None
        } else {
            Some(SecKeychain::disable_user_interaction()?)
        };
        call()
    }

    pub(super) fn read(path: &Path, may_ask: bool) -> Result<Option<String>, Error> {
        match with_dialogs(may_ask, || {
            generic_password(PasswordOptions::new_generic_password(
                SERVICE,
                &account(path),
            ))
        }) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub(super) fn write(path: &Path, json: &str, may_ask: bool) -> Result<(), Error> {
        with_dialogs(may_ask, || {
            set_generic_password(SERVICE, &account(path), json.as_bytes())
        })
    }

    pub(super) fn delete(path: &Path, may_ask: bool) -> Result<(), Error> {
        match with_dialogs(may_ask, || delete_generic_password(SERVICE, &account(path))) {
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(()),
            done => done,
        }
    }
}

/// No Keychain off macOS: [`CredentialsStore::system`] never tries one here,
/// and were it to, every call would refuse, so the file would be used.
#[cfg(not(target_os = "macos"))]
mod keychain {
    use std::path::Path;

    #[derive(Debug)]
    pub(super) struct NoKeychain;

    impl std::fmt::Display for NoKeychain {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("there is no Keychain on this system")
        }
    }

    pub(super) const fn read(_path: &Path, _may_ask: bool) -> Result<Option<String>, NoKeychain> {
        Err(NoKeychain)
    }

    pub(super) const fn write(_path: &Path, _json: &str, _may_ask: bool) -> Result<(), NoKeychain> {
        Err(NoKeychain)
    }

    pub(super) const fn delete(_path: &Path, _may_ask: bool) -> Result<(), NoKeychain> {
        Err(NoKeychain)
    }
}
