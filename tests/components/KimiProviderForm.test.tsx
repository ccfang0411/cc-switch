import { QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { KimiProviderForm } from "@/components/providers/forms/KimiProviderForm";
import { parse as parseToml } from "smol-toml";
import {
  kimiProviderPresets,
  type KimiProviderConfig,
} from "@/config/kimiProviderPresets";
import { kimiConfigToToml } from "@/components/providers/forms/helpers/kimiToml";
import type { ProviderCategory } from "@/types";
import { createTestQueryClient } from "../utils/testQueryClient";
import { server } from "../msw/server";

beforeAll(() => {
  Element.prototype.scrollIntoView ??= vi.fn();
});

vi.mock("@/components/JsonEditor", () => ({
  default: ({
    id,
    value,
    onChange,
  }: {
    id: string;
    value: string;
    onChange: (text: string) => void;
  }) => (
    <textarea
      id={id}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

const config = {
  provider: {
    type: "kimi",
    base_url: "https://api.kimi.com/coding/v1",
    api_key: "",
    oauth: { storage: "file", key: "kimi-code" },
    custom_headers: { "X-Custom": "keep" },
  },
  models: {
    "kimi-code/k3": {
      provider: "managed:kimi-code",
      model: "k3",
      max_context_size: 1048576,
      capabilities: ["thinking", "tool_use"],
      overrides: { default_effort: "max" },
    },
  },
};

// 模型参数收在每行的展开区里，和 Pi、OpenCode 一样。
function expandModels() {
  for (const toggle of screen.getAllByRole("button", {
    name: "opencode.toggleModelDetails",
  }))
    fireEvent.click(toggle);
}

function setup(
  onSubmit = vi.fn(),
  category?: ProviderCategory,
  settingsConfig: Record<string, unknown> = config,
) {
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <KimiProviderForm
        appId="kimi"
        providerId="managed:kimi-code"
        initialData={{ name: "Kimi Code", settingsConfig, category }}
        submitLabel="Save"
        onSubmit={onSubmit}
        onCancel={() => {}}
      />
    </QueryClientProvider>,
  );
  return onSubmit;
}

type KimiManagedAuth = "codex_oauth" | "github_copilot" | "xai_oauth";

function setupManaged(authProvider: KimiManagedAuth, usable = true) {
  const requests = vi.fn();
  const catalog = [
    {
      id: "first",
      contextWindow: 200000,
      displayName: "First",
      supportedReasoningEfforts: ["low", "high"],
      defaultReasoningEffort: "high",
      inputModalities: ["text", "image"],
    },
    {
      id: "second",
      contextWindow: 400000,
      displayName: "Second",
      supportedReasoningEfforts: ["low", "xhigh"],
      defaultReasoningEffort: "xhigh",
      inputModalities: ["text"],
    },
    { id: "unselected", contextWindow: 100000 },
  ];
  const command =
    authProvider === "codex_oauth"
      ? "get_codex_oauth_models"
      : authProvider === "xai_oauth"
        ? "get_xai_oauth_models"
        : "copilot_get_models_for_account";
  server.use(
    http.post("http://tauri.local/auth_get_status", () =>
      HttpResponse.json({
        provider: authProvider,
        authenticated: usable,
        default_account_id: "account-1",
        accounts: [
          {
            id: "account-1",
            provider: authProvider,
            login: "test",
            is_default: true,
            requires_reauth: !usable,
            reauth_required: !usable,
          },
        ],
      }),
    ),
    http.post(`http://tauri.local/${command}`, async ({ request }) => {
      requests(await request.json());
      return HttpResponse.json(
        authProvider === "github_copilot"
          ? catalog.map((model) => ({
              id: model.id,
              name: model.displayName ?? model.id,
              vendor: "Test",
              model_picker_enabled: true,
              context_window: model.contextWindow,
              reasoning_effort: model.supportedReasoningEfforts,
              supports_vision: model.inputModalities?.includes("image"),
            }))
          : catalog.map((model) => ({ ...model, ownedBy: "Test" })),
      );
    }),
  );
  const submit = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <KimiProviderForm
        appId="kimi"
        providerId="managed-test"
        initialData={{
          name: "Managed Test",
          settingsConfig: {
            provider: {
              type:
                authProvider === "github_copilot"
                  ? "openai"
                  : "openai_responses",
              base_url: "https://managed.example/v1",
              api_key: "MANAGED_AUTH",
            },
            models: {
              "managed-test/first": {
                model: "first",
                max_context_size: 1,
                futureField: "keep",
              },
            },
            default_model: "managed-test/first",
            futureSetting: { keep: true },
          },
          meta: {
            providerType: authProvider,
            authBinding: {
              source: "managed_account",
              authProvider,
              accountId: "account-1",
            },
          },
        }}
        submitLabel="Save"
        onSubmit={submit}
        onCancel={() => {}}
      />
    </QueryClientProvider>,
  );
  return { submit, requests };
}

describe("KimiProviderForm", () => {
  it.each(["codex_oauth", "github_copilot", "xai_oauth"] as const)(
    "uses the selected %s account catalog and preserves multiple model choices",
    async (authProvider) => {
      const user = userEvent.setup();
      const { submit, requests } = setupManaged(authProvider);
      expect(screen.queryByLabelText("API Key")).not.toBeInTheDocument();
      await waitFor(() =>
        expect(requests).toHaveBeenCalledWith({ accountId: "account-1" }),
      );
      await user.click(
        await screen.findByRole("button", {
          name: "providerForm.modelPicker.title",
        }),
      );
      await user.click(screen.getByRole("checkbox", { name: "second" }));
      await user.click(
        screen.getByRole("button", { name: "providerForm.modelPicker.apply" }),
      );
      await user.click(screen.getByLabelText("kimicode.form.defaultModel"));
      await user.click(screen.getByRole("option", { name: "Second" }));
      expandModels();
      await user.click(
        document.getElementById("kimi-effort-managed-test/first")!,
      );
      await user.click(screen.getByRole("option", { name: "low" }));
      await user.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(submit).toHaveBeenCalledOnce());
      const payload = submit.mock.calls[0][0];
      const saved = JSON.parse(payload.settingsConfig);
      expect(payload.meta).toMatchObject({
        providerType: authProvider,
        authBinding: {
          source: "managed_account",
          authProvider,
          accountId: "account-1",
        },
      });
      expect(saved.provider.api_key).toBe("MANAGED_AUTH");
      expect(saved.default_model).toBe("managed-test/second");
      expect(Object.keys(saved.models)).toEqual([
        "managed-test/first",
        "managed-test/second",
      ]);
      expect(saved.models["managed-test/first"]).toMatchObject({
        provider: "managed-test",
        max_context_size: 200000,
        default_effort: "low",
        support_efforts: ["low", "high"],
        capabilities: ["image_in", "thinking", "tool_use"],
        futureField: "keep",
      });
      expect(saved.models["managed-test/second"]).toMatchObject({
        provider: "managed-test",
        max_context_size: 400000,
        support_efforts: ["low", "xhigh"],
        capabilities: ["tool_use", "thinking"],
      });
      expect(saved.futureSetting).toEqual({ keep: true });
    },
  );

  it.each(["codex_oauth", "github_copilot", "xai_oauth"] as const)(
    "cannot save or fetch models from an expired %s account",
    async (authProvider) => {
      const { submit, requests } = setupManaged(authProvider, false);
      await waitFor(() =>
        expect(screen.getByRole("button", { name: "Save" })).toBeDisabled(),
      );
      fireEvent.click(
        screen.getByRole("button", { name: "providerForm.fetchModels" }),
      );
      expect(requests).not.toHaveBeenCalled();
      expect(submit).not.toHaveBeenCalled();
    },
  );

  it("keeps the configured models when refreshing an account catalog fails", async () => {
    const user = userEvent.setup();
    const { submit, requests } = setupManaged("codex_oauth");
    await waitFor(() => expect(requests).toHaveBeenCalledOnce());
    server.use(
      http.post("http://tauri.local/get_codex_oauth_models", () =>
        HttpResponse.json("offline", { status: 500 }),
      ),
    );
    await user.click(
      await screen.findByRole("button", {
        name: "providerForm.modelPicker.title",
      }),
    );
    await user.click(
      screen.getByRole("button", { name: "providerForm.modelPicker.refresh" }),
    );
    await user.click(
      screen.getByRole("button", { name: "providerForm.modelPicker.apply" }),
    );
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "providerForm.fetchModelsFailed",
      ),
    );
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    expect(
      Object.keys(JSON.parse(submit.mock.calls[0][0].settingsConfig).models),
    ).toEqual(["managed-test/first"]);
  });
  it("keeps the selected default model when a new provider key is renamed", async () => {
    const user = userEvent.setup();
    const submit = vi.fn();
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <KimiProviderForm
          appId="kimi"
          submitLabel="Save"
          onSubmit={submit}
          onCancel={() => {}}
        />
      </QueryClientProvider>,
    );
    for (const [label, value] of [
      ["opencode.providerKey", "original"],
      ["provider.name", "Custom"],
      ["opencode.baseUrl", "https://example.test/v1"],
      ["API Key", "test-key"],
    ]) {
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
    }
    await user.click(screen.getByRole("button", { name: "opencode.addModel" }));
    await user.click(screen.getByRole("button", { name: "opencode.addModel" }));
    const modelInputs = screen.getAllByLabelText("opencode.modelId");
    fireEvent.change(modelInputs[0], { target: { value: "first" } });
    fireEvent.change(modelInputs[1], { target: { value: "second" } });
    await user.click(screen.getByLabelText("kimicode.form.defaultModel"));
    await user.click(screen.getByRole("option", { name: "second" }));
    fireEvent.change(screen.getByLabelText("opencode.providerKey"), {
      target: { value: "renamed" },
    });
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.default_model).toBe("renamed/model-2");
    expect(saved.models["renamed/model-2"].model).toBe("second");
    expect(
      Object.values(saved.models as KimiProviderConfig["models"]).every(
        (model) => model.provider === "renamed",
      ),
    ).toBe(true);
  });

  it("repairs the default after removing its model", async () => {
    const user = userEvent.setup();
    const { submit, requests } = setupManaged("codex_oauth");
    await waitFor(() => expect(requests).toHaveBeenCalledOnce());
    await user.click(
      await screen.findByRole("button", {
        name: "providerForm.modelPicker.title",
      }),
    );
    await user.click(screen.getByRole("checkbox", { name: "second" }));
    await user.click(
      screen.getByRole("button", { name: "providerForm.modelPicker.apply" }),
    );
    await user.click(screen.getByLabelText("kimicode.form.defaultModel"));
    await user.click(screen.getByRole("option", { name: "Second" }));
    await user.click(
      screen.getAllByRole("button", { name: "common.delete" })[1],
    );
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.default_model).toBe("managed-test/first");
    expect(Object.keys(saved.models)).toEqual(["managed-test/first"]);
  });

  it.each(["import", "toml"] as const)(
    "preserves native capabilities from %s without exposing audio or video controls",
    async (source) => {
      const nativeConfig = structuredClone(config);
      const model = nativeConfig.models["kimi-code/k3"];
      model.capabilities = [
        "image_in",
        "video_in",
        "audio_in",
        "image_out",
        "audio_out",
        "future_capability",
        "thinking",
        "tool_use",
      ];
      const native = {
        ...nativeConfig,
        models: {
          "kimi-code/k3": {
            ...model,
            overrides: {
              ...model.overrides,
              capabilities: [
                "video_in",
                "image_in",
                "audio_in",
                "future_override",
                "always_thinking",
              ],
            },
          },
        },
      };
      const submit = setup(
        vi.fn(),
        undefined,
        source === "import" ? native : config,
      );
      if (source === "toml") {
        fireEvent.change(
          screen.getByLabelText("providerForm.nativeConfig.title"),
          {
            target: {
              value: kimiConfigToToml(
                native as KimiProviderConfig,
                "managed:kimi-code",
              ),
            },
          },
        );
      }
      expandModels();
      expect(
        screen.getByRole("switch", { name: "kimi.capabilities.image_in" }),
      ).toBeChecked();
      expect(
        screen.queryByRole("switch", { name: "kimi.capabilities.audio_in" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("switch", { name: "kimi.capabilities.video_in" }),
      ).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(submit).toHaveBeenCalledOnce());
      const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
      expect(saved.models["kimi-code/k3"]).toMatchObject({
        capabilities: native.models["kimi-code/k3"].capabilities,
        overrides: native.models["kimi-code/k3"].overrides,
      });
      expect(saved.provider).toEqual(config.provider);
    },
  );

  it("keeps image input configurable on models", async () => {
    const native = structuredClone(config);
    native.models["kimi-code/k3"].capabilities.push(
      "audio_in",
      "future_capability",
    );
    const submit = setup(vi.fn(), undefined, native);
    expandModels();
    fireEvent.click(
      screen.getByRole("switch", { name: "kimi.capabilities.image_in" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    expect(
      JSON.parse(submit.mock.calls[0][0].settingsConfig).models["kimi-code/k3"]
        .capabilities,
    ).toEqual([
      "thinking",
      "tool_use",
      "audio_in",
      "future_capability",
      "image_in",
    ]);
  });

  it.each([
    { group: "vendor", name: "Kimi" },
    { group: "thirdparty", name: "88API" },
    { group: "cloud", name: "Microsoft Foundry" },
    { group: "local", name: "Ollama" },
    { group: "local", name: "LM Studio" },
    { group: "local", name: "vLLM" },
  ])(
    "adds $name from the $group catalog using its native configuration",
    async ({ group, name }) => {
      const user = userEvent.setup();
      const submit = vi.fn();
      const selected = kimiProviderPresets.find(
        (preset) => preset.name === name,
      )!;
      render(
        <QueryClientProvider client={createTestQueryClient()}>
          <KimiProviderForm
            appId="kimi"
            submitLabel="Save"
            onSubmit={submit}
            onCancel={() => {}}
          />
        </QueryClientProvider>,
      );
      await user.click(
        screen.getByRole("button", {
          name: new RegExp(`^providerPreset\\.group\\.${group} `),
        }),
      );
      await user.click(screen.getByRole("button", { name }));
      expect(screen.getByLabelText("opencode.providerKey")).toHaveValue(
        selected.providerKey,
      );
      expect(screen.getByLabelText("opencode.baseUrl")).toHaveValue(
        selected.settingsConfig.provider.base_url,
      );
      expect(
        screen.queryByLabelText("kimi.providerCategory"),
      ).not.toBeInTheDocument();
      if (selected.category !== "local") {
        fireEvent.change(screen.getByLabelText("API Key"), {
          target: { value: "test-key" },
        });
      }
      await user.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(submit).toHaveBeenCalledOnce());
      const saved = submit.mock.calls[0][0];
      expect(saved).toMatchObject({
        presetCategory: selected.category,
        providerKey: selected.providerKey,
      });
      const settings = JSON.parse(saved.settingsConfig);
      expect(settings.provider).toMatchObject({
        ...selected.settingsConfig.provider,
        api_key: selected.category === "local" ? "local" : "test-key",
      });
      for (const [key, model] of Object.entries(
        selected.settingsConfig.models,
      )) {
        expect(settings.models[key]).toEqual({
          ...model,
          provider: selected.providerKey,
        });
      }
    },
  );

  it.each([
    "custom",
    "cn_official",
    "third_party",
    "aggregator",
    "cloud_provider",
    "local",
  ] as const)(
    "preserves the category when editing an existing %s provider",
    async (category) => {
      const user = userEvent.setup();
      const submit = setup(vi.fn(), category);
      await user.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(submit).toHaveBeenCalledOnce());
      expect(submit.mock.calls[0][0].presetCategory).toBe(category);
      expect(JSON.parse(submit.mock.calls[0][0].settingsConfig)).toEqual(
        config,
      );
    },
  );

  it("edits only the models of an official account, keeping its login", async () => {
    const submit = setup();
    expect(
      screen.getByText("kimi.oauth.connectionManaged"),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("opencode.baseUrl")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("API Key")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.provider).toEqual(config.provider);
    expect(saved.models).toEqual(config.models);
    expect(submit.mock.calls[0][0].providerKey).toBe("managed:kimi-code");
  });

  it("adds models from the official account's own catalog", async () => {
    const user = userEvent.setup();
    let requested: unknown;
    server.use(
      http.post("http://tauri.local/kimi_oauth_models", async ({ request }) => {
        requested = await request.json();
        return HttpResponse.json({
          "kimi-code/k3": {
            provider: "managed:kimi-code",
            model: "k3",
            max_context_size: 1048576,
          },
          "kimi-code/k3.1-pro": {
            provider: "managed:kimi-code",
            model: "k3.1-pro",
            display_name: "K3.1 Pro",
            max_context_size: 1048576,
            capabilities: ["tool_use", "thinking"],
            support_efforts: ["high", "max"],
            default_effort: "max",
          },
        });
      }),
    );
    const submit = setup();
    await user.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );
    await user.click(await screen.findByRole("checkbox", { name: "k3.1-pro" }));
    await user.click(
      screen.getByRole("button", { name: "providerForm.modelPicker.apply" }),
    );
    expect(requested).toEqual({ accountId: "managed:kimi-code" });
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.models["kimi-code/k3.1-pro"]).toEqual({
      provider: "managed:kimi-code",
      model: "k3.1-pro",
      display_name: "K3.1 Pro",
      max_context_size: 1048576,
      capabilities: ["tool_use", "thinking"],
      support_efforts: ["high", "max"],
      default_effort: "max",
    });
    expect(saved.models["kimi-code/k3"]).toEqual(config.models["kimi-code/k3"]);
  });

  it("shows request headers inline like Pi", () => {
    const { oauth: _oauth, ...apiKeyProvider } = config.provider;
    setup(vi.fn(), undefined, {
      ...config,
      provider: { ...apiKeyProvider, type: "openai", api_key: "sk-test" },
    });
    expect(screen.getByDisplayValue("X-Custom")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: "providerForm.advancedOptionsToggle",
      }),
    ).not.toBeInTheDocument();
  });

  it("keeps provider key composition local until the IME commits", () => {
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <KimiProviderForm
          appId="kimi"
          submitLabel="Save"
          onSubmit={vi.fn()}
          onCancel={() => {}}
        />
      </QueryClientProvider>,
    );
    const key = screen.getByLabelText("opencode.providerKey");

    fireEvent.compositionStart(key);
    fireEvent.change(key, { target: { value: "Te好st" } });
    // The IME still owns the marked text; normalizing it here would replace the
    // composition range and duplicate the typed letters.
    expect(key).toHaveValue("Te好st");

    fireEvent.compositionEnd(key, { data: "Te好st" });
    expect(key).toHaveValue("test");
  });

  it("flags a preset whose provider key collides with an existing card", async () => {
    const user = userEvent.setup();
    const selected = kimiProviderPresets.find(
      (preset) => preset.name === "Kimi",
    )!;
    server.use(
      http.post("http://tauri.local/get_providers", () =>
        HttpResponse.json({
          [selected.providerKey]: {
            id: selected.providerKey,
            name: "Kimi",
            settingsConfig: {},
          },
        }),
      ),
    );
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <KimiProviderForm
          appId="kimi"
          submitLabel="Save"
          onSubmit={vi.fn()}
          onCancel={() => {}}
        />
      </QueryClientProvider>,
    );
    await user.click(
      screen.getByRole("button", { name: /^providerPreset\.group\.vendor / }),
    );
    await user.click(screen.getByRole("button", { name: "Kimi" }));
    expect(
      await screen.findByText("opencode.providerKeyDuplicate"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("opencode.providerKey")).toHaveValue(
      selected.providerKey,
    );
  });

  it("shows this provider's tables in config.toml layout", () => {
    setup();
    const text = (
      screen.getByLabelText(
        "providerForm.nativeConfig.title",
      ) as HTMLTextAreaElement
    ).value;
    expect(text).toContain('[providers."managed:kimi-code"]');
    expect(text).toContain('[models."kimi-code/k3"]');
    expect(parseToml(text)).toEqual({
      providers: { "managed:kimi-code": config.provider },
      models: config.models,
    });
  });

  it.each([
    ["broken syntax", "[providers", "providerForm.nativeConfig.invalid"],
    [
      "another provider's table",
      '[providers.other]\ntype = "openai"\n',
      "kimi.configTomlScope",
    ],
    [
      "a global setting",
      `default_model = "kimi-code/k3"\n${kimiConfigToToml(config as KimiProviderConfig, "managed:kimi-code")}`,
      "kimi.configTomlScope",
    ],
    [
      "a model without context size",
      '[providers."managed:kimi-code"]\ntype = "kimi"\n\n[models.x]\nmodel = "k3"\n',
      "kimi.configTomlSchema",
    ],
  ])("blocks %s until the TOML is corrected", (_, value, message) => {
    const submit = setup();
    const editor = screen.getByLabelText("providerForm.nativeConfig.title");
    fireEvent.change(editor, { target: { value } });
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.getByText(message)).toBeInTheDocument();
    fireEvent.change(editor, {
      target: {
        value: kimiConfigToToml(
          config as KimiProviderConfig,
          "managed:kimi-code",
        ),
      },
    });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    expect(screen.queryByText(message)).not.toBeInTheDocument();
    expect(submit).not.toHaveBeenCalled();
  });

  it("requires at least one model with a positive context length", () => {
    setup();
    expandModels();
    fireEvent.change(screen.getByLabelText("kimi.contextSize"), {
      target: { value: "0" },
    });
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("kimi.contextSize"), {
      target: { value: "262144" },
    });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "common.delete" }));
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("adds fetched models and fills their window and capabilities from models.dev", async () => {
    const user = userEvent.setup();
    let fetchRequest: Record<string, unknown> | undefined;
    server.use(
      http.post(
        "http://tauri.local/fetch_models_for_config",
        async ({ request }) => {
          fetchRequest = (await request.json()) as Record<string, unknown>;
          return HttpResponse.json([
            { id: "model-x", ownedBy: "example" },
            { id: "model-y", ownedBy: "example" },
          ]);
        },
      ),
      http.post("http://tauri.local/fetch_models_dev", () =>
        HttpResponse.json(
          JSON.stringify({
            example: {
              api: "https://api.example.com/v1",
              models: {
                "model-x": {
                  limit: { context: 123456 },
                  reasoning: true,
                  modalities: { input: ["text", "image"], output: ["text"] },
                },
              },
            },
          }),
        ),
      ),
    );
    const submit = vi.fn();
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <KimiProviderForm
          appId="kimi"
          submitLabel="Save"
          onSubmit={submit}
          onCancel={() => {}}
        />
      </QueryClientProvider>,
    );
    fireEvent.change(screen.getByLabelText("opencode.providerKey"), {
      target: { value: "acme" },
    });
    fireEvent.change(screen.getByLabelText("opencode.baseUrl"), {
      target: { value: "https://api.example.com/v1" },
    });
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "sk-test" },
    });
    fireEvent.change(screen.getByLabelText("provider.name"), {
      target: { value: "Acme" },
    });

    await user.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );
    await user.click(await screen.findByRole("checkbox", { name: "model-x" }));
    await user.click(await screen.findByRole("checkbox", { name: "model-y" }));
    await user.click(
      screen.getByRole("button", { name: "providerForm.modelPicker.apply" }),
    );

    expect(fetchRequest).toMatchObject({
      baseUrl: "https://api.example.com/v1",
      apiKey: "sk-test",
    });
    await waitFor(() =>
      expect(
        (
          parseToml(
            (
              screen.getByLabelText(
                "providerForm.nativeConfig.title",
              ) as HTMLTextAreaElement
            ).value,
          ) as { models: Record<string, { max_context_size: number }> }
        ).models["acme/model-x"].max_context_size,
      ).toBe(123456),
    );
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.models).toEqual({
      "acme/model-x": {
        provider: "acme",
        model: "model-x",
        max_context_size: 123456,
        capabilities: ["tool_use", "image_in", "thinking"],
      },
      // models.dev 没有这款：保留占位窗口，用户自己核对。
      "acme/model-y": {
        provider: "acme",
        model: "model-y",
        max_context_size: 262144,
        capabilities: ["tool_use"],
      },
    });
  });

  it("uses the Anthropic auth header when fetching models for Anthropic providers", async () => {
    const user = userEvent.setup();
    let fetchRequest: Record<string, unknown> | undefined;
    server.use(
      http.post(
        "http://tauri.local/fetch_models_for_config",
        async ({ request }) => {
          fetchRequest = (await request.json()) as Record<string, unknown>;
          return HttpResponse.json([]);
        },
      ),
    );
    setup(vi.fn(), undefined, {
      ...config,
      provider: {
        type: "anthropic",
        base_url: "https://api.example.com",
        api_key: "sk-test",
      },
    });
    await user.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );
    await waitFor(() =>
      expect(fetchRequest).toMatchObject({ apiFormat: "anthropic-messages" }),
    );
  });
});
