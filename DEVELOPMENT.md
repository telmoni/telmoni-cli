# Telmoni CLI & SDK Development Guide

How to set up, run and check the CLI and SDKs locally. How they are built is described in [architecture/](architecture/README.md): commands, sign-in, transport, the SDKs, and build and release.

---

## Environment Variables

| Variable | Description |
|---|---|
| `TELMONI_ENDPOINT` | Telmoni endpoint URL (default: `https://telmoni.com`) |
| `TELMONI_API_KEY` | An API key for `telmoni login`; the CLI keeps it in the credentials file |
| `TELMONI_ORG` | Organization override for `status`, `whoami`, and `logout`: its ID, or its slug when typed by hand. Scripts carry the ID; a rename moves the slug. |

A debug build (`cargo run`) also reads these from `.env` in the working directory; release builds never read `.env`.

---

## Development Workflows

### Prerequisites

- Rust 1.98.1 (pinned in `rust-toolchain.toml`; `rustup` installs it automatically)
- Node.js 22.12 or later & npm (for the TypeScript SDK; its locked test tooling requires it)
- Go 1.22+ (for Go SDK)
- Python 3.9+ (for Python SDK)
- `cargo-deny` (`cargo install cargo-deny`)

### Running the CI Gate

```console
cargo xtask ci
```

This runs:
1. `cargo fmt --all --check`
2. a check that `rust-version` in `Cargo.toml` matches `rust-toolchain.toml`
3. `cargo clippy --workspace --all-targets --locked -- -D warnings`
4. `cargo build --workspace --locked` (with `RUSTFLAGS="-D warnings"`)
5. `cargo test --workspace --locked` (with `RUSTFLAGS="-D warnings"`)
6. `cargo doc --no-deps --workspace --locked` (with `RUSTDOCFLAGS="-D warnings"`)
7. `cargo deny check`

### Building Releases

```console
cargo xtask dist
```

Builds the binary for this machine and writes its tarball and `SHA256SUMS` into `dist/`. The release workflow runs it on each platform; see [architecture/build.md](architecture/build.md).
