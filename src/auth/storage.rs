//! The saved login: in the macOS Keychain on a Mac, else in a credentials
//! file only its owner can read.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::transport::REDACTED;

/// Type of authentication stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    /// Interactive device authorization grant (RFC 8628).
    Device,
    /// Static API key bearer authentication.
    ApiKey,
}

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
    /// The slug the console's paths name it by, as `/cli/me` last answered.
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

/// Locally persisted credentials.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// Authentication type.
    pub auth_type: AuthType,
    /// Endpoint associated with this session.
    pub endpoint: String,
    /// OAuth access token (set for `Device`).
    pub access_token: Option<String>,
    /// OAuth refresh token (set for `Device`).
    pub refresh_token: Option<String>,
    /// Access token expiration timestamp in Unix seconds.
    pub expires_at: Option<i64>,
    /// Session row ID (UUID) for Active Sessions management.
    pub session_row_id: Option<String>,
    /// Person information.
    pub person: Option<StoredPerson>,
    /// Organizations the person belongs to.
    #[serde(default)]
    pub organizations: Vec<StoredOrganization>,
    /// ID of the active organization context.
    pub active_organization_id: Option<String>,
    /// Static API key (set for `ApiKey`).
    pub api_key: Option<String>,
    /// Last updated timestamp in Unix seconds.
    pub updated_at: i64,
}

/// ⚠ By hand, not derived: a derived `Debug` would print the tokens and the
/// API key into any log line or test failure that formats the credentials.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Destructured, so a field added later has to be placed here: shown,
        // or redacted.
        let Self {
            auth_type,
            endpoint,
            access_token,
            refresh_token,
            expires_at,
            session_row_id,
            person,
            organizations,
            active_organization_id,
            api_key,
            updated_at,
        } = self;
        f.debug_struct("Credentials")
            .field("auth_type", auth_type)
            .field("endpoint", endpoint)
            .field("access_token", &access_token.as_ref().map(|_| REDACTED))
            .field("refresh_token", &refresh_token.as_ref().map(|_| REDACTED))
            .field("expires_at", expires_at)
            .field("session_row_id", session_row_id)
            .field("person", person)
            .field("organizations", organizations)
            .field("active_organization_id", active_organization_id)
            .field("api_key", &api_key.as_ref().map(|_| REDACTED))
            .field("updated_at", updated_at)
            .finish()
    }
}

impl Credentials {
    /// Constructs a new credentials object for API key authentication.
    pub fn for_api_key(endpoint: String, key: String) -> Self {
        Self {
            auth_type: AuthType::ApiKey,
            endpoint,
            access_token: None,
            refresh_token: None,
            expires_at: None,
            session_row_id: None,
            person: None,
            organizations: Vec::new(),
            active_organization_id: None,
            api_key: Some(key),
            updated_at: chrono::Utc::now().timestamp(),
        }
    }

    /// Constructs a new credentials object from successful device authorization.
    pub fn from_device_auth(
        endpoint: String,
        authn: crate::auth::device::AuthnResult,
        me: crate::auth::device::Me,
    ) -> Self {
        let stored_orgs = me
            .organizations
            .iter()
            .map(StoredOrganization::from)
            .collect();
        Self {
            auth_type: AuthType::Device,
            endpoint,
            access_token: Some(authn.access_token),
            refresh_token: authn.refresh_token,
            expires_at: Some(chrono::Utc::now().timestamp() + authn.expires_in),
            session_row_id: me.session_row_id,
            person: Some(StoredPerson::from(&me.person)),
            organizations: stored_orgs,
            active_organization_id: me.active_organization_id,
            api_key: None,
            updated_at: chrono::Utc::now().timestamp(),
        }
    }

    /// Updates stored person, organizations, and session row from `/cli/me`.
    /// Without `preserve_active_org` the active organization becomes the one
    /// `/me` acted in. With it, for a request that named an organization of
    /// its own, it is kept while it is still one of the person's; one they
    /// have left would be sent on every request after, so it gives way to
    /// their default, never to the one the request named.
    pub fn update_from_me(&mut self, me: &crate::auth::device::Me, preserve_active_org: bool) {
        self.person = Some(StoredPerson::from(&me.person));
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
        if me.session_row_id.is_some() {
            self.session_row_id = me.session_row_id.clone();
        }
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Applies refreshed authentication tokens. The grant spent the refresh
    /// token it was handed whatever it answers, and a spent one presented
    /// again after a short grace ends the whole session, so an answer without
    /// one leaves none to keep.
    pub fn apply_refresh(&mut self, authn: &crate::auth::device::AuthnResult) {
        self.access_token = Some(authn.access_token.clone());
        self.refresh_token = authn.refresh_token.clone();
        self.expires_at = Some(chrono::Utc::now().timestamp() + authn.expires_in);
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Finds an organization by its ID.
    pub fn find_organization(&self, org_id: &str) -> Option<&StoredOrganization> {
        self.organizations
            .iter()
            .find(|o| o.organization_id == org_id)
    }

    /// The cached organization an id or a slug names. The two never look
    /// alike: a slug has no underscore, and an id always has one. A slug is
    /// the one `/cli/me` last answered, so a URL change since is not known
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

    /// Returns the currently active organization record, if found.
    pub fn active_organization(&self) -> Option<&StoredOrganization> {
        self.active_organization_id
            .as_deref()
            .and_then(|id| self.find_organization(id))
    }
}

/// Helper to print active organization according to the CLI output contract:
/// - `<label> (<org_id>, <role>)` if known
/// - `<org_id>` if not in list
/// - "No organization" if none
pub fn print_active_organization(org: Option<&StoredOrganization>, raw_id: Option<&str>) {
    if let Some(org) = org {
        println!("Active organization: {}", org.display_summary());
    } else if let Some(id) = raw_id {
        println!("Active organization: {id}");
    } else {
        println!("No organization");
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
}

impl CredentialsStore {
    /// A store that keeps the credentials in the file at `path` alone.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            keychain: false,
        }
    }

    /// The store the binary runs with. On macOS the credentials go to the
    /// Keychain, in an item named for `path`, so a run under another home
    /// keeps an item of its own; the file at `path` holds them only while
    /// the Keychain refuses, as a locked one over SSH does. Elsewhere the
    /// file alone: the CLI runs in containers and on CI runners, which have
    /// no keychain to ask.
    pub fn system(path: PathBuf) -> Self {
        Self {
            path,
            keychain: cfg!(target_os = "macos"),
        }
    }

    /// Loads the saved credentials, if any. Nothing saved reads as not
    /// signed in; a copy that is there but cannot be read says so on stderr
    /// first, since the person did sign in once and has to again. With a
    /// copy in the Keychain and one in the file, which a save the Keychain
    /// refused left, the newer is read.
    pub fn load(&self) -> Result<Option<Credentials>> {
        let mut unanswered = None;
        let in_keychain = if self.keychain {
            match keychain::read(&self.path) {
                Ok(Some(content)) => parse(&content, "the Keychain"),
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
        let in_file = self.load_file();
        if in_keychain.is_none()
            && in_file.is_none()
            && let Some(err) = unanswered
        {
            eprintln!(
                "note: the Keychain did not answer ({err}), so a login saved there is not read"
            );
        }
        Ok(match (in_keychain, in_file) {
            (Some(keychain), Some(file)) if file.updated_at > keychain.updated_at => Some(file),
            (Some(keychain), _) => Some(keychain),
            (None, file) => file,
        })
    }

    fn load_file(&self) -> Option<Credentials> {
        let path = self.path.display();
        if !self.path.exists() {
            debug!(%path, "no credentials file");
            return None;
        }
        match std::fs::read_to_string(&self.path) {
            Ok(content) => parse(&content, "the credentials file"),
            Err(err) => {
                debug!(%path, %err, "the credentials file cannot be read");
                eprintln!("note: the saved credentials cannot be read; sign in again");
                None
            }
        }
    }

    /// Saves the credentials: to the Keychain where there is one that
    /// answers, deleting the file a refused save left; else to the file.
    pub fn save(&self, creds: &Credentials) -> Result<()> {
        let json = serde_json::to_string_pretty(creds).context("serializing credentials")?;
        if self.keychain {
            match keychain::write(&self.path, &json) {
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
        self.save_file(&json)
    }

    /// Writes the credentials file.
    ///
    /// ⚠ The tokens never sit in a file anyone else can read, not even for a
    /// moment: they go to a sibling created `0600` from the start, which is
    /// then renamed over the old file. The rename also means a crash mid-write
    /// leaves the previous file whole rather than a torn one that reads as
    /// signed out, and replaces whatever mode an older file had.
    fn save_file(&self, json: &str) -> Result<()> {
        let path = &self.path;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        // Only directories created here are made private; one that already
        // exists (the system temp directory, a config root) is not ours to
        // tighten.
        let mut dirs = std::fs::DirBuilder::new();
        dirs.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut dirs, 0o700);
        dirs.create(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;

        let file_name = path
            .file_name()
            .context("credentials path has no file name")?
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
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options
                .open(&tmp)
                .with_context(|| format!("creating {}", tmp.display()))?;
            file.write_all(json.as_bytes())
                .with_context(|| format!("writing {}", tmp.display()))?;
            file.sync_all()
                .with_context(|| format!("flushing {}", tmp.display()))?;
            std::fs::rename(&tmp, path)
                .with_context(|| format!("writing credentials to {}", path.display()))
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
        debug!(path = %path.display(), "credentials saved");
        Ok(())
    }

    /// Deletes the saved credentials, from the Keychain and the file both.
    /// One already gone is fine; one that cannot be deleted is an error,
    /// since the token or key it holds still signs in.
    pub fn clear(&self) -> Result<()> {
        let mut kept = Vec::new();
        if self.keychain {
            match keychain::delete(&self.path) {
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

/// Reads saved credentials out of `content`, from `origin`; `None`, with a
/// note, for a copy that does not parse or lacks what it signs in with.
fn parse(content: &str, origin: &str) -> Option<Credentials> {
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
            eprintln!("note: the saved credentials cannot be read; sign in again");
            return None;
        }
    };
    debug!(origin, auth_type = ?creds.auth_type, "credentials read");

    match creds.auth_type {
        AuthType::Device => {
            if creds.access_token.is_none() || creds.person.is_none() {
                eprintln!(
                    "note: device credentials missing access token; treating as not signed in"
                );
                return None;
            }
        }
        AuthType::ApiKey => {
            if creds.api_key.is_none() {
                eprintln!("note: api_key credentials missing key; treating as not signed in");
                return None;
            }
        }
    }
    Some(creds)
}

/// Deletes the file at `path`; one already gone is no error.
fn remove_if_present(path: &std::path::Path) -> std::io::Result<()> {
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
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, generic_password, set_generic_password,
    };

    const SERVICE: &str = "telmoni";

    /// `errSecItemNotFound`: no item under the service and account.
    const ITEM_NOT_FOUND: i32 = -25300;

    fn account(path: &Path) -> String {
        path.display().to_string()
    }

    pub(super) fn read(path: &Path) -> Result<Option<String>, Error> {
        match generic_password(PasswordOptions::new_generic_password(
            SERVICE,
            &account(path),
        )) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub(super) fn write(path: &Path, json: &str) -> Result<(), Error> {
        set_generic_password(SERVICE, &account(path), json.as_bytes())
    }

    pub(super) fn delete(path: &Path) -> Result<(), Error> {
        match delete_generic_password(SERVICE, &account(path)) {
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

    pub(super) const fn read(_path: &Path) -> Result<Option<String>, NoKeychain> {
        Err(NoKeychain)
    }

    pub(super) const fn write(_path: &Path, _json: &str) -> Result<(), NoKeychain> {
        Err(NoKeychain)
    }

    pub(super) const fn delete(_path: &Path) -> Result<(), NoKeychain> {
        Err(NoKeychain)
    }
}
