import { useCallback, useRef, useState } from "react";
import { CloudUpload, Download, Loader2, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { settingsApi } from "@/lib/api";
import type { OssSqlBackupEntry, OssSqlBackupResult } from "@/lib/api/settings";
import type { S3SyncSettings } from "@/types";
import { syncCurrentProvidersLiveSafe } from "@/utils/postChangeSync";

interface Props {
  config?: S3SyncSettings;
  selectedFile: string;
  disabled: boolean;
  onImportSuccess?: () => void | Promise<void>;
}

interface ImportedRemoteBackup {
  profile: string;
  fileName: string;
  backupId?: string;
}

function isOssConfig(config?: S3SyncSettings): boolean {
  if (
    !config?.bucket ||
    !config.accessKeyId ||
    !config.region ||
    !config.endpoint
  ) {
    return false;
  }
  try {
    const endpoint = config.endpoint.includes("://")
      ? config.endpoint
      : `https://${config.endpoint}`;
    const url = new URL(endpoint);
    const bucketPrefix = `${config.bucket}.`;
    const host = url.hostname.startsWith(bucketPrefix)
      ? url.hostname.slice(bucketPrefix.length)
      : url.hostname;
    return (
      url.protocol === "https:" &&
      !url.username &&
      !url.password &&
      !url.port &&
      url.pathname === "/" &&
      !url.search &&
      !url.hash &&
      /^oss-[a-z0-9-]+\.aliyuncs\.com(?:\.cn)?$/.test(host)
    );
  } catch {
    return false;
  }
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatModified(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

export function OssSqlBackupSection({
  config,
  selectedFile,
  disabled,
  onImportSuccess,
}: Props) {
  const { t } = useTranslation();
  const pending = useRef(false);
  const [uploading, setUploading] = useState<"current" | "selected" | null>(
    null,
  );
  const [result, setResult] = useState<OssSqlBackupResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [remoteBackups, setRemoteBackups] = useState<
    OssSqlBackupEntry[] | null
  >(null);
  const [loadingList, setLoadingList] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const [confirmEntry, setConfirmEntry] = useState<OssSqlBackupEntry | null>(
    null,
  );
  const [importingKey, setImportingKey] = useState<string | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [imported, setImported] = useState<ImportedRemoteBackup | null>(null);

  const configured = isOssConfig(config);
  const destination = configured
    ? `oss://${config!.bucket}/${(config!.remoteRoot || "cc-switch-sync").replace(/^\/+|\/+$/g, "")}/sql-backups/${config!.profile || "default"}/`
    : null;

  const upload = async (source: "current" | "selected") => {
    if (pending.current || disabled || !configured) return;
    pending.current = true;
    setUploading(source);
    setResult(null);
    setError(null);
    try {
      setResult(
        await settingsApi.uploadSqlBackupToOss(
          source === "selected" ? selectedFile : undefined,
        ),
      );
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      pending.current = false;
      setUploading(null);
    }
  };

  const loadRemote = useCallback(async () => {
    if (loadingList || !configured) return;
    setLoadingList(true);
    setListError(null);
    try {
      setRemoteBackups(await settingsApi.listSqlBackupsFromOss());
    } catch (error) {
      setRemoteBackups(null);
      setListError(error instanceof Error ? error.message : String(error));
    } finally {
      setLoadingList(false);
    }
  }, [configured, loadingList]);

  const importRemote = async () => {
    if (!confirmEntry || importingKey) return;
    const entry = confirmEntry;
    setImportingKey(entry.key);
    setImportError(null);
    setImported(null);
    try {
      const response = await settingsApi.importSqlBackupFromOss(entry.key);
      if (!response.success) {
        setImportError(
          response.message || t("settings.ossSqlBackup.remoteImportFailed"),
        );
        setConfirmEntry(null);
        return;
      }
      await onImportSuccess?.();
      const syncResult = await syncCurrentProvidersLiveSafe();
      if (!syncResult.ok) {
        console.error(
          "[OssSqlBackupSection] Failed to sync live config after remote import",
          syncResult.error,
        );
      }
      setImported({
        profile: entry.profile,
        fileName: entry.fileName,
        backupId: response.backupId,
      });
      setConfirmEntry(null);
    } catch (error) {
      setImportError(error instanceof Error ? error.message : String(error));
      setConfirmEntry(null);
    } finally {
      setImportingKey(null);
    }
  };

  return (
    <div className="space-y-3 border-t border-border pt-4">
      <h4 className="text-sm font-medium text-fg-1">
        {t("settings.ossSqlBackup.title")}
      </h4>
      <p className="text-xs text-fg-2">{t("settings.ossSqlBackup.hint")}</p>
      {destination ? (
        <p className="break-all text-xs font-mono text-fg-2">{destination}</p>
      ) : (
        <p className="text-xs text-warning-text">
          {t("settings.ossSqlBackup.configureHint")}
        </p>
      )}
      <div className="flex flex-wrap gap-3">
        <Button
          type="button"
          variant="outline"
          disabled={disabled || !!uploading || !!importingKey || !configured}
          onClick={() => void upload("current")}
        >
          {uploading === "current" ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <CloudUpload className="h-4 w-4" />
          )}
          {uploading === "current"
            ? t("settings.ossSqlBackup.uploading")
            : t("settings.ossSqlBackup.exportUpload")}
        </Button>
        {selectedFile && (
          <Button
            type="button"
            variant="outline"
            disabled={disabled || !!uploading || !!importingKey || !configured}
            onClick={() => void upload("selected")}
          >
            {uploading === "selected" ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <CloudUpload className="h-4 w-4" />
            )}
            {uploading === "selected"
              ? t("settings.ossSqlBackup.uploading")
              : t("settings.ossSqlBackup.uploadSelected")}
          </Button>
        )}
      </div>
      {uploading && (
        <p role="status" className="text-xs text-fg-2">
          {t("settings.ossSqlBackup.verifying")}
        </p>
      )}
      {error && (
        <p role="alert" className="break-all text-sm text-danger-text">
          {t("settings.ossSqlBackup.failed", { message: error })}
        </p>
      )}
      {result && (
        <div
          role="status"
          className="space-y-2 rounded-lg bg-success-soft p-3 text-xs text-success-text"
        >
          <p>
            {t("settings.ossSqlBackup.success", {
              size: (result.sizeBytes / 1024).toFixed(1),
            })}
          </p>
          <p className="break-all font-mono select-text">{result.ossPath}</p>
        </div>
      )}

      <div className="space-y-3 border-t border-border pt-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="space-y-1">
            <h5 className="text-sm font-medium text-fg-1">
              {t("settings.ossSqlBackup.remoteTitle")}
            </h5>
            <p className="text-xs text-fg-2">
              {t("settings.ossSqlBackup.remoteHint")}
            </p>
          </div>
          <Button
            type="button"
            variant="outline"
            size="compact"
            disabled={disabled || loadingList || !!importingKey || !configured}
            onClick={() => void loadRemote()}
          >
            {loadingList ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <RefreshCw className="h-3.5 w-3.5" />
            )}
            {loadingList
              ? t("settings.ossSqlBackup.remoteLoading")
              : t("settings.ossSqlBackup.remoteRefresh")}
          </Button>
        </div>

        {listError && (
          <p role="alert" className="break-all text-sm text-danger-text">
            {t("settings.ossSqlBackup.remoteFailed", { message: listError })}
          </p>
        )}
        {remoteBackups && remoteBackups.length === 0 && (
          <p className="text-xs text-fg-2">
            {t("settings.ossSqlBackup.remoteEmpty")}
          </p>
        )}
        {remoteBackups && remoteBackups.length > 0 && (
          <ul className="max-h-64 space-y-2 overflow-y-auto">
            {remoteBackups.slice(0, 20).map((entry) => (
              <li
                key={entry.key}
                className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border px-3 py-2"
              >
                <div className="min-w-0 flex-1 space-y-0.5">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-body font-medium text-fg-1">
                      {entry.profile}
                    </span>
                    {entry.profile === config?.profile && (
                      <Badge
                        variant="secondary"
                        className="px-1.5 py-0 text-badge"
                      >
                        {t("settings.ossSqlBackup.localProfile")}
                      </Badge>
                    )}
                  </div>
                  <div className="break-all font-mono text-caption text-fg-2">
                    {entry.fileName}
                  </div>
                  <div className="text-caption text-fg-3">
                    {formatSize(entry.sizeBytes)} ·{" "}
                    {formatModified(entry.lastModified)}
                  </div>
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="compact"
                  disabled={disabled || !!importingKey}
                  onClick={() => {
                    setConfirmEntry(entry);
                    setImportError(null);
                  }}
                >
                  {importingKey === entry.key ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Download className="h-3.5 w-3.5" />
                  )}
                  {importingKey === entry.key
                    ? t("settings.ossSqlBackup.remoteImporting")
                    : t("settings.ossSqlBackup.remoteImport")}
                </Button>
              </li>
            ))}
          </ul>
        )}
        {importError && (
          <p role="alert" className="break-all text-sm text-danger-text">
            {t("settings.ossSqlBackup.remoteImportFailed", {
              message: importError,
            })}
          </p>
        )}
        {imported && (
          <div
            role="status"
            className="space-y-1 rounded-lg bg-success-soft p-3 text-xs text-success-text"
          >
            <p>
              {t("settings.ossSqlBackup.remoteImportSuccess", {
                profile: imported.profile,
                file: imported.fileName,
              })}
            </p>
            {imported.backupId && (
              <p className="break-all font-mono">{imported.backupId}</p>
            )}
          </div>
        )}
      </div>

      <ConfirmDialog
        isOpen={!!confirmEntry}
        variant="destructive"
        title={t("settings.ossSqlBackup.remoteConfirmTitle")}
        message={t("settings.ossSqlBackup.remoteConfirmBody", {
          profile: confirmEntry?.profile ?? "",
          file: confirmEntry?.fileName ?? "",
        })}
        confirmText={t("settings.ossSqlBackup.remoteImport")}
        pending={!!importingKey}
        onConfirm={() => void importRemote()}
        onCancel={() => {
          if (!importingKey) setConfirmEntry(null);
        }}
      />
    </div>
  );
}
