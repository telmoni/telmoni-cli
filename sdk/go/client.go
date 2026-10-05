package telmoni

import (
	"fmt"
	"os"
	"strings"
)

// DefaultEndpoint is the default Telmoni API endpoint URL.
const DefaultEndpoint = "https://telmoni.com"

// Config holds client configuration for the Telmoni platform.
type Config struct {
	Endpoint       string `json:"endpoint"`
	OrganizationID string `json:"organization_id,omitempty"`
	// APIKey is the API key (telmoni_…). It is never marshalled, and String
	// and GoString redact it: a configuration logged or written out would
	// otherwise carry the key.
	APIKey string `json:"-"`
}

// String formats the configuration for %v and %s, with the API key redacted.
func (c Config) String() string {
	return fmt.Sprintf("{Endpoint:%s OrganizationID:%s APIKey:%s}",
		c.Endpoint, c.OrganizationID, redacted(c.APIKey))
}

// GoString formats the configuration for %#v, with the API key redacted.
func (c Config) GoString() string {
	return fmt.Sprintf("telmoni.Config{Endpoint:%q, OrganizationID:%q, APIKey:%q}",
		c.Endpoint, c.OrganizationID, redacted(c.APIKey))
}

func redacted(secret string) string {
	if secret == "" {
		return ""
	}
	return "<redacted>"
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

// String formats the client for %v and %s, with the API key redacted. fmt
// reaches an unexported field by reflection, past Config's own String, so
// without this the client would print the key.
func (c Client) String() string {
	return "{config:" + c.config.String() + "}"
}

// GoString formats the client for %#v, with the API key redacted.
func (c Client) GoString() string {
	return "telmoni.Client{config:" + c.config.GoString() + "}"
}
