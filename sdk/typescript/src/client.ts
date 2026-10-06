import type { Config } from './types.js';

export const DEFAULT_ENDPOINT = 'https://telmoni.com';

// Not /\/+$/: that regex takes quadratic time on a long run of slashes that
// something follows, and the endpoint can come from anywhere.
function trimTrailingSlashes(value: string): string {
  let end = value.length;
  while (end > 0 && value[end - 1] === '/') end--;
  return value.slice(0, end);
}

/**
 * The primary client for the Telmoni platform.
 */
export class Telmoni {
  public readonly config: Required<Pick<Config, 'endpoint'>> & Config;

  constructor(config?: Config) {
    const rawEndpoint = config?.endpoint || DEFAULT_ENDPOINT;
    this.config = {
      endpoint: trimTrailingSlashes(rawEndpoint),
      organizationId: config?.organizationId,
    };
    // Not enumerable: console.log, util.inspect and JSON.stringify skip it, so
    // a client or a configuration logged or serialized whole leaves the key out.
    Object.defineProperty(this.config, 'apiKey', { value: config?.apiKey, enumerable: false });
  }

  /**
   * Initializes a Telmoni client automatically from environment variables:
   * - `TELMONI_ENDPOINT`
   * - `TELMONI_API_KEY`
   * - `TELMONI_ORG`
   */
  public static fromEnv(): Telmoni {
    const env = typeof process !== 'undefined' ? process.env : undefined;

    const endpoint = env?.['TELMONI_ENDPOINT'] || DEFAULT_ENDPOINT;
    const apiKey = env?.['TELMONI_API_KEY'];
    const organizationId = env?.['TELMONI_ORG'];

    return new Telmoni({
      endpoint,
      apiKey,
      organizationId,
    });
  }
}
