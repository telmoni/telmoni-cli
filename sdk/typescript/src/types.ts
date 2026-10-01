/**
 * Client configuration options for the Telmoni SDK.
 */
export interface Config {
  /**
   * Telmoni API endpoint URL (default: `https://telmoni.com`).
   */
  endpoint?: string | undefined;

  /**
   * API Key or bearer token used for authenticating requests.
   */
  apiKey?: string | undefined;

  /**
   * Organization identifier (`org_…`).
   */
  organizationId?: string | undefined;
}
