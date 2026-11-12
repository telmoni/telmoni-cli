# Telmoni CLI

[![CI](https://github.com/telmoni/telmoni-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/telmoni/telmoni-cli/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

`telmoni`, the command-line client for [Telmoni](https://github.com/telmoni/telmoni), privacy-first, open-source monitoring for AI agents: sign in from a terminal or CI, see who you are, and switch organizations. Telmoni is self-hosted: the CLI talks to one platform, your deployment, and to nothing else, and has no default, so it calls nothing until you name it with `telmoni login --endpoint <URL>`, `TELMONI_ENDPOINT` or `telmoni config set endpoint <URL>`. The SDKs that record an agent's traces are the platform's, under [`sdk/`](https://github.com/telmoni/telmoni/tree/main/sdk) in that repository, and documented in [`web/content/docs/api/sdks.mdx`](https://github.com/telmoni/telmoni/blob/main/web/content/docs/api/sdks.mdx); prompts stay in your process unless you turn content on.

## Install

```console
curl -fsSL https://github.com/telmoni/telmoni-cli/releases/latest/download/install.sh | sh
```

The installer, attached to each [GitHub release](https://github.com/telmoni/telmoni-cli/releases), fetches the latest release for x86_64 Linux, checks it against the release's `SHA256SUMS`, and installs `telmoni` into `/usr/local/bin` or `~/.local/bin`. `TELMONI_VERSION` pins a release and `TELMONI_INSTALL_DIR` picks the directory. On another platform, a Mac among them until its build is signed, build from source with the Rust toolchain:

```console
cargo install --locked telmoni-cli
```

Each release's archives and `install.sh` also carry build provenance, signed through Sigstore for the release workflow; with the GitHub CLI you can check a download against it:

```console
gh attestation verify telmoni-linux-x86_64.tar.gz --repo telmoni/telmoni-cli
```

## Use

```console
telmoni login --endpoint https://telmoni.example.com
                    # your deployment's URL, saved with the login; then a code to type into any browser
telmoni status      # who you are and the active organization (--json for scripts)
telmoni org list    # the organizations you belong to; `org switch` changes the active one
telmoni logout
```

For CI and servers, set `TELMONI_API_KEY`: each command uses the key and saves nothing, and at a terminal asks once, for each new key, whether to use it ahead of the saved login. `telmoni login --with-key < key.txt` saves one instead, read from standard input so it never sits on the command line. On a Mac the saved login lives in the Keychain. Every command, flag and variable is documented in [`web/content/docs/api/cli.mdx`](https://github.com/telmoni/telmoni/blob/main/web/content/docs/api/cli.mdx).

Add `-v` to any command to log to stderr which files it read and wrote, where its endpoint came from, and each request and answer. It never logs a token or a key.

## Layout

```text
src/            the CLI: commands, the device-flow sign-in, the HTTP transport
tests/          mock-based suites, no network
xtask/          the gate (`cargo xtask ci`) and release packaging (`cargo xtask dist`)
install.sh      the installer: this machine's archive from a release, checked against SHA256SUMS
ARCHITECTURE.md how it is built, and why, on one page
```

## Contributing

How to contribute is in [CONTRIBUTING.md](https://github.com/telmoni/.github/blob/main/CONTRIBUTING.md), with the [AI policy](https://github.com/telmoni/telmoni/blob/main/web/content/docs/contributing/ai-policy.mdx) and the [Code of Conduct](https://github.com/telmoni/telmoni/blob/main/web/content/docs/contributing/code-of-conduct.mdx) beside it in the docs; how to report a vulnerability is in [SECURITY.md](https://github.com/telmoni/.github/blob/main/SECURITY.md). [Developing](https://github.com/telmoni/telmoni/blob/main/web/content/docs/contributing/developing.mdx#the-cli) gets you building.

## License

Apache-2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)). Unless you say otherwise, a contribution you submit is licensed the same way.
