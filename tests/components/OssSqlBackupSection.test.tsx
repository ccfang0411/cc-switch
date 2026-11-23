import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { OssSqlBackupSection } from "@/components/settings/OssSqlBackupSection";
import { settingsApi } from "@/lib/api";

vi.mock("@/lib/api", () => ({
  settingsApi: {
    uploadSqlBackupToOss: vi.fn(),
    listSqlBackupsFromOss: vi.fn(),
    importSqlBackupFromOss: vi.fn(),
  },
}));
vi.mock("@/utils/postChangeSync", () => ({
  syncCurrentProvidersLiveSafe: vi.fn().mockResolvedValue({ ok: true }),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, values?: { message?: string }) =>
      `${key}${values?.message ? `: ${values.message}` : ""}`,
  }),
}));

const config = {
  enabled: false,
  region: "cn-hangzhou",
  bucket: "test-bucket",
  accessKeyId: "testId",
  endpoint: "oss-cn-hangzhou.aliyuncs.com",
  remoteRoot: "cc-switch-sync",
  profile: "My-Mac",
};
const result = {
  ossPath: "oss://test-bucket/backup.sql",
  sizeBytes: 1234,
  sha256: "abc",
  uploadedAt: "2026-10-13T10:00:00Z",
};
const remoteBackup = {
  key: "cc-switch-sync/sql-backups/Other-Mac/cc-switch-backup-1.sql",
  profile: "Other-Mac",
  fileName: "cc-switch-backup-1.sql",
  sizeBytes: 2048,
  lastModified: "2026-10-19T06:58:05.000Z",
  ossPath:
    "oss://test-bucket/cc-switch-sync/sql-backups/Other-Mac/cc-switch-backup-1.sql",
};
const localBackup = {
  ...remoteBackup,
  key: "cc-switch-sync/sql-backups/My-Mac/cc-switch-backup-2.sql",
  profile: "My-Mac",
  fileName: "cc-switch-backup-2.sql",
};

describe("OSS SQL backup", () => {
  beforeEach(() => {
    vi.mocked(settingsApi.uploadSqlBackupToOss).mockReset();
    vi.mocked(settingsApi.listSqlBackupsFromOss).mockReset();
    vi.mocked(settingsApi.importSqlBackupFromOss).mockReset();
  });

  it("requires saved OSS cloud configuration and does not expose credential inputs", () => {
    render(<OssSqlBackupSection disabled={false} selectedFile="" />);
    expect(
      screen.getByText("settings.ossSqlBackup.configureHint"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.exportUpload",
      }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.remoteRefresh",
      }),
    ).toBeDisabled();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("reuses cloud settings even when automatic sync is disabled", async () => {
    vi.mocked(settingsApi.uploadSqlBackupToOss).mockResolvedValue(result);
    render(
      <OssSqlBackupSection config={config} disabled={false} selectedFile="" />,
    );
    expect(
      screen.getByText(/cc-switch-sync\/sql-backups\/My-Mac/),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.exportUpload",
      }),
    );
    await waitFor(() =>
      expect(screen.getByText(result.ossPath)).toBeInTheDocument(),
    );
    expect(settingsApi.uploadSqlBackupToOss).toHaveBeenCalledWith(undefined);
  });

  it("uploads the selected SQL file without importing it", async () => {
    vi.mocked(settingsApi.uploadSqlBackupToOss).mockResolvedValue(result);
    render(
      <OssSqlBackupSection
        config={config}
        disabled={false}
        selectedFile="/tmp/my-backup.sql"
      />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.uploadSelected",
      }),
    );
    await waitFor(() =>
      expect(screen.getByText(result.ossPath)).toBeInTheDocument(),
    );
    expect(settingsApi.uploadSqlBackupToOss).toHaveBeenCalledWith(
      "/tmp/my-backup.sql",
    );
  });

  it("blocks duplicate uploads, reports failures, and allows retry", async () => {
    let reject!: (reason: Error) => void;
    vi.mocked(settingsApi.uploadSqlBackupToOss).mockReturnValue(
      new Promise((_, fail) => {
        reject = fail;
      }),
    );
    render(
      <OssSqlBackupSection
        config={config}
        disabled={false}
        selectedFile="/tmp/file.sql"
      />,
    );
    const upload = screen.getByRole("button", {
      name: "settings.ossSqlBackup.exportUpload",
    });
    fireEvent.click(upload);
    fireEvent.click(upload);
    expect(settingsApi.uploadSqlBackupToOss).toHaveBeenCalledTimes(1);
    expect(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.uploadSelected",
      }),
    ).toBeDisabled();
    reject(new Error("Forbidden"));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("Forbidden"),
    );
    expect(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.exportUpload",
      }),
    ).toBeEnabled();
    expect(screen.queryByText(result.ossPath)).not.toBeInTheDocument();
  });

  it("lists backups from other hosts and imports one after confirmation", async () => {
    vi.mocked(settingsApi.listSqlBackupsFromOss).mockResolvedValue([
      remoteBackup,
      localBackup,
    ]);
    vi.mocked(settingsApi.importSqlBackupFromOss).mockResolvedValue({
      success: true,
      message: "ok",
      backupId: "backup-1",
    });
    const onImportSuccess = vi.fn();
    render(
      <OssSqlBackupSection
        config={config}
        disabled={false}
        selectedFile=""
        onImportSuccess={onImportSuccess}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.remoteRefresh",
      }),
    );
    await screen.findByText("Other-Mac");
    expect(
      screen.getByText("settings.ossSqlBackup.localProfile"),
    ).toBeInTheDocument();

    fireEvent.click(
      screen.getAllByRole("button", {
        name: "settings.ossSqlBackup.remoteImport",
      })[0],
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(dialog).getByRole("button", {
        name: "settings.ossSqlBackup.remoteImport",
      }),
    );

    await waitFor(() =>
      expect(settingsApi.importSqlBackupFromOss).toHaveBeenCalledWith(
        remoteBackup.key,
      ),
    );
    await waitFor(() => expect(onImportSuccess).toHaveBeenCalledTimes(1));
    expect(
      screen.getByText("settings.ossSqlBackup.remoteImportSuccess"),
    ).toBeInTheDocument();
  });

  it("keeps a successful import visible while reporting post-import sync warnings", async () => {
    vi.mocked(settingsApi.listSqlBackupsFromOss).mockResolvedValue([
      remoteBackup,
    ]);
    vi.mocked(settingsApi.importSqlBackupFromOss).mockResolvedValue({
      success: true,
      message: "ok",
      backupId: "backup-before-import",
      warning: "MCP synchronization failed",
    });
    render(
      <OssSqlBackupSection config={config} disabled={false} selectedFile="" />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.remoteRefresh",
      }),
    );
    await screen.findByText("Other-Mac");
    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.remoteImport",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(dialog).getByRole("button", {
        name: "settings.ossSqlBackup.remoteImport",
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "MCP synchronization failed",
      ),
    );
    expect(
      screen.getByText("settings.ossSqlBackup.remoteImportSuccess"),
    ).toBeInTheDocument();
    expect(screen.getByText("backup-before-import")).toBeInTheDocument();
  });

  it("shows remote listing and import failures", async () => {
    vi.mocked(settingsApi.listSqlBackupsFromOss).mockRejectedValue(
      new Error("AccessDenied"),
    );
    render(
      <OssSqlBackupSection config={config} disabled={false} selectedFile="" />,
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.remoteRefresh",
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("AccessDenied"),
    );

    vi.mocked(settingsApi.listSqlBackupsFromOss).mockResolvedValue([
      remoteBackup,
    ]);
    vi.mocked(settingsApi.importSqlBackupFromOss).mockResolvedValue({
      success: false,
      message: "SQL file is corrupted",
    });
    fireEvent.click(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.remoteRefresh",
      }),
    );
    await screen.findByText("Other-Mac");
    fireEvent.click(
      screen.getAllByRole("button", {
        name: "settings.ossSqlBackup.remoteImport",
      })[0],
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(dialog).getByRole("button", {
        name: "settings.ossSqlBackup.remoteImport",
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "SQL file is corrupted",
      ),
    );
  });

  it("disables OSS backups while importing and rejects non-OSS endpoints", () => {
    const { rerender } = render(
      <OssSqlBackupSection config={config} disabled selectedFile="" />,
    );
    expect(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.remoteRefresh",
      }),
    ).toBeDisabled();
    rerender(
      <OssSqlBackupSection
        config={{
          ...config,
          endpoint: "oss-cn-hangzhou.aliyuncs.com.evil.test",
        }}
        disabled={false}
        selectedFile=""
      />,
    );
    expect(
      screen.getByRole("button", {
        name: "settings.ossSqlBackup.remoteRefresh",
      }),
    ).toBeDisabled();
  });
});
