import React, { createContext, useContext } from "react";
import type { UpdateInfo } from "../lib/updater";

interface UpdateContextValue {
  // 更新状态
  hasUpdate: boolean;
  updateInfo: UpdateInfo | null;
  isChecking: boolean;
  error: string | null;

  // 提示状态
  isDismissed: boolean;
  dismissUpdate: () => void;

  // 操作方法
  checkUpdate: () => Promise<boolean>;
  resetDismiss: () => void;
}

const UpdateContext = createContext<UpdateContextValue | undefined>(undefined);

// This independent branch must never replace itself with an upstream release.
const disabledUpdates: UpdateContextValue = {
  hasUpdate: false,
  updateInfo: null,
  isChecking: false,
  error: null,
  isDismissed: true,
  dismissUpdate: () => {},
  checkUpdate: async () => false,
  resetDismiss: () => {},
};

export function UpdateProvider({ children }: { children: React.ReactNode }) {
  return (
    <UpdateContext.Provider value={disabledUpdates}>
      {children}
    </UpdateContext.Provider>
  );
}

export function useUpdate() {
  const context = useContext(UpdateContext);
  if (!context) throw new Error("useUpdate must be used within UpdateProvider");
  return context;
}
