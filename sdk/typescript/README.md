# Telmoni TypeScript / JavaScript SDK

A scaffold: configuration (endpoint, API key, organization) and nothing that
calls the platform yet. It grows a client once its contract exists.

---

## Installation

Not published yet. Build it from a checkout of this repository:

```console
cd sdk/typescript && npm ci && npm run build
```

---

## Quickstart

### 1. From Environment Variables

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
  organizationId: 'org_123',
});
```

---

## JavaScript (CommonJS) Usage

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
| `TELMONI_API_KEY` / `TELMONI_AUTH_TOKEN` | API key |
| `TELMONI_ORG` | Default organization ID |

---

## License

Apache-2.0
