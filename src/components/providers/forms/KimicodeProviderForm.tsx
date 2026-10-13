import { useEffect, useState } from "react";
import { z } from "zod";
import { useQuery } from "@tanstack/react-query";
import type { ManagedAuthProvider } from "@/lib/api/auth";
import { authApi } from "@/lib/api";
import { resolveManagedAccountId } from "@/lib/authBinding";
import { isOAuthProviderType } from "@/config/constants";
import { CopilotAuthSection } from "./CopilotAuthSection";
import { CodexOAuthSection } from "./CodexOAuthSection";
import { XaiOAuthSection } from "./XaiOAuthSection";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Form, FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { ImeSafeInput } from "@/components/ui/ime-safe-input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import JsonEditor from "@/components/JsonEditor";
import { useDarkMode } from "@/hooks/useDarkMode";
import { useProvidersQuery } from "@/lib/query/queries";
import type { ProviderFormData } from "@/lib/schemas/provider";
import {
  KIMICODE_PROVIDER_TYPES,
  kimicodeProviderPresets,
} from "@/config/kimicodeProviderPresets";
import { BasicFormFields } from "./BasicFormFields";
import { ProviderPresetSelector } from "./ProviderPresetSelector";
import { RequestHeadersEditor } from "./RequestHeadersEditor";
import { ApiKeySection, ModelInputWithFetch } from "./shared";
import {
  fetchCodexOauthModels,
  type FetchedModel,
} from "@/lib/api/model-fetch";
import { normalizeRequestHeaders } from "./helpers/requestHeaders";
import {
  KimicodeModelCatalog,
  withOfficialMetadata,
  type KimiCatalogModel,
} from "./KimicodeModelCatalog";
import type { ProviderFormProps } from "./ProviderForm";

const PRESET_ENTRIES = kimicodeProviderPresets.map((preset, index) => ({
  id: String(index),
  preset,
}));
const configSchema = z
  .object({
    type: z.string().optional(),
    base_url: z.string().optional(),
    api_key: z.string().optional(),
    api_key_env: z.string().optional(),
    env: z.record(z.string(), z.string()).optional(),
    custom_headers: z.record(z.string(), z.string()).optional(),
    models: z.array(z.record(z.string(), z.unknown())).optional(),
    model: z
      .object({
        model: z.string().optional(),
        max_context_size: z.number().optional(),
        capabilities: z.array(z.string()).optional(),
        display_name: z.string().optional(),
        support_efforts: z.array(z.string()).optional(),
        default_effort: z.string().optional(),
      })
      .passthrough()
      .optional(),
  })
  .passthrough();
type KimicodeConfig = z.infer<typeof configSchema>;
const KEY_PATTERN = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const normalizeKey = (value: string) =>
  value.toLowerCase().replace(/[^a-z0-9-]/g, "");

const EMPTY_CONFIG: KimicodeConfig = {
  kind: "custom",
  enabled: true,
  type: "kimi",
  base_url: "",
  api_key: "",
  model: { model: "", max_context_size: 1048576 },
};

const formatCapabilities = (capabilities: string[] | undefined) =>
  (capabilities ?? []).join(", ");
const parseCapabilities = (text: string) =>
  text
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);

export function KimicodeProviderForm({
  providerId,
  initialData,
  onSubmit,
  onCancel,
  submitLabel,
  showButtons = true,
  onSubmittingChange,
  onSubmitReadyChange,
  onManageAuthAccounts,
}: ProviderFormProps) {
  const { t } = useTranslation();
  const isDarkMode = useDarkMode();
  const [config, setConfig] = useState<KimicodeConfig>(
    initialData?.settingsConfig ?? EMPTY_CONFIG,
  );
  const [jsonText, setJsonText] = useState(JSON.stringify(config, null, 2));
  const [jsonValid, setJsonValid] = useState(true);
  const [presetId, setPresetId] = useState("custom");
  const preset =
    presetId === "custom"
      ? undefined
      : kimicodeProviderPresets[Number(presetId)];
  const authProvider = preset?.providerType ?? initialData?.meta?.providerType;
  const managedAuth = isOAuthProviderType(authProvider)
    ? (authProvider as ManagedAuthProvider)
    : undefined;
  const [selectedAccountId, setSelectedAccountId] = useState<string | null>(
    authProvider
      ? resolveManagedAccountId(initialData?.meta, authProvider)
      : null,
  );
  const { data: authStatus } = useQuery({
    queryKey: ["managed-auth-status", managedAuth],
    queryFn: () => authApi.authGetStatus(managedAuth!),
    enabled: Boolean(managedAuth),
  });
  const effectiveAccountId =
    selectedAccountId ?? authStatus?.default_account_id;
  const accountReady = authStatus?.accounts.some(
    (account) =>
      account.id === effectiveAccountId &&
      !account.requires_reauth &&
      !account.reauth_required,
  );
  const codexModels = useQuery({
    queryKey: ["kimicode-codex-models", effectiveAccountId],
    queryFn: () => fetchCodexOauthModels(effectiveAccountId),
    enabled: managedAuth === "codex_oauth" && Boolean(accountReady),
    staleTime: 60_000,
    retry: false,
    refetchOnWindowFocus: false,
  });
  const normalizedBase = (url: string) => url.replace(/\/+$/, "");
  const matchingPresets = kimicodeProviderPresets.filter(
    (entry) =>
      entry.modelCatalog?.length &&
      entry.settingsConfig.type === config.type &&
      normalizedBase(entry.settingsConfig.base_url) ===
        normalizedBase(config.base_url ?? ""),
  );
  const catalogPreset =
    preset ??
    matchingPresets.find((entry) => entry.providerKey === providerId) ??
    matchingPresets.find((entry) =>
      entry.modelCatalog?.some((model) => model.model === config.model?.model),
    ) ??
    matchingPresets[0];
  const availableModels: FetchedModel[] =
    managedAuth === "codex_oauth"
      ? accountReady
        ? (codexModels.data ?? [])
        : []
      : (catalogPreset?.modelCatalog ?? []).map((model) => ({
          id: model.model,
          ownedBy: catalogPreset?.name ?? null,
          alias: model.alias,
          inputModalities: model.capabilities?.includes("image_in")
            ? ["text", "image"]
            : ["text"],
          displayName: model.display_name,
          contextWindow: model.max_context_size,
          supportedReasoningEfforts: model.support_efforts,
          defaultReasoningEffort: model.default_effort,
        }));
  const category = initialData?.category ?? preset?.category ?? "custom";
  const [capabilitiesText, setCapabilitiesText] = useState(
    formatCapabilities(config.model?.capabilities),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  // The key becomes `providers."cc-switch:<key>"` and the default-model
  // reference embeds it, so it is fixed once the provider exists.
  const isEdit = Boolean(initialData);
  const [providerKey, setProviderKey] = useState(providerId ?? "");
  const { data: existing } = useProvidersQuery("kimicode");
  const keyTaken =
    !isEdit &&
    Object.prototype.hasOwnProperty.call(
      existing?.providers ?? {},
      providerKey,
    );
  const keyInvalid =
    !isEdit && providerKey !== "" && !KEY_PATTERN.test(providerKey);
  const form = useForm<ProviderFormData>({
    defaultValues: {
      name: initialData?.name ?? "",
      notes: initialData?.notes ?? "",
      websiteUrl: initialData?.websiteUrl ?? "",
      icon: initialData?.icon ?? "",
      iconColor: initialData?.iconColor ?? "",
      settingsConfig: jsonText,
    },
  });
  const name = form.watch("name");
  const ready = Boolean(
    jsonValid &&
      name.trim() &&
      (isEdit || (providerKey && !keyInvalid && !keyTaken)) &&
      config.base_url?.trim() &&
      (managedAuth
        ? accountReady
        : config.api_key?.trim() || config.api_key_env?.trim()) &&
      config.model?.model?.trim() &&
      (config.model?.max_context_size ?? 0) > 0 &&
      (config.models ?? []).every(
        (model) =>
          typeof model.model === "string" &&
          typeof model.max_context_size === "number" &&
          model.max_context_size > 0,
      ),
  );
  useEffect(() => {
    onSubmitReadyChange?.(ready);
  }, [ready, onSubmitReadyChange]);
  const update = (next: KimicodeConfig) => {
    setConfig(next);
    setJsonText(JSON.stringify(next, null, 2));
    setJsonValid(true);
  };
  const updateModel = (next: Partial<NonNullable<KimicodeConfig["model"]>>) => {
    const existing =
      next.model && config.models?.find((entry) => entry.model === next.model);
    const changedId =
      next.model !== undefined && next.model !== config.model?.model;
    const merged = {
      ...(existing ||
        (changedId
          ? { model: next.model, max_context_size: 0 }
          : config.model)),
      ...next,
    };
    const model = withOfficialMetadata(
      merged,
      next.model
        ? availableModels.find((item) => item.id === next.model)
        : undefined,
    );
    setCapabilitiesText(formatCapabilities(model.capabilities));
    update({
      ...config,
      model,
      ...(config.models
        ? {
            models: existing
              ? config.models.map((entry) =>
                  entry.model === model.model ? model : entry,
                )
              : config.models.map((entry) =>
                  entry.model === config.model?.model ? model : entry,
                ),
          }
        : {}),
    });
  };
  const catalogModels: KimiCatalogModel[] = [
    ...(config.model ? [config.model] : []),
    ...(config.models ?? []).filter(
      (model) => model.model !== config.model?.model,
    ),
  ];
  useEffect(() => {
    if (managedAuth !== "codex_oauth" || !codexModels.data) return;
    const enrich = (model: KimiCatalogModel) =>
      withOfficialMetadata(
        model,
        codexModels.data?.find((item) => item.id === model.model),
      );
    const next = {
      ...config,
      ...(config.model ? { model: enrich(config.model) } : {}),
      ...(config.models ? { models: config.models.map(enrich) } : {}),
    };
    if (JSON.stringify(next) !== JSON.stringify(config)) {
      update(next);
      setCapabilitiesText(formatCapabilities(next.model?.capabilities));
    }
    // Synchronize fresh official metadata without reapplying it on every user edit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [codexModels.data, managedAuth]);

  const choosePreset = (id: string) => {
    setPresetId(id);
    setSelectedAccountId(null);
    const selected =
      id === "custom" ? undefined : kimicodeProviderPresets[Number(id)];
    const next = selected?.settingsConfig ?? EMPTY_CONFIG;
    update(next);
    setCapabilitiesText(formatCapabilities(next.model?.capabilities));
    setProviderKey(selected?.providerKey ?? "");
    form.reset({
      name: selected?.name ?? "",
      notes: "",
      websiteUrl: selected?.websiteUrl ?? "",
      icon: selected?.icon ?? "",
      iconColor: selected?.iconColor ?? "",
      settingsConfig: JSON.stringify(next),
    });
  };
  return (
    <Form {...form}>
      <form
        id="provider-form"
        className="space-y-6"
        onSubmit={form.handleSubmit(async (identity) => {
          if (!ready || busy) return;
          setBusy(true);
          onSubmittingChange?.(true);
          setError("");
          try {
            const headers = normalizeRequestHeaders(
              config.custom_headers ?? {},
            );
            const next = { ...config };
            if (managedAuth) delete next.api_key_env;
            if (Object.keys(headers).length) next.custom_headers = headers;
            else delete next.custom_headers;
            await onSubmit({
              ...identity,
              name: identity.name.trim(),
              meta: {
                ...initialData?.meta,
                providerType: managedAuth,
                ...(managedAuth
                  ? {
                      apiFormat:
                        managedAuth === "github_copilot"
                          ? "openai_chat"
                          : "openai_responses",
                    }
                  : {}),
                authBinding: managedAuth
                  ? {
                      source: "managed_account",
                      authProvider: managedAuth,
                      accountId: effectiveAccountId ?? undefined,
                    }
                  : undefined,
              },
              providerKey: isEdit ? providerId : providerKey,
              presetCategory: category,
              settingsConfig: JSON.stringify({
                ...next,
                name: identity.name.trim(),
                kind: "custom",
                ...(managedAuth ? { api_key: "MANAGED_AUTH" } : {}),
              }),
            });
          } catch (error) {
            setError(String(error));
          } finally {
            setBusy(false);
            onSubmittingChange?.(false);
          }
        })}
      >
        {!initialData && (
          <ProviderPresetSelector
            selectedPresetId={presetId}
            presetEntries={PRESET_ENTRIES}
            presetCategoryLabels={{
              custom: t("providerPreset.custom"),
              cn_official: t("providerForm.categoryCnOfficial"),
              aggregator: t("providerForm.categoryAggregation"),
              third_party: t("providerForm.categoryThirdParty"),
            }}
            onPresetChange={choosePreset}
            category={category}
          />
        )}
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        <fieldset
          disabled={busy || !jsonValid}
          className="min-w-0 space-y-6 border-0 p-0 disabled:opacity-50"
        >
          <BasicFormFields
            form={form}
            beforeNameSlot={
              <div className="space-y-2">
                <Label htmlFor="kimicode-provider-key">
                  {t("opencode.providerKey")}
                  <span aria-hidden="true" className="text-destructive ml-1">
                    *
                  </span>
                </Label>
                <Input
                  id="kimicode-provider-key"
                  value={providerKey}
                  onChange={(event) =>
                    setProviderKey(normalizeKey(event.target.value))
                  }
                  disabled={isEdit}
                  placeholder={t("opencode.providerKeyPlaceholder")}
                  autoComplete="off"
                  className={keyTaken || keyInvalid ? "border-destructive" : ""}
                />
                <p
                  className={
                    keyTaken || keyInvalid
                      ? "text-xs text-destructive"
                      : "text-xs text-fg-2"
                  }
                >
                  {keyTaken
                    ? t("opencode.providerKeyDuplicate")
                    : keyInvalid
                      ? t("opencode.providerKeyInvalid")
                      : isEdit
                        ? t("opencode.providerKeyLockedHint")
                        : t("opencode.providerKeyHint")}
                </p>
              </div>
            }
          />
          {/* Provider Type */}
          <div className="space-y-2">
            <FormLabel htmlFor="kimicode-provider-type">
              {t("opencode.npmPackage", { defaultValue: "接口格式" })}
            </FormLabel>
            <Select
              value={config.type ?? "kimi"}
              onValueChange={(type) => update({ ...config, type })}
            >
              <SelectTrigger id="kimicode-provider-type">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {KIMICODE_PROVIDER_TYPES.map((type) => (
                  <SelectItem key={type} value={type}>
                    {type}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-fg-2">
              {t("kimicode.form.providerTypeHint")}
            </p>
          </div>

          {managedAuth === "github_copilot" && (
            <CopilotAuthSection
              mode="select"
              selectedAccountId={selectedAccountId}
              onAccountSelect={setSelectedAccountId}
              onManageAccounts={
                onManageAuthAccounts
                  ? () => onManageAuthAccounts("github_copilot")
                  : undefined
              }
            />
          )}
          {managedAuth === "codex_oauth" && (
            <CodexOAuthSection
              mode="select"
              selectedAccountId={selectedAccountId}
              onAccountSelect={setSelectedAccountId}
              onManageAccounts={
                onManageAuthAccounts
                  ? () => onManageAuthAccounts("codex_oauth")
                  : undefined
              }
              allowUnboundSelection={false}
            />
          )}
          {managedAuth === "xai_oauth" && (
            <XaiOAuthSection
              mode="select"
              selectedAccountId={selectedAccountId}
              onAccountSelect={setSelectedAccountId}
            />
          )}
          {/* API Key */}
          {!managedAuth && (
            <ApiKeySection
              value={config.api_key ?? ""}
              onChange={(api_key) => update({ ...config, api_key })}
              category={category}
              shouldShowLink={Boolean(preset?.apiKeyUrl)}
              websiteUrl={preset?.apiKeyUrl ?? ""}
              isPartner={preset?.isPartner}
              partnerPromotionKey={preset?.partnerPromotionKey}
            />
          )}

          {/* Base URL */}
          <div className="space-y-2">
            <FormLabel htmlFor="kimicode-base-url">
              {t("opencode.baseUrl", { defaultValue: "Base URL" })}
            </FormLabel>
            <ImeSafeInput
              id="kimicode-base-url"
              value={config.base_url ?? ""}
              onValueChange={(base_url) => update({ ...config, base_url })}
              placeholder="https://api.kimi.com/coding/v1"
            />
            <p className="text-xs text-fg-2">
              {t("opencode.baseUrlHint", {
                defaultValue: "自定义 API 端点地址",
              })}
            </p>
          </div>

          <RequestHeadersEditor
            headers={config.custom_headers ?? {}}
            onHeadersChange={(custom_headers) =>
              update({ ...config, custom_headers })
            }
          />

          {
            <KimicodeModelCatalog
              available={availableModels}
              models={catalogModels}
              defaultId={config.model?.model ?? ""}
              onChange={(models, defaultId) => {
                const model = models.find((entry) => entry.model === defaultId);
                if (!model) return;
                update({ ...config, model, models });
                setCapabilitiesText(formatCapabilities(model.capabilities));
              }}
            />
          }

          {/* Model */}
          <div className="space-y-3 border-l border-border pl-3">
            <FormLabel>
              {t("opencode.models", { defaultValue: "模型配置" })}
            </FormLabel>
            {managedAuth === "codex_oauth" && (
              <div className="space-y-1">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!accountReady || codexModels.isFetching}
                  onClick={() => void codexModels.refetch()}
                >
                  {t("providerForm.fetchModels")}
                </Button>
                <p className="text-xs text-fg-2">
                  {t("kimicode.form.codexModelsHint")}
                </p>
                {codexModels.isSuccess && !codexModels.data.length && (
                  <p className="text-xs text-fg-2">
                    {t("providerForm.fetchModelsEmpty")}
                  </p>
                )}
                {codexModels.isError && (
                  <p role="alert" className="text-xs text-destructive">
                    {t("providerForm.fetchModelsFailed")}
                  </p>
                )}
              </div>
            )}
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
              <div className="space-y-1">
                <Label
                  htmlFor="kimicode-model-id"
                  className="text-xs text-fg-2"
                >
                  {t("opencode.modelId", { defaultValue: "模型 ID" })}
                </Label>
                {managedAuth === "codex_oauth" ? (
                  <ModelInputWithFetch
                    id="kimicode-model-id"
                    value={config.model?.model ?? ""}
                    onChange={(model) => updateModel({ model })}
                    fetchedModels={accountReady ? (codexModels.data ?? []) : []}
                    isLoading={codexModels.isFetching}
                  />
                ) : (
                  <ImeSafeInput
                    id="kimicode-model-id"
                    value={config.model?.model ?? ""}
                    onValueChange={(model) => updateModel({ model })}
                    placeholder="k3"
                  />
                )}
              </div>
              <div className="space-y-1">
                <FormLabel
                  htmlFor="kimicode-model-name"
                  className="text-xs text-fg-2"
                >
                  {t("opencode.modelName", { defaultValue: "显示名称" })}
                </FormLabel>
                <ImeSafeInput
                  id="kimicode-model-name"
                  value={config.model?.display_name ?? ""}
                  onValueChange={(display_name) =>
                    updateModel({ display_name })
                  }
                  placeholder="K3"
                />
              </div>
            </div>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
              <div className="space-y-1">
                <FormLabel
                  htmlFor="kimicode-model-context"
                  className="text-xs text-fg-2"
                >
                  {t("opencode.limitContext", { defaultValue: "上下文" })}
                </FormLabel>
                <Input
                  id="kimicode-model-context"
                  type="number"
                  min={1}
                  step={1}
                  value={config.model?.max_context_size ?? ""}
                  onChange={(event) => {
                    const trimmed = event.target.value.trim();
                    if (trimmed === "") {
                      const model = { ...config.model };
                      delete model.max_context_size;
                      update({ ...config, model });
                      return;
                    }
                    const parsed = Number(trimmed);
                    if (!Number.isFinite(parsed) || parsed < 1) return;
                    updateModel({ max_context_size: Math.trunc(parsed) });
                  }}
                  placeholder="1048576"
                />
              </div>
              <div className="space-y-1">
                <FormLabel
                  htmlFor="kimicode-model-capabilities"
                  className="text-xs text-fg-2"
                >
                  {t("kimicode.form.capabilities")}
                </FormLabel>
                <ImeSafeInput
                  id="kimicode-model-capabilities"
                  value={capabilitiesText}
                  onValueChange={(text) => {
                    setCapabilitiesText(text);
                    const capabilities = parseCapabilities(text);
                    const model = { ...config.model };
                    if (capabilities.length) model.capabilities = capabilities;
                    else delete model.capabilities;
                    update({ ...config, model });
                  }}
                  placeholder="thinking, image_in, tool_use"
                />
              </div>
            </div>
            <p className="text-xs text-fg-2">
              {t("kimicode.form.capabilitiesHint")}
            </p>
          </div>
        </fieldset>
        <div className="space-y-2">
          <Label htmlFor="kimicode-settings-config">
            {t("provider.configJson")}
          </Label>
          <JsonEditor
            id="kimicode-settings-config"
            ariaLabel={t("provider.configJson")}
            value={jsonText}
            darkMode={isDarkMode}
            language="json"
            showValidation
            height={Math.max(1, jsonText.split("\n").length) * 20 + 20}
            onChange={(text) => {
              setJsonText(text);
              try {
                const next = configSchema.parse(JSON.parse(text));
                setConfig(next);
                setCapabilitiesText(
                  formatCapabilities(next.model?.capabilities),
                );
                setJsonValid(true);
              } catch {
                setJsonValid(false);
              }
            }}
          />
        </div>
        {showButtons && (
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              onClick={onCancel}
              disabled={busy}
            >
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={busy || !ready}>
              {submitLabel}
            </Button>
          </div>
        )}
      </form>
    </Form>
  );
}
