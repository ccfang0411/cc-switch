import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionManagerPage } from "@/components/sessions/SessionManagerPage";
import { DEFAULT_VISIBLE_APPS } from "@/config/appConfig";
import { sessionsApi } from "@/lib/api/sessions";
import type { SessionMeta } from "@/types";
import { setSessionFixtures, setSettings } from "../msw/state";

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getTotalSize: () => count * 100,
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: index * 100,
      })),
    measureElement: () => undefined,
    scrollToIndex: () => undefined,
  }),
}));

const renderPage = () => {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <SessionManagerPage appId="kimicode" />
    </QueryClientProvider>,
  );
};

describe("SessionRename", () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.localStorage.setItem(
      "cc-switch.sessionManager.expandedProjects",
      JSON.stringify(["/mock/kimi"]),
    );

    const now = Date.now();
    const sessions: SessionMeta[] = [
      {
        providerId: "kimicode",
        sessionId: "session-kimi-1",
        title: "Kimi Session",
        summary: "Kimi summary",
        projectDir: "/mock/kimi",
        createdAt: now - 3600 * 1000,
        lastActiveAt: now - 1800 * 1000,
        sourcePath: "/mock/kimi/session-kimi-1/agents/main/wire.jsonl",
        resumeCommand: "kimi --session session-kimi-1",
      },
    ];
    setSessionFixtures(sessions, {});
    setSettings({ visibleApps: { ...DEFAULT_VISIBLE_APPS, kimicode: true } });
  });

  it("renames a Kimi Code session from the row menu", async () => {
    const rename = vi.spyOn(sessionsApi, "rename");
    renderPage();

    await screen.findByText("Kimi Session");
    await userEvent.click(
      screen.getByRole("button", { name: "Kimi Session 的更多操作" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "重命名…" }),
    );

    const dialog = await screen.findByRole("dialog");
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: "重命名会话" }),
      { target: { value: "Renamed Session" } },
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));

    await waitFor(() => expect(rename).toHaveBeenCalledTimes(1));
    expect(rename).toHaveBeenCalledWith({
      providerId: "kimicode",
      sessionId: "session-kimi-1",
      sourcePath: "/mock/kimi/session-kimi-1/agents/main/wire.jsonl",
      title: "Renamed Session",
    });
    expect(await screen.findByText("Renamed Session")).toBeInTheDocument();
    rename.mockRestore();
  });
});
