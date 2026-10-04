package telmoni

import (
	"os"
	"strings"
)

// DefaultEndpoint is the default Telmoni API endpoint URL.
const DefaultEndpoint = "https://telmoni.com"

// Config holds client configuration for the Telmoni platform.
type Config struct {
	Endpoint       string `json:"endpoint"`
	OrganizationID string `json:"organization_id,omitempty"`
	APIKey         string `json:"api_key,omitempty"`
}

// DefaultConfig returns the default SDK configuration.
func DefaultConfig() Config {
	return Config{
		Endpoint: DefaultEndpoint,
	}
}

// ConfigFromEnv reads configuration from standard Telmoni environment variables.
func ConfigFromEnv() Config {
	endpoint := os.Getenv("TELMONI_ENDPOINT")
	if endpoint == "" {
		endpoint = DefaultEndpoint
	}

	return Config{
		Endpoint:       strings.TrimRight(endpoint, "/"),
		OrganizationID: os.Getenv("TELMONI_ORG"),
		APIKey:         os.Getenv("TELMONI_API_KEY"),
	}
}

// Client is the primary entrypoint for communicating with Telmoni.
type Client struct {
	config Config
}

// Telmoni is a convenience alias for Client.
type Telmoni = Client

// New creates a new Telmoni client with the provided configuration.
func New(cfg Config) *Client {
	if cfg.Endpoint == "" {
		cfg.Endpoint = DefaultEndpoint
	}
	cfg.Endpoint = strings.TrimRight(cfg.Endpoint, "/")

	return &Client{
		config: cfg,
	}
}

// NewFromEnv creates a new Telmoni client initialized from environment variables.
func NewFromEnv() *Client {
	return New(ConfigFromEnv())
}

// Config returns the active client configuration.
func (c *Client) Config() Config {
	return c.config
}
