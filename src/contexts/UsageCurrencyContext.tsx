import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  type ReactNode,
} from "react";
import { fmtCny as formatCny } from "@/components/usage/format";
import { useUsdCnyRateQuery } from "@/lib/query/exchangeRate";

const FALLBACK_RATE = 7.14;

export interface UsageCurrency {
  /** 1 USD = rate CNY */
  rate: number;
  /** Date of the quoted rate */
  rateDate: string | null;
  /** er-api | sina | cached | fallback */
  source: string;
  /** Using a previous fetch or the hard fallback */
  stale: boolean;
  /** Format a USD amount as CNY using the current rate */
  fmtCny: (value: unknown, digits?: number, fallback?: string) => string;
}

const fallbackFmtCny = (value: unknown, digits = 2, fallback = "--") =>
  formatCny(value, digits, FALLBACK_RATE, fallback);

const UsageCurrencyContext = createContext<UsageCurrency>({
  rate: FALLBACK_RATE,
  rateDate: null,
  source: "fallback",
  stale: true,
  fmtCny: fallbackFmtCny,
});

export function UsageCurrencyProvider({ children }: { children: ReactNode }) {
  const { data } = useUsdCnyRateQuery();
  const rate =
    data?.rate != null && Number.isFinite(data.rate) && data.rate > 0
      ? data.rate
      : FALLBACK_RATE;
  const fmtCny = useCallback(
    (value: unknown, digits = 2, fallback = "--") =>
      formatCny(value, digits, rate, fallback),
    [rate],
  );
  const value = useMemo<UsageCurrency>(
    () => ({
      rate,
      rateDate: data?.rateDate ?? null,
      source: data?.source ?? "fallback",
      stale: data?.stale ?? true,
      fmtCny,
    }),
    [data?.rateDate, data?.source, data?.stale, fmtCny, rate],
  );

  return (
    <UsageCurrencyContext.Provider value={value}>
      {children}
    </UsageCurrencyContext.Provider>
  );
}

export const useUsageCurrency = () => useContext(UsageCurrencyContext);
