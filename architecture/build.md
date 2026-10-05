# Build, release and CI

The CLI ships as one binary per platform. It is built by `cargo xtask dist`, published by a tag-triggered workflow, and installed by `install.sh`, which checks the archive's checksum. A single gate, `cargo xtask ci`, runs the same checks locally and in CI.

## Contents

- [The gate](#the-gate)
- [Tests](#tests)
- [Lints and policy](#lints-and-policy)
- [Packaging](#packaging)
- [install.sh](#installsh)
- [Release](#release)
- [CI](#ci)
- [Where it lives](#where-it-lives)

## The gate

`cargo xtask ci` (`xtask/src/main.rs`). The `xtask` alias lives in `.cargo/config.toml`. The `xtask` crate is never shipped, and depends only on `anyhow` and `clap`.

Each step runs from the workspace root. Its command is echoed, and the first failure stops the run:

1. `cargo fmt --all --check`
2. **Toolchain agreement.** The `rust-version` in `Cargo.toml` must equal the `channel` in `rust-toolchain.toml`, so a contributor cannot build with a different compiler than CI does.
3. `cargo clippy --workspace --all-targets --locked -- -D warnings`
4. `cargo build --workspace --locked`, with `RUSTFLAGS="-D warnings"`
5. `cargo test --workspace --locked`, with `RUSTFLAGS="-D warnings"`
6. `cargo doc --no-deps --workspace --locked`, with `RUSTDOCFLAGS="-D warnings"`
7. `cargo deny --locked check`: advisories, bans, licenses and sources

Why it is built this way:
- ⚠ **`--locked` on every step that resolves dependencies** (fmt alone takes no such flag). The gate runs against the lockfile as committed. A build that would quietly resolve new versions fails instead. Any dependency change is the user's to approve (AGENTS.md).
- **`--workspace` matters.** `default-members` is the CLI alone, so without it the gate would skip `xtask` and the Rust SDK.
- **What the gate does not cover.** The TypeScript, Go and Python SDKs are outside it. CI runs them separately (see [CI](#ci)).

## Tests

`tests/auth_tests.rs` is the CLI's suite. It holds mock-based tests, with no network and no sleeps.

- **HTTP is injected** through the `Transport` trait (see [transport](transport.md)). `MockTransport` answers from a queue of scripted responses and records every request. An unexpected request is an error, and "no network" is asserted as zero requests made. The three tests that build the real transport only check that it refuses plain HTTP, which it does before opening a connection.
- **The poll loop's sleep and clock are injected as closures.** A device-flow poll runs in microseconds, and the tests record the durations it asked to sleep. The wall clock is not injected: refresh decisions read the real time, and tests set expiries relative to it.
- **The credentials path is a value**, so tests write under `std::env::temp_dir()`, never to the person's real configuration directory.
- **The environment is passed in as arguments**, never set.

**Not covered by tests:**
- `main.rs`;
- the configuration file, whose path resolves to the real directory;
- the interactive `login`, which opens a browser and sleeps for real;
- logout under an API key;
- a transport failure. The mock can inject one, but no test does.

Each SDK has its own contract tests (see [SDKs](sdk.md#what-the-tests-pin)).

## Lints and policy

**Workspace lints** (`Cargo.toml`):
- `unsafe_code` and `unreachable_pub` are **denied**.
- Clippy's `all` group is on. `unwrap_used`, `expect_used`, `panic`, `indexing_slicing` and `string_slice` are **denied**.
- A further set warns, which the gate's `-D warnings` makes fatal. It includes:
  - the cast lints;
  - `todo` and `unimplemented`;
  - `dbg_macro`;
  - `allow_attributes`, and `allow_attributes_without_reason`.

  An exception is therefore written `#[expect(lint, reason = "…")]`, which fails once it is no longer needed.
- `clippy.toml` allows `unwrap`, `expect`, `panic`, indexing and `dbg!` in tests.
- `#![forbid(unsafe_code)]` heads the CLI's library and binary, and `xtask`.

**Supply chain** (`deny.toml`):
- The dependency graph is judged for the two targets this project builds: `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin`.
- Yanked crates are denied, and no advisory is ignored.
- Licenses are an allowlist, read with a confidence threshold.
- Duplicate versions warn.
- Wildcard versions are denied, except for paths.
- Unknown registries and git sources are denied.
- Some async runtimes are banned; the CLI runs on tokio.

**Toolchain.** `rust-toolchain.toml` pins the compiler, with rustfmt and clippy. The workspace uses edition 2024 and resolver 3.

## Packaging

`cargo xtask dist` (`xtask/src/dist.rs`) packages the binary for the machine it runs on:
- **The build.** `cargo build --package telmoni-cli --bin telmoni --release`.
  - The release profile uses thin LTO, one codegen unit, and stripped symbols.
  - It does not pass `--locked`.
- **Where it looks.** It expects the binary at `<workspace>/target/release/telmoni`. A custom `CARGO_TARGET_DIR` is not followed.
- **The archive.** `dist/telmoni-<os>-<arch>.tar.gz`, named from the host's `std::env::consts`, for example `telmoni-macos-aarch64` or `telmoni-linux-x86_64`. The archive holds the bare `telmoni` binary at its root.
- **The checksum.** `dist/SHA256SUMS` holds `<sha256>  <archive>`, written with `shasum -a 256` or `sha256sum`, and replaced on each run.

There is no cross-compilation. Each platform is built natively by the release workflow.

## install.sh

`install.sh` is POSIX `sh`, run with `set -eu`, and is installed by `curl … | sh`:

1. **Detect the platform.** `uname -s` and `uname -m` are mapped to an archive name. Only `macos-aarch64` and `linux-x86_64` are built. Any other platform gets a message to build from source.
2. **Preflight.** It needs `curl`, `tar`, and `sha256sum` or `shasum`.
3. **Choose where to install:**
   - `TELMONI_INSTALL_DIR` if set;
   - otherwise `/usr/local/bin`, when writable or running as root;
   - otherwise `$HOME/.local/bin`.
4. **Choose the version.** `TELMONI_VERSION` pins a release. Without it, the script asks GitHub's API for the latest release, which skips prereleases.
5. **Download and verify.** The archive and `SHA256SUMS` are fetched into a temporary directory, which is removed on exit or interrupt. The archive's hash must match its line in `SHA256SUMS`; a mismatch or a missing line stops the install.
6. **Install.** It extracts, moves the binary into place and sets mode `0755`.
   - When the target directory is not writable, it escalates through `sudo` or `doas`.
   - It warns if the directory is not on `PATH`.

`TELMONI_INSTALL_DRY_RUN=1` prints the commands instead of running them.

**The checksum file comes from the same release as the archive.** It catches a corrupted download, not a compromised release. The archive's provenance is what catches that, and `install.sh` does not check it: verifying needs `gh`, which a `curl … | sh` host may not have (see [Release](#release)).

## Release

`.github/workflows/release.yml` runs on a pushed `v*` tag:

1. **`check-version`.**
   - The tag, without `v` and without any `-suffix`, must equal the workspace `version` in `Cargo.toml`.
   - The tag must be annotated, because the release notes are taken from the annotation.
2. **`dist-macos` and `dist-linux`.** Each runs natively on its own runner, `macos-15` and `ubuntu-24.04`. Each runs `cargo xtask dist` and uploads its archive and `SHA256SUMS`. The release path does not run the gate itself; the gate already ran on the commit, in CI.
3. **`publish`**, the only job with write permission:
   - It merges the two checksum files and adds `install.sh` and its hash.
   - It attests the two archives and `install.sh` (`actions/attest`): SLSA build provenance, signed through Sigstore for this workflow's OIDC identity and stored with the repository's attestations. The job's `id-token` and `attestations` permissions are for that alone.
   - It creates the GitHub release from the tag (`--verify-tag --notes-from-tag`). A tag containing `-` is marked a prerelease.
   - Its assets are the two archives, `SHA256SUMS` and `install.sh`.

**Checking a release.** `gh attestation verify <file> --repo telmoni/telmoni-cli` checks that the file was built by this repository's workflows; `--signer-workflow telmoni/telmoni-cli/.github/workflows/release.yml` narrows that to the release workflow. ⚠ Unlike the checksum, which sits beside the archive in the release, the provenance is signed for the workflow run that built the file: an asset swapped into the release afterwards has none. No key is kept anywhere: the signing certificate is minted per run. There is no GPG or cosign signature besides.

## CI

**`ci.yml`** runs on pushes to `main` and on every pull request, with read-only permissions. A newer run on the same ref cancels an older one.

| Job | Does |
|---|---|
| `gate` | `cargo xtask ci` on `ubuntu-24.04` and `macos-15`, the two platforms the CLI ships for. `cargo-deny` is installed per run. |
| `sdks` | The TypeScript, Go and Python SDKs' own checks, on Linux (see [SDKs](sdk.md#building-and-testing)) |
| `lint-workflows` | `actionlint`, downloaded at a pinned version and checked by hash |
| `ci-status` | The one required check. It asserts every needed job succeeded or was skipped. |

**Other automation:**
- **`audit.yml`** runs `cargo deny check advisories` daily against the committed lockfile. It covers Rust only.
- **`dependabot.yml`** keeps GitHub Actions current, weekly, as one grouped pull request. It does not touch the dependencies themselves; those are the user's to approve.

Actions are pinned by major version tag.

## Where it lives

| Concern | File |
|---|---|
| The gate and packaging | `xtask/src/main.rs`, `xtask/src/dist.rs`, `.cargo/config.toml` |
| The CLI's tests | `tests/auth_tests.rs` |
| Lints, profiles, workspace | `Cargo.toml`, `clippy.toml` |
| Supply-chain policy | `deny.toml` |
| Toolchain | `rust-toolchain.toml` |
| Installer | `install.sh` |
| Release, CI, audit | `.github/workflows/{release,ci,audit}.yml`, `.github/dependabot.yml`, `.github/actionlint.yaml` |
