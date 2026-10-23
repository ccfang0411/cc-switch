import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { check } from "@tauri-apps/plugin-updater";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "@/lib/toast";
import { AboutSection } from "@/components/settings/AboutSection";
import { DatabaseUpgrade } from "@/components/DatabaseUpgrade";
import { UpdateProvider } from "@/contexts/UpdateContext";

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("4.0.18-kimicode.20261023105539"),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));
vi.mock("@/components/WhatsNewDialog", () => ({
  WhatsNewDialog: () => null,
}));

function renderAbout(isPortable = false) {
  return render(
    <UpdateProvider>
      <AboutSection isPortable={isPortable} />
    </UpdateProvider>,
  );
}

describe("about page private updates", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(check).mockResolvedValue(null);
  });

  it("does not check for updates on mount", async () => {
    renderAbout();
    await screen.findByText(/4\.0\.18-kimicode\.20261023105539/);
    expect(check).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("reports up-to-date after a manual check", async () => {
    renderAbout();
    await screen.findByText(/4\.0\.18-kimicode\.20261023105539/);

    fireEvent.click(
      screen.getByRole("button", { name: /settings.checkForUpdates/ }),
    );

    await waitFor(() => expect(check).toHaveBeenCalledTimes(1));
    expect(toast.success).toHaveBeenCalledWith("settings.upToDate", {
      closeButton: true,
    });
  });

  it("installs an available private update", async () => {
    vi.mocked(check).mockResolvedValue({
      version: "4.0.18-kimicode.20261023120000",
      body: "私有版本更新说明",
      date: "2026-10-23T04:00:00Z",
    } as any);
    vi.mocked(invoke).mockResolvedValue(true);
    renderAbout();
    await screen.findByText(/4\.0\.18-kimicode\.20261023105539/);

    fireEvent.click(
      screen.getByRole("button", { name: /settings.checkForUpdates/ }),
    );

    await screen.findByText("settings.updateAvailable");
    expect(screen.getByText("私有版本更新说明")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /settings.updateTo/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("install_update_and_restart"),
    );
  });

  it("offers the OSS DMG when the app is portable", async () => {
    vi.mocked(check).mockResolvedValue({
      version: "4.0.18-kimicode.20261023120000",
    } as any);
    renderAbout(true);
    await screen.findByText(/4\.0\.18-kimicode\.20261023105539/);

    fireEvent.click(
      screen.getByRole("button", { name: /settings.checkForUpdates/ }),
    );
    await screen.findByText("settings.updateAvailable");

    fireEvent.click(screen.getByRole("button", { name: /settings.updateTo/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "open_external",
        expect.objectContaining({
          url: expect.stringContaining("cc-switch-updates"),
        }),
      ),
    );
  });

  it("keeps database recovery local and never checks an update", async () => {
    render(
      <DatabaseUpgrade
        payload={{
          db_version: 99,
          supported_version: 22,
          path: "/tmp/cc-switch.db",
        }}
      />,
    );
    expect(screen.getByText("dbUpgrade.branchRecovery")).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "打开配置目录" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("open_app_config_folder"),
    );
    expect(check).not.toHaveBeenCalled();
  });
});
