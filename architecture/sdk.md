# The SDKs

`sdk/` holds four client SDK scaffolds: Rust, TypeScript, Go and Python. Today each is **configuration only**. It holds an endpoint, an organization and a credential, and makes no HTTP calls. None is published, and the CLI does not use them.

That is deliberate. The platform's code is the contract, and an SDK grows a client only once the platform serves the lanes it would call (AGENTS.md). Today the platform's public API is two read lanes, `GET /v1/organization` and `GET /v1/members`, described in the core's generated `contract/openapi.json`. A client written ahead of its contract would freeze a guess.

## Contents

- [What each holds](#what-each-holds)
- [Defaults and the environment](#defaults-and-the-environment)
- [What the tests pin](#what-the-tests-pin)
- [Credentials in configuration](#credentials-in-configuration)
- [Building and testing](#building-and-testing)
- [Versions and publishing](#versions-and-publishing)
- [Growing a client](#growing-a-client)
- [Where it lives](#where-it-lives)

## What each holds

| SDK | Package | Configuration | Entry points |
|---|---|---|---|
| Rust | `telmoni-sdk` (`publish = false`) | `Config { endpoint, organization_id, auth_token }` | `Config::new`, `Client::new`, `from_env`, `with_config`, `config()`; `Telmoni` is an alias of `Client` |
| TypeScript | `telmoni` | `Config { endpoint?, apiKey?, organizationId? }` | `new Telmoni(config)`, `Telmoni.fromEnv()`, the read-only `config` |
| Go | `github.com/telmoni/telmoni-cli/sdk/go` | `Config{Endpoint, OrganizationID, AuthToken}` | `DefaultConfig`, `ConfigFromEnv`, `New`, `NewFromEnv`, `Config()`; `Telmoni` is an alias |
| Python | `telmoni` | `@dataclass Config(endpoint, api key, organization)` | `Telmoni(config=None)`, `Telmoni.from_env()` |

Their dependencies are minimal:
- **Rust:** serde only.
- **TypeScript:** no runtime dependencies. It builds to CommonJS and ESM with type declarations, using tsup.
- **Go:** the standard library only.
- **Python:** no runtime dependencies. It ships `py.typed`.

Each one strips a trailing `/` from the endpoint, so a request path can always be appended with a `/`.

## Defaults and the environment

Each SDK defaults to `https://telmoni.com`, the same base URL as the CLI's `DEFAULT_TELMONI_ENDPOINT` (`src/config.rs`). Each reads `TELMONI_ENDPOINT`, `TELMONI_API_KEY` and `TELMONI_ORG` from the environment, as the CLI does.

⚠ **An SDK takes the organization as an id.** The CLI also accepts a slug there, because it has a cached `/cli/me` to resolve one against. An SDK has no session, and holds whatever the variable says.
- **Nothing sends it today, and `/v1` could not take it.** `/v1` names no organization anywhere: a token is its organization, and the console's `/v1` relay forwards only `authorization`, `accept`, `content-type` and `content-length`. The field waits for a lane that takes one.
- **Where the platform does read `x-organization-id`** (the person lanes behind the `/cli` door), the door refuses a value that is not an organization id before auth sees it: `400` `/errors/bad-request`, "x-organization-id is not an organization id". Auth's `/me` alone would read such a value as absent and answer the person's own organization, so a slug sent there by mistake would have acted elsewhere.

They are not yet uniform at the edges:

| Behaviour | Rust | TypeScript | Go | Python |
|---|---|---|---|---|
| `TELMONI_API_URL` as a fallback endpoint | no | yes | yes | yes |
| `TELMONI_AUTH_TOKEN` as a fallback credential | no | yes | no | yes |
| An empty `TELMONI_ENDPOINT` falls back to the default | no | yes | yes | yes |
| An empty endpoint passed explicitly falls back to the default | no | yes | yes | no |

When an SDK grows a client, these should converge on the CLI's rules first. The CLI reads only the `TELMONI_*` variables named in [commands](commands.md#the-environment-at-the-edge), and treats an empty or whitespace endpoint as unset.

## What the tests pin

Each SDK has one contract test file:
- **Rust:** `sdk/rust/tests/contract.rs`.
- **TypeScript:** `sdk/typescript/tests/contract.test.ts`.
- **Go:** `sdk/go/client_test.go`, plus `example_test.go`.
- **Python:** `sdk/python/tests/test_contract.py`.

| Pinned | Rust | TypeScript | Go | Python |
|---|---|---|---|---|
| The default endpoint, as the constant and as the literal URL | yes | yes | the constant only | yes |
| A trailing `/` is stripped | yes | yes | — | yes |
| Building from the environment gives the default | yes | yes | yes (non-nil) | yes |

Not pinned anywhere:
- the environment variable names and their precedence (no test sets one);
- the organization field;
- the credential.

Go's example has no `// Output:` line, so it compiles but never runs. The TypeScript tests import the sources, not the built package, so its dual CommonJS and ESM exports go unexercised.

## Credentials in configuration

⚠ **Each SDK's configuration can print its credential.**
- In Rust, `Config` and `Client` derive `Debug`, and `Config` also derives `Serialize`, with the token included.
- Python's dataclass `repr` includes it.
- Go's struct carries a JSON tag on it.

AGENTS.md's rule is to never print a token or an API key. The CLI follows it. The SDKs will need a redacting `Debug`/`repr` and no serialized credential before they carry real keys.

## Building and testing

| SDK | Locally | In CI (`.github/workflows/ci.yml`, job `sdks`) |
|---|---|---|
| Rust | Part of the workspace, so `cargo xtask ci` covers it | The `gate` job |
| TypeScript | `npm test --prefix sdk/typescript`, or `npm run verify` there (typecheck, build, test) | `npm ci`, then `npm run verify` |
| Go | `go test ./...` in `sdk/go` | `go test -v ./...` |
| Python | `sdk/python/.venv/bin/pytest sdk/python` | `pip install -e ".[dev]"`, then `ruff check`, `ruff format --check`, `mypy`, `pytest` |

Only the Rust SDK is under the Rust gate's lints. Its `#![warn(missing_docs)]` becomes fatal there.

## Versions and publishing

- **Nothing is published.** Every SDK README says so, and the release workflow ships only the CLI (see [build](build.md#release)).
- **Versions differ.** The Rust crate follows the workspace version. The TypeScript and Python packages carry their own.
- **Go needs its own tags.** A Go module in a subdirectory is versioned by tags prefixed with its path (`sdk/go/v…`). The release workflow's `v*` trigger does not match those, so publishing the Go SDK will need its own tags.

## Growing a client

The order, when a lane exists to call:

1. **The platform serves it first.** It must be in `telmoni/telmoni`'s `V1_LANES`, which generates both the router and `contract/openapi.json`.
2. **Then the SDK grows a client for that lane**, from the platform's code. If what an SDK needs disagrees with the platform, report it there rather than work around it here.
3. **The client follows the CLI's transport rules** (see [transport](transport.md)):
   - one base URL;
   - a `User-Agent` naming the SDK;
   - errors read in the platform's three shapes;
   - no credential in any output.

## Where it lives

| Concern | File |
|---|---|
| Rust | `sdk/rust/src/lib.rs`, `sdk/rust/tests/contract.rs`, `sdk/rust/Cargo.toml` |
| TypeScript | `sdk/typescript/src/`, `sdk/typescript/tests/`, `sdk/typescript/package.json`, `sdk/typescript/tsup.config.ts` |
| Go | `sdk/go/client.go`, `sdk/go/client_test.go`, `sdk/go/go.mod` |
| Python | `sdk/python/src/telmoni/`, `sdk/python/tests/`, `sdk/python/pyproject.toml` |
| CI for the SDKs | `.github/workflows/ci.yml` (`sdks`) |
