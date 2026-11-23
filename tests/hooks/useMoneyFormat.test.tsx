import type { ReactNode } from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useMoneyFormat } from "@/hooks/useMoneyFormat";
import type { Settings } from "@/types";
import { createTestQueryClient } from "../utils/testQueryClient";

const renderWithSettings = (
  settings: Partial<Settings>,
  dailyRate?: number,
) => {
  const client = createTestQueryClient();
  client.setQueryData(["settings"], settings);
  if (dailyRate != null)
    client.setQueryData(["usdCnyRate"], { rate: dailyRate });
  return renderHook(() => useMoneyFormat(), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  }).result.current;
};

describe("useMoneyFormat", () => {
  it("shows USD when the saved display currency is USD", () => {
    const fmt = renderWithSettings({
      usageDisplay: { currency: "USD" },
    });
    expect(fmt.currency).toBe("USD");
    expect(fmt.money(1.5, 2)).toBe("$1.50");
    expect(fmt.usdHint(1.5)).toBeUndefined();
    expect(fmt.moneyTitle(1.5, 6)).toBe("$1.500000");
  });

  it("defaults to CNY with the preset rate", () => {
    const fmt = renderWithSettings({});
    expect(fmt.currency).toBe("CNY");
    expect(fmt.rate).toBe(6.8);
    expect(fmt.money(1.5, 2)).toBe("¥10.20");
    expect(fmt.usdHint(1.5)).toBe("$1.500000");
  });

  it("keeps daily CNY conversion for legacy settings", () => {
    expect(renderWithSettings({}, 7.14).rate).toBe(7.14);
  });

  it("honors the explicit rate source even when a daily CNY rate is cached", () => {
    expect(
      renderWithSettings(
        { usageDisplay: { currency: "CNY", rateSource: "manual" } },
        7.14,
      ).rate,
    ).toBe(6.8);
    expect(
      renderWithSettings(
        {
          usageDisplay: {
            currency: "CNY",
            rateSource: "auto",
            autoExchangeRates: { CNY: 7.2 },
          },
        },
        7.14,
      ).rate,
    ).toBe(7.2);
  });

  it("converts with the saved rate and keeps the USD value for hover", () => {
    const fmt = renderWithSettings({
      usageDisplay: { currency: "CNY", exchangeRates: { CNY: 7 } },
    });
    expect(fmt.currency).toBe("CNY");
    expect(fmt.rate).toBe(7);
    expect(fmt.money(1.5, 2)).toBe("¥10.50");
    expect(fmt.moneyShort(41.2)).toBe("¥288");
    expect(fmt.usdHint(1.5)).toBe("$1.500000");
    expect(fmt.moneyTitle(1.5, 6)).toBe("¥10.50 ($1.500000)");
  });
});
