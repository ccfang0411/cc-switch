import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { usageSyncApi } from "@/lib/api/usageSync";
import { cn } from "@/lib/utils";
import { settingsCardClass, settingsGroupTitleClass } from "./settingsStyles";

const STATUS_KEY = ["usage-sync-status"] as const;

function formatSyncTime(ts: number | null): string {
  if (ts == null) return "";
  return new Date(ts * 1000).toLocaleString();
}

export function UsageSyncSection() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data, isLoading } = useQuery({
    queryKey: STATUS_KEY,
    queryFn: usageSyncApi.status,
  });

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: STATUS_KEY });
    void queryClient.invalidateQueries();
  };

  const save = useMutation({
    mutationFn: (next: { enabled: boolean; autoSync: boolean }) =>
      usageSyncApi.saveSettings(next.enabled, next.autoSync),
    onSuccess: invalidate,
    onError: (error) =>
      toast.error(t("usage.multiDevice.saveFailed", { error: String(error) })),
  });

  const sync = useMutation({
    mutationFn: usageSyncApi.syncNow,
    onSuccess: (status) => {
      const details = status.peers.reduce(
        (sum, peer) => sum + peer.importedDetails,
        0,
      );
      const rollups = status.peers.reduce(
        (sum, peer) => sum + peer.importedRollups,
        0,
      );
      toast.success(t("usage.multiDevice.syncSuccess", { details, rollups }));
      invalidate();
    },
    onError: (error) =>
      toast.error(t("usage.multiDevice.syncFailed", { error: String(error) })),
  });

  const forget = useMutation({
    mutationFn: usageSyncApi.forgetDevice,
    onSuccess: invalidate,
    onError: (error) =>
      toast.error(
        t("usage.multiDevice.removeFailed", { error: String(error) }),
      ),
  });

  const enabled = data?.enabled ?? false;
  const autoSync = data?.autoSync ?? true;

  return (
    <section className="flex flex-col gap-2">
      <h3 className={settingsGroupTitleClass}>
        {t("usage.multiDevice.title")}
      </h3>
      <div className={cn(settingsCardClass, "gap-3")}>
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              <h4 className="m-0 text-strong font-semibold text-fg-1">
                {t("usage.multiDevice.enable")}
              </h4>
            </div>
            <p className="m-0 mt-1 text-caption text-fg-2">
              {t("usage.multiDevice.description")}
            </p>
            {data && (
              <p className="m-0 mt-1 text-caption text-fg-3">
                {t("usage.multiDevice.thisDevice", { name: data.deviceName })}
              </p>
            )}
          </div>
          <Switch
            checked={enabled}
            disabled={isLoading || save.isPending}
            onCheckedChange={(next) => save.mutate({ enabled: next, autoSync })}
            aria-label={t("usage.multiDevice.enable")}
          />
        </div>

        {enabled && (
          <>
            <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
              <div>
                <div className="text-body font-medium text-fg-1">
                  {t("usage.multiDevice.auto")}
                </div>
                <div className="text-caption text-fg-3">
                  {t("usage.multiDevice.autoHint")}
                </div>
              </div>
              <Switch
                checked={autoSync}
                disabled={save.isPending}
                onCheckedChange={(next) =>
                  save.mutate({ enabled, autoSync: next })
                }
                aria-label={t("usage.multiDevice.auto")}
              />
            </div>

            <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
              <p className="m-0 text-caption text-fg-2">
                {t("usage.multiDevice.cloudHint")}
              </p>
              <Button
                type="button"
                variant="neutral"
                size="compact"
                disabled={sync.isPending}
                onClick={() => sync.mutate()}
              >
                {sync.isPending ? (
                  <Loader2
                    aria-hidden="true"
                    className="h-3.5 w-3.5 motion-safe:animate-spin"
                  />
                ) : (
                  <RefreshCw aria-hidden="true" className="h-3.5 w-3.5" />
                )}
                {t("usage.multiDevice.syncNow")}
              </Button>
            </div>

            <div className="border-t border-border pt-3">
              <div className="mb-2 text-caption font-medium text-fg-2">
                {t("usage.multiDevice.peers")}
              </div>
              {data && data.peers.length > 0 ? (
                <ul className="m-0 flex list-none flex-col gap-2 p-0">
                  {data.peers.map((peer) => (
                    <li
                      key={peer.deviceId}
                      className="flex items-start justify-between gap-2 rounded-[5px] bg-subtle px-2.5 py-2"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-body text-fg-1">
                          {peer.name || peer.deviceId}
                        </div>
                        <div className="text-caption text-fg-3">
                          {peer.lastSyncAt
                            ? t("usage.multiDevice.lastSync", {
                                time: formatSyncTime(peer.lastSyncAt),
                              })
                            : t("usage.multiDevice.never")}
                          {" · "}
                          {t("usage.multiDevice.counts", {
                            details: peer.importedDetails,
                            rollups: peer.importedRollups,
                          })}
                        </div>
                        {peer.error && (
                          <div className="mt-1 break-words text-caption text-danger-text">
                            {peer.error}
                          </div>
                        )}
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="compact"
                        disabled={forget.isPending}
                        aria-label={t("usage.multiDevice.remove")}
                        onClick={() => {
                          if (
                            window.confirm(
                              t("usage.multiDevice.removeConfirm", {
                                name: peer.name || peer.deviceId,
                              }),
                            )
                          ) {
                            forget.mutate(peer.deviceId);
                          }
                        }}
                      >
                        <Trash2 aria-hidden="true" className="h-3.5 w-3.5" />
                      </Button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="m-0 text-caption text-fg-3">
                  {t("usage.multiDevice.noPeers")}
                </p>
              )}
            </div>
          </>
        )}
      </div>
    </section>
  );
}
