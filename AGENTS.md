# Telmoni CLI — Agent Guidelines

The `telmoni` command-line client ([`telmoni/telmoni-cli`](https://github.com/telmoni/telmoni-cli)): a Rust workspace (`src/`). It is a client of the platform ([`telmoni/telmoni`](https://github.com/telmoni/telmoni)) and of nothing else: nothing here runs on a server, holds a database or verifies a token. Its requests go to one base URL, the machine host, `https://api.telmoni.com`, unless `TELMONI_ENDPOINT` says otherwise (`http://localhost:8082` locally, the platform's server); no other hostname appears in `src/`. The SDKs, which send an agent's spans to the platform's ingest, are the platform's, under `sdk/` in `telmoni/telmoni`.

## Ground Rules
- **Pre-launch:** nothing has shipped. Change commands, flags, the credentials file and the config file directly to their ideal shape; no migration of an old file, no deprecated aliases, no shims.
- **Quality gate:** never weaken a test or leave the tree broken.
- **Ask first:** any dependency change (any edit to a `Cargo.toml` or `Cargo.lock`, lockfile-only refreshes included; nothing in the current scope needs one, and anything added must pass `cargo deny check`) and any external-state change, such as publishing a release or a package.
- **Stateless machines:** the CLI runs over SSH, in containers, on CI runners and on hosts with no browser. Nothing may depend on a browser reaching the machine, a listening port, or state beyond the saved login (the macOS Keychain on a Mac, the credentials file standing in wherever it refuses) and the config file (`config.json` beside it).

## Where Things Live
| Path | What |
|---|---|
| `src/` | The CLI: `main.rs` and `login`'s `--endpoint` flag (the only environment and standard-input reads, beside reqwest's proxy variables), `auth/` (device flow, a key for one command, the saved login), `commands/`, `client.rs` (`/v1`), `transport.rs` (HTTP, errors, User-Agent). |
| `tests/` | Mock-based suites: no network, no sleeps. |
| `xtask/` | The gate (`cargo xtask ci`) and packaging (`cargo xtask dist`). |
| `ARCHITECTURE.md` | How the CLI is built, and why, on one page: the repository's one internal document. |
| [`telmoni/telmoni`](https://github.com/telmoni/telmoni) | Sibling repo, checked out as `../telmoni`: the platform and its four SDKs (`sdk/`), and the authority on the wire contract. |
| [`telmoni/docs`](https://github.com/telmoni/docs) | Sibling repo, checked out as `../docs`: the customer docs (`content/`), the CLI's page (`content/api/cli.mdx`) among them, which the platform's console serves at `/docs`. |

## Commands
- **After every change** (no permission needed; report failures verbatim): `cargo xtask ci`, which runs fmt, checks that the toolchain channel matches `rust-version`, then clippy, build, test, doc and `cargo deny check`, all `--locked`.
- **Locally:** `cargo run -- <command>`; with the platform's `make up` running, `TELMONI_ENDPOINT=http://localhost:8082` points it at the server, which serves the `/cli` lanes.

## Code Rules
- **Tests are hermetic:** HTTP, and the poll loop's sleeps and clock, are injected (`Transport`, closures); the credentials path is a value, so tests write under `std::env::temp_dir()`.
- **Environment only at the edge:** only `main.rs`, and clap's `env` attributes on `login`'s flags, read the environment and pass values down; reqwest reads the proxy variables on its own. `main.rs` reads a `.env` in debug builds only, so a checkout the CLI runs in cannot redirect the endpoint or supply a key.
- **Output:** stdout carries only the command's output; diagnostics go to stderr or `tracing`.

## Contracts
The platform's code is the contract: in [`telmoni/telmoni`](https://github.com/telmoni/telmoni), the `/cli/v1` lanes (`crates/auth/src/handler/cli.rs`, behind the door in `crates/telmoni/src/door.rs`), `/me` (`crates/auth/src/handler/me.rs`) and `/v1` (`crates/auth/src/handler/v1.rs`). Read them before touching `src/auth/` or `src/client.rs`; report a disagreement rather than working around it here.
- **Sign-in is the RFC 8628 device flow** behind the `/cli` door, never an identity provider directly: `POST /cli/v1/auth/device`, print the code and URL (open it when a browser exists), poll `POST /cli/v1/auth/device/poll` every `interval` (202 not yet, `slow_down` adds 5 s, 403 denied, 400 expired, 401 a code auth does not know), then `POST /cli/v1/me`. No loopback redirect, listener or PKCE.
- **Refresh** is `POST /cli/v1/auth/refresh` with `{ refreshToken, sessionRowId }`. **Sign-out** is `POST /cli/v1/sessions/{sessionRowId}/revoke`, with `x-organization-id` when an organization is cached, then the saved login goes whatever the answer.
- **A 401** says by its problem type what ended. `unauthenticated` on `/cli/v1/me` or revoke, and `unauthenticated` or `invalid-token` on refresh, end the session: delete the saved login, print "run `telmoni login`", don't retry. `token-expired` or `invalid-token` on `/cli/v1/me` or revoke get one refresh and one retry: the sweep turns an expired bearer's `token-expired` into `invalid-token`. Any other 401 ends nothing: `service-credential-rejected` is the console's own hop to the server failing. A 401 under an API key on `/v1` leaves the saved login alone; a 5xx never deletes it.
- **Errors** are parsed in three shapes: RFC 9457 problem details (print `title: detail`), `{ "error": … }` (nothing the CLI calls — `/cli`, `/v1` — sends it today; kept for whatever sits in front of them), or anything else (`request failed (<status>)` plus the first 200 characters). Never print a token, refresh token, device code or API key; the user code is the only code shown.
- **Every request** carries `User-Agent: telmoni-cli/<version> (<os>; <arch>)`, and every bearer request that acts in an organization `x-organization-id`: always an id — the active organization's from `/cli/v1/me`, or the one `TELMONI_ORG` or `org switch` names by id or exact slug, resolved against the cached list (the `/cli` door refuses anything but an `org_…` id; a label is never accepted). Nullable server fields are `Option`s.
- **Labels and slugs:** an organization's label is its trimmed `name`; never `ownerEmail`. Its slug is set when it is created: the URL its owner chose, else one derived from its name when that is free, else a placeholder (`org-` and ten random characters); after that only a change to its URL in the console's Settings moves it, never a rename.
- **API keys** start with `telmoni_` and open only `{endpoint}/v1`, with the `read` scope, and `{endpoint}/otlp/v1/traces`, with the `ingest` scope; never `/cli`. Every `/v1` lane is a read today; ingest is written by the platform's SDKs alone, and the CLI calls it nowhere.
- **Credentials** live in the macOS Keychain on a Mac (service `telmoni`, the credentials file's path as the account), else in `dirs::config_dir()/telmoni/credentials.json`, mode `0600`, unencrypted. A key in `TELMONI_API_KEY` (at a terminal, once the person approved it, the answer kept by the key's SHA-256 in `config.json`), or from `api_key_helper`, is used for one command and never saved; no key is ever an argument.
- **Scope:** the commands are `login`, `logout`, `status`/`whoami`, `org list|switch` and `config`; one saved login, no profiles. A command that touches customer data needs its lane in the platform first: `/cli` for a signed-in person, `/v1` for an API key, or another lane the platform's code defines.

## Agent Hygiene
- **This repo only:** change nothing in another repository (`telmoni/telmoni`, any other) unless the user says so for this task; that binds subagents too. Reading is fine.
- Edit `AGENTS.md` (`.github/copilot-instructions.md` points here) only when the user asks outright; otherwise propose a diff.
- Comments are for a person reading the code, and short: they say *why*, never *what* — a doc comment in a line or two, an inline comment in one, a test's doc in one line on what it proves. The reasoning behind a change (a race worked through, the designs turned down, its history) goes in `ARCHITECTURE.md` or the commit message, never above a function, and editing a comment means shortening its whole block. No AI signatures anywhere.
- No scripted bulk edits: edit each file deliberately (formatters, generators and an approved lockfile write excepted). One-off scripts, backups and logs go in `/tmp/telmoni/`.
- Never read, print, diff or recreate `.env` or the real saved login, the Keychain item `telmoni` on a Mac and the credentials file: they hold an API key or a refresh token. Change one key by name, after `cp -p` to `/tmp/telmoni/`.
- Keep the repo slim: fix a real problem where it lives. No guard script, lint gate, CI job or xtask step for a problem that is not happening.

## Git Rules
- Commit only when asked: never `git add` or `git commit` unprompted.
- Conventional commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`) of 1–5 lines and the sign-off: a subject, then optionally a blank line and up to 3 lines on *why*. No file lists, test output, AI signatures or `Co-authored-by` trailers.
- Every commit is signed off by the developer it is made for, certifying the [Developer Certificate of Origin](https://developercertificate.org/): `git commit -s` adds `Signed-off-by: Name <email>` from git's `user.name` and `user.email`, which name a person, never an agent. A merge commit too: a branch is merged in a clone, `git merge --no-ff --signoff`, and pushed, never from GitHub's merge button, whose commit carries none.
- No branches, worktrees, pushes or pull requests.

## Definition of Done
- [ ] `cargo xtask ci` passes, or the failure is reported verbatim.
- [ ] No secret, token, device code, API key or PII reaches output, a log or a commit.
- [ ] `ARCHITECTURE.md` still says what the code does: a change that alters a line there updates it in the same change.
- [ ] A change a user can see names the customer pages it leaves wrong (`content/api/cli.mdx` in `telmoni/docs`).
- [ ] Nothing that needs asking happened unasked (dependencies, external state, other repos, commits).
