import type { ReactElement } from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render as rtlRender,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fetchCodexOauthModels } from "@/lib/api/model-fetch";
import { KimicodeProviderForm } from "@/components/providers/forms/KimicodeProviderForm";
import { kimicodeProviderPresets } from "@/config/kimicodeProviderPresets";
import { setProviders } from "../msw/state";
import { createTestQueryClient } from "../utils/testQueryClient";

vi.mock("@/lib/api/model-fetch", () => ({
  fetchCodexOauthModels: vi.fn(),
}));
beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.mocked(fetchCodexOauthModels).mockReset().mockResolvedValue([]);
});

vi.mock("@/components/JsonEditor", () => ({
  default: ({
    id,
    value,
    onChange,
  }: {
    id: string;
    value: string;
    onChange: (value: string) => void;
  }) => (
    <textarea
      id={id}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

const render = (ui: ReactElement) =>
  rtlRender(
    <QueryClientProvider client={createTestQueryClient()}>
      {ui}
    </QueryClientProvider>,
  );

const original = {
  kind: "custom",
  enabled: true,
  type: "kimi",
  base_url: "https://api.kimi.com/coding/v1",
  api_key: "local-test-key",
  custom_headers: { "X-Test": "keep" },
  model: {
    model: "k3",
    max_context_size: 1048576,
    capabilities: ["thinking", "image_in", "tool_use"],
    display_name: "K3",
  },
  futureSetting: { keep: true },
};

describe("KimicodeProviderForm", () => {
  it("saves a managed account binding without requesting an API key", async () => {
    vi.mocked(fetchCodexOauthModels).mockResolvedValue([
      {
        id: "account-available-model",
        ownedBy: "Codex",
        contextWindow: 200000,
      },
    ]);
    const client = createTestQueryClient();
    client.setQueryDefaults(["managed-auth-status"], { staleTime: Infinity });
    client.setQueryData(["managed-auth-status", "codex_oauth"], {
      authenticated: true,
      default_account_id: "account-1",
      accounts: [{ id: "account-1", login: "test", requires_reauth: false }],
    });
    const submit = vi.fn();
    rtlRender(
      <QueryClientProvider client={client}>
        <KimicodeProviderForm
          appId="kimicode"
          providerId="chatgpt"
          initialData={{
            name: "ChatGPT",
            settingsConfig: { ...original, api_key: "" },
            meta: {
              providerType: "codex_oauth",
              authBinding: {
                source: "managed_account",
                authProvider: "codex_oauth",
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
    expect(screen.queryByLabelText("API Key")).not.toBeInTheDocument();
    await waitFor(() =>
      expect(fetchCodexOauthModels).toHaveBeenCalledWith("account-1"),
    );
    expect(document.getElementById("kimicode-model-id")).toHaveValue("k3");
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Select model",
      }),
    );
    fireEvent.click(await screen.findByText("account-available-model"));
    expect(submit).not.toHaveBeenCalled();
    vi.mocked(fetchCodexOauthModels).mockRejectedValueOnce(
      new Error("offline"),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "providerForm.fetchModelsFailed",
    );
    expect(document.getElementById("kimicode-model-id")).toHaveValue(
      "account-available-model",
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    expect(submit.mock.calls[0][0].meta.authBinding.accountId).toBe(
      "account-1",
    );
    expect(JSON.parse(submit.mock.calls[0][0].settingsConfig).api_key).toBe(
      "MANAGED_AUTH",
    );
    expect(JSON.parse(submit.mock.calls[0][0].settingsConfig).model.model).toBe(
      "account-available-model",
    );
  });

  it("lets users choose several official models and keep a separate default and effort", async () => {
    vi.mocked(fetchCodexOauthModels).mockResolvedValue([
      {
        id: "first",
        ownedBy: "Codex",
        contextWindow: 200000,
        supportedReasoningEfforts: ["low", "high"],
        defaultReasoningEffort: "high",
      },
      {
        id: "second",
        ownedBy: "Codex",
        contextWindow: 400000,
        supportedReasoningEfforts: ["low", "xhigh"],
        defaultReasoningEffort: "xhigh",
      },
      { id: "unselected", ownedBy: "Codex", contextWindow: 100000 },
    ]);
    const client = createTestQueryClient();
    client.setQueryDefaults(["managed-auth-status"], { staleTime: Infinity });
    client.setQueryData(["managed-auth-status", "codex_oauth"], {
      authenticated: true,
      default_account_id: "account-1",
      accounts: [{ id: "account-1", login: "test", requires_reauth: false }],
    });
    const submit = vi.fn();
    rtlRender(
      <QueryClientProvider client={client}>
        <KimicodeProviderForm
          appId="kimicode"
          providerId="chatgpt"
          initialData={{
            name: "ChatGPT",
            settingsConfig: {
              ...original,
              model: { model: "first", max_context_size: 1 },
            },
            meta: {
              providerType: "codex_oauth",
              authBinding: {
                source: "managed_account",
                authProvider: "codex_oauth",
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
    fireEvent.click(
      await screen.findByRole("button", { name: "kimicode.form.chooseModels" }),
    );
    fireEvent.click(screen.getByRole("checkbox", { name: /second/ }));
    fireEvent.click(screen.getByRole("button", { name: "common.done" }));
    fireEvent.click(
      screen.getByRole("button", { name: "kimicode.form.setDefaultModel" }),
    );
    fireEvent.click(document.getElementById("kimi-catalog-effort-0")!);
    fireEvent.click(await screen.findByRole("option", { name: "low" }));
    // Selecting an already selected model through the original dropdown must not duplicate it.
    fireEvent.click(screen.getByRole("button", { name: "Select model" }));
    fireEvent.click(await screen.findByRole("option", { name: "first" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.model.model).toBe("first");
    expect(saved.model.max_context_size).toBe(200000);
    expect(saved.models).toHaveLength(2);
    expect(saved.model.capabilities).toContain("thinking");
    expect(
      saved.models.find((model: { model: string }) => model.model === "second"),
    ).toMatchObject({
      max_context_size: 400000,
      default_effort: "low",
      support_efforts: ["low", "xhigh"],
    });
    expect(
      saved.models.some(
        (model: { model: string }) => model.model === "unselected",
      ),
    ).toBe(false);
  });

  it("edits native fields while preserving model options and unknown settings", async () => {
    const submit = vi.fn();
    render(
      <KimicodeProviderForm
        appId="kimicode"
        providerId="existing"
        initialData={{ name: "Existing", settingsConfig: original }}
        submitLabel="Save"
        onSubmit={submit}
        onCancel={() => {}}
      />,
    );
    fireEvent.change(screen.getByLabelText("provider.name"), {
      target: { value: "Renamed" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved).toEqual({ ...original, name: "Renamed" });
    expect(submit.mock.calls[0][0].providerKey).toBe("existing");
    expect(screen.getByLabelText("API Key")).toHaveAttribute(
      "type",
      "password",
    );
  });

  it("edits the model section without touching the rest of the config", async () => {
    const submit = vi.fn();
    render(
      <KimicodeProviderForm
        appId="kimicode"
        providerId="existing"
        initialData={{ name: "Existing", settingsConfig: original }}
        submitLabel="Save"
        onSubmit={submit}
        onCancel={() => {}}
      />,
    );
    fireEvent.change(screen.getByLabelText("kimicode.form.capabilities"), {
      target: { value: "thinking, tool_use" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved).toEqual({
      ...original,
      name: "Existing",
      model: {
        ...original.model,
        capabilities: ["thinking", "tool_use"],
      },
    });
  });

  it("keeps a rejected save open and displays the failure", async () => {
    render(
      <KimicodeProviderForm
        appId="kimicode"
        initialData={{ name: "Existing", settingsConfig: original }}
        submitLabel="Save"
        onSubmit={async () => {
          throw new Error("configuration busy");
        }}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "configuration busy",
    );
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  // 同一家（family: kimi）的版本按 PRESET_FAMILIES 的 planOrder 排序：payg 在
  // coding 前，所以直接点「Kimi」行选中的是「Moonshot 开放平台(国内)」。
  it("applies the family's first version when the Kimi row is picked", async () => {
    const submit = vi.fn();
    render(
      <KimicodeProviderForm
        appId="kimicode"
        submitLabel="Save"
        onSubmit={submit}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Kimi" }));
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "test-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.type).toBe("kimi");
    expect(saved.base_url).toBe("https://api.moonshot.cn/v1");
    expect(saved.api_key).toBe("test-key");
    expect(saved.model).toMatchObject({
      model: "kimi-k2.7-code",
      max_context_size: 262144,
      capabilities: ["thinking", "tool_use"],
      display_name: "Kimi K2.7 Code",
    });
    expect(submit.mock.calls[0][0].providerKey).toBe("moonshot-cn");
  });

  it("picks the official subscription version when search hits it", async () => {
    const submit = vi.fn();
    render(
      <KimicodeProviderForm
        appId="kimicode"
        submitLabel="Save"
        onSubmit={submit}
        onCancel={() => {}}
      />,
    );
    fireEvent.change(
      screen.getByPlaceholderText("providerPreset.searchPlaceholder"),
      { target: { value: "官方订阅" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Kimi" }));
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "test-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.type).toBe("kimi");
    expect(saved.base_url).toBe("https://api.kimi.com/coding/v1");
    expect(saved.api_key).toBe("test-key");
    expect(saved.model).toMatchObject({
      model: "k3",
      max_context_size: 1048576,
      capabilities: ["thinking", "image_in", "tool_use"],
      display_name: "Kimi K3",
    });
    expect(submit.mock.calls[0][0].providerKey).toBe("kimi-code");
  });

  it("selects both DeepSeek models and saves the chosen default and effort", async () => {
    const submit = vi.fn();
    render(
      <KimicodeProviderForm
        appId="kimicode"
        submitLabel="Save"
        onSubmit={submit}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "DeepSeek" }));
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "test-key" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "kimicode.form.chooseModels" }),
    );
    fireEvent.click(screen.getByRole("checkbox", { name: /deepseek-v4-pro/ }));
    fireEvent.click(screen.getByRole("button", { name: "common.done" }));
    fireEvent.click(
      screen.getByRole("button", { name: "kimicode.form.setDefaultModel" }),
    );
    fireEvent.click(document.getElementById("kimi-catalog-effort-0")!);
    fireEvent.click(await screen.findByRole("option", { name: "max" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.type).toBe("anthropic");
    expect(saved.base_url).toBe("https://api.deepseek.com/anthropic");
    expect(saved.model).toMatchObject({
      model: "deepseek-v4-pro",
      max_context_size: 1048576,
      default_effort: "max",
    });
    expect(saved.models).toHaveLength(2);
    expect(
      saved.models.find(
        (model: { model: string }) => model.model === "deepseek-flash",
      ).capabilities,
    ).toContain("image_in");
  });

  it("applies the standalone Anthropic-compatible preset", async () => {
    const anthropic = kimicodeProviderPresets.find(
      (preset) => preset.name === "Anthropic 兼容端点",
    )!;
    const submit = vi.fn();
    render(
      <KimicodeProviderForm
        appId="kimicode"
        submitLabel="Save"
        onSubmit={submit}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Anthropic 兼容端点" }));
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "test-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const saved = JSON.parse(submit.mock.calls[0][0].settingsConfig);
    expect(saved.type).toBe("anthropic");
    expect(saved.base_url).toBe(anthropic.settingsConfig.base_url);
    expect(saved.model.model).toBe("kimi-k2.7-code");
    expect(submit.mock.calls[0][0].providerKey).toBe("moonshot-anthropic");
  });

  it("requires a well-formed, unused provider key for new providers", async () => {
    setProviders("kimicode", {
      taken: { id: "taken", name: "Taken", settingsConfig: original },
    });
    const submit = vi.fn();
    render(
      <KimicodeProviderForm
        appId="kimicode"
        submitLabel="Save"
        onSubmit={submit}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Kimi" }));
    fireEvent.change(screen.getByLabelText("API Key"), {
      target: { value: "test-key" },
    });
    const key = screen.getByLabelText(/opencode\.providerKey/);
    const save = screen.getByRole("button", { name: "Save" });
    expect(key).toHaveValue("moonshot-cn");
    expect(save).toBeEnabled();

    fireEvent.change(key, { target: { value: "" } });
    expect(save).toBeDisabled();
    fireEvent.change(key, { target: { value: "-bad-" } });
    expect(screen.getByText("opencode.providerKeyInvalid")).toBeInTheDocument();
    expect(save).toBeDisabled();
    fireEvent.change(key, { target: { value: "taken" } });
    expect(
      await screen.findByText("opencode.providerKeyDuplicate"),
    ).toBeInTheDocument();
    expect(save).toBeDisabled();

    fireEvent.change(key, { target: { value: "My Key!" } });
    expect(key).toHaveValue("mykey");
    fireEvent.click(save);
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    expect(submit.mock.calls[0][0].providerKey).toBe("mykey");
  });

  it("locks the provider key when editing", () => {
    render(
      <KimicodeProviderForm
        appId="kimicode"
        providerId="existing"
        initialData={{ name: "Existing", settingsConfig: original }}
        submitLabel="Save"
        onSubmit={vi.fn()}
        onCancel={() => {}}
      />,
    );
    const key = screen.getByLabelText(/opencode\.providerKey/);
    expect(key).toHaveValue("existing");
    expect(key).toBeDisabled();
    expect(
      screen.getByText("opencode.providerKeyLockedHint"),
    ).toBeInTheDocument();
  });

  it("keeps invalid JSON drafts from breaking the structured fields", () => {
    render(
      <KimicodeProviderForm
        appId="kimicode"
        initialData={{ name: "Existing", settingsConfig: original }}
        submitLabel="Save"
        onSubmit={vi.fn()}
        onCancel={() => {}}
      />,
    );
    fireEvent.change(screen.getByLabelText("provider.configJson"), {
      target: { value: '{"model":{"max_context_size":"big"}}' },
    });
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("provider.configJson"), {
      target: { value: JSON.stringify(original) },
    });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });
});
