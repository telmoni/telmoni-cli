# Commands

The `telmoni` binary is a thin shell over a library. `src/lib.rs` exports `auth`, `client`, `commands`, `config` and `transport`, and the tests drive that library directly. `src/main.rs` is the only place that reads the environment. It hands values down, so every command can be tested without touching the real environment.

## Contents

- [Startup](#startup)
- [The environment at the edge](#the-environment-at-the-edge)
- [The configuration file](#the-configuration-file)
- [The commands](#the-commands)
- [Organizations](#organizations)
- [Output](#output)
- [Where it lives](#where-it-lives)

## Startup

`main` (`src/main.rs`) runs in this order:

1. **Parse the command line** with clap. The global `-v` switches the stderr log filter to debug. There are no `tracing` calls in `src/` yet, so it changes nothing today.
2. **Resolve the credentials path:** `dirs::config_dir()/telmoni/credentials.json` (falling back to `./telmoni/credentials.json` if no user config directory is resolved).
3. **Build the HTTP transport** (see [transport](transport.md)).
4. **Read the configuration file.** A malformed file emits a note on stderr and becomes the default.
5. **Read the environment** (below), and dispatch.
6. **On error**, print it to stderr and exit 1. Only the outermost context is printed, so a network failure reads "transport send failed for `<url>`", without its cause.

## The environment at the edge

| Variable | Read by | Used for |
|---|---|---|
| `TELMONI_ENDPOINT` | `main`, and clap for `login --endpoint` | The base URL at sign-in, and for a signed-out `status` |
| `TELMONI_ORGANIZATION` (or `TELMONI_ORG`) | `main` | `logout` and `status`/`whoami`: an organization to act in instead of the stored active one, by id or slug |
| `TELMONI_API_KEY` | clap, for `login --key`; then `main`'s `.env` fallback | Signing in with an API key. Later commands read the key from the credentials file, never the variable. |

**Debug builds also read a `.env`** from the working directory, after the real environment. Release builds compile that out.
- ⚠ A released binary run inside somebody else's checkout must not quietly take that checkout's endpoint or API key.

**A variable that is set but blank** is treated as unset by `main`. clap does not apply that filter, so an exported but empty `TELMONI_API_KEY` reaches `login` as an empty key and is refused.

**Two reads happen outside `main`:**
- `dirs` reads the platform's own variables to find the configuration directory;
- clap reads the two variables above directly.

## The configuration file

`dirs::config_dir()/telmoni/config.json`: `~/Library/Application Support/telmoni/` on macOS, `~/.config/telmoni/` on Linux (`src/config.rs`).

| Key | Use |
|---|---|
| `endpoint` | The base URL to sign in against |
| `output_format` | Accepted and stored. No command reads it yet. |

- `telmoni config set` checks the key, not the value.
- The file is written with a plain write, not atomically and with the default file mode. It holds no secret.

**Where the base URL comes from** (`resolve_endpoint`), first match wins:
1. `--endpoint`, which clap also fills from `TELMONI_ENDPOINT`;
2. `TELMONI_ENDPOINT`, as `main` read it;
3. the configuration file's `endpoint`;
4. `DEFAULT_TELMONI_ENDPOINT`, `https://telmoni.com`.

Blank values are skipped and a trailing `/` is trimmed. A value with no scheme gets `https://`, or `http://` when its host is this machine, by the same test the transport applies (see [transport](transport.md#the-seam)).

⚠ **Only `login`, and a signed-out `status`, resolve the endpoint.** Every other command uses the endpoint recorded in the credentials file at sign-in. A session belongs to the platform that issued it.

## The commands

| Command | Calls | stdout | stderr |
|---|---|---|---|
| `login` | The device flow, then `/cli/me` (see [auth](auth.md#the-device-flow)) | The user code and URL; then who signed in and the active organization | Browser-open failures; "Waiting for approval…" |
| `login --key <key>` | Nothing: the key is checked for shape only | "Signed in with API key", and the endpoint | — |
| `logout` | Refresh if needed, then the session revoke (see [auth](auth.md#signing-out)) | "Signed out", or "Not signed in" | A note when the server did not confirm |
| `status` / `whoami [--json]` | With an API key: `GET /v1/organization`. Signed in: refresh if needed, then `/cli/me` with the active organization, rewriting the cached person and organizations. | Who and which organization, the endpoint, the token's remaining lifetime; or JSON | — |
| `organization list` (alias `org list`) | Nothing: reads the cache | One organization per line (id, slug, label, role), `*` marking the active one | — |
| `organization switch <organization>` (alias `org switch`) | Refresh if needed, then `/cli/me` with that organization's id. It must come back as the active one. | The new active organization | — |
| `config get` / `set` / `list` | Nothing | The value, or the listing | — |

A few details the table leaves out:
- **Signed-out `status`** prints the endpoint to stdout, then fails with a message saying how to sign in.
- **`status` checks the answer.** With `TELMONI_ORGANIZATION` (or `TELMONI_ORG`) set, it checks the organization against the cache before calling, and against the server's answer after. `/me` falls back to another organization when the one asked for is no longer the person's.
- **`org list` and `org switch` refuse an API key.** A key belongs to one organization.
- **`org switch` takes an id, a slug or a label**, and refuses one the cache does not hold before making any request. A label two organizations share is refused too, naming each by slug and id.

## Organizations

**"Active organization" is the CLI's idea, not a server's.** The platform picks the organization per request, from the `x-organization-id` header. The CLI:
- caches the person's organizations (id, slug, label, role) from `/cli/me`;
- keeps one as active in the credentials file, by id;
- sends its id on the requests that act inside an organization.

`TELMONI_ORGANIZATION` (or `TELMONI_ORG`) overrides it for one command.

**Three ways to name one** (`Credentials::organization_named`, `resolve_target_org`):

| Name | Where it is taken | Matched |
|---|---|---|
| Id (`org_…`) | `org switch`, the variable | Exactly |
| Slug | `org switch`, the variable | Exactly. It is what the console's URL shows: `/{slug}`. |
| Label | `org switch` only | Ignoring ASCII case. Refused when several organizations share it. |

- **An id or a slug is read before a label.** Each names one organization. A slug has no underscore and an id always has one, so the two never collide.
- ⚠ **The slug is a name for people, never what goes on the wire.** The CLI resolves it to the id against its cache, and sends the id. The `/cli` door forwards no other organization header.
- ⚠ **A slug follows the organization's name, so a rename in the console moves it.** The cache holds the slug `/cli/me` last answered. An older one is unknown here until `status`, or a successful `org switch`, rewrites the cache. The id never moves.

**An organization's label** is its trimmed `name`, else its owner's address, else "Organization". The platform leaves a new organization unnamed, and labels it by its owner.

## Output

- **stdout carries only the command's output**, so it can be piped and parsed. `status --json` is the structured form.
- **Diagnostics go to stderr:** notes, warnings, errors and `tracing`.
- **Nothing secret is ever printed:** no token, refresh token, device code or API key. The user code is the only code shown, and URLs are shown in full.

## Where it lives

| Concern | File |
|---|---|
| Entry point, the environment, dispatch | `src/main.rs` |
| Commands | `src/commands/{login,logout,status,org,config_cmd}.rs` |
| Configuration file, base URL | `src/config.rs` |
| Library surface | `src/lib.rs` |
