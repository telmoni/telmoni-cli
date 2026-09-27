# Telmoni Go SDK

The official Go SDK for the **Telmoni** platform.

## Installation

```console
go get github.com/kendricklawton/telmoni/sdk/go
```

## Quick Start

```go
package main

import (
	telmoni "github.com/kendricklawton/telmoni/sdk/go"
)

func main() {
	// Initialize from environment (TELMONI_ENDPOINT, TELMONI_API_KEY, TELMONI_TENANT_ID)
	client := telmoni.NewFromEnv()
	_ = client
}
```
