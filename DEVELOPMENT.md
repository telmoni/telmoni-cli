# Telmoni CLI & SDK Development Guide

How to set up, run and check the CLI and SDKs locally. How they are built is described in [architecture/](architecture/README.md): commands, sign-in, transport, the SDKs, and build and release.

---

## Environment Variables

| Variable | Description |
|---|---|
| `TELMONI_ENDPOINT` | Telmoni endpoint URL (default: `https://telmoni.com`) |
| `TELMONI_API_KEY` | An API key for `telmoni login`; the CLI keeps it in the credentials file |
| `TELMONI_ORG` | Organization override for `status`, `whoami`, and `logout`: its ID, or its slug when typed by hand. Scripts carry the ID; the slug moves (a first name that reads as a slug replaces the placeholder, and after that only a URL change moves it). |

A debug build (`cargo run`) also reads these from `.env` in the working directory; release builds never read `.env`.

---

## Development Workflows

### Prerequisites

- Rust 1.98.1 (pinned in `rust-toolchain.toml`; `rustup` installs it automatically)
- Node.js 22.12+, 24 or 26+ & npm (for the TypeScript SDK: the locked vitest's engines, which leave out 23 and 25)
- Go 1.22+ (for Go SDK)
- Python 3.9+ (for the Python SDK; `python3 -m venv sdk/python/.venv && sdk/python/.venv/bin/pip install -e "sdk/python[dev]"` makes the `.venv` the test commands name)
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
7. `cargo deny --locked check`

### Building Releases

```console
cargo xtask dist
```

Builds the binary for this machine and writes its tarball and `SHA256SUMS` into `dist/`. The release workflow runs it on each platform; see [architecture/build.md](architecture/build.md).
