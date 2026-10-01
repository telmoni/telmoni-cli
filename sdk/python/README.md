# Telmoni Python SDK

A scaffold: configuration (endpoint, API key, organization) and nothing that
calls the platform yet. It grows a client once its contract exists.

## Installation

Not published yet. Install it from a checkout of this repository:

```console
pip install ./sdk/python
```

## Quick Start

```python
from telmoni import Telmoni

# Reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG
client = Telmoni.from_env()
```
