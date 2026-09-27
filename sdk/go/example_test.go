package telmoni_test

import (
	telmoni "github.com/kendricklawton/telmoni/sdk/go"
)

func ExampleTelmoni() {
	// Initialize using default configuration
	_ = telmoni.New(telmoni.DefaultConfig())

	// Or initialize from environment variables (TELMONI_ENDPOINT, TELMONI_API_KEY)
	_ = telmoni.NewFromEnv()
}
