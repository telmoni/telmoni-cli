# Telmoni CLI & SDK Architecture & Development Guide

This document describes the internal architecture of the Telmoni CLI and multi-language SDKs, design principles, authentication lifecycle, and development workflows.

---

## Architecture Overview

Telmoni CLI is designed as an ergonomic, host-safe command-line tool written in modern Rust, backed by a unified workspace managing:

1. **`telmoni-cli` (`src/`)**: The main CLI binary and library.
2. **`telmoni-sdk` (`sdk/rust/`)**: The official Rust SDK client.
3. **`xtask` (`xtask/`)**: Dev orchestration tooling (CI gate, release packaging, distribution).
4. **Client SDKs (`sdk/`)**: Idiomatic SDKs for TypeScript, Go, Python, and Rust.

---

## Authentication Architecture

Telmoni supports two primary authentication modes:

### 1. Device Authorization Grant (RFC 8628, Interactive Sign-In)

- **Standard**: RFC 8628 (OAuth 2.0 Device Authorization Grant) through Telmoni's `/cli` door.
- **Door Lanes**:
  - `POST /cli/auth/device`: Requests a device authorization. Telmoni returns a `deviceCode`, a `userCode` (e.g. `ABCD-EFGH`), `verificationUri`, `verificationUriComplete`, `expiresIn`, and poll `interval`.
  - The CLI prints the code and verification URL to stdout (and opens `verificationUriComplete` if a browser is available). The person can approve the code from any browser on any machine.
  - `POST /cli/auth/device/poll`: The CLI polls with `{ "deviceCode": ... }` every `interval` seconds until approved (200 `AuthnResult`), pending (202), `slow_down` (202, adds 5 seconds to interval), rate-limited (429), expired (400), or denied (403).
  - `POST /cli/me`: Once granted, the CLI fetches the user profile and organization list with `Authorization: Bearer <accessToken>`.
- **Session Management**:
  - CLI sessions appear on the user's Active sessions page at `telmoni.com` (labelled e.g. "Telmoni CLI (macOS)" or "Telmoni CLI (Linux)").
  - Ending the session from the Active sessions page at telmoni.com ends the CLI's session immediately; the next request receives a 401, clears credentials, and requires logging in again.
- **Refresh**:
  - Token refresh occurs through `POST /cli/auth/refresh` sending `{ "refreshToken": ..., "sessionRowId": ... }`.
- **Sign-Out (`telmoni logout`)**:
  - Revokes the session row via `POST /cli/sessions/{sessionRowId}/revoke` with `Authorization: Bearer <accessToken>` and `x-organization-id`, then removes the credentials file.
- **Credentials Storage**:
  - Saved at `~/Library/Application Support/telmoni/credentials.json` on macOS and `~/.config/telmoni/credentials.json` on Linux with `0600` permissions. Credentials are not encrypted.

### 2. API Key Authentication (Programmatic / CI)

- For automated environments, CI/CD runners, and headless servers, users can authenticate directly via:
  ```console
  telmoni login --key telmoni_xxxxxxxxxxxx
  ```
- Or set `TELMONI_API_KEY=telmoni_...` in the environment. A debug build (`cargo run`) also reads it from `.env` in the working directory; release builds never read `.env`.
- API keys communicate with `{endpoint}/v1` and never use `/cli`.

---

## Environment Variables

| Variable | Description |
|---|---|
| `TELMONI_ENDPOINT` | Telmoni endpoint URL (default: `https://telmoni.com`) |
| `TELMONI_API_KEY` | Direct API key for CLI operations / CI |
| `TELMONI_ORG` | Organization ID override for `status`, `whoami`, and `logout` |

---

## Development Workflows

### Prerequisites

- Rust 1.98.1 (pinned in `rust-toolchain.toml`; `rustup` installs it automatically)
- Node.js 20+ & npm (for TypeScript SDK)
- Go 1.22+ (for Go SDK)
- Python 3.11+ (for Python SDK)
- `cargo-deny` (`cargo install cargo-deny`)

### Running the CI Gate

```console
cargo xtask ci
```

This runs:
1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets --locked -- -D warnings`
3. `cargo build --workspace --locked`
4. `cargo test --workspace --locked`
5. `cargo doc --no-deps --workspace --locked`
6. `cargo deny check`

### Building Releases

```console
cargo xtask dist
```

Outputs release tarballs and `SHA256SUMS` directly into `dist/`.
