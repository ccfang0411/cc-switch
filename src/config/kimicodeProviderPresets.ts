import { codexProviderPresets } from "./codexProviderPresets";
import type { ProviderCategory } from "@/types";
import {
  providerPresets as claudeProviderPresets,
  type PresetTheme,
} from "./claudeProviderPresets";
import type { PresetFamilyFields } from "./presetFamilies";

/**
 * Kimi Code provider protocol (`type` under `[providers.*]` in
 * `~/.kimi-code/config.toml`).
 */
export const KIMICODE_PROVIDER_TYPES = [
  "kimi",
  "anthropic",
  "openai",
  "openai_responses",
  "google-genai",
  "vertexai",
] as const;

export type KimicodeProviderType = (typeof KIMICODE_PROVIDER_TYPES)[number];

/**
 * The model a Kimi Code provider serves. Projected to
 * `[models."cc-switch:<key>/<model>"]` in config.toml; `max_context_size`
 * is required by Kimi Code, the rest are optional.
 */
export interface KimicodePresetModel {
  model: string;
  /** Kimi Code 模型表别名；上游模型 id 含 "/" 时必填（如 deepseek/deepseek-v4.1-flash → deepseek-v4.1-flash） */
  alias?: string;
  max_context_size: number;
  capabilities?: string[];
  display_name?: string;
  support_efforts?: string[];
  default_effort?: string;
  // Kimi Code 读自己认识的键，其余原样保留（与表单的 passthrough schema 对齐）
  [key: string]: unknown;
}

/**
 * DB-side provider payload for kimicode (`settings_config` JSON). The backend
 * projects the provider fields and the `model` section into Kimi Code's TOML.
 */
export interface KimicodeSettingsConfig {
  name: string;
  kind: string;
  enabled: boolean;
  type: KimicodeProviderType;
  base_url: string;
  api_key: string;
  env?: Record<string, string>;
  custom_headers?: Record<string, string>;
  model: KimicodePresetModel;
  models?: KimicodePresetModel[];
  // 顶层同样允许后端/未来版本认识的额外键
  [key: string]: unknown;
}

export interface KimicodeProviderPreset extends PresetFamilyFields {
  providerType?: "github_copilot" | "codex_oauth" | "xai_oauth";
  requiresOAuth?: boolean;
  name: string;
  nameKey?: string;
  providerKey: string;
  websiteUrl: string;
  apiKeyUrl?: string;
  settingsConfig: KimicodeSettingsConfig;
  modelCatalog?: KimicodePresetModel[];
  category?: ProviderCategory;
  isPartner?: boolean;
  primePartner?: boolean;
  partnerPromotionKey?: string;
  theme?: PresetTheme;
  icon?: string;
  iconColor?: string;
}

// 模型参数与 codex/hermes 预设里的同一型号对齐：k3 = 1M 上下文、思考 + 图像 +
// 工具调用（官方 Kimi Code 文档的 capabilities）；kimi-k2.7-code = 256K、思考 +
// 工具调用（官方标注不支持关思考，不声明图像输入）。
const K3_MODEL: KimicodePresetModel = {
  model: "k3",
  max_context_size: 1048576,
  capabilities: ["thinking", "image_in", "tool_use"],
  display_name: "K3",
};

const K27_CODE_MODEL: KimicodePresetModel = {
  model: "kimi-k2.7-code",
  max_context_size: 262144,
  capabilities: ["thinking", "tool_use"],
  display_name: "Kimi K2.7 Code",
};

// ===== 原生预设：Kimi 官方与 Moonshot 平台（文件顺序与 README 赞助商表对齐；应用内一律按显示名排序，不置顶）=====
const kimicodeNativePresets: KimicodeProviderPreset[] = [
  {
    name: "Kimi Code 官方订阅",
    family: "kimi",
    planKey: "coding",
    regionKey: "cn",
    primePartner: true,
    providerKey: "kimi-code",
    websiteUrl: "https://www.kimi.com/code/?aff=cc-switch",
    apiKeyUrl: "https://www.kimi.com/code/?aff=cc-switch",
    settingsConfig: {
      name: "Kimi Code 官方订阅",
      kind: "custom",
      enabled: true,
      type: "kimi",
      base_url: "https://api.kimi.com/coding/v1",
      api_key: "",
      model: K3_MODEL,
    },
    category: "cn_official",
    icon: "kimi",
    iconColor: "#1783FF",
  },
  {
    name: "Kimi Code 官方订阅(国际)",
    family: "kimi",
    planKey: "coding",
    regionKey: "intl",
    providerKey: "kimi-code-intl",
    websiteUrl: "https://www.kimi.ai/code?aff=cc-switch",
    apiKeyUrl: "https://www.kimi.ai/code?aff=cc-switch",
    settingsConfig: {
      name: "Kimi Code 官方订阅(国际)",
      kind: "custom",
      enabled: true,
      type: "kimi",
      base_url: "https://api.kimi.ai/coding/v1",
      api_key: "",
      model: K3_MODEL,
    },
    category: "cn_official",
    icon: "kimi",
    iconColor: "#1783FF",
  },
  {
    name: "Moonshot 开放平台(国内)",
    family: "kimi",
    planKey: "payg",
    regionKey: "cn",
    primePartner: true,
    providerKey: "moonshot-cn",
    websiteUrl:
      "https://platform.kimi.com?track_id=track-7cf2b91dcde043eda6ef9a95951a042c&aff=cc-switch",
    apiKeyUrl: "https://platform.kimi.com/console/api-keys?aff=cc-switch",
    settingsConfig: {
      name: "Moonshot 开放平台(国内)",
      kind: "custom",
      enabled: true,
      type: "kimi",
      base_url: "https://api.moonshot.cn/v1",
      api_key: "",
      model: K27_CODE_MODEL,
    },
    category: "cn_official",
    partnerPromotionKey: "kimi",
    icon: "kimi",
    iconColor: "#1783FF",
  },
  {
    name: "Moonshot 开放平台(国际)",
    family: "kimi",
    planKey: "payg",
    regionKey: "intl",
    providerKey: "moonshot-intl",
    websiteUrl:
      "https://platform.kimi.ai?track_id=track-674ed6e2af924a5682a87421f7cf753a&aff=cc-switch",
    apiKeyUrl: "https://platform.kimi.ai/console/api-keys?aff=cc-switch",
    settingsConfig: {
      name: "Moonshot 开放平台(国际)",
      kind: "custom",
      enabled: true,
      type: "kimi",
      base_url: "https://api.moonshot.ai/v1",
      api_key: "",
      model: K27_CODE_MODEL,
    },
    category: "cn_official",
    partnerPromotionKey: "kimi",
    icon: "kimi",
    iconColor: "#1783FF",
  },
  {
    // Anthropic 协议端点（api.moonshot.ai/anthropic）：与其他 Kimi 版本不同协议，
    // 不打 family，单独一行，避免和「Moonshot 开放平台(国际)」撞版本分组。
    name: "Anthropic 兼容端点",
    providerKey: "moonshot-anthropic",
    websiteUrl:
      "https://platform.kimi.ai?track_id=track-674ed6e2af924a5682a87421f7cf753a&aff=cc-switch",
    apiKeyUrl: "https://platform.kimi.ai/console/api-keys?aff=cc-switch",
    settingsConfig: {
      name: "Anthropic 兼容端点",
      kind: "custom",
      enabled: true,
      type: "anthropic",
      base_url: "https://api.moonshot.ai/anthropic",
      api_key: "",
      model: K27_CODE_MODEL,
    },
    category: "cn_official",
    partnerPromotionKey: "kimi",
    icon: "kimi",
    iconColor: "#1783FF",
  },
];

// ===== 派生预设：供应商覆盖面与 Claude Code 对齐 =====
// Claude Code 的中转供应商多为 Anthropic Messages 协议，对应 Kimi Code 的
// `type = "anthropic"`；openai_chat / openai_responses 格式映射到 Kimi 同名类型。
// 无 ANTHROPIC_MODEL 的纯端点预设回退到 Claude Code 默认模型 claude-sonnet-4-5。

type ClaudeEnvConfig = { env?: Record<string, string> };

const CLAUDE_FORMAT_TO_KIMI_TYPE: Record<string, KimicodeProviderType> = {
  anthropic: "anthropic",
  openai_chat: "openai",
  openai_responses: "openai_responses",
};

const CLAUDE_DEFAULT_MODEL: KimicodePresetModel = {
  model: "claude-sonnet-4-5",
  max_context_size: 200000,
  capabilities: ["thinking", "image_in", "tool_use"],
};

function slugifyProviderKey(name: string, websiteUrl: string): string | null {
  const fromName = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (fromName) return fromName;
  // 非拉丁名称（如「模力方舟」）退化为官网域名首段
  try {
    const fromHost = (new URL(websiteUrl).hostname.split(".")[0] ?? "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "");
    return fromHost || null;
  } catch {
    return null;
  }
}

function deriveFromClaudePresets(): KimicodeProviderPreset[] {
  const usedKeys = new Set(kimicodeNativePresets.map((p) => p.providerKey));
  return claudeProviderPresets.flatMap((preset) => {
    if (
      preset.isOfficial || // 官方登录预设没有可移植的端点
      preset.hidden ||
      preset.templateValues || // 含模板变量的预设无法在 Kimi 侧解析
      preset.family === "kimi" // 已由上方原生 Kimi 预设覆盖
    ) {
      return [];
    }
    const type = CLAUDE_FORMAT_TO_KIMI_TYPE[preset.apiFormat ?? "anthropic"];
    if (!type) return []; // gemini_native 等无法映射的格式
    const env = (preset.settingsConfig as ClaudeEnvConfig).env ?? {};
    const baseUrl = env.ANTHROPIC_BASE_URL ?? "";
    if (!/^https:\/\//.test(baseUrl)) return [];
    const providerKey = slugifyProviderKey(preset.name, preset.websiteUrl);
    if (!providerKey || usedKeys.has(providerKey)) return [];
    usedKeys.add(providerKey);
    const modelName = env.ANTHROPIC_MODEL?.trim();
    // 上游模型 id 含 "/"（如 moonshotai/kimi-k3）时，取末段作为 Kimi 模型表别名
    const alias = modelName?.includes("/")
      ? modelName.split("/").pop()?.trim()
      : undefined;
    if (modelName?.includes("/") && !alias) return [];
    const model: KimicodePresetModel = modelName
      ? {
          model: modelName,
          ...(alias ? { alias } : {}),
          max_context_size: 200000,
          capabilities: modelName.startsWith("claude")
            ? ["thinking", "image_in", "tool_use"]
            : ["thinking", "tool_use"],
        }
      : CLAUDE_DEFAULT_MODEL;
    return [
      {
        ...preset,
        providerKey,
        settingsConfig: {
          name: preset.name,
          kind: "custom",
          enabled: true,
          type,
          base_url: baseUrl,
          api_key: preset.requiresOAuth ? "MANAGED_AUTH" : "",
          model,
        },
      },
    ];
  });
}

// Share maintained model catalogs with Codex; Kimi keeps its native protocol endpoint.
// Catalog entries are suggestions; only models explicitly selected by the user are saved.
function withCodexCatalog(
  preset: KimicodeProviderPreset,
): KimicodeProviderPreset {
  const source = codexProviderPresets.find((entry) =>
    preset.family
      ? entry.family === preset.family &&
        entry.planKey === preset.planKey &&
        entry.regionKey === preset.regionKey
      : entry.name === preset.name,
  );
  if (!source?.modelCatalog?.length) return preset;
  const modelCatalog = source.modelCatalog
    .filter((model) => Number(model.contextWindow) > 0)
    .map(
      (model): KimicodePresetModel => ({
        model: model.model,
        ...(model.model.includes("/")
          ? { alias: model.model.split("/").pop() }
          : {}),
        display_name: model.displayName ?? model.model,
        max_context_size: Number(model.contextWindow),
        ...(model.reasoningLevels
          ? { support_efforts: model.reasoningLevels }
          : {}),
        ...(model.defaultReasoningLevel
          ? { default_effort: model.defaultReasoningLevel }
          : preset.providerKey === "deepseek"
            ? { default_effort: "high" }
            : {}),
        capabilities: [
          ...(model.reasoningLevels?.length ? ["thinking"] : []),
          ...(model.inputModalities?.includes("image") ||
          (!model.inputModalities &&
            model.model === preset.settingsConfig.model.model &&
            preset.settingsConfig.model.capabilities?.includes("image_in"))
            ? ["image_in"]
            : []),
          "tool_use",
        ],
      }),
    );
  if (!modelCatalog.length) return preset;
  // Native Kimi aliases cannot contain '/'; disambiguate equal final path segments.
  const aliases = new Set<string>();
  for (const model of modelCatalog) {
    let alias = model.alias ?? model.model;
    if (aliases.has(alias)) {
      alias = model.model.split("/").join("--");
      const base = alias;
      let suffix = 2;
      while (aliases.has(alias)) alias = `${base}-${suffix++}`;
      model.alias = alias;
    }
    aliases.add(alias);
  }
  const preferred =
    modelCatalog.find(
      (model) =>
        model.model ===
        (preset.providerKey === "deepseek"
          ? "deepseek-flash"
          : preset.settingsConfig.model.model),
    ) ?? modelCatalog[0];
  return {
    ...preset,
    modelCatalog,
    settingsConfig: { ...preset.settingsConfig, model: preferred },
  };
}

export const kimicodeProviderPresets: KimicodeProviderPreset[] = [
  ...kimicodeNativePresets.map(withCodexCatalog),
  ...deriveFromClaudePresets().map(withCodexCatalog),
];
