# Telmoni CLI — Agent Guidelines

The `telmoni` command-line client and client SDK scaffolds ([`telmoni/telmoni-cli`](https://github.com/telmoni/telmoni-cli)): a Rust workspace (`src/`) and SDKs (`sdk/`). It is a client of the platform ([`telmoni/telmoni`](https://github.com/telmoni/telmoni)) and of nothing else: nothing here runs on a server, holds a database or verifies a token. Every request goes to one base URL, `https://telmoni.com` unless `TELMONI_ENDPOINT` says otherwise (`http://localhost:3000` locally), and no other hostname appears in `src/` or `sdk/`.

## Ground Rules
- **Pre-launch:** nothing has shipped. Change commands, flags, the credentials file and the config file directly to their ideal shape; no migration of an old file, no deprecated aliases, no shims.
- **Quality gate:** never weaken a test or leave the tree broken.
- **Ask first:** any dependency change (any edit to a `Cargo.toml`, `Cargo.lock` or an SDK's lockfile, lockfile-only refreshes included; nothing in the current scope needs one, and anything added must pass `cargo deny check`) and any external-state change, such as publishing a release or a package.
- **Stateless machines:** the CLI runs over SSH, in containers, on CI runners and on hosts with no browser. Nothing may depend on a browser reaching the machine, a listening port, or state beyond the credentials file.

## Where Things Live
| Path | What |
|---|---|
| `src/` | The CLI: `main.rs` (the only environment reads), `auth/` (device flow, credentials), `commands/`, `client.rs` (`/v1`), `transport.rs` (HTTP, errors, User-Agent). |
| `sdk/` | SDK scaffolds (`rust/`, `typescript/`, `go/`, `python/`): configuration structs, no HTTP. |
| `tests/` | Mock-based suites: no network, no sleeps. |
| `xtask/` | The gate (`cargo xtask ci`) and packaging (`cargo xtask dist`). |
| [`telmoni/telmoni`](https://github.com/telmoni/telmoni) | Sibling repo, checked out as `../telmoni`: the platform, and the authority on the wire contract. |
| [`telmoni/docs`](https://github.com/telmoni/docs) | Sibling repo, checked out as `../docs`: the customer docs site, including the CLI and SDK pages. |

## Commands
- **After every change** (no permission needed; report failures verbatim): `cargo xtask ci`, which runs fmt, checks that the toolchain channel matches `rust-version`, then clippy, build, test, doc and `cargo deny check`, all `--locked`.
- **SDK scaffolds:** `npm test --prefix sdk/typescript`, `go test ./...` in `sdk/go`, `sdk/python/.venv/bin/pytest sdk/python`.
- **Locally:** `cargo run -- <command>`; with the platform's `make up` running, `TELMONI_ENDPOINT=http://localhost:3000` points it there.

## Code Rules
- **Tests are hermetic:** HTTP, and the poll loop's sleeps and clock, are injected (`Transport`, closures); the credentials path is a value, so tests write under `std::env::temp_dir()`.
- **Environment only at the edge:** only `main.rs` reads the environment and passes values down. `main.rs` reads a `.env` in debug builds only, so a checkout the CLI runs in cannot redirect the endpoint or supply a key.
- **Output:** stdout carries only the command's output; diagnostics go to stderr or `tracing`.

## Contracts
The platform's code is the contract: in [`telmoni/telmoni`](https://github.com/telmoni/telmoni), the `/cli` door (`web/app/cli/[...path]/route.ts`), `/me` (`crates/auth/src/handler/me.rs`) and `/v1` (`crates/auth/src/handler/v1.rs`). Read them before touching `src/auth/` or `src/client.rs`; report a disagreement rather than working around it here.
- **Sign-in is the RFC 8628 device flow** behind the `/cli` door, never an identity provider directly: `POST /cli/auth/device`, print the code and URL (open it when a browser exists), poll `POST /cli/auth/device/poll` every `interval` (202 not yet, `slow_down` adds 5 s, 403 denied, 400 expired, 401 a code auth does not know), then `POST /cli/me`. No loopback redirect, listener or PKCE.
- **Refresh** is `POST /cli/auth/refresh` with `{ refreshToken, sessionRowId }`. **Sign-out** is `POST /cli/sessions/{sessionRowId}/revoke`, with `x-organization-id` when an organization is cached, then the credentials file goes whatever the answer.
- **A 401** on `/cli/me`, refresh or revoke ends the session: delete the credentials file, print "run `telmoni login`", don't retry. Except `/errors/auth/token-expired`, which one refresh cures. A 401 under an API key on `/v1` leaves the file alone; a 5xx never deletes it.
- **Errors** are parsed in three shapes: RFC 9457 problem details (print `title: detail`), `{ "error": … }` (nothing the CLI calls — `/cli`, `/v1` — sends it today; kept for whatever sits in front of them), or anything else (`request failed (<status>)` plus the first 200 characters). Never print a token, refresh token, device code or API key; the user code is the only code shown.
- **Every request** carries `User-Agent: telmoni-cli/<version> (<os>; <arch>)`, and every bearer request that acts in an organization `x-organization-id`: always an id — the active organization's from `/cli/me`, or the one `TELMONI_ORG` or `org switch` names by id or exact slug, resolved against the cached list (the `/cli` door refuses anything but an `org_…` id; a label is never accepted). Nullable server fields are `Option`s.
- **Labels and slugs:** an organization's label is its trimmed `name`, else `"Organization"`; never `ownerEmail`. Its slug is a placeholder (`org-` and ten random characters) until the first name its owner gives it reads as a slug and replaces it; after that only a change to its URL in the console's Settings moves it, never a rename.
- **API keys** start with `telmoni_` and open only `{endpoint}/v1`, never `/cli`; every `/v1` lane is a read today.
- **Credentials** live in `dirs::config_dir()/telmoni/credentials.json`, mode `0600`, unencrypted.
- **Scope:** the commands are `login`, `logout`, `status`/`whoami`, `org list|switch` and `config`; one credentials file, no profiles. A command that touches customer data needs its lane in the platform first: `/cli` for a signed-in person, `/v1` for an API key, or another lane the platform's code defines. The SDKs stay configuration-only until their contract exists.

## Agent Hygiene
- **This repo only:** change nothing in another repository (`telmoni/telmoni`, `telmoni/docs`, any other) unless the user says so for this task; that binds subagents too. Reading is fine.
- Edit `AGENTS.md` (`.github/copilot-instructions.md` points here) only when the user asks outright; otherwise propose a diff.
- Comments say *why*, never *what*. No AI signatures anywhere.
- No scripted bulk edits: edit each file deliberately (formatters, generators and an approved lockfile write excepted). One-off scripts, backups and logs go in `/tmp/telmoni/`.
- Never read, print, diff or recreate `.env` or the real credentials file: they hold an API key or a refresh token. Change one key by name, after `cp -p` to `/tmp/telmoni/`.
- Keep the repo slim: fix a real problem where it lives. No guard script, lint gate, CI job or xtask step for a problem that is not happening.

## Git Rules
- Commit only when asked: never `git add` or `git commit` unprompted.
- Conventional commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`) of 1–5 lines: a subject, then optionally a blank line and up to 3 lines on *why*. No file lists, test output, AI signatures or `Co-authored-by` trailers.
- No branches, worktrees, pushes or pull requests.

## Definition of Done
- [ ] `cargo xtask ci` passes, or the failure is reported verbatim.
- [ ] No secret, token, device code, API key or PII reaches output, a log or a commit.
- [ ] A change a user can see names the customer pages it leaves wrong (`api/cli.mdx`, `api/sdks.mdx` in `telmoni/docs`).
- [ ] Nothing that needs asking happened unasked (dependencies, external state, other repos, commits).
