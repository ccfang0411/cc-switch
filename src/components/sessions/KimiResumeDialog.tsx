import { useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  sessionsApi,
  type KimiResumeAction,
  type KimiResumeKind,
  type KimiResumeTarget,
} from "@/lib/api/sessions";

const errorKeys = new Set([
  "invalidSession",
  "invalidConfig",
  "proxyNotReady",
  "modelChanged",
  "missingProject",
  "terminalFailed",
  "clipboardFailed",
]);

function errorKey(error: unknown) {
  return typeof error === "string" && errorKeys.has(error) ? error : "internal";
}

export function KimiResumeDialog({
  target,
  kind,
  terminal,
  onClose,
}: {
  target: KimiResumeTarget;
  kind: KimiResumeKind;
  terminal: string | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const modelId = useId();
  const [chosenAlias, setChosenAlias] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const options = useQuery({
    queryKey: [
      "kimi-session-resume-options",
      target.sessionId,
      target.sourcePath,
    ],
    queryFn: () => sessionsApi.getKimiResumeOptions(target),
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    refetchOnMount: "always",
    gcTime: 0,
    retry: false,
  });
  const models = options.data?.models ?? [];
  const selected = models.find(
    (model) =>
      model.alias ===
      (chosenAlias ?? models.find((item) => item.isDefault)?.alias),
  );
  const error = failure ?? (options.isError ? errorKey(options.error) : null);
  const disabled =
    options.isFetching ||
    options.isError ||
    pending ||
    !selected ||
    error === "modelChanged";
  const run = async (action: KimiResumeAction) => {
    if (disabled || !selected) return;
    setPending(true);
    setFailure(null);
    try {
      const outcome = await sessionsApi.continueKimiSession(
        target,
        { alias: selected.alias, revision: selected.revision },
        kind,
        action,
      );
      if (outcome.kind === "command") {
        try {
          await navigator.clipboard.writeText(outcome.command);
        } catch {
          setFailure("clipboardFailed");
          return;
        }
        toast.success(t("sessionManager.kimiResume.copied"));
      } else {
        toast.success(
          t(
            kind === "original"
              ? "sessionManager.kimiResume.terminalOpened"
              : "sessionManager.kimiResume.newTerminalOpened",
          ),
        );
      }
      onClose();
    } catch (cause) {
      setFailure(errorKey(cause));
    } finally {
      setPending(false);
    }
  };
  const reload = () => {
    setChosenAlias("");
    setFailure(null);
    void options.refetch();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && !pending && onClose()}>
      <DialogContent className="gap-4 p-6">
        <div className="space-y-1.5">
          <DialogTitle className="text-title">
            {t(
              kind === "original"
                ? "sessionManager.kimiResume.title"
                : "sessionManager.kimiResume.newTitle",
            )}
          </DialogTitle>
          <DialogDescription className="text-body text-fg-2">
            {t(
              kind === "original"
                ? "sessionManager.kimiResume.description"
                : "sessionManager.kimiResume.newDescription",
            )}
          </DialogDescription>
        </div>
        {options.isFetching ? (
          <p role="status" className="text-body text-fg-2">
            {t("common.loading")}
          </p>
        ) : models.length ? (
          <div className="grid gap-2">
            <label htmlFor={modelId} className="text-body font-medium">
              {t("sessionManager.kimiResume.model", {
                mode: t(
                  `sessionManager.kimiResume.mode.${options.data?.mode ?? "direct"}`,
                ),
              })}
            </label>
            <select
              id={modelId}
              value={selected?.alias ?? ""}
              onChange={(event) => setChosenAlias(event.target.value)}
              disabled={pending || error === "modelChanged"}
              className="w-full min-w-0 rounded-md border border-border bg-surface px-3 py-2 text-body text-fg-1"
            >
              <option value="" disabled>
                {t("sessionManager.kimiResume.selectModel")}
              </option>
              {models.map((model) => (
                <option key={model.alias} value={model.alias}>
                  {model.isDefault
                    ? `${t("sessionManager.kimiResume.defaultModel")} · `
                    : ""}
                  {model.provider} · {model.displayName || model.model} (
                  {model.alias})
                </option>
              ))}
            </select>
          </div>
        ) : (
          !error && (
            <p className="text-body text-fg-2">
              {t("sessionManager.kimiResume.noModels")}
            </p>
          )
        )}
        {error && (
          <div className="flex items-start gap-3">
            <p role="alert" className="flex-1 text-body text-danger-text">
              {t(`sessionManager.kimiResume.errors.${error}`)}
            </p>
            {(options.isError || error === "modelChanged") && (
              <Button
                variant="neutral"
                size="regular"
                onClick={reload}
                disabled={pending || options.isFetching}
              >
                {t("sessionManager.kimiResume.reload")}
              </Button>
            )}
          </div>
        )}
        <div className="flex flex-wrap justify-end gap-2">
          <Button
            variant="neutral"
            size="regular"
            onClick={onClose}
            disabled={pending}
          >
            {t("common.cancel", { defaultValue: "取消" })}
          </Button>
          <Button
            variant={terminal ? "neutral" : "solid"}
            size="regular"
            disabled={disabled}
            onClick={() => void run("copy")}
          >
            {t("sessionManager.kimiResume.copyCommand")}
          </Button>
          {terminal && (
            <Button
              variant="solid"
              size="regular"
              disabled={disabled}
              onClick={() => void run("open")}
            >
              {t("sessionManager.kimiResume.open", { terminal })}
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

export function KimiContinuationDialog({
  context,
  onCopy,
  onClose,
  onNewSession,
}: {
  context: string;
  onCopy: (text: string, message: string) => void;
  onClose: () => void;
  onNewSession?: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="gap-4 p-6">
        <div className="space-y-1.5">
          <DialogTitle className="text-title">
            {t("sessionManager.kimiResume.contextTitle")}
          </DialogTitle>
          <DialogDescription className="text-body text-fg-2">
            {t("sessionManager.kimiResume.contextDescription")}
          </DialogDescription>
        </div>
        <textarea
          aria-label={t("sessionManager.kimiResume.contextTitle")}
          className="min-h-24 w-full rounded-md border border-border bg-subtle p-3 text-body text-fg-1"
          rows={9}
          readOnly
          value={context}
        />
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="neutral" size="regular" onClick={onClose}>
            {t("common.close")}
          </Button>
          {onNewSession && (
            <Button variant="neutral" size="regular" onClick={onNewSession}>
              {t("sessionManager.kimiResume.contextNewSession")}
            </Button>
          )}
          <Button
            variant="solid"
            size="regular"
            onClick={() =>
              onCopy(context, t("sessionManager.kimiResume.contextCopied"))
            }
          >
            {t("sessionManager.kimiResume.contextCopy")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
