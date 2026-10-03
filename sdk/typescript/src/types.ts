/**
 * Client configuration options for the Telmoni SDK.
 */
export interface Config {
  /**
   * Telmoni API endpoint URL (default: `https://telmoni.com`).
   */
  endpoint?: string | undefined;

  /**
   * The API key (`telmoni_…`); `/v1` takes no other bearer.
   */
  apiKey?: string | undefined;

  /**
   * Organization identifier (`org_…`).
   */
  organizationId?: string | undefined;
}
