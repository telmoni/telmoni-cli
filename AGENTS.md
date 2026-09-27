# Telmoni CLI — Agent Guidelines

These are the ground rules for modifying this repository. `CLAUDE.md` and
`.github/copilot-instructions.md` defer here.

**Stack:** Rust workspace (`src/`) for the `telmoni` CLI and client SDK scaffolds (`sdk/`). It is a **client of Telmoni and of nothing else**. The platform it talks to lives in a separate repository, `~/Desktop/telmoni`; nothing in this tree runs on a server, holds a database, or verifies a token. Every request goes to one base URL, the Telmoni endpoint, which defaults to `https://telmoni.com` and is overridden by `TELMONI_ENDPOINT` (for local work, the console dev server at `http://localhost:3000`). No other hostname appears anywhere in this tree.

## 🚧 Status & Ground Rules
- **Client of Telmoni Only:** Nothing in this tree runs on a server, holds a database, or verifies a token. Every request goes to the Telmoni endpoint.
- **Stateless Machines:** The CLI runs on stateless machines: SSH sessions, containers, CI runners, hosts with no browser. Nothing in it may depend on a browser reaching the machine the CLI runs on, on a listening port, or on state that outlives the credentials file.
- **Ask First (External/Deps):** No new dependencies without asking Kendrick first. Nothing in the current scope needs one. What is added must pass `cargo deny check`.
- **Quality Gate Unchanged:** `cargo xtask ci` must remain 100% green at all times. Never leave the tree broken.
- **Do Not Commit, Branch or Push:** Kendrick reviews and commits. Never create branches or run `git push`.

## 🗺️ Where Things Live
| Path | What |
|---|---|
| `src/main.rs` | CLI entry point, argument parsing via `clap`, and top-level environment variable reads. |
| `src/lib.rs` | Library root re-exporting authentication, configuration, commands, and transport modules. |
| `src/auth/` | Authentication implementations: RFC 8628 device flow (`device.rs`) and storage (`storage.rs`). |
| `src/commands/` | CLI command handlers: `login`, `logout`, `status` (`whoami`), `org`, `config`. |
| `src/client.rs` | Public `/v1` client operations for static API tokens (`telmoni_...`). |
| `src/config.rs` | Local configuration loading (`~/.config/telmoni/config.toml` or OS equivalent). |
| `src/transport.rs` | Transport trait, `ReqwestTransport`, error response parser, and User-Agent builder. |
| `sdk/` | Client SDK scaffolds (`rust/`, `typescript/`, `go/`, `python/`). Pure configuration structs with no HTTP calls. |
| `tests/` | Mock-based unit and integration test suites (`auth_tests.rs`). Zero network, zero sleeps. |
| `xtask/` | Workspace CI and automation task runner (`cargo xtask ci`). |

## 🛠️ Verification & Commands
- **The Gate (`cargo xtask ci`):** Must pass before reporting any task done. Every step runs with `--locked`:
  - Format: `cargo fmt --all -- --check`
  - Lints: `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - Build: `cargo build --workspace --locked`
  - Tests: `cargo test --workspace --locked`
  - Documentation: `cargo doc --no-deps --workspace --locked`
  - Dependency & License audits: `cargo deny check`
- **Fast Local Checks:** `cargo check --workspace`, `cargo fmt --all`, `cargo test`.
- **SDK Scaffolds Verification:**
  - TypeScript: `npm test --prefix sdk/typescript`
  - Go: `go test ./...` in `sdk/go`
  - Python: `pytest sdk/python` (using `sdk/python/.venv/bin/pytest`)

## 🏗️ Architecture & Code Rules
- **No Panics:** Zero panics in production code. Clippy denies `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, and `unreachable` at the workspace level. Use `Result` and `Option` with `anyhow` or a typed error. Tests may `unwrap` and `expect`; production code may not.
- **No Unsafe Code:** `#![forbid(unsafe_code)]` on every crate.
- **Test Isolation (No Network, No Real Sleeps):** HTTP calls, sleeps, and the clock are injected as closures or a small trait (`Transport`). The credentials path is a value (`CredentialsStore { path }`) so tests write under `std::env::temp_dir()`, never touching the real credentials file.
- **Environment Variables:** Library code never reads `std::env`; commands read the environment at their entry point and pass values down. (Edition 2024 makes `set_var` unsafe, and unsafe is forbidden, so env-reading library code cannot be tested.)
- **Output & Logging Hygiene:** Do not write to `stdout` except for the command's own output. `println!` is correct for a CLI; diagnostics go to `stderr` or `tracing`.
- **Agent Hygiene:** No obvious comments (document *why*, never *what*). No AI signatures in code or commits. Edit files deliberately without bulk script regexes.

## 🔐 Authentication & Wire Contract
**The contract is `~/Desktop/telmoni/docs/feature-cli.md`, and it is authoritative.** Read it before touching `src/auth/` or `src/client.rs`. If the platform does not behave as that page says, report the disagreement; the server side is fixed in the platform repository, never worked around here.

- **Zero WorkOS Direct Exposure:** The CLI never talks to WorkOS. It holds no WorkOS client id, no WorkOS URL, and no secret. Telmoni handles authentication behind its own `/cli` door, which is the CLI's sole authentication surface.
- **Interactive Sign-In (RFC 8628 Device Flow):**
  - Calls `POST /cli/auth/device` to obtain a short code and verification URL.
  - Prints both (and opens the complete URL when a browser is available).
  - Polls `POST /cli/auth/device/poll` every `interval` seconds until HTTP 200.
  - HTTP 202 is "not yet"; on `slow_down` add 5 seconds to the interval; HTTP 403 is denied and HTTP 400 is expired, both of which terminate the attempt.
  - Calls `POST /cli/me` upon successful authorization.
- **No Redirects / No Listener / No PKCE:** There is no loopback redirect, no local listener port, and no PKCE. Do not reintroduce them.
- **Token Refresh:** Refresh is `POST /cli/auth/refresh` sending `{ refreshToken, sessionRowId }`, never a call to the provider.
- **Sign-Out & Revocation:** Calls `POST /cli/sessions/{sessionRowId}/revoke` with the active team context (`x-team-id` / `x-team-id`), which the server requires of anyone who belongs to an organization. Then the credentials file is deleted whatever the answer.
- **Session Termination Invariants (HTTP 401):**
  - A 401 on `/cli/me`, refresh, or revoke means the session was ended (from the web console's Active Sessions page, by account deletion, or by expiry). Delete the credentials file, print "run `telmoni login`", and do not retry.
  - The one exception is an RFC 9457 problem typed `/errors/auth/token-expired`, which a single refresh cures.
  - A 401 on the device code poll lane means the device code is dead.
  - A 401 under an API key on `/v1` is `{ "error": … }` and leaves the credentials file alone.
  - A 503 or 5xx server error never deletes credentials.
- **Nullable Fields as Option:** Every field the server marks nullable is an `Option`: `email`, `firstName`, `lastName`, `refreshToken`, `authMethod`, `displayName`, `ownerEmail`, `ownerDisplayName`, `sessionRowId`, `verificationUriComplete`.
- **Team Label Resolution:** `name` when non-empty after trimming, else `ownerEmail` when non-null, else the literal `"Team"`.
- **Error Parser (Three Shapes):**
  - Shape 1: RFC 9457 `application/problem+json` (`{ type, title, status, detail }`) — prints `title: detail`.
  - Shape 2: `{ "error": string }`.
  - Shape 3: Raw fallback `request failed (<status>)` plus the first 200 characters of the body.
  - Never print a token, a refresh token, a device code, or an API key in any error or status message. The user code is the only code displayed.
- **Standardized User-Agent:** Every request carries `User-Agent: telmoni-cli/<version> (<os>; <arch>)` built from `CARGO_PKG_VERSION`, `std::env::consts::OS`, and `std::env::consts::ARCH`. The server labels the person's session from this string.
- **Tenant Context Headers:** Dual-dispatched via `x-team-id` and `x-team-id` on every bearer lane, taken from what `/cli/me` answered or what `TELMONI_TEAM` / `TELMONI_TEAM` specifies.
- **API Keys (`telmoni_…`):** The non-interactive path that opens only `{endpoint}/v1` reads (`/v1/team` or `/v1/team`). They do not go through `/cli`. A key that does not start with `telmoni_` is an error.
- **Local Credentials File:** Stored at `dirs::config_dir()/telmoni/credentials.json` (`~/Library/Application Support/telmoni/` on macOS, `~/.config/telmoni/` on Linux) with mode `0600`. Unencrypted.

## 🔭 Scope & Boundaries
- **CLI Commands:** The CLI's commands are strictly `login`, `logout`, `status` (alias `whoami`), `config`, `team list`, and `team switch` (with `team` aliases). A new command that reads or writes customer data needs a lane on the `/cli` door first, which is platform repository work.
- **SDK Scaffolds (`sdk/`):** Pure configuration structs without HTTP dependencies. They stay that way until their wire contract exists; the only edit they take is the endpoint hostname.

## 📝 Git Rules (Strict)
- **Commit Only on Command:** Never run `git add` or `git commit` unless explicitly instructed.
- **Format:** Conventional commits (`feat:`, `fix:`, `refactor:`, `chore:`). Hard limit of 1–5 lines (1 subject line + optional blank line + max 3 body lines explaining *why*). No file lists, test outputs, or task summaries.
- **No Branches or Pushes:** Kendrick reviews and commits. Never create branches (`checkout -b`, `switch -c`, `branch`, or worktrees) and never run `git push`.

## ✅ Definition of Done
Before reporting a change as finished:
- [ ] `cargo xtask ci` passes all 6 gates with `--locked` (fmt, clippy, build, test, doc, cargo deny check).
- [ ] Zero panics (`unwrap`, `expect`, `panic`, `todo`, `unimplemented`, `unreachable`) and zero `unsafe` in production code.
- [ ] No new dependencies added to `Cargo.toml`.
- [ ] Library code does not read `std::env`.
- [ ] Tests run deterministically without network access or real sleeps, writing under `temp_dir()`.
- [ ] All HTTP requests carry the standardized `User-Agent` and appropriate tenant headers.
- [ ] No secrets, tokens, or PII are exposed in logs or console output.
- [ ] No unasked tests or benchmarks ran; no `git add`, `git commit`, branching, or `git push` performed.
