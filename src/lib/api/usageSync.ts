import { invoke } from "@tauri-apps/api/core";

export interface UsageSyncPeerStatus {
  deviceId: string;
  name: string;
  lastSyncAt: number | null;
  importedDetails: number;
  importedRollups: number;
  error: string | null;
}

export interface UsageSyncStatus {
  enabled: boolean;
  autoSync: boolean;
  deviceId: string;
  deviceName: string;
  peers: UsageSyncPeerStatus[];
}

export const usageSyncApi = {
  status: () => invoke<UsageSyncStatus>("usage_sync_status"),
  saveSettings: (enabled: boolean, autoSync: boolean) =>
    invoke<UsageSyncStatus>("usage_sync_save_settings", { enabled, autoSync }),
  syncNow: () => invoke<UsageSyncStatus>("usage_sync_now"),
  forgetDevice: (deviceId: string) =>
    invoke<void>("usage_sync_forget_device", { deviceId }),
};
