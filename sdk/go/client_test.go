package telmoni_test

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	telmoni "github.com/telmoni/telmoni-cli/sdk/go"
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

func TestNeverPrintsTheKey(t *testing.T) {
	cfg := telmoni.Config{OrganizationID: "org_1", APIKey: "telmoni_secret_key"}
	client := telmoni.New(cfg)

	marshalled, err := json.Marshal(cfg)
	if err != nil {
		t.Fatalf("marshalling the configuration: %v", err)
	}
	for _, printed := range []string{
		fmt.Sprint(cfg), fmt.Sprintf("%+v", cfg), fmt.Sprintf("%#v", cfg),
		fmt.Sprint(client), fmt.Sprintf("%+v", client), fmt.Sprintf("%#v", client),
		string(marshalled),
	} {
		if strings.Contains(printed, "telmoni_secret_key") {
			t.Errorf("printed the key: %s", printed)
		}
		if !strings.Contains(printed, "org_1") {
			t.Errorf("lost the organization: %s", printed)
		}
	}
	if client.Config().APIKey != "telmoni_secret_key" {
		t.Errorf("Expected the key to stay readable, got %q", client.Config().APIKey)
	}
}

func TestClientFromEnv(t *testing.T) {
	client := telmoni.NewFromEnv()
	if client == nil {
		t.Fatalf("Expected client to be initialized from env")
	}
}
