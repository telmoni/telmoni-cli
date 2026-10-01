# Contributing to Telmoni CLI

Thanks for your interest in contributing to the Telmoni CLI (`telmoni`).

All contributors are expected to adhere to our [Code of Conduct](CODE_OF_CONDUCT.md). For security vulnerabilities, please refer to our [Security Policy](SECURITY.md).

---

## Developer Certificate of Origin (DCO)

We do not require a Contributor License Agreement (CLA). Instead, we use the standard [Developer Certificate of Origin (DCO)](https://developercertificate.org/).

By adding a `Signed-off-by:` line to your commit message, you certify that you have the right to submit the work under the project's [Apache-2.0 License](LICENSE).

Sign your commits using `git commit -s`:

```console
git commit -s -m "feat(commands): add organization switch command"
```

---

## Guidelines & Principles

- **Open an issue first for major changes.** Before starting work on new subcommands or significant structural changes, open an issue so design and scope can be aligned.
- **Client of Telmoni Only:** The CLI communicates strictly with the Telmoni platform via HTTP/JSON (`TELMONI_ENDPOINT`). It does not run a server, access a database directly, or evaluate business logic.
- **Zero Unhandled Panics:** Workspace clippy lints strictly deny `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, and `unreachable`. All fallible operations must use `Result` or `Option` with descriptive error messages.
- **No Unsafe Code:** `#![forbid(unsafe_code)]` is enforced on every crate.
- **Security & Secret Hygiene:** Authentication follows the RFC 8628 device authorization grant through Telmoni's `/cli` door and token storage (`0600` file permissions). Never log or expose secrets, access tokens, refresh tokens, device codes, or API keys.
- **Test Isolation:** Unit and integration tests must run without network access or real clock sleeps. Inject dependencies via traits and mock responses.

---

## Running Tests and Validation

Every change must pass our workspace CI task runner:

```console
cargo xtask ci
```

`cargo xtask ci` enforces with `--locked`:
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo build --workspace --locked`
- `cargo test --workspace --locked`
- `cargo doc --no-deps --workspace --locked`
- `cargo deny check`

For quick local iteration, you can run individual checks:

```console
cargo check --workspace
cargo fmt --all
cargo test
```

---

## Submitting Pull Requests

1. **Keep Pull Requests Focused:** Submit PRs that address a single issue or feature.
2. **Commit Style:** Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `refactor:`, `chore:`).
3. **Sign Your Commits:** Ensure every commit includes the DCO sign-off (`-s`).
4. **Ensure Clean CI:** Verify that `cargo xtask ci` passes cleanly before requesting review.
