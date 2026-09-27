//! Telmoni CLI (`telmoni`).

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt};

use telmoni_cli::auth::CredentialsStore;
use telmoni_cli::commands::{config_cmd, login, logout, profile, status, team};
use telmoni_cli::config::{load_config, read_dotenv_var};
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

    #[arg(
        short = 'p',
        long,
        global = true,
        help = "Specify the configuration profile to use"
    )]
    profile: Option<String>,

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

    /// Display the currently authenticated user and team (alias for status).
    Whoami(status::StatusArgs),

    /// Manage team contexts.
    #[command(subcommand)]
    Team(team::TeamCommand),

    /// Manage local CLI configuration.
    Config(config_cmd::ConfigArgs),

    /// Manage connection profiles.
    #[command(subcommand)]
    Profile(profile::ProfileCommand),
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

    let config_dir = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let creds_path = config_dir.join("telmoni").join("credentials.json");
    let store = CredentialsStore::new(creds_path);

    let transport = match ReqwestTransport::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let config = load_config().unwrap_or_default();
    // The environment is read here and nowhere else, so every library
    // function takes values and can be tested without touching it.
    let telmoni_team_env = std::env::var("TELMONI_TEAM")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| read_dotenv_var("TELMONI_TEAM"));
    let endpoint_env = std::env::var("TELMONI_ENDPOINT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| read_dotenv_var("TELMONI_ENDPOINT"));

    let profile_name = telmoni_cli::config::active_profile(cli.profile.as_deref(), &config);

    let res = match cli.cmd {
        Cmd::Login(args) => {
            login::execute(
                args,
                &transport,
                &store,
                &config,
                endpoint_env,
                &profile_name,
            )
            .await
        }
        Cmd::Logout(args) => {
            logout::execute(args, &transport, &store, &profile_name, telmoni_team_env).await
        }
        Cmd::Status(args) | Cmd::Whoami(args) => {
            status::execute(
                args,
                &transport,
                &store,
                &config,
                telmoni_team_env,
                endpoint_env,
                &profile_name,
            )
            .await
        }
        Cmd::Team(cmd) => team::execute(cmd, &transport, &store, &profile_name).await,
        Cmd::Config(args) => config_cmd::execute(args),
        Cmd::Profile(cmd) => profile::execute(cmd, config),
    };

    if let Err(e) = res {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
