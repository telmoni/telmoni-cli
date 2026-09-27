# Telmoni Rust SDK

The official Rust SDK (`telmoni-sdk`) for the **Telmoni** platform.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
telmoni-sdk = "0.0.1"
```

## Quick Start

```rust
use telmoni_sdk::Telmoni;

// Automatically loads from TELMONI_ENDPOINT, TELMONI_API_KEY, TELMONI_TENANT_ID
let client = Telmoni::from_env();
```
