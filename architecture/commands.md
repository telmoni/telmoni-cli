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

1. **Parse the command line** with clap. The global `-v` switches the stderr log filter to debug: which files were read and written, where the endpoint came from, each request and answer, and what the device flow and the 401 rules decided. Nothing secret is logged (see [transport](transport.md#what-is-never-printed)). Colour only on a terminal, since `-v` is for CI logs as much as for people.
2. **Resolve the paths:** `dirs::config_dir()/telmoni/`, holding `credentials.json` and `config.json`, each handed down as a value. With no user config directory the CLI says so on stderr and exits 1, whatever the command: it never keeps either file under the working directory.
3. **Build the HTTP transport** (see [transport](transport.md)).
4. **Read the configuration file.** A malformed file emits a note on stderr and becomes the default. A missing one is the default without a note.
5. **Read the environment** (below), and dispatch. `login` is also handed the real browser and the real wait between polls, which tests replace.
6. **On error**, print it to stderr and exit 1. Only the outermost context is printed; a request that got no answer carries its cause in that one line (see [transport](transport.md#the-seam)).

## The environment at the edge

| Variable | Read by | Used for |
|---|---|---|
| `TELMONI_ENDPOINT` | `main`, and clap for `login --endpoint` | The base URL at sign-in, and for a signed-out `status` |
| `TELMONI_ORG` | `main` | `logout` and `status`/`whoami`: an organization to act in instead of the stored active one, by id or slug. ⚠ A script carries the id: a URL change moves the slug, and the CLI resolves a slug against its cache, with no request (see [organizations](#organizations)) |
| `TELMONI_API_KEY` | clap, for `login --key`; then `main`'s `.env` fallback | Signing in with an API key. Later commands read the key from the credentials file, never the variable. `login --help` hides its value (`hide_env_values`): clap would otherwise print the exported key beside the flag, into a terminal or a CI log. |

**Debug builds also read a `.env`** from the working directory, after the real environment. Release builds compile that out.
- ⚠ A released binary run inside somebody else's checkout must not quietly take that checkout's endpoint or API key.

**A variable that is set but blank** is treated as unset by `main`. clap does not apply that filter, so an exported but empty `TELMONI_API_KEY` reaches `login` as an empty key and is refused.

**Two reads happen outside `main`'s own code:**
- `dirs`, which `main` alone calls, reads the platform's own variables (`HOME`, `XDG_CONFIG_HOME`) to find the configuration directory;
- clap reads the two variables above directly.

## The configuration file

`dirs::config_dir()/telmoni/config.json`: `~/Library/Application Support/telmoni/` on macOS, `~/.config/telmoni/` on Linux (`src/config.rs`). `main` resolves the path and hands it down, as it does the credentials path, so tests read and write one under the temp directory.

| Key | Use |
|---|---|
| `endpoint` | The base URL to sign in against |
| `output_format` | Accepted and stored. No command reads it yet. |

- `telmoni config set` checks the key, not the value.
- The file is written with a plain write, not atomically and with the default file mode. It holds no secret.
- ⚠ **It is never read from the working directory.** It names the endpoint `login` signs in against, so a checkout the CLI is run in could send the sign-in, and the API key after it, to a host of its choosing. Nor is the credentials file kept there (see [startup](#startup)), which a login would leave in the checkout or CI workspace.

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
| `org list` | Nothing: reads the cache | One organization per line (id, slug, label, role), `*` marking the active one | — |
| `org switch <organization>` | Refresh if needed, then `/cli/me` with that organization's id. It must come back as the active one. | The new active organization | — |
| `config get` / `set` / `list` | Nothing | The value, or the listing | — |

A few details the table leaves out:
- **Signed-out `status`** prints the endpoint to stdout, then fails with a message saying how to sign in. With `--json` it prints nothing to stdout: a script reading JSON gets that or nothing, and the refusal on stderr.
- **`status` checks the answer.** With `TELMONI_ORG` set, it checks the organization against the cache before calling, and against the server's answer after: that the server acted in it, and that the answer's list still gives it that name. `/me` falls back to another organization when the one asked for is no longer the person's.
- **`org list` and `org switch` refuse an API key.** A key belongs to one organization.
- **`org switch` takes an id or a slug**, never a label, and refuses one the cache does not hold before making any request. It checks the answer as `status` does.

## Organizations

**"Active organization" is the CLI's idea, not a server's.** The platform picks the organization per request, from the `x-organization-id` header; where none is named, or the one named is no longer the person's, `/me` acts in their default organization (the one they chose in Account Settings, else the oldest they own, else the oldest they belong to). A `login` names none, so the CLI's active organization starts as the default. The CLI:
- caches the person's organizations (id, slug, label, role) from `/cli/me`;
- keeps one as active in the credentials file, by id;
- sends its id on the requests that act inside an organization.

`TELMONI_ORG` overrides it for one command.

**Two ways to name one** (`Credentials::organization_named`), in `org switch` and in the variable alike:

| Name | Matched |
|---|---|
| Id (`org_…`) | Exactly |
| Slug | Exactly. It is what the console's URL shows: `/{slug}`. |

- **Each names one organization**, and the two never look alike: a slug has no underscore and an id always has one. So a name never names two, and nothing is picked silently.
- ⚠ **A label is not a name.** Labels are neither unique nor stable, so taking one means rules for the collisions that follow, and a wrong guess acts in another organization. `org list` shows the slug beside each; that is the human handle.
- ⚠ **The slug is a name for people, never what goes on the wire, and never what a script carries.** The CLI resolves it to the id against its cache, and sends the id. The `/cli` door forwards no other organization header. A script that pins an organization pins its id: a URL change moves the slug, and the next run fails.
- ⚠ **A slug is derived once, from the name the organization is born with** ("Ada's organization" reads as `adas-organization`; a name with no Latin letter or digit gives a placeholder, `org-` and ten random characters), **and after that moves only when its URL is changed on the console's Settings page** (a rename moves nothing); another organization may then take the one it left. The cache holds the slug `/cli/me` last answered, and nothing is fetched to resolve a name. So:
  - **The new slug is unknown here**, and refused before any request, until `login`, `status`, or an `org switch` by a name the cache does hold, rewrites the cache.
  - **The old slug still resolves, to the organization that held it.** `status` and `org switch` hold it to the answer: `/cli/me`'s list must give that organization the same name. If it does not, the command fails (`status`: "no longer names the organization it did"; `org switch` says whether the name is now unknown or now another organization's), keeps the fresh list, and leaves the active organization where it was. The next run reads the name as the console does.
  - **Membership moves too.** A switch to an organization the person has left fails the same way ("you are no longer in …") and keeps the fresh list, so `org list` stops showing it. Whenever the list is rewritten, an active organization that is no longer the person's gives way to the one `/me` acted in, their default: kept, it would be sent on every request after. Without the check, the CLI reported on, or switched to, an organization the console no longer shows at that URL.
  - `logout` makes no such call. A moved slug there puts the revoke's audit record on the organization the cache names.
  - The id never moves.

**An organization's label** is its `name`, as the console shows it, and never its owner's address, which `/cli/me` and `/v1/organization` carry beside it. The platform never leaves a name empty: a new organization is born named after its holder ("Ada's organization", or "My organization" for someone with no name), and its owner renames it on Settings. So `name` is a `String`, not an `Option`: an answer without one does not decode.

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
