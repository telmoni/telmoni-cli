# Telmoni Rust SDK

`telmoni-sdk`, a scaffold: configuration (endpoint, API key, organization) and
nothing that calls the platform yet. It grows a client once its contract
exists.

## Installation

Not published yet. Depend on it from this repository:

```toml
[dependencies]
telmoni-sdk = { git = "https://github.com/telmoni/telmoni-cli" }
```

## Quick Start

```rust
use telmoni_sdk::Telmoni;

// Reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG
let client = Telmoni::from_env();
```
