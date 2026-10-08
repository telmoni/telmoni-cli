# Telmoni CLI

[![CI](https://github.com/telmoni/telmoni-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/telmoni/telmoni-cli/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

`telmoni`, the command-line client for [Telmoni](https://github.com/telmoni/telmoni), and the client SDKs. It talks to one platform, `https://telmoni.com` unless `TELMONI_ENDPOINT` names another, and to nothing else.

## Install

```console
curl -fsSL https://raw.githubusercontent.com/telmoni/telmoni-cli/main/install.sh | sh
```

The installer fetches the latest release for Apple-silicon macOS or x86_64 Linux, checks it against the release's `SHA256SUMS`, and installs `telmoni` into `/usr/local/bin` or `~/.local/bin`. `TELMONI_VERSION` pins a release and `TELMONI_INSTALL_DIR` picks the directory. On another platform, build from source with the Rust toolchain:

```console
cargo install --git https://github.com/telmoni/telmoni-cli.git --locked telmoni-cli
```

Each release's archives and `install.sh` also carry build provenance, signed through Sigstore for the release workflow; with the GitHub CLI you can check a download against it:

```console
gh attestation verify telmoni-linux-x86_64.tar.gz --repo telmoni/telmoni-cli
```

## Use

```console
telmoni login       # device-code sign-in: a code to type into any browser
telmoni status      # who you are and the active organization (--json for scripts)
telmoni org list    # the organizations you belong to; `org switch` changes the active one
telmoni logout
```

`telmoni login --key telmoni_…` signs in with an API key, for CI and servers. Every command, flag and variable is documented at [telmoni.com/docs/api/cli](https://telmoni.com/docs/api/cli).

Add `-v` to any command to log to stderr which files it read and wrote, where its endpoint came from, and each request and answer. It never logs a token or a key.

## SDKs

`sdk/` holds configuration-only scaffolds for TypeScript, Python, Go and Rust; each grows a client once its contract exists. See [telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks).

## Layout

```text
src/            the CLI: commands, the device-flow sign-in, the HTTP transport
sdk/            the SDK scaffolds: typescript/, python/, go/, rust/
tests/          mock-based suites, no network
xtask/          the gate (`cargo xtask ci`) and release packaging (`cargo xtask dist`)
architecture/   how it is built, and why
```

## Contributing

How to contribute, the AI policy, the Code of Conduct and the security policy are in the docs: [telmoni.com/docs/contributing](https://telmoni.com/docs/contributing/introduction). [Developing](https://telmoni.com/docs/contributing/developing#the-cli-and-sdks) gets you building.

## License

Apache-2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)). Unless you say otherwise, a contribution you submit is licensed the same way.
