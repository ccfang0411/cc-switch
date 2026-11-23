import { describe, expect, it } from "vitest";
import {
  kimiConfigSchema,
  kimiProviderPresets,
} from "@/config/kimiProviderPresets";
import { kimicodeProviderPresets } from "@/config/kimicodeProviderPresets";

const normalizeUrl = (url?: string) => url?.replace(/\/+$/, "");

describe("Kimi canonical provider catalog", () => {
  it.each(kimiProviderPresets)(
    "validates $name without dropping native fields",
    (preset) => {
      expect(kimiConfigSchema.parse(preset.settingsConfig)).toEqual(
        preset.settingsConfig,
      );
      expect(Object.keys(preset.settingsConfig.models).length).toBeGreaterThan(
        0,
      );
      expect(preset.settingsConfig).not.toHaveProperty("api");
      expect(preset.settingsConfig).not.toHaveProperty("compat");
      for (const model of Object.values(preset.settingsConfig.models)) {
        expect(model).not.toHaveProperty("compat");
        expect(model).not.toHaveProperty("thinkingLevelMap");
        expect(model).not.toHaveProperty("input");
      }
      for (const model of preset.modelCatalog ?? []) {
        expect(model.max_context_size).toBeGreaterThan(0);
        if (model.default_effort && model.support_efforts) {
          expect(model.support_efforts).toContain(model.default_effort);
        }
      }
    },
  );

  it.each(kimicodeProviderPresets)(
    "retains the private $name connection",
    (legacy) => {
      expect(
        kimiProviderPresets.some(
          (preset) =>
            preset.providerType === legacy.providerType &&
            preset.settingsConfig.provider.type ===
              legacy.settingsConfig.type &&
            normalizeUrl(preset.settingsConfig.provider.base_url) ===
              normalizeUrl(legacy.settingsConfig.base_url),
        ),
      ).toBe(true);
    },
  );

  it("gives connection variants distinct provider and model keys", () => {
    const keys = kimiProviderPresets.map((preset) => preset.providerKey);
    const models = kimiProviderPresets.flatMap((preset) =>
      Object.keys(preset.settingsConfig.models),
    );
    expect(new Set(keys).size).toBe(keys.length);
    expect(new Set(models).size).toBe(models.length);
  });

  it("retains all three managed-account providers", () => {
    expect(
      new Set(
        kimiProviderPresets.flatMap((preset) =>
          preset.providerType ? [preset.providerType] : [],
        ),
      ),
    ).toEqual(new Set(["codex_oauth", "github_copilot", "xai_oauth"]));
  });

  it("keeps DeepSeek V4 Pro and its reasoning levels available without saving the whole catalog", () => {
    const preset = kimiProviderPresets.find(
      (entry) => entry.providerKey === "deepseek",
    )!;
    expect(
      preset.modelCatalog?.find((model) => model.model === "deepseek-v4-pro"),
    ).toMatchObject({
      max_context_size: 1048576,
      support_efforts: ["low", "high", "max"],
      default_effort: "high",
    });
    expect(Object.values(preset.settingsConfig.models).length).toBeLessThan(
      preset.modelCatalog!.length,
    );
  });
});
