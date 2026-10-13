import { useRef, useState } from "react";
import { CloudUpload, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { settingsApi } from "@/lib/api";
import type { OssSqlBackupResult } from "@/lib/api/settings";
import type { S3SyncSettings } from "@/types";

interface Props {
  config?: S3SyncSettings;
  selectedFile: string;
  disabled: boolean;
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

export function OssSqlBackupSection({ config, selectedFile, disabled }: Props) {
  const { t } = useTranslation();
  const pending = useRef(false);
  const [uploading, setUploading] = useState<"current" | "selected" | null>(
    null,
  );
  const [result, setResult] = useState<OssSqlBackupResult | null>(null);
  const [error, setError] = useState<string | null>(null);
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
          disabled={disabled || !!uploading || !configured}
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
            disabled={disabled || !!uploading || !configured}
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
    </div>
  );
}
