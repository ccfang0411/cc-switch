import { invoke } from "@tauri-apps/api/core";

export interface UsdCnyRate {
  /** 1 USD = rate CNY */
  rate: number;
  /** Date of the quoted rate, e.g. 2026-10-19 */
  rateDate: string;
  /** Local date when this value was fetched */
  fetchedDate: string;
  /** er-api | sina | cached | fallback */
  source: string;
  /** Using a previous fetch or the hard fallback */
  stale: boolean;
}

export const exchangeRateApi = {
  async get(): Promise<UsdCnyRate> {
    return await invoke("get_usd_cny_rate");
  },
};
