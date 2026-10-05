import { inspect } from 'node:util';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { Telmoni, DEFAULT_ENDPOINT, type Config } from '../src/index';

afterEach(() => vi.unstubAllEnvs());

describe('Telmoni TypeScript SDK', () => {
  it('should initialize with default config', () => {
    const client = new Telmoni();
    expect(client.config.endpoint).toBe(DEFAULT_ENDPOINT);
    expect(client.config.endpoint).toBe('https://telmoni.com');
  });

  it('should initialize with custom config and strip trailing slashes', () => {
    const config: Config = {
      endpoint: 'https://custom.endpoint///',
      apiKey: 'telmoni_test_key_123',
    };
    const client = new Telmoni(config);
    expect(client.config.endpoint).toBe('https://custom.endpoint');
    expect(client.config.apiKey).toBe('telmoni_test_key_123');
  });

  it('never prints or serializes the API key', () => {
    const client = new Telmoni({ apiKey: 'telmoni_secret_key', organizationId: 'org_1' });
    for (const printed of [
      inspect(client),
      JSON.stringify(client),
      inspect(client.config),
      JSON.stringify(client.config),
    ]) {
      expect(printed).not.toContain('telmoni_secret_key');
      expect(printed).toContain('org_1');
    }
    expect(client.config.apiKey).toBe('telmoni_secret_key');
  });

  it('should initialize from environment', () => {
    // The shell running the suite may point the CLI at a local stack; the
    // SDK's default is what is under test.
    vi.stubEnv('TELMONI_ENDPOINT', undefined);
    const client = Telmoni.fromEnv();
    expect(client.config.endpoint).toBe(DEFAULT_ENDPOINT);
  });
});
