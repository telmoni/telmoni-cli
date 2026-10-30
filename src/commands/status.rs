//! `telmoni status` and `telmoni whoami` commands.

use anyhow::{Result, bail};
use clap::Args;
use serde_json::json;

use crate::auth::device::{NotSignedIn, fetch_me_for_session, refresh_if_needed};
use crate::auth::key::{KeySource, SuppliedKey};
use crate::auth::storage::{Credentials, CredentialsStore, DeviceSession, unix_now};
use crate::client::fetch_v1_organization;
use crate::commands::org::print_active_organization;
use crate::config::Config;
use crate::transport::{Transport, loggable_url};

/// Arguments for `telmoni status` and `telmoni whoami`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Format output as machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

/// Executes the `status` / `whoami` check. `telmoni_org_env` is `TELMONI_ORG`
/// and `endpoint_env` is `TELMONI_ENDPOINT`, both as `main` read them.
pub async fn execute(
    args: StatusArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    config: &Config,
    telmoni_org_env: Option<String>,
    endpoint_env: Option<String>,
) -> Result<()> {
    let Some(mut creds) = store.load()? else {
        // A script that asked for JSON reads stdout as JSON: it gets that or
        // nothing, and the refusal on stderr.
        if !args.json {
            let endpoint = crate::config::resolve_endpoint(None, endpoint_env.as_deref(), config);
            println!("Endpoint: {}", loggable_url(&endpoint));
        }
        return Err(
            NotSignedIn("Not signed in. Run telmoni login, or set TELMONI_API_KEY.").into(),
        );
    };

    match &mut creds {
        Credentials::ApiKey(saved) => {
            report_api_key(
                args,
                transport,
                &saved.endpoint,
                &saved.api_key,
                KeySource::Saved,
                None,
            )
            .await
        }
        Credentials::Device(session) => {
            execute_device(args, transport, store, session, telmoni_org_env).await
        }
    }
}

/// The saved login a key supplied for this command passed over, as `status`
/// marks it after its report; read for this and nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnusedLogin {
    /// A device sign-in, named by its address.
    Device { email: String },
    /// A saved API key.
    ApiKey,
}

impl UnusedLogin {
    /// What `saved` is, if anything was saved.
    pub fn of(saved: Option<&Credentials>) -> Option<Self> {
        match saved? {
            Credentials::Device(session) => Some(Self::Device {
                email: session.person.email.clone(),
            }),
            Credentials::ApiKey(_) => Some(Self::ApiKey),
        }
    }

    /// The `--json` field's value.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Device { .. } => "device",
            Self::ApiKey => "api_key",
        }
    }

    /// The line after the report.
    pub fn line(&self) -> String {
        match self {
            Self::Device { email } => {
                format!("Saved login, not in use: a device sign-in for {email}")
            }
            Self::ApiKey => "Saved login, not in use: a saved API key".to_owned(),
        }
    }
}

/// Executes `status` for a key supplied for this command alone, at
/// `endpoint` (the flag-less one: `TELMONI_ENDPOINT`, the configuration
/// file, the default). The key stands in for the saved login, which is
/// never written here; `unused` is that login, named as not in use.
pub async fn execute_with_key(
    args: StatusArgs,
    transport: &impl Transport,
    key: &SuppliedKey,
    endpoint: &str,
    unused: Option<UnusedLogin>,
) -> Result<()> {
    report_api_key(args, transport, endpoint, &key.key, key.source, unused).await
}

/// What an API key opens, asked of `/v1`; `source` is where the key came
/// from, and `unused` the saved login a supplied key passed over.
async fn report_api_key(
    args: StatusArgs,
    transport: &impl Transport,
    endpoint: &str,
    api_key: &str,
    source: KeySource,
    unused: Option<UnusedLogin>,
) -> Result<()> {
    let org = fetch_v1_organization(transport, endpoint, api_key).await?;

    if args.json {
        let mut out = json!({
            "authType": "api_key",
            "keySource": source.as_str(),
            "endpoint": loggable_url(endpoint),
            "organization": {
                "organizationId": org.organization_id,
                "slug": org.slug,
                "label": org.label(),
            }
        });
        if let (Some(unused), Some(fields)) = (&unused, out.as_object_mut()) {
            fields.insert("unusedLogin".to_owned(), json!(unused.kind()));
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        match source {
            KeySource::Saved => println!("API key for {} ({})", org.organization_id, org.label()),
            KeySource::Environment | KeySource::Helper => println!(
                "API key from {} for {} ({})",
                source.name(),
                org.organization_id,
                org.label()
            ),
        }
        println!("Endpoint: {}", loggable_url(endpoint));
        if let Some(unused) = &unused {
            println!("{}", unused.line());
        }
    }
    Ok(())
}

async fn execute_device(
    args: StatusArgs,
    transport: &impl Transport,
    store: &CredentialsStore,
    session: &mut DeviceSession,
    telmoni_org_env: Option<String>,
) -> Result<()> {
    // The cache, not the truth: a slug the organization took since, by a URL
    // change, is unknown here until `/cli/v1/me` is read again.
    let active_org_id = session
        .organization_from_env(telmoni_org_env.as_deref())?
        .map(|named| named.organization_id.clone())
        .or_else(|| session.active_organization_id.clone());

    refresh_if_needed(transport, store, session).await?;

    let me = fetch_me_for_session(transport, store, session, active_org_id.as_deref()).await?;
    session.update_from_me(&me, telmoni_org_env.is_some());
    // Only the cached list is at stake: the tokens a refresh rotated were
    // saved by the refresh itself, or it failed.
    if let Err(err) = store.save(&*session) {
        eprintln!(
            "note: the organization list could not be saved ({err}); the next command fetches it again"
        );
    }

    // `/me` falls back to the person's default organization — the one they
    // chose in Account Settings, else the oldest they own, else the oldest
    // they belong to — when the one asked for is no longer theirs, so the
    // cached list's say-so is not enough.
    if let Some(env_org) = telmoni_org_env.as_deref()
        && me.active_organization_id != active_org_id
    {
        bail!("you are no longer in {env_org}");
    }

    // ⚠ Nor is its say-so on a slug. A URL change moves one and another
    // organization may take it since, so the list just answered has to
    // give the organization the same name; else this would report on one
    // the console no longer shows at that URL.
    if let Some(env_org) = telmoni_org_env.as_deref() {
        let named_now = session.organization_named(env_org);
        if named_now.map(|o| &o.organization_id) != active_org_id.as_ref() {
            bail!("{env_org} no longer names the organization it did; run telmoni org list");
        }
    }

    let effective_active_id = if telmoni_org_env.is_some() {
        active_org_id.as_deref()
    } else {
        session.active_organization_id.as_deref()
    };

    let token_expires_in_secs = (session.expires_at - unix_now()).max(0);

    if args.json {
        print_device_json(session, effective_active_id, token_expires_in_secs)?;
    } else {
        print_device_text(session, effective_active_id, token_expires_in_secs);
    }
    Ok(())
}

fn print_device_json(
    session: &DeviceSession,
    effective_active_id: Option<&str>,
    token_expires_in_secs: i64,
) -> Result<()> {
    let active_org = effective_active_id
        .and_then(|id| session.find_organization(id))
        .map(|o| {
            json!({
                "organizationId": o.organization_id,
                "slug": o.slug,
                "label": o.label,
                "role": o.role,
            })
        });

    let out = json!({
        "authType": "device",
        "endpoint": loggable_url(&session.endpoint),
        "person": {
            "userId": session.person.user_id,
            "email": session.person.email,
            "displayName": session.person.display_name,
        },
        "activeOrganization": active_org,
        "organizationCount": session.organizations.len(),
        "tokenExpiresInSecs": token_expires_in_secs,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn print_device_text(
    session: &DeviceSession,
    effective_active_id: Option<&str>,
    token_expires_in_secs: i64,
) {
    let person = &session.person;
    if let Some(ref name) = person.display_name {
        println!("Signed in as {} ({})", person.email, name);
    } else {
        println!("Signed in as {}", person.email);
    }

    let active_org = effective_active_id.and_then(|id| session.find_organization(id));
    print_active_organization(active_org, effective_active_id);

    println!("Organizations: {}", session.organizations.len());
    println!("Endpoint: {}", loggable_url(&session.endpoint));
    let minutes = token_expires_in_secs / 60;
    println!("Token: expires in {minutes} minutes");
}
