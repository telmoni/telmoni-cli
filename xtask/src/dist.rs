//! Packaging step for telmoni CLI releases.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{cargo, target_dir, workspace_root};

const BINARY_NAME: &str = "telmoni";

/// The third-party notices beside the binary in the archive.
const NOTICES: &str = "THIRD_PARTY_LICENSES.txt";

/// Packages the CLI release for `target`, else this host's target. The
/// archive keeps the host's OS and architecture in its name, so a musl build
/// takes the name `install.sh` asks for.
pub(crate) fn dist(target: Option<&str>) -> Result<()> {
    let mut args = vec![
        "build",
        "--locked",
        "--package",
        "telmoni-cli",
        "--bin",
        BINARY_NAME,
        "--release",
    ];
    args.extend(target.iter().flat_map(|triple| ["--target", *triple]));
    cargo(&args)?;
    let release_dir = match target {
        Some(triple) => target_dir().join(triple).join("release"),
        None => target_dir().join("release"),
    };
    let root = workspace_root();
    let dist_dir = root.join("dist");

    std::fs::create_dir_all(&dist_dir)
        .with_context(|| format!("creating {}", dist_dir.display()))?;

    let binary = release_dir.join(BINARY_NAME);
    if !binary.is_file() {
        bail!("expected binary at {}", binary.display());
    }

    // The linked crates' notices, which several of their licences ask to travel
    // with a copy: `about.toml` and `about.hbs`, from Cargo.lock.
    let notices = release_dir.join(NOTICES);
    cargo(&[
        "about",
        "generate",
        "--locked",
        "--fail",
        "about.hbs",
        "--output-file",
        notices.to_str().unwrap_or_default(),
    ])
    .context("cargo-about writes the third-party notices: cargo install --locked cargo-about --features cli")?;

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let archive_name = format!("telmoni-{os}-{arch}.tar.gz");
    let archive_path = dist_dir.join(&archive_name);

    // Apache-2.0 § 4 asks a redistribution to carry the licence and the notice.
    let mut cmd = Command::new("tar");
    cmd.current_dir(&release_dir);
    cmd.args([
        "-czf",
        archive_path.to_str().unwrap_or_default(),
        BINARY_NAME,
        NOTICES,
        "-C",
        root.to_str().unwrap_or_default(),
        "LICENSE",
        "NOTICE",
    ]);
    let status = cmd.status().context("creating tar archive")?;
    if !status.success() {
        bail!("tar exited with {}", status);
    }

    // Compute sha256
    let sha = sha256_file(&archive_path)?;
    let sha_entry = format!("{sha}  {archive_name}\n");
    let sums_path = dist_dir.join("SHA256SUMS");
    std::fs::write(&sums_path, sha_entry).context("writing SHA256SUMS")?;

    println!("wrote {}", archive_path.display());
    println!("wrote {}", sums_path.display());
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let commands = [("shasum", &["-a", "256"][..]), ("sha256sum", &[][..])];
    for (prog, flags) in commands {
        let mut cmd = Command::new(prog);
        cmd.args(flags);
        cmd.arg(path);
        if let Ok(out) = cmd.output() {
            if !out.status.success() {
                continue;
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Some(hash) = stdout.split_whitespace().next() {
                return Ok(hash.to_string());
            }
        }
    }
    bail!("failed to compute sha256 using shasum or sha256sum");
}
