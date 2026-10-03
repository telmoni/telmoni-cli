//! Telmoni CLI (`telmoni`).

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt};

use telmoni_cli::auth::CredentialsStore;
use telmoni_cli::commands::{config_cmd, login, logout, org, status};
use telmoni_cli::config::{Config, load_config};
use telmoni_cli::transport::ReqwestTransport;

#[derive(Parser)]
#[command(
    name = "telmoni",
    version,
    about = "Telmoni CLI",
    long_about = "The official command-line interface for Telmoni."
)]
struct Cli {
    #[arg(short, long, global = true, help = "Enable verbose debug logging")]
    verbose: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Authenticate with Telmoni via interactive device authorization or an API key.
    Login(login::LoginArgs),

    /// Log out and clear saved credentials.
    Logout(logout::LogoutArgs),

    /// Check authentication and session status.
    Status(status::StatusArgs),

    /// Display the currently authenticated user and organization (alias for status).
    Whoami(status::StatusArgs),

    /// Manage organization contexts.
    #[command(subcommand, alias = "org")]
    Organization(org::OrgCommand),

    /// Manage local CLI configuration.
    Config(config_cmd::ConfigArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("telmoni=debug,info")
    } else {
        EnvFilter::new("telmoni=info,warn")
    };

    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .try_init();

    // ⚠ Never a path under the working directory, as for the config file: a
    // login there would leave the refresh token or the API key in whatever
    // checkout or CI workspace the CLI was run in.
    let Some(config_dir) = dirs::config_dir() else {
        eprintln!("no configuration directory to keep the credentials file in; set HOME");
        std::process::exit(1);
    };
    let creds_path = config_dir.join("telmoni").join("credentials.json");
    let store = CredentialsStore::new(creds_path);

    let transport = match ReqwestTransport::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("note: could not load config file: {e}");
            Config::default()
        }
    };
    // The environment is read here and nowhere else, so every library
    // function takes values and can be tested without touching it.
    let telmoni_org_env = env_var("TELMONI_ORGANIZATION").or_else(|| env_var("TELMONI_ORG"));
    let endpoint_env = env_var("TELMONI_ENDPOINT");

    let res = match cli.cmd {
        Cmd::Login(mut args) => {
            if args.key.is_none() {
                args.key = dotenv_var("TELMONI_API_KEY");
            }
            login::execute(args, &transport, &store, &config, endpoint_env).await
        }
        Cmd::Logout(args) => logout::execute(args, &transport, &store, telmoni_org_env).await,
        Cmd::Status(args) | Cmd::Whoami(args) => {
            status::execute(
                args,
                &transport,
                &store,
                &config,
                telmoni_org_env,
                endpoint_env,
            )
            .await
        }
        Cmd::Organization(cmd) => org::execute(cmd, &transport, &store).await,
        Cmd::Config(args) => config_cmd::execute(args),
    };

    if let Err(e) = res {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

/// A non-blank variable from the environment, else from `.env` in a debug build.
fn env_var(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| dotenv_var(key))
}

/// Unquotes a single or double-quoted value and strips surrounding whitespace.
#[cfg(debug_assertions)]
fn unquote(value: &str) -> &str {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
        .trim()
}

/// Parses a single `.env` line looking for `target_key`.
#[cfg(debug_assertions)]
fn parse_dotenv_line(line: &str, target_key: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, value) = trimmed.split_once('=')?;
    if key.trim() != target_key {
        return None;
    }
    let unquoted = unquote(value);
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}

/// `.env` in the working directory serves `cargo run` from the repository
/// root and is compiled out of release builds: a released binary run inside
/// someone else's checkout would otherwise take that checkout's endpoint and
/// API key without a word.
#[cfg(debug_assertions)]
fn dotenv_var(key: &str) -> Option<String> {
    let content = std::fs::read_to_string(".env").ok()?;
    content
        .lines()
        .find_map(|line| parse_dotenv_line(line, key))
}

#[cfg(not(debug_assertions))]
fn dotenv_var(_key: &str) -> Option<String> {
    None
}
