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

/// Organization summary stored in credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOrganization {
    /// Organization ID (`org_...`).
    pub organization_id: String,
    /// Organization human-readable label.
    pub label: String,
    /// User's role in the organization (`owner`, `admin`, `member`).
    pub role: String,
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

    /// Loads the saved credentials, if any. A missing, unreadable or
    /// unparseable file reads as not signed in.
    pub fn load(&self) -> Result<Option<Credentials>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Ok(None);
        };
        let Ok(creds) = serde_json::from_str::<Credentials>(&content) else {
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
