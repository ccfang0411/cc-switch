import { describe, expect, it } from "vitest";
import { codexProviderPresets } from "./codexProviderPresets";
import {
  KIMICODE_PROVIDER_TYPES,
  kimicodeProviderPresets,
} from "./kimicodeProviderPresets";

import {
  presetGroup,
  presetNeedsRouting,
} from "@/components/providers/forms/presetGroups";

const preset = (name: string) =>
  kimicodeProviderPresets.find((candidate) => candidate.name === name);

describe("kimicodeProviderPresets", () => {
  it("uses only Kimi Code provider types and well-formed unique keys", () => {
    const keys = new Set<string>();
    for (const entry of kimicodeProviderPresets) {
      expect(KIMICODE_PROVIDER_TYPES).toContain(entry.settingsConfig.type);
      expect(entry.providerKey).toMatch(/^[a-z0-9]+(-[a-z0-9]+)*$/);
      expect(keys.has(entry.providerKey)).toBe(false);
      keys.add(entry.providerKey);
      expect(entry.settingsConfig.name).toBe(entry.name);
      expect(entry.settingsConfig.kind).toBe("custom");
      expect(entry.settingsConfig.enabled).toBe(true);
      expect(entry.settingsConfig.base_url).toMatch(/^https:\/\//);
      expect(
        entry.settingsConfig.model.max_context_size,
        entry.name,
      ).toBeGreaterThan(0);
    }
  });

  it("ships the Kimi Code official subscription with the K3 model", () => {
    const official = preset("Kimi Code 官方订阅");
    expect(official).toBeDefined();
    expect(official!.settingsConfig.type).toBe("kimi");
    expect(official!.settingsConfig.base_url).toBe(
      "https://api.kimi.com/coding/v1",
    );
    expect(official!.settingsConfig.model).toMatchObject({
      model: "k3",
      max_context_size: 1048576,
      capabilities: ["thinking", "image_in", "tool_use"],
      display_name: "Kimi K3",
    });
    expect(official!.icon).toBe("kimi");
    expect(official!.primePartner).toBe(true);
  });

  it("offers the Moonshot open platform on both regions with the shared Codex Kimi catalog", () => {
    expect(preset("Moonshot 开放平台(国内)")!.settingsConfig.base_url).toBe(
      "https://api.moonshot.cn/v1",
    );
    expect(preset("Moonshot 开放平台(国际)")!.settingsConfig.base_url).toBe(
      "https://api.moonshot.ai/v1",
    );
    for (const name of ["Moonshot 开放平台(国内)", "Moonshot 开放平台(国际)"]) {
      const entry = preset(name)!;
      expect(entry.settingsConfig.type).toBe("kimi");
      expect(entry.settingsConfig.model.model).toBe("kimi-k2.7-code");
      expect(entry.modelCatalog?.map((model) => model.model)).toEqual([
        "kimi-k3",
        "kimi-k2.7-code",
      ]);
      expect(entry.settingsConfig.model.max_context_size).toBe(262144);
      expect(entry.partnerPromotionKey).toBe("kimi");
    }
  });

  it("aligns DeepSeek models with Codex while keeping the native Anthropic endpoint", () => {
    const entry = preset("DeepSeek")!;
    expect(entry.settingsConfig.base_url).toBe(
      "https://api.deepseek.com/anthropic",
    );
    expect(entry.settingsConfig.type).toBe("anthropic");
    expect(entry.settingsConfig.model.model).toBe("deepseek-flash");
    expect(entry.settingsConfig.models).toBeUndefined();
    expect(entry.modelCatalog?.map((model) => model.model)).toEqual([
      "deepseek-flash",
      "deepseek-v4-pro",
    ]);
    for (const model of entry.modelCatalog!) {
      expect(model.max_context_size).toBe(1048576);
      expect(model.support_efforts).toEqual(["low", "high", "max"]);
      expect(model.default_effort).toBe("high");
    }
    expect(entry.modelCatalog![0].capabilities).toContain("image_in");
    expect(entry.modelCatalog![1].capabilities).not.toContain("image_in");
  });

  it("shares every matching Codex catalog without importing all models into the configuration", () => {
    let matched = 0;
    for (const entry of kimicodeProviderPresets) {
      const source = codexProviderPresets.find((candidate) =>
        entry.family
          ? candidate.family === entry.family &&
            candidate.planKey === entry.planKey &&
            candidate.regionKey === entry.regionKey
          : candidate.name === entry.name,
      );
      const models = source?.modelCatalog?.filter(
        (model) => Number(model.contextWindow) > 0,
      );
      if (!models?.length) continue;
      matched++;
      expect(
        entry.modelCatalog?.map((model) => model.model),
        entry.name,
      ).toEqual(models.map((model) => model.model));
      expect(entry.settingsConfig.models, entry.name).toBeUndefined();
      const aliases = entry.modelCatalog!.map(
        (model) => model.alias ?? model.model,
      );
      expect(new Set(aliases).size, entry.name).toBe(aliases.length);
      expect(
        aliases.every((alias) => !alias.includes("/")),
        entry.name,
      ).toBe(true);
    }
    expect(matched).toBeGreaterThan(20);
  });

  it("exposes the Anthropic-compatible endpoint as a standalone preset", () => {
    const anthropic = preset("Anthropic 兼容端点");
    expect(anthropic).toBeDefined();
    expect(anthropic!.settingsConfig.type).toBe("anthropic");
    expect(anthropic!.settingsConfig.base_url).toBe(
      "https://api.moonshot.ai/anthropic",
    );
    expect(anthropic!.settingsConfig.model.model).toBe("kimi-k2.7-code");
    // 与其他 Kimi 版本协议不同：不打 family，避免撞「Moonshot 开放平台(国际)」的版本分组
    expect(anthropic!.family).toBeUndefined();
  });

  it("derives Claude Code relay presets as anthropic-type providers", () => {
    // 与 Claude Code 供应商列表对齐：中转商预设批量派生
    const packy = preset("PackyCode");
    expect(packy).toBeDefined();
    expect(packy!.providerKey).toBe("packycode");
    expect(packy!.settingsConfig.type).toBe("anthropic");
    expect(packy!.settingsConfig.base_url).toBe("https://www.packyapi.ai");
    // 纯端点预设回退到 Claude Code 默认模型
    expect(packy!.settingsConfig.model.model).toBe("claude-sonnet-4-5");
    expect(packy!.settingsConfig.model.max_context_size).toBe(200000);
    // 合作伙伴元数据随派生保留
    expect(packy!.isPartner).toBe(true);
    expect(packy!.partnerPromotionKey).toBe("packycode");
  });

  it("maps openai-format Claude presets to the matching Kimi provider type", () => {
    const nvidia = preset("Nvidia");
    expect(nvidia).toBeDefined();
    expect(nvidia!.settingsConfig.type).toBe("openai");
    // 上游模型 id 带 "/" 时派生末段别名，后端投影为合法的 Kimi 模型表键
    expect(nvidia!.settingsConfig.model.model).toBe("moonshotai/kimi-k3");
    expect(nvidia!.settingsConfig.model.alias).toBe("kimi-k3");
    expect(preset("Command Code")!.settingsConfig.type).toBe("openai");
    expect(preset("Command Code")!.settingsConfig.model.alias).toBe(
      "deepseek-v4.1-flash",
    );
  });

  it("skips Claude presets Kimi Code cannot express", () => {
    // 官方登录、OAuth、模板变量、gemini_native 与 Kimi family（原生预设已覆盖）
    for (const name of [
      "Claude Official",
      "Gemini Native",
      "Kimi",
      "Kimi Global",
      "Kimi For Coding",
      "Kimi For Coding Global",
    ]) {
      expect(preset(name), name).toBeUndefined();
    }
    // 模型 id 含 "/" 的预设必须派生出不带 "/" 的别名（后端 validate_provider 同样强制）
    for (const entry of kimicodeProviderPresets) {
      const model = entry.settingsConfig.model;
      if (model.model.includes("/")) {
        expect(model.alias, entry.name).toBeTruthy();
        expect(model.alias, entry.name).not.toContain("/");
      }
    }
  });
});

describe("Kimi Code managed login presets", () => {
  it("offers ChatGPT, Copilot and xAI without storing credentials", () => {
    for (const type of ["codex_oauth", "github_copilot", "xai_oauth"]) {
      const entry = kimicodeProviderPresets.find(
        (p) => p.providerType === type,
      );
      expect(entry?.requiresOAuth).toBe(true);
      expect(presetGroup(entry!)).toBe("login");
      expect(presetNeedsRouting("kimicode", { id: type, preset: entry! })).toBe(
        true,
      );
      expect(entry?.settingsConfig.api_key).toBe("MANAGED_AUTH");
      expect(entry?.settingsConfig.model.model).toBeTruthy();
    }
  });
});
