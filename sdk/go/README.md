# Telmoni Go SDK

A scaffold: configuration (endpoint, API key, organization) and nothing that
calls the platform yet. It grows a client once its contract exists.

## Installation

Not published yet. Use it from this repository:

```console
go get github.com/telmoni/telmoni-cli/sdk/go
```

## Quick Start

```go
package main

import (
	telmoni "github.com/telmoni/telmoni-cli/sdk/go"
)

func main() {
	// Reads TELMONI_ENDPOINT, TELMONI_API_KEY and TELMONI_ORG
	client := telmoni.NewFromEnv()
	_ = client
}
```
