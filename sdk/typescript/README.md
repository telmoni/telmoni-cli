# Telmoni TypeScript / JavaScript SDK

A scaffold: configuration (endpoint, API key, organization) and nothing that
calls the platform yet. It grows a client once its contract exists.

## Installation

Not published yet. Build it from a checkout of this repository:

```console
cd sdk/typescript && npm ci && npm run build
```

## Quick Start

```ts
import { Telmoni } from 'telmoni';

// Reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG
const telmoni = Telmoni.fromEnv();
```

The variables, their fallbacks and the other SDKs are documented at
[telmoni.com/docs/api/sdks](https://telmoni.com/docs/api/sdks).
