# Transport

Every request the CLI makes goes through one seam, the `Transport` trait (`src/transport.rs`), to one base URL. Behind it are the platform's console routes:
- the `/cli` door, for the person's session;
- `/v1`, for API keys.

The CLI talks to nothing else.

⚠ **One base URL, and no other hostname.** It is `https://telmoni.com` unless `TELMONI_ENDPOINT` says otherwise, and `http://localhost:3000` locally. No other host appears in `src/` or `sdk/` (AGENTS.md). The console relays everything to the server, so the CLI never needs the server's address.

## Contents

- [The seam](#the-seam)
- [What every request carries](#what-every-request-carries)
- [Errors](#errors)
- [The /v1 client](#the-v1-client)
- [What is never printed](#what-is-never-printed)
- [Where it lives](#where-it-lives)

## The seam

`Transport::send(LaneRequest) -> LaneAnswer`:
- `LaneRequest` carries the method, the URL, an optional bearer, an optional organization and an optional JSON body.
- `LaneAnswer` carries the status, the content type and the body.

The real transport is `reqwest`. Tests substitute a mock that answers from a queue and records every request (see [build](build.md#tests)), so no test touches the network.

**TLS** is rustls with bundled web PKI roots, not the operating system's trust store. The CLI trusts the same roots on every machine.

⚠ **Plain HTTP reaches only this machine** (`refuse_cleartext`). The real transport refuses an `http://` URL before anything is sent, unless its host is `localhost`, a name under it, or a loopback address (`127.0.0.0/8`, `[::1]`).
- **It does not ask what the request carries.** Every lane sends a credential or is answered one: the device start sends none, and is answered the device code. Judged request by request, a login against a plain-HTTP host printed a code and opened a browser before its first poll was refused.
- **The host is read from the parsed URL.** An IPv6 address comes in brackets, and `localhost.example.com` is not this machine.
- So `http://localhost:3000` works, and a plain-HTTP endpoint anywhere else fails at its first request. That includes a local platform reached by another name, from inside a container for one.

**There is no client-side timeout.** The client sets only its user agent, and `reqwest` defaults to none. The console's door cuts its own hop to the platform's server at its upstream timeout, but a stalled connection to the console itself would wait indefinitely.

## What every request carries

- **`User-Agent: telmoni-cli/<version> (<os>; <arch>)`**, set on the client, so it is on every request. The platform records it on the session, and the person sees "Telmoni CLI (macOS)" under Active sessions on their Privacy page.
- **`Authorization: Bearer …`** on `/cli/me` and the session revoke (the access token), and on `/v1` (the API key). The device flow's start and poll, and the refresh, carry none; the refresh token travels in the body.
- **`x-organization-id`** when the call acts inside an organization: `status`'s and `org switch`'s `/cli/me`, and the session revoke. It always carries the id, however the person named the organization (see [commands](commands.md#organizations)).
  - Calls that do not send it: login's first `/cli/me`, logout's fallback `/cli/me`, `/v1` (an API key is its organization), and a revoke when no organization is cached.
  - The door forwards the header only on lanes that carry a bearer.
- **JSON bodies, in camel case.** The door converts the refresh body to the platform's snake case, and re-encodes every body through its own schema, so only known fields cross.

## Errors

`parse_lane_error` reads a failed answer in the platform's three shapes, and a fallback:

| Answer | Printed as |
|---|---|
| An RFC 9457 problem (`application/problem+json`) | `title: detail`. `retry_after_secs` is read from the body, not the header. |
| `{ "error": "…" }` (nothing the CLI calls answers this shape today — the `/cli` door and `/v1` answer problem documents even for an unreachable server or database; the parser stays for whatever sits in front of them) | That string |
| Anything else | `request failed (<status>)`, and the first characters of the body |
| 426 | "this CLI is too old; upgrade it". Nothing on the platform sends a 426 yet. |

**The error keeps its status and problem type**, so callers can downcast it and act on it. That is how the 401 rules work (see [auth](auth.md#when-the-session-has-ended)). The device flow's start is the one call that flattens its error to text.

## The /v1 client

`src/client.rs` is the CLI's only `/v1` call: `GET {endpoint}/v1/organization`, with the API key as the bearer.
- The answer is the platform's snake-case organization: its id, its slug, its name, and its owner, or none.
- The platform checks the key on every request: live, its organization active, the organization's beta access and the public API switched on.
- The console's `/v1` relay allows only `GET` and `HEAD`, and meters each key and each source address.

A `/v1` error never touches the credentials file. An API key's validity is the platform's to decide, request by request.

## What is never printed

- The access token, the refresh token, the device code and the API key.
- The user code, the verification URL and the endpoint are shown. They are not secrets.

⚠ **`Debug` is derived unmasked** on `Credentials`, `AuthnResult`, `DeviceStart` and `LaneRequest`. No code formats them today. Formatting one, in a log line or a test failure, would print a token. The platform masks its equivalents by hand.

## Where it lives

| Concern | File |
|---|---|
| The seam, the real transport, the plain-HTTP refusal, error shapes | `src/transport.rs` |
| The `/v1` client | `src/client.rs` |
| The platform's side | `telmoni/telmoni`: `web/app/cli/[...path]/route.ts` (the `/cli` door), `web/app/v1/[...path]/route.ts` (the `/v1` relay), `crates/auth/src/handler/v1.rs` |
