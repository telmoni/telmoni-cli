# Telmoni TypeScript / JavaScript SDK

The official client SDK for interacting with the Telmoni platform.

---

## Installation

```console
npm install telmoni
```

---

## Quickstart

### 1. Default Client (Environment Variables)

```ts
import { Telmoni } from 'telmoni';

const telmoni = Telmoni.fromEnv();
```

### 2. Custom Configuration

```ts
import { Telmoni } from 'telmoni';

const telmoni = new Telmoni({
  endpoint: 'https://telmoni.com',
  apiKey: process.env.TELMONI_API_KEY,
  tenantId: 'org_123',
});
```

---

## JavaScript (CommonJS) Usage

Fully compatible with standard Node.js CommonJS:

```js
const { Telmoni } = require('telmoni');

const telmoni = new Telmoni({
  apiKey: process.env.TELMONI_API_KEY,
});
```

---

## Environment Variables

| Variable | Description |
|---|---|
| `TELMONI_ENDPOINT` / `TELMONI_API_URL` | Telmoni endpoint |
| `TELMONI_API_KEY` / `TELMONI_AUTH_TOKEN` | Bearer token / API key |
| `TELMONI_TENANT_ID` / `TELMONI_ORG` | Default organization or tenant ID |

---

## License

Apache-2.0
