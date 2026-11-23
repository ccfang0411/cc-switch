import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { UpdateProvider, useUpdate } from "@/contexts/UpdateContext";

const mocks = vi.hoisted(() => ({
  checkForUpdate: vi.fn(),
  getAutoUpdateStatus: vi.fn(),
  downloadAppUpdate: vi.fn(),
}));

vi.mock("@/lib/api/settings", () => ({
  settingsApi: {
    getAutoUpdateStatus: mocks.getAutoUpdateStatus,
    downloadAppUpdate: mocks.downloadAppUpdate,
  },
}));

vi.mock("@/lib/updater", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/updater")>()),
  checkForUpdate: mocks.checkForUpdate,
}));

let ctx: ReturnType<typeof useUpdate>;
function Probe() {
  ctx = useUpdate();
  return (
    <div data-testid="probe">
      {String(ctx.hasUpdate)}:{ctx.updateInfo?.availableVersion ?? "-"}
    </div>
  );
}

const renderProvider = () =>
  render(
    <UpdateProvider>
      <Probe />
    </UpdateProvider>,
  );

const available = (version: string) => ({
  status: "available",
  info: { currentVersion: "4.0.38-kimicode.dev", availableVersion: version },
});

describe("private branch manual updates", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    localStorage.clear();
    mocks.checkForUpdate.mockReset();
    mocks.getAutoUpdateStatus.mockResolvedValue({
      supported: true,
      enabled: true,
      downloading: false,
      ready: { version: "4.1.0" },
      routingInUse: false,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllEnvs();
  });

  it("does not check or download after startup, including production builds", async () => {
    vi.stubEnv("DEV", false);
    renderProvider();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(7 * 60 * 60 * 1000);
    });
    expect(mocks.checkForUpdate).not.toHaveBeenCalled();
    expect(mocks.getAutoUpdateStatus).not.toHaveBeenCalled();
    expect(mocks.downloadAppUpdate).not.toHaveBeenCalled();
    expect(screen.getByTestId("probe")).toHaveTextContent("false:-");
  });

  it("checks only on request and never starts an automatic download", async () => {
    mocks.checkForUpdate.mockResolvedValue(
      available("4.0.38-kimicode.20261122120000"),
    );
    renderProvider();
    await act(async () => {
      expect(await ctx.checkUpdate()).toBe(true);
    });
    expect(mocks.checkForUpdate).toHaveBeenCalledTimes(1);
    expect(mocks.checkForUpdate).toHaveBeenCalledWith({ timeout: 30000 });
    expect(screen.getByTestId("probe")).toHaveTextContent(
      "true:4.0.38-kimicode.20261122120000",
    );
    expect(mocks.downloadAppUpdate).not.toHaveBeenCalled();
  });

  it("keeps legacy automatic-update callers inert", async () => {
    renderProvider();
    await act(async () => {
      await ctx.refreshAutoUpdate();
      await ctx.startAutoDownload();
    });
    expect(ctx.autoUpdateSupported).toBe(false);
    expect(ctx.autoUpdateDownloading).toBe(false);
    expect(ctx.readyUpdate).toBeNull();
    expect(mocks.getAutoUpdateStatus).not.toHaveBeenCalled();
    expect(mocks.downloadAppUpdate).not.toHaveBeenCalled();
    expect(mocks.checkForUpdate).not.toHaveBeenCalled();
  });

  it("clears stale update details and exposes manual-check errors", async () => {
    mocks.checkForUpdate.mockResolvedValueOnce(available("4.1.0"));
    renderProvider();
    await act(async () => {
      await ctx.checkUpdate();
    });
    mocks.checkForUpdate.mockRejectedValueOnce(new Error("offline"));
    await act(async () => {
      await expect(ctx.checkUpdate()).rejects.toThrow("offline");
    });
    expect(ctx.error).toBe("offline");
    expect(ctx.isChecking).toBe(false);
    expect(screen.getByTestId("probe")).toHaveTextContent("false:-");
  });

  it("remembers dismissal without introducing an automatic network request", async () => {
    localStorage.setItem("dismissedUpdateVersion", "4.1.0");
    mocks.checkForUpdate.mockResolvedValue(available("4.1.0"));
    renderProvider();
    await act(async () => {
      await ctx.checkUpdate();
    });
    expect(ctx.isDismissed).toBe(true);
    expect(localStorage.getItem("dismissedUpdateVersion")).toBeNull();
    act(() => ctx.resetDismiss());
    expect(ctx.isDismissed).toBe(false);
    act(() => ctx.dismissUpdate());
    expect(localStorage.getItem("ccswitch:update:dismissedVersion")).toBe(
      "4.1.0",
    );
    expect(mocks.checkForUpdate).toHaveBeenCalledTimes(1);
  });
});
