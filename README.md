# Telmoni CLI & Multi-Language SDKs

[![CI](https://github.com/telmoni/telmoni-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/telmoni/telmoni-cli/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

The official command-line interface (`telmoni`) and client SDKs for Telmoni.

---

## Installation

### Shell Installer (macOS & Linux)

```console
curl -fsSL https://raw.githubusercontent.com/telmoni/telmoni-cli/main/install.sh | sh
```

Pin a specific release or override the install directory:

```console
curl -fsSL https://raw.githubusercontent.com/telmoni/telmoni-cli/main/install.sh | TELMONI_VERSION=0.0.1 sh
```

### From Source (Rust toolchain required)

```console
git clone https://github.com/telmoni/telmoni-cli.git
cd telmoni-cli
cargo install --path . --locked
```

---

## CLI Usage

### 1. Authentication

#### Interactive Login (Device Authorization Grant, RFC 8628)

Authenticate via device-code login through Telmoni's `/cli` door, signing in like the GitHub and Stripe CLIs:

```console
telmoni login
```

The CLI prints a one-time verification code and URL. Approve the code in any browser on any machine:

```
First copy your one-time code: ABCD-EFGH
Then open https://telmoni.com/device and enter it.
```

Your session appears on the Active sessions page at `telmoni.com` (labelled e.g. "Telmoni CLI (macOS)"). Ending the session on the Active sessions page at telmoni.com ends the CLI's session.

Credentials are saved with `0600` permissions at `~/Library/Application Support/telmoni/credentials.json` on macOS and `~/.config/telmoni/credentials.json` on Linux. Credentials are not encrypted.

#### Programmatic / CI Login (API Key)

For automated environments, CI/CD runners, and headless servers, authenticate using a Telmoni API key:

```console
telmoni login --key telmoni_your_api_key_here
```

### 2. Verify Session Status

```console
telmoni status
# Or alias:
telmoni whoami
```

Output formatted as JSON:

```console
telmoni status --json
```

### 3. Organizations

List organizations your account belongs to:

```console
telmoni org list
```

Switch your active organization context, by its ID or its slug (the first segment of its console URL):

```console
telmoni org switch org_xxxxxxxxxxxx
telmoni org switch acme-robotics
```

You can also temporarily override the organization context for a single command invocation with `TELMONI_ORG=org_...`, or with its slug. A script should carry the ID: a rename moves the slug, and the CLI resolves a slug against the organizations it cached at its last `status` or `org switch`, without asking the server.

### 4. Log Out

```console
telmoni logout
```

Revokes your CLI session row on the server and deletes the local credentials file.

### 5. Configuration

Configure CLI defaults such as the Telmoni endpoint:

```console
# List current configuration
telmoni config list

# Set a configuration value
telmoni config set endpoint https://telmoni.com

# Get a configuration value
telmoni config get endpoint
```

#### Supported Configuration Keys

| Key | Description | Default |
|---|---|---|
| `endpoint` | Telmoni endpoint URL | `https://telmoni.com` |

#### Environment Variables

| Variable | Description |
|---|---|
| `TELMONI_ENDPOINT` | Telmoni endpoint URL (default: `https://telmoni.com`) |
| `TELMONI_API_KEY` | Direct API key for CLI operations / CI |
| `TELMONI_ORG` | Organization override for `status`, `whoami`, and `logout`: its ID, or its slug when typed by hand. Scripts carry the ID; a rename moves the slug. |

---

## Repository Layout

```text
├── architecture/   # How the CLI and SDKs are built, and why
├── src/            # Telmoni CLI source code (telmoni-cli crate)
├── xtask/          # Dev orchestration (CI gate, release packaging)
├── sdk/            # Multi-language client SDKs
│   ├── go/         # Go SDK
│   ├── python/     # Python SDK
│   ├── rust/       # Rust SDK (telmoni-sdk crate)
│   └── typescript/ # TypeScript / JavaScript SDK
└── tests/          # Integration & contract tests
```

---

## Multi-Language SDKs

The SDKs are scaffolds: configuration (endpoint, API key, organization) and nothing that calls the platform yet. Each grows a client once its contract exists.

| Language | Directory | Package name |
|---|---|---|
| **TypeScript / JS** | [`sdk/typescript`](sdk/typescript/) | `telmoni` |
| **Go** | [`sdk/go`](sdk/go/) | `github.com/telmoni/telmoni-cli/sdk/go` |
| **Python** | [`sdk/python`](sdk/python/) | `telmoni` |
| **Rust** | [`sdk/rust`](sdk/rust/) | `telmoni-sdk` |

---

## Development & Testing

Run the full local gate (formatting, strict clippy, tests, docs, dependency check):

```console
cargo xtask ci
```

Run tests across all SDKs:

```console
# TypeScript SDK
cd sdk/typescript && npm run verify

# Go SDK
cd sdk/go && go test -v ./...

# Python SDK
sdk/python/.venv/bin/pytest sdk/python
```

For setting up and working on the code, see [DEVELOPMENT.md](DEVELOPMENT.md); for how it is built, see [architecture/](architecture/README.md).

---

## Security

Please report vulnerabilities following our [Security Policy](SECURITY.md). Do not open public issues for security vulnerabilities.

---

## Community & License

- [Contributing](CONTRIBUTING.md) — DCO requirements, code standards, and PR workflows.
- [Code of Conduct](CODE_OF_CONDUCT.md) — Contributor Covenant v2.1.
- [Architecture](architecture/README.md) — How the CLI and SDKs are built, and why.
- [Development Guide](DEVELOPMENT.md) — Setting up, running and checking the code.
- [Security Policy](SECURITY.md) — Vulnerability reporting channels and safe harbor.
- [License](LICENSE) — Licensed under the Apache License, Version 2.0.
