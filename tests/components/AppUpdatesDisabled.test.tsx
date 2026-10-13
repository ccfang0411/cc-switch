import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { check } from "@tauri-apps/plugin-updater";
import { invoke } from "@tauri-apps/api/core";
import { AboutSection } from "@/components/settings/AboutSection";
import { DatabaseUpgrade } from "@/components/DatabaseUpgrade";
import { UpdateProvider, useUpdate } from "@/contexts/UpdateContext";
import { checkForUpdate } from "@/lib/updater";

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("4.0.9-kimicode.2"),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn().mockResolvedValue({ version: "4.0.10" }),
}));
vi.mock("@/components/WhatsNewDialog", () => ({
  WhatsNewDialog: () => null,
}));

function UpdateProbe() {
  const updates = useUpdate();
  return (
    <button onClick={() => void updates.checkUpdate()}>
      {updates.hasUpdate ? "update available" : "updates disabled"}
    </button>
  );
}

describe("independent branch application updates", () => {
  beforeEach(() => vi.clearAllMocks());

  it("does not contact the updater through either public checking API", async () => {
    expect(await checkForUpdate()).toEqual({ status: "up-to-date" });
    render(
      <UpdateProvider>
        <UpdateProbe />
      </UpdateProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "updates disabled" }));
    expect(check).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("shows the branch status without offering check or install actions", async () => {
    render(<AboutSection isPortable={false} />);
    await screen.findByText(/4\.0\.9-kimicode\.2/);
    expect(
      screen.getByText("settings.branchUpdatesDisabled"),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: /settings.checkForUpdates|settings.updateTo/,
      }),
    ).not.toBeInTheDocument();
    expect(check).not.toHaveBeenCalled();
  });

  it("keeps database recovery local and never checks or installs an update", async () => {
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
