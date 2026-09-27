package telmoni_test

import (
	"testing"

	telmoni "github.com/kendricklawton/telmoni/sdk/go"
)

func TestClientInit(t *testing.T) {
	client := telmoni.New(telmoni.Config{})
	if client == nil {
		t.Fatalf("Expected client to be initialized")
	}
	if client.Config().Endpoint != telmoni.DefaultEndpoint {
		t.Errorf("Expected endpoint %s, got %s", telmoni.DefaultEndpoint, client.Config().Endpoint)
	}
}

func TestClientFromEnv(t *testing.T) {
	client := telmoni.NewFromEnv()
	if client == nil {
		t.Fatalf("Expected client to be initialized from env")
	}
}
