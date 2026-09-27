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

/// Team summary stored in credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTeam {
    /// Team ID (`team_...`).
    pub team_id: String,
    /// Team human-readable label.
    pub label: String,
    /// User's role in the team (`owner`, `admin`, `member`).
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
    /// Teams the person belongs to.
    #[serde(default)]
    pub teams: Vec<StoredTeam>,
    /// ID of the active team context.
    pub active_team_id: Option<String>,
    /// Static API key (set for `ApiKey`).
    pub api_key: Option<String>,
    /// Last updated timestamp in Unix seconds.
    pub updated_at: i64,
}

/// Store for managing credentials persistence on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CredentialsFile {
    #[serde(flatten)]
    pub profiles: std::collections::HashMap<String, Credentials>,
}

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

    fn load_file(&self) -> Result<CredentialsFile> {
        let path = &self.path;
        if !path.exists() {
            return Ok(CredentialsFile {
                profiles: std::collections::HashMap::new(),
            });
        }
        let content = std::fs::read_to_string(path)?;
        if let Ok(file) = serde_json::from_str::<CredentialsFile>(&content) {
            return Ok(file);
        }
        // Fallback for v1 credentials.json
        if let Ok(creds) = serde_json::from_str::<Credentials>(&content) {
            let mut profiles = std::collections::HashMap::new();
            profiles.insert("default".to_string(), creds);
            let file = CredentialsFile { profiles };
            let _ = self.save_file(&file); // auto migrate
            return Ok(file);
        }
        Ok(CredentialsFile {
            profiles: std::collections::HashMap::new(),
        })
    }

    fn save_file(&self, file: &CredentialsFile) -> Result<()> {
        let path = &self.path;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }

        let json = serde_json::to_string_pretty(file).context("serializing credentials file")?;
        std::fs::write(path, json)
            .with_context(|| format!("writing credentials to {}", path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)
                .context("reading metadata of credentials file")?
                .permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(path, perms)
                .context("setting permissions on credentials file")?;
        }
        Ok(())
    }

    /// Loads the saved credentials for a profile, if any.
    pub fn load(&self, profile: &str) -> Result<Option<Credentials>> {
        let file = match self.load_file() {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };
        let creds = match file.profiles.get(profile) {
            Some(c) => c.clone(),
            None => return Ok(None),
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

    /// Saves the credentials to disk for a profile.
    pub fn save(&self, profile: &str, creds: &Credentials) -> Result<()> {
        let mut file = self.load_file()?;
        file.profiles.insert(profile.to_string(), creds.clone());
        self.save_file(&file)
    }

    /// Clears the saved credentials for a profile.
    pub fn clear(&self, profile: &str) -> Result<()> {
        let mut file = self.load_file()?;
        file.profiles.remove(profile);
        if file.profiles.is_empty() {
            if self.path.exists() {
                std::fs::remove_file(&self.path).ok();
            }
            Ok(())
        } else {
            self.save_file(&file)
        }
    }
}
