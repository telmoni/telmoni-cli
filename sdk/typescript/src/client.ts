import type { Config } from './types.js';

export const DEFAULT_ENDPOINT = 'https://telmoni.com';

/**
 * The primary client for the Telmoni platform.
 */
export class Telmoni {
  public readonly config: Required<Pick<Config, 'endpoint'>> & Config;

  constructor(config?: Config) {
    const rawEndpoint = config?.endpoint || DEFAULT_ENDPOINT;
    this.config = {
      endpoint: rawEndpoint.replace(/\/+$/, ''),
      apiKey: config?.apiKey,
      tenantId: config?.tenantId,
    };
  }

  /**
   * Initializes a Telmoni client automatically from environment variables:
   * - `TELMONI_ENDPOINT` / `TELMONI_API_URL`
   * - `TELMONI_API_KEY` / `TELMONI_AUTH_TOKEN`
   * - `TELMONI_TENANT_ID` / `TELMONI_TEAM_ID`
   */
  public static fromEnv(): Telmoni {
    const env = typeof process !== 'undefined' ? process.env : undefined;

    const endpoint = env?.['TELMONI_ENDPOINT'] || env?.['TELMONI_API_URL'] || DEFAULT_ENDPOINT;
    const apiKey = env?.['TELMONI_API_KEY'] || env?.['TELMONI_AUTH_TOKEN'];
    const tenantId = env?.['TELMONI_TENANT_ID'] || env?.['TELMONI_TEAM_ID'];

    return new Telmoni({
      endpoint,
      apiKey,
      tenantId,
    });
  }
}
