//! Secure local storage for Telmoni credentials.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn update_from_me(&mut self, me: &crate::auth::device::Me, preserve_active_org: bool) {
        self.person = Some(StoredPerson::from(&me.person));
        self.organizations = me
            .organizations
            .iter()
            .map(StoredOrganization::from)
            .collect();
        if !preserve_active_org {
            self.active_organization_id = me.active_organization_id.clone();
        }
        if me.session_row_id.is_some() {
            self.session_row_id = me.session_row_id.clone();
        }
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Applies refreshed authentication tokens.
    pub fn apply_refresh(&mut self, authn: &crate::auth::device::AuthnResult) {
        self.access_token = Some(authn.access_token.clone());
        if let Some(ref new_rt) = authn.refresh_token {
            self.refresh_token = Some(new_rt.clone());
        }
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
    /// the one `/cli/me` last answered, so a rename since is not known here.
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

/// Store for managing credentials persistence on disk.
#[derive(Debug, Clone)]
pub struct CredentialsStore {
    /// Path to credentials file.
    pub path: PathBuf,
}

impl CredentialsStore {
    /// Creates a new credentials store at the given path.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Loads the saved credentials, if any. A missing file reads as not
    /// signed in; one that is there but cannot be read says so on stderr
    /// first, since the person did sign in once and has to again.
    pub fn load(&self) -> Result<Option<Credentials>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let readable = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|content| serde_json::from_str::<Credentials>(&content).ok());
        let Some(creds) = readable else {
            eprintln!("note: the saved credentials cannot be read; sign in again");
            return Ok(None);
        };

        match creds.auth_type {
            AuthType::Device => {
                if creds.access_token.is_none() || creds.person.is_none() {
                    eprintln!(
                        "note: device credentials missing access token; treating as not signed in"
                    );
                    return Ok(None);
                }
            }
            AuthType::ApiKey => {
                if creds.api_key.is_none() {
                    eprintln!("note: api_key credentials missing key; treating as not signed in");
                    return Ok(None);
                }
            }
        }
        Ok(Some(creds))
    }

    /// Saves the credentials to disk.
    ///
    /// ⚠ The tokens never sit in a file anyone else can read, not even for a
    /// moment: they go to a sibling created `0600` from the start, which is
    /// then renamed over the old file. The rename also means a crash mid-write
    /// leaves the previous file whole rather than a torn one that reads as
    /// signed out, and replaces whatever mode an older file had.
    pub fn save(&self, creds: &Credentials) -> Result<()> {
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

        let json = serde_json::to_string_pretty(creds).context("serializing credentials")?;

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
        Ok(())
    }

    /// Deletes the credentials file.
    pub fn clear(&self) -> Result<()> {
        if self.path.exists() {
            std::fs::remove_file(&self.path).ok();
        }
        Ok(())
    }
}
