import { useQuery } from "@tanstack/react-query";
import { exchangeRateApi, type UsdCnyRate } from "@/lib/api/exchangeRate";

export const exchangeRateKeys = {
  current: ["usdCnyRate"] as const,
};

/** 当天 USD→CNY 汇率；后端按自然日缓存，这里每小时检查一次跨天更新。 */
export const useUsdCnyRateQuery = () =>
  useQuery<UsdCnyRate>({
    queryKey: exchangeRateKeys.current,
    queryFn: () => exchangeRateApi.get(),
    staleTime: 60 * 60 * 1000,
    refetchInterval: 60 * 60 * 1000,
    retry: 1,
  });
