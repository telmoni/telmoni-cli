# Architecture

The Telmoni CLI and its SDKs on one page: what they are for, what they are made of, how a command reaches the platform, what they keep, how they fail, and why they are built this way. It is the repository's one document on how the CLI and its SDKs are built: the detail behind each line is in the code it names, the code's comments and its tests.

The platform the CLI talks to has a design of its own, `ARCHITECTURE.md` in [`telmoni/telmoni`](https://github.com/telmoni/telmoni/blob/main/ARCHITECTURE.md), checked out beside this repo as `../telmoni/ARCHITECTURE.md`: its `/cli` door, `/me`, `/v1` and sessions. This page covers the client.

**Where it stands:** version 0.0.1, and no release has been cut, so `install.sh` has nothing to fetch yet and the CLI is built from source. It signs in and out, says who it is signed in as, and chooses an organization. Beyond its own session it changes nothing on the platform, except that `/cli/me` makes a first organization for a person who belongs to none, as a console sign-in does. The four SDKs are configuration only, and none is published.

## Contents

- [Purpose and scope](#purpose-and-scope)
- [Goals and constraints](#goals-and-constraints)
- [Context](#context)
- [Building blocks](#building-blocks)
- [The SDKs](#the-sdks)
- [Key flows](#key-flows)
- [Data](#data)
- [Security](#security)
- [Distribution](#distribution)
- [When something fails](#when-something-fails)
- [What comes next](#what-comes-next)
- [Decisions](#decisions)
- [Known gaps](#known-gaps)

## Purpose and scope

The CLI, `telmoni`, is how a person or a machine uses Telmoni from a terminal. Today it:
- signs a person in with a code approved in any browser, or a machine in with an API key;
- says who it is signed in as, and in which organization;
- lists the person's organizations, and chooses the one it acts in;
- keeps a configuration file: the endpoint to sign in against.

It is **one Rust binary**, a thin shell over a library the tests drive directly, and a **client of the platform and of nothing else**: nothing here runs on a server, holds a database or verifies a token. Beside it, `sdk/` holds the client SDKs for Rust, TypeScript, Go and Python: scaffolds that hold configuration and make no call until the platform serves the lanes they would call.

| For | Read |
|---|---|
| Using the CLI | [telmoni.com/docs/api/cli](https://telmoni.com/docs/api/cli) |
| Using the SDKs | [telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks) |
| Developing it | `README.md`, `AGENTS.md`, and [Developing](https://telmoni.com/docs/contributing/developing#the-cli-and-sdks) |

## Goals and constraints

**What the design must hold:**

| Goal | How |
|---|---|
| **It talks to one platform, and to nothing else** | One base URL, `https://telmoni.com` unless `TELMONI_ENDPOINT` names another, and nothing in `src/` or `sdk/` calls another host; plain HTTP only to a loopback endpoint; no redirect followed |
| **It works where no browser can reach it** | The device flow: the person approves a code in any browser, on any machine, and nothing listens here |
| **A secret is never shown** | No token, refresh token, device code or API key in output or logs, and `Debug` written by hand on every struct that holds one |
| **A key never rides a command line, and need not be saved** | `login --with-key` reads it from standard input; `TELMONI_API_KEY`, or what `api_key_helper` prints, supplies one for a single command and is never saved |
| **A crash or a blip never loses the session** | The saved login written atomically, to the Keychain or the file; a 5xx or a dropped connection never deletes it; a refresh whose answer was lost asked again once |
| **The platform decides when a session ends** | A 401 read by its problem type, one refresh deciding a bearer refused as expired or unknown |
| **What a checkout holds cannot redirect it** | The environment read only at the edge, a `.env` only in debug builds, and the configuration never from the working directory |
| **Tests never touch the network or the person's files** | HTTP, sleeps, the browser, standard input and both files' paths injected, and the Keychain left to the binary's own store, under an item named for its home |
| **A download can be checked** | A checksum beside every archive, and build provenance signed for the release workflow |

**What it is not, on purpose:**
- **A server, a database or a verifier.** The platform owns the session; the CLI holds its opaque tokens.
- **A loopback sign-in.** No listener, no redirect, no PKCE: a browser may never reach the machine.
- **Profiles.** One saved login, one session at a time.
- **SDK clients written ahead of the platform.** A client waits for its lane in the platform's `V1_LANES`.

**What it must live with:**
- **Nothing has shipped.** Commands, flags and both files change in place, with no migration of an older file and no deprecated alias (`AGENTS.md`).
- **The platform's code is the contract.** A disagreement is recorded, and reported there, never worked around here.
- **Two platforms are built:** Apple-silicon macOS and x86_64 Linux. On Intel macOS and ARM Linux, `install.sh` points to building from source, and it refuses any other platform.

## Context

Who and what the CLI talks to.

```mermaid
flowchart LR
  Person["A person, in a terminal"]
  Machine["A script or CI job, holding an API key"]
  Browser["A browser, anywhere"]
  subgraph Repo["This repository"]
    CLI["telmoni"]
    SDKs["SDK scaffolds: no calls yet"]
  end
  Proxy["A proxy, if the system names one"]
  subgraph Platform["The platform, at one base URL"]
    Console["Console: the /cli door, the /v1 relay, the /auth/device page"]
    Server["Server: auth"]
  end
  GitHub["GitHub: releases and their provenance"]
  Person --> CLI
  Machine --> CLI
  CLI -->|"HTTPS"| Console
  CLI -.->|"or through"| Proxy
  Proxy -.-> Console
  Console -->|"bearer and service secret"| Server
  Browser -.->|"approves the code at /auth/device"| Console
  Person -.->|"install.sh"| GitHub
```

| Party | What it is to the CLI | What crosses |
|---|---|---|
| **A person** | Who signs in, at a terminal, over SSH or in a container | Commands; the user code and the URL on stdout |
| **A script or CI job** | Whoever holds an organization's API key | The key in `TELMONI_API_KEY` or from `api_key_helper` for each command, or saved once by `login --with-key`; then `/v1` reads |
| **A browser, anywhere** | Where the person approves the code, on any machine | The console's `/auth/device` page; nothing reaches the CLI |
| **The platform's console** | The one host the CLI calls: the `/cli` door for a session, the `/v1` relay for a key | Every request, with the CLI's `User-Agent` and, when it acts in one, the organization's id |
| **The platform's server** | Behind the console, never called directly: it issues and ends sessions, and answers `/me` and `/v1` | Nothing directly |
| **A proxy** | Whatever the system's proxy variables name | Every request to another host, tunnelled through it and still encrypted; never one to this machine, which goes direct |
| **GitHub** | Where releases, their checksums and their provenance live | The archives, `SHA256SUMS`, `install.sh` |

## Building blocks

The binary's modules, in `src/`, each arrow one building or calling another; the types they share are left out. The last three rows of the table stand beside the binary.

```mermaid
flowchart TB
  Main["main.rs: the environment, standard input, the paths, dispatch"]
  Cmds["commands/: login, logout, status, org, config"]
  Device["auth/device.rs: the device flow, refresh, the 401 rules"]
  Key["auth/key.rs: a key for one command"]
  Store["auth/storage.rs: the saved login"]
  Config["config.rs: the configuration file, the base URL"]
  V1["client.rs: /v1"]
  T["transport.rs: the Transport seam"]
  Main --> Cmds
  Main --> Key
  Main --> Store
  Main --> Config
  Main --> T
  Cmds --> Device
  Cmds --> Store
  Cmds --> Config
  Cmds --> V1
  Device --> Store
  Device --> T
  V1 --> T
```

| Block | What it is | What it keeps |
|---|---|---|
| `main.rs` | The only place that reads the environment and standard input: it resolves both files' paths, builds the transport, dispatches, and prints an error and exits 1 | — |
| `commands/` | One module per command, each handed its inputs as values | — |
| `auth/device.rs` | The device flow, refresh, and the reading of a 401 by its type | — |
| `auth/key.rs` | A key supplied for one command: `TELMONI_API_KEY`, else what `api_key_helper` prints, checked by its shape and never saved | — |
| `auth/storage.rs` | The saved login: the macOS Keychain on a Mac, else the credentials file, read leniently and written atomically | The Keychain item `telmoni`, `credentials.json` |
| `config.rs` | The configuration file, and where the base URL comes from | `config.json` |
| `client.rs` | The one `/v1` call, under an API key | — |
| `transport.rs` | The seam every request goes through: TLS, deadlines, the plain-HTTP refusal, this machine past any proxy, the platform's error shapes | — |
| `sdk/` | Four configuration-only scaffolds, one per language | — |
| `xtask/` | The gate, `cargo xtask ci`, and packaging, `cargo xtask dist` | — |
| `install.sh` | The installer: the machine's archive, a version, the checksum | — |

## The SDKs

Four scaffolds that hold configuration and make no call yet, one contract between them:

| SDK | Package | Configuration | Entry points |
|---|---|---|---|
| Rust | `telmoni-sdk` (`publish = false`) | `Config { endpoint, organization_id, api_key }` | `Config::new`, `Client::new`, `from_env`, `with_config`, `config()`; `Telmoni` is an alias of `Client` |
| TypeScript | `telmoni` | `Config { endpoint?, apiKey?, organizationId? }` | `new Telmoni(config)`, `Telmoni.fromEnv()`, the read-only `config` |
| Go | `github.com/telmoni/telmoni-cli/sdk/go` | `Config{Endpoint, OrganizationID, APIKey}` | `DefaultConfig`, `ConfigFromEnv`, `New`, `NewFromEnv`, `Config()`; `Telmoni` is an alias |
| Python | `telmoni` | `@dataclass Config(endpoint, organization_id, api_key)` | `Telmoni(config=None)`, `Telmoni.from_env()` |

- **Dependencies:** Rust serde alone; TypeScript none at run time, built to CommonJS and ESM with type declarations by tsup; Go the standard library; Python none, with `py.typed`.
- **Defaults and the environment.** Each defaults to the CLI's base URL, `DEFAULT_TELMONI_ENDPOINT` (`src/config.rs`), strips a trailing `/` from the endpoint, and reads `TELMONI_ENDPOINT`, `TELMONI_API_KEY` and `TELMONI_ORG`, as the CLI does, and no other variable.
- ⚠ **An SDK takes the organization as an id.** The CLI also takes a slug, which it resolves against its cached `/cli/me`; an SDK has no session. Nothing sends the id today, since `/v1` names no organization: a key carries its own.
- ⚠ **No SDK's configuration prints or serializes its credential.** Rust's `Config` has a hand-written `Debug` and `#[serde(skip_serializing)]` on `api_key`; Python's `api_key` is `field(repr=False)`, though `dataclasses.asdict` still carries it; Go's `APIKey` is `json:"-"`, with `String` and `GoString` on `Config` and `Client`; TypeScript's `apiKey` is not enumerable, so logging, `JSON.stringify` and a spread leave it out.

They are not yet one in the rows below; when an SDK grows a client, each is answered in all four as the CLI does (`src/main.rs` and `src/config.rs`: an empty or whitespace endpoint, organization or key is unset, from any source):

| Behaviour | Rust | TypeScript | Go | Python |
|---|---|---|---|---|
| An empty `TELMONI_ENDPOINT` falls back to the default | yes, whitespace too | yes | yes | yes |
| An empty endpoint passed explicitly falls back to the default | no | yes | yes | no |
| An empty `TELMONI_ORG` or `TELMONI_API_KEY` reads as unset | yes, whitespace too | no | yes | no |

What each one's contract test pins (`sdk/rust/tests/contract.rs`, `sdk/typescript/tests/contract.test.ts`, `sdk/go/client_test.go` with `example_test.go`, `sdk/python/tests/test_contract.py`):

| Pinned | Rust | TypeScript | Go | Python |
|---|---|---|---|---|
| The default endpoint, as the constant and as the literal URL | yes | yes | the constant only | yes |
| A trailing `/` is stripped | yes | yes | — | yes |
| Building from the environment gives the default | non-empty only | yes, with `TELMONI_ENDPOINT` cleared first | non-nil only | yes, with `TELMONI_ENDPOINT` cleared first |
| The credential is never printed, and stays readable | `Debug` of `Config` and `Client` | `util.inspect` and `JSON.stringify` of the client and its `config` | `%v`, `%+v`, `%#v` of `Config` and `Client`, and `json.Marshal` | `repr` |

Pinned nowhere: the variables' names and precedence, and Rust's `Serialize` leaving the credential out.

**Growing a client**, once a lane exists to call:
1. The platform serves it first, in `telmoni/telmoni`'s `V1_LANES`, which generates both its router and `contract/openapi.json`.
2. The SDK grows a client for that lane from the platform's code; a disagreement is reported there, never worked around here.
3. The client follows the CLI's transport rules: one base URL, a `User-Agent` naming the SDK, errors read in the platform's three shapes, no credential in any output.
4. The four grow together: a behaviour added to one lands in all four in the same change, with its row in the tables above.

They are checked by CI's `sdks` job beside the Rust gate, which covers the Rust SDK as a workspace member. Nothing is published: the Rust crate takes the workspace's version, TypeScript and Python carry their own, and Go will need tags of its own, `sdk/go/v…`, which the release workflow's `v*` does not match.

## Key flows

### Signing in

`telmoni login` asks the `/cli` door for a device code, prints the user code and the URL, and opens a browser only on the endpoint's own origin. It polls at the interval the platform sets until the person approves or denies the code or it expires, then asks `/cli/me` who signed in, and saves the login. `login --with-key` reads a key from standard input, never from a terminal, checks its shape and saves it, with no call.

### A key for one command

`status` with `TELMONI_API_KEY` set, or else with `api_key_helper` configured, asks `/v1` with that key, at the endpoint `TELMONI_ENDPOINT`, the configuration file or the default names, and neither reads nor writes the saved login. A blank variable is no variable. The helper runs through `sh` with nothing on its standard input and 30 seconds to answer; what it prints is the key, never shown, and its stderr is the person's. `login`, `logout` and `org`, which manage the saved login, say once that the variable outranks it.

### A command that acts in an organization

`status` and `org switch` refresh the access token when it expires within a minute, then ask `/cli/me` with the organization's id in `x-organization-id`, rewrite the cached person and organizations from the answer, and check that the platform acted in the organization they named. An organization is named by id or slug, resolved against the cache and sent as its id, and never by its label.

### Signing out

`telmoni logout` refreshes if it must, then revokes the session row through the lane the console's Active sessions uses, falling back when the cached organization is one the person has left. It deletes the saved login, in the Keychain and the file, whatever the answer, and says so when the platform did not confirm.

### When the platform says no

A failed answer is read in the platform's three error shapes, and a 401 by its problem type. A 401 typed `unauthenticated` ends the session; `token-expired` or `invalid-token` on a call gets one refresh, whose answer decides; any other 401, a 5xx or a dropped connection ends nothing.

## Data

The CLI's state is two files in the person's configuration directory, `~/Library/Application Support/telmoni/` on macOS and `~/.config/telmoni/` on Linux, and never in the working directory; on a Mac, a Keychain item holds the login in the credentials file's place.

| File | Holds | Written |
|---|---|---|
| `credentials.json`, or on a Mac the Keychain item | The endpoint fixed at sign-in; for the device flow the access and refresh tokens, the access token's expiry, the session row's id, the person, the cached organizations and the active one; or an API key | At sign-in, after every refresh, and by `status` and `org switch`. On a Mac to the Keychain, as a generic password under the service `telmoni` with the file's path as its account, the file only while the Keychain refuses, as a locked one over SSH does; with both, the newer is read, and a save the Keychain takes deletes the file. Elsewhere the file, atomically, mode `0600`, not encrypted |
| `config.json` | The endpoint to sign in against, `api_key_helper`, a command that prints an API key, and an output format no command reads yet | By `config set`, a plain write: it holds no secret, the helper's command and never its key |

A copy that cannot be read or parsed reads as signed out, or as the default configuration, with a note that never quotes it, and a missing one silently. The `config` commands alone fail on a `config.json` that does not parse.

## Security

- **One host.** Every request goes to the base URL, a redirect is answered as an error so a bearer never follows one, and the browser opens only on the endpoint's own origin.
- **TLS to every endpoint but a loopback one.** rustls with bundled web PKI roots, the same on every host; plain HTTP is refused before anything is sent unless the host is a loopback one, and a request to this machine never goes through a proxy.
- **Every request has a deadline**, set past the console's own wait on the server, so a stalled host fails in seconds.
- **No secret is printed.** No token, refresh token, device code or API key reaches stdout, stderr or `-v`'s log, a key refused by its shape is named by where it came from, never by its value, and saved credentials that do not parse are logged under `-v` by line and column, never by their contents.
- **A key never rides a command line.** `login --with-key` reads standard input and refuses a terminal, so no key reaches `ps`, shell history or a CI log, and `TELMONI_API_KEY` is read once, at the edge, and never saved.
- **A checkout cannot steer a released binary.** A `.env` is read in debug builds only, and neither file is ever read from the working directory.
- **The supply chain is checked.** `--locked` on every gate step that resolves dependencies, `cargo deny` on every run and daily, `unsafe` forbidden, and every release's archives and installer attested for the release workflow.

## Distribution

- **The gate** is `cargo xtask ci`, the same locally and in CI, run in CI on both shipped platforms.
- **A release** is an annotated `v*` tag matching the workspace's version: each platform built natively, the archives and `install.sh` attested, and a GitHub release made from the tag.
- **`install.sh`** picks the archive for the machine, checks it against `SHA256SUMS`, and installs it. The checksum catches a corrupted download; the provenance, which `gh attestation verify` checks, catches a swapped one.
- **The SDKs are not released** (*The SDKs*).

## When something fails

| Fails | What a person sees | What recovers it |
|---|---|---|
| **The platform, or the network** | The platform's message or, when nothing answered, the URL and the cause; the session kept | The platform |
| **A refresh whose answer is lost** | Nothing: it is asked again once, within the platform's grace | — |
| **A slow or stalled host** | A timeout in seconds, never a hang | The host |
| **The session, ended in the console** | "session ended; run `telmoni login`", and the saved login deleted | `telmoni login` |
| **A 401 of another type** | Its message; the session kept | The platform |
| **A clock that runs slow** | Nothing: the refresh after the fact covers what the early one missed | — |
| **A crash mid-write** | The previous credentials file, whole | — |
| **A locked Keychain, over SSH** | The login kept in the file instead, with a note; one saved in the Keychain unread, with a note | Unlocking it, or signing in again |
| **A failing `api_key_helper`** | Its stderr, then `api_key_helper failed`, printed nothing, or did not answer; no request | The helper, or `config set api_key_helper` |
| **A file that does not parse** | Signed out, or the default configuration, with a note; the `config` commands fail on it | `telmoni login` for the credentials; mending or deleting `config.json` |
| **No configuration directory** | The command says so and exits 1 | The environment |
| **A browser that cannot open** | A note; the code and the URL are printed already | Any browser |
| **An organization's URL changed** | The new slug refused until the cache is rewritten; the old one held to the platform's answer by `status` and `org switch` | `telmoni status` |

## What comes next

- **The SDKs grow clients together**, once the platform serves the lanes they would call (*The SDKs*, "Growing a client").
- **A command that touches customer data** comes after its lane in the platform: `/cli` for a signed-in person, `/v1` for an API key (`AGENTS.md`).
- **The platform plans a host for machines**, apart from its console, and the console's `/v1` and `/cli` relays go with it ([the platform's design](https://github.com/telmoni/telmoni/blob/main/ARCHITECTURE.md#where-it-goes-next)). Those are the lanes the CLI calls.

## Decisions

| Decision | Why | What it gives up |
|---|---|---|
| **The device flow, not a loopback redirect** | The CLI runs over SSH, in containers and on hosts with no browser | The person types a code |
| **One base URL, through the console's doors** | One address to configure, and the CLI never needs the server's | Every request takes the console's hop |
| **The platform's opaque tokens, the platform deciding** | No token is verified here, so nothing here can be wrong about one | A request to learn whether a session lives |
| **A 401 read by its type, one refresh deciding** | A blip, a botched secret rotation or a proxy cannot sign every CLI out | The rules follow the platform's problem types, held in step by hand |
| **One retry of a refresh with no answer** | The platform rotates the refresh token on use, and a spent one presented late ends the session | A second request when the first answer was lost |
| **The login in the macOS Keychain on a Mac, else a `0600` file** | The Keychain encrypts it and asks before another program reads it; a container, a CI runner or a locked Keychain over SSH has none to ask, so the file, written atomically, stands in | On Linux, whoever can read the person's files can read the tokens; a Mac may ask again after the binary is replaced |
| **A key for one command from `TELMONI_API_KEY` or a helper, ahead of the saved login** | A CI job or a secret manager needs no `login` and leaves no key on disk, as `GH_TOKEN` and `ANTHROPIC_API_KEY` work | A key in the environment reaches every process the shell starts |
| **A blank variable is no variable** | An exported empty `TELMONI_API_KEY` would otherwise stand in for the saved login with no key at all; Anthropic's SDKs tell people to `unset` such a variable, and this needs no such rule | `TELMONI_API_KEY=""` cannot name an empty key |
| **`login --with-key` reads standard input, never an argument** | A key on the command line reaches `ps`, shell history and CI logs | A key typed at a terminal is refused: it is piped, or read from a file |
| **The environment at the edge, a `.env` in debug builds only** | A released binary run in somebody else's checkout cannot take its endpoint or key | A `.env` does nothing for a released binary |
| **Organizations by id or slug, never by label** | A label is neither unique nor stable, and a wrong guess acts in another organization | People type a slug, not a name; and a slug typed with a capital letter, which the console accepts, is unknown here |
| **No redirect followed, and plain HTTP only to a loopback endpoint** | A bearer must neither follow a redirect nor cross a network in the clear | A platform behind a redirect, or plain HTTP to another host, does not work |
| **This machine past any proxy** | A proxy would carry plain HTTP to this machine off it in the clear, and a remote one cannot reach its ports | A proxy set up to watch local traffic sees none of the CLI's |
| **rustls with bundled roots** | The same trust on every machine | A root added to the system's store is not trusted |
| **`Debug` written by hand wherever a secret lives** | A log line or a failing test prints no token, and a field added later must be placed | Each such struct carries its own `Debug` |
| **SDKs configuration-only, growing together** | A client ahead of its contract freezes a guess, and a language that lags is a customer who cannot start | No SDK call works today |
| **One gate, locally and in CI** | CI's Rust job is this same command, on both shipped platforms | The SDKs' checks and the workflow lint are CI jobs beside it, outside the gate |
| **Native builds, and provenance instead of a key** | Each archive is built on the platform it runs on, where CI's gate passed, and no signing key is kept anywhere: the certificate is minted per run | Two platforms; checking provenance needs `gh` |

## Known gaps

The ones that shape the design, each kept here until it is closed:
- **Two CLI processes writing at once** are not locked against each other: the last rename wins.
- **Signing in again** leaves the earlier session live, under Active sessions, until it ends there.
- **`install.sh` checks the checksum, not the provenance**, since checking it needs `gh`.
- **`slow_down` grows the interval for good**, where the platform holds a poll to the interval it started with; harmless.
- **Some numbers are unnamed literals:** the 60-second refresh skew, the 5-second `slow_down` step and the 1-second interval floor.
- **Untested:** the real transport's deadlines and connection failures, the direct client's lack of a proxy, the browser opening, and a signed-in run of the binary.
- **The SDKs differ at the edges**, over an empty endpoint passed explicitly, a whitespace `TELMONI_ENDPOINT`, and an empty `TELMONI_ORG` or `TELMONI_API_KEY`; Go's example never runs, and TypeScript's tests import the sources rather than the built package.
- **`output_format` is accepted and stored**, and no command reads it.
