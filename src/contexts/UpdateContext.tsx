import React, {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  useRef,
} from "react";
import type { ReadyUpdate, UpdateInfo } from "../lib/updater";
import { checkForUpdate } from "../lib/updater";
import { extractErrorMessage } from "../utils/errorUtils";

interface UpdateContextValue {
  hasUpdate: boolean;
  updateInfo: UpdateInfo | null;
  isChecking: boolean;
  error: string | null;
  isDismissed: boolean;
  dismissUpdate: () => void;
  checkUpdate: () => Promise<boolean>;
  resetDismiss: () => void;
  readyUpdate: ReadyUpdate | null;
  autoUpdateSupported: boolean;
  routingInUse: boolean;
  autoUpdateDownloading: boolean;
  refreshAutoUpdate: () => Promise<void>;
  startAutoDownload: () => Promise<void>;
}

const UpdateContext = createContext<UpdateContextValue | undefined>(undefined);
const DISMISSED_VERSION_KEY = "ccswitch:update:dismissedVersion";
const LEGACY_DISMISSED_KEY = "dismissedUpdateVersion";
const automaticUpdatesDisabled = async () => {};

function dismissedVersion(): string | null {
  const current = localStorage.getItem(DISMISSED_VERSION_KEY);
  if (current) return current;
  const legacy = localStorage.getItem(LEGACY_DISMISSED_KEY);
  if (legacy) {
    localStorage.setItem(DISMISSED_VERSION_KEY, legacy);
    localStorage.removeItem(LEGACY_DISMISSED_KEY);
  }
  return legacy;
}

export function UpdateProvider({ children }: { children: React.ReactNode }) {
  const [hasUpdate, setHasUpdate] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [isChecking, setIsChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isDismissed, setIsDismissed] = useState(false);
  const isCheckingRef = useRef(false);

  useEffect(() => {
    const version = updateInfo?.availableVersion;
    if (version) setIsDismissed(dismissedVersion() === version);
  }, [updateInfo?.availableVersion]);

  const checkUpdate = useCallback(async () => {
    if (isCheckingRef.current) return false;
    isCheckingRef.current = true;
    setIsChecking(true);
    setError(null);

    try {
      const result = await checkForUpdate({ timeout: 30000 });
      if (result.status === "available") {
        setHasUpdate(true);
        setUpdateInfo(result.info);
        setIsDismissed(dismissedVersion() === result.info.availableVersion);
        return true;
      }
      setHasUpdate(false);
      setUpdateInfo(null);
      setIsDismissed(false);
      return false;
    } catch (err) {
      setError(extractErrorMessage(err) || "检查更新失败");
      setHasUpdate(false);
      setUpdateInfo(null);
      throw err;
    } finally {
      setIsChecking(false);
      isCheckingRef.current = false;
    }
  }, []);

  const dismissUpdate = useCallback(() => {
    setIsDismissed(true);
    if (updateInfo?.availableVersion) {
      localStorage.setItem(DISMISSED_VERSION_KEY, updateInfo.availableVersion);
      localStorage.removeItem(LEGACY_DISMISSED_KEY);
    }
  }, [updateInfo?.availableVersion]);

  const resetDismiss = useCallback(() => {
    setIsDismissed(false);
    localStorage.removeItem(DISMISSED_VERSION_KEY);
    localStorage.removeItem(LEGACY_DISMISSED_KEY);
  }, []);

  const value: UpdateContextValue = {
    hasUpdate,
    updateInfo,
    isChecking,
    error,
    isDismissed,
    dismissUpdate,
    checkUpdate,
    resetDismiss,
    readyUpdate: null,
    autoUpdateSupported: false,
    routingInUse: false,
    autoUpdateDownloading: false,
    refreshAutoUpdate: automaticUpdatesDisabled,
    startAutoDownload: automaticUpdatesDisabled,
  };

  return (
    <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>
  );
}

export function useUpdate() {
  const context = useContext(UpdateContext);
  if (!context) {
    throw new Error("useUpdate must be used within UpdateProvider");
  }
  return context;
}
