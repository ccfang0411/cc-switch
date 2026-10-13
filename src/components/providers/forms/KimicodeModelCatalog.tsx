import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { FetchedModel } from "@/lib/api/model-fetch";

export interface KimiCatalogModel {
  model?: string;
  alias?: string;
  max_context_size?: number;
  display_name?: string;
  capabilities?: string[];
  support_efforts?: string[];
  default_effort?: string;
  [key: string]: unknown;
}

/** Only apply values actually reported by the official account catalog. */
export function withOfficialMetadata(
  model: KimiCatalogModel,
  official?: FetchedModel,
): KimiCatalogModel {
  if (!official) return model;
  const levels = official.supportedReasoningEfforts;
  const currentEffort = model.default_effort;
  const effort = levels
    ? currentEffort && levels.includes(currentEffort)
      ? currentEffort
      : official.defaultReasoningEffort &&
          levels.includes(official.defaultReasoningEffort)
        ? official.defaultReasoningEffort
        : undefined
    : currentEffort;
  return {
    ...model,
    ...(official.alias ? { alias: official.alias } : {}),
    ...(official.inputModalities
      ? {
          capabilities: Array.from(
            new Set([
              ...(model.capabilities ?? []).filter(
                (capability) => capability !== "image_in",
              ),
              ...(official.inputModalities.includes("image")
                ? ["image_in"]
                : []),
              ...(levels?.some((level) => level !== "none")
                ? ["thinking"]
                : []),
              "tool_use",
            ]),
          ),
        }
      : {}),
    ...(official.contextWindow
      ? { max_context_size: official.contextWindow }
      : {}),
    ...(official.displayName ? { display_name: official.displayName } : {}),
    ...(levels ? { support_efforts: levels, default_effort: effort } : {}),
    ...(!official.inputModalities && levels?.some((level) => level !== "none")
      ? {
          capabilities: Array.from(
            new Set([...(model.capabilities ?? []), "thinking", "tool_use"]),
          ),
        }
      : {}),
  };
}

interface Props {
  available: FetchedModel[];
  models: KimiCatalogModel[];
  defaultId: string;
  onChange: (models: KimiCatalogModel[], defaultId: string) => void;
}

export function KimicodeModelCatalog({
  available,
  models,
  defaultId,
  onChange,
}: Props) {
  const { t } = useTranslation();
  const [picking, setPicking] = useState(false);
  const [query, setQuery] = useState("");
  const [manualId, setManualId] = useState("");
  const selected = new Set(models.map((model) => model.model));
  const toggle = (official: FetchedModel, checked: boolean) => {
    const next = checked
      ? [
          ...models.filter((model) => model.model?.trim()),
          withOfficialMetadata(
            { model: official.id, max_context_size: 0 },
            official,
          ),
        ]
      : models.filter((model) => model.model !== official.id);
    if (!next.length) return;
    onChange(
      next,
      next.some((model) => model.model === defaultId)
        ? defaultId
        : (next[0].model ?? ""),
    );
  };
  const edit = (id: string, patch: Partial<KimiCatalogModel>) =>
    onChange(
      models.map((model) =>
        model.model === id ? { ...model, ...patch } : model,
      ),
      defaultId,
    );
  return (
    <div className="space-y-3">
      <div className="flex gap-2">
        <Input
          aria-label={t("kimicode.form.manualModelId")}
          placeholder={t("kimicode.form.manualModelId")}
          value={manualId}
          onChange={(event) => setManualId(event.target.value)}
        />
        <Button
          type="button"
          variant="outline"
          disabled={!manualId.trim() || selected.has(manualId.trim())}
          onClick={() => {
            const id = manualId.trim();
            const official = available.find((model) => model.id === id);
            const model = withOfficialMetadata(
              {
                model: id,
                ...(id.includes("/") ? { alias: id.split("/").pop() } : {}),
                max_context_size: 0,
              },
              official,
            );
            onChange(
              [...models.filter((entry) => entry.model?.trim()), model],
              defaultId || id,
            );
            setManualId("");
          }}
        >
          {t("kimicode.form.addModel")}
        </Button>
      </div>
      {available.length > 0 && (
        <Button
          type="button"
          variant="outline"
          onClick={() => setPicking(!picking)}
        >
          {t("kimicode.form.chooseModels")}
        </Button>
      )}
      {picking && (
        <div className="space-y-2 rounded-panel border border-border p-3">
          <Input
            aria-label={t("kimicode.form.searchModels")}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t("kimicode.form.searchModels")}
          />
          <div className="max-h-64 overflow-auto space-y-2">
            {available
              .filter((model) =>
                model.id.toLowerCase().includes(query.toLowerCase()),
              )
              .map((model) => (
                <label
                  key={model.id}
                  className="flex items-center gap-2 text-sm"
                >
                  <Checkbox
                    checked={selected.has(model.id)}
                    disabled={models.length === 1 && selected.has(model.id)}
                    onCheckedChange={(checked) => toggle(model, checked)}
                  />
                  <span>{model.id}</span>
                  {model.contextWindow && (
                    <span className="text-xs text-fg-2">
                      {model.contextWindow.toLocaleString()}
                    </span>
                  )}
                </label>
              ))}
          </div>
          <Button
            type="button"
            variant="outline"
            onClick={() => setPicking(false)}
          >
            {t("common.done")}
          </Button>
        </div>
      )}
      {models.map((model, index) => {
        const id = model.model ?? "";
        const official = available.find((item) => item.id === id);
        return (
          <div
            key={model.alias ?? id}
            className="space-y-2 rounded-panel border border-border p-3"
          >
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="font-medium">{model.display_name || id}</span>
              <div className="flex items-center gap-2">
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={id === defaultId}
                  onClick={() => onChange(models, id)}
                >
                  {t(
                    id === defaultId
                      ? "kimicode.form.defaultModel"
                      : "kimicode.form.setDefaultModel",
                  )}
                </Button>
                {models.length > 1 && (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    aria-label={t("kimicode.form.removeModel", { model: id })}
                    onClick={() => {
                      const next = models.filter((item) => item.model !== id);
                      onChange(
                        next,
                        id === defaultId ? (next[0].model ?? "") : defaultId,
                      );
                    }}
                  >
                    {t("common.delete")}
                  </Button>
                )}
              </div>
            </div>
            <Label htmlFor={`kimi-catalog-context-${index}`}>
              {t("opencode.limitContext")}
            </Label>
            <Input
              id={`kimi-catalog-context-${index}`}
              type="number"
              min="1"
              value={model.max_context_size ?? ""}
              onChange={(event) =>
                edit(id, { max_context_size: Number(event.target.value) })
              }
            />
            {official?.contextWindow && (
              <p className="text-xs text-fg-2">
                {t("kimicode.form.officialContext", {
                  count: official.contextWindow,
                })}
              </p>
            )}
            {(model.support_efforts?.length ?? 0) > 0 && (
              <div className="space-y-1">
                <Label htmlFor={`kimi-catalog-effort-${index}`}>
                  {t("kimicode.form.reasoningEffort")}
                </Label>
                <Select
                  value={model.default_effort || "__default__"}
                  onValueChange={(value) =>
                    edit(id, {
                      default_effort:
                        value === "__default__" ? undefined : value,
                    })
                  }
                >
                  <SelectTrigger
                    id={`kimi-catalog-effort-${index}`}
                    aria-label={t("kimicode.form.effortForModel", {
                      model: id,
                    })}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="__default__">
                      {t("kimicode.form.modelDefault")}
                    </SelectItem>
                    {model.support_efforts?.map((level) => (
                      <SelectItem key={level} value={level}>
                        {level}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
