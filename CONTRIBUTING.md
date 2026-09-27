# Contributing to Telmoni CLI

Thanks for your interest in contributing to the Telmoni CLI (`telmoni`).

## Guidelines

- **Open an issue first for major changes.** Before starting work on new subcommands or significant structural changes, open an issue so design and scope can be aligned.
- **Sign your commits.** Use `git commit -s` to add a `Signed-off-by:` line under the [Developer Certificate of Origin](https://developercertificate.org/). Contributions are licensed under **Apache-2.0** (see [`LICENSE`](LICENSE)).
- **Keep `cargo xtask ci` green.** Every change must pass:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo build --workspace --locked`
  - `cargo test --workspace --locked`
  - `cargo doc --no-deps --workspace --locked`
  - `cargo deny check`
- **Zero unhandled panics.** Workspace clippy lints strictly deny `unwrap_used`, `expect_used`, `panic`, `todo`, and `unimplemented`. All fallible operations must use `Result` or `Option` with descriptive error messages.
- **Security & Auth.** Authentication follows the RFC 8628 device authorization grant through Telmoni's `/cli` door and token storage (`0600` file permissions). Never log or expose secrets, access tokens, refresh tokens, device codes, or API keys.

## Running Tests and Validation

```console
cargo xtask ci
```
