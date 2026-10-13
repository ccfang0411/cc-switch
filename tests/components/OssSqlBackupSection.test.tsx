import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { OssSqlBackupSection } from "@/components/settings/OssSqlBackupSection";
import { settingsApi } from "@/lib/api";

vi.mock("@/lib/api", () => ({
  settingsApi: { uploadSqlBackupToOss: vi.fn() },
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

describe("OSS SQL backup", () => {
  beforeEach(() => {
    vi.mocked(settingsApi.uploadSqlBackupToOss).mockReset();
  });

  it("requires saved OSS cloud configuration and does not expose credential inputs", () => {
    render(<OssSqlBackupSection disabled={false} selectedFile="" />);
    expect(
      screen.getByText("settings.ossSqlBackup.configureHint"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button")).toBeDisabled();
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

  it("disables OSS backups while importing and rejects non-OSS endpoints", () => {
    const { rerender } = render(
      <OssSqlBackupSection config={config} disabled selectedFile="" />,
    );
    expect(screen.getByRole("button")).toBeDisabled();
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
    expect(screen.getByRole("button")).toBeDisabled();
  });
});
