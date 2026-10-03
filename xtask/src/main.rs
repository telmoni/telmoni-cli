//! `cargo xtask <cmd>`, dev orchestration for the telmoni CLI workspace.

#![forbid(unsafe_code)]

mod dist;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "xtask",
    version,
    about = "Dev orchestration for the telmoni CLI workspace",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Host-safe gate: fmt · toolchain = `rust-version` · clippy `-D warnings` ·
    /// build · test · docs · cargo-deny.
    Ci,
    /// Package this host's release under `dist/`.
    Dist,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Ci => ci(),
        Cmd::Dist => dist::dist(),
    }
}

/// The host-safe gate. `--locked` on every step that resolves dependencies;
/// fmt alone takes no such flag.
fn ci() -> Result<()> {
    cargo(&["fmt", "--all", "--check"])?;
    toolchain_msrv_agree(&workspace_root())?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ])?;
    cargo_env(
        &["build", "--workspace", "--locked"],
        &[("RUSTFLAGS", "-D warnings")],
    )?;
    cargo_env(
        &["test", "--workspace", "--locked"],
        &[("RUSTFLAGS", "-D warnings")],
    )?;
    cargo_env(
        &["doc", "--no-deps", "--workspace", "--locked"],
        &[("RUSTDOCFLAGS", "-D warnings")],
    )?;
    cargo(&["deny", "--locked", "check"])?;
    println!("✓ ci: gate passed");
    Ok(())
}

/// The absolute path to the repository root.
pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| std::process::exit(1))
        .to_path_buf()
}

pub(crate) fn target_dir() -> PathBuf {
    workspace_root().join("target")
}

pub(crate) fn cargo(args: &[&str]) -> Result<()> {
    cargo_env(args, &[])
}

pub(crate) fn cargo_env(args: &[&str], envs: &[(&str, &str)]) -> Result<()> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()));
    cmd.args(args).current_dir(workspace_root());
    for (k, v) in envs {
        cmd.env(k, v);
    }

    let mut print = vec!["cargo"];
    print.extend(args.iter().copied());
    println!("$ {}", print.join(" "));

    let status = cmd.status().context("running cargo")?;
    if !status.success() {
        bail!("`cargo {}` exited with {}", args.join(" "), status);
    }
    Ok(())
}

fn toolchain_msrv_agree(root: &Path) -> Result<()> {
    let manifest_msrv = std::fs::read_to_string(root.join("Cargo.toml"))?
        .lines()
        .find(|l| l.contains("rust-version") && l.contains('"'))
        .and_then(|l| l.split('"').nth(1))
        .map(String::from)
        .context("no rust-version in Cargo.toml")?;
    let toolchain_ch = std::fs::read_to_string(root.join("rust-toolchain.toml"))?
        .lines()
        .find(|l| l.contains("channel") && l.contains('"'))
        .and_then(|l| l.split('"').nth(1))
        .map(String::from)
        .context("no channel in rust-toolchain.toml")?;
    if manifest_msrv != toolchain_ch {
        bail!("rust-version \"{manifest_msrv}\" != toolchain \"{toolchain_ch}\"");
    }
    Ok(())
}
