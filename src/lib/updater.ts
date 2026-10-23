import { getVersion } from "@tauri-apps/api/app";

export type UpdateChannel = "stable" | "beta";

export interface UpdateInfo {
  currentVersion: string;
  availableVersion: string;
  notes?: string;
  pubDate?: string;
}

export interface CheckOptions {
  timeout?: number;
  channel?: UpdateChannel;
}

/** 私有版本安装包在 OSS 上的公共前缀（与 scripts/publish-update.mjs 保持一致）。 */
const PRIVATE_UPDATE_BASE =
  "https://digital-attic.oss-cn-hangzhou.aliyuncs.com/cc-switch-updates";

export async function getCurrentVersion(): Promise<string> {
  try {
    return await getVersion();
  } catch {
    return "";
  }
}

/** 手动下载地址：与更新包一起发布在 OSS 上，便携版或安装失败时使用。 */
export function privateDownloadUrl(version: string): string {
  const fileName = `CC Switch_${version}_aarch64.dmg`;
  return `${PRIVATE_UPDATE_BASE}/${encodeURIComponent(fileName)}`;
}

/**
 * 检查 OSS 上的私有版本更新。只由「关于」页手动触发，不做启动自动检查。
 */
export async function checkForUpdate(
  opts: CheckOptions = {},
): Promise<
  { status: "up-to-date" } | { status: "available"; info: UpdateInfo }
> {
  // 动态引入，避免在未安装插件时导致打包期问题
  const { check } = await import("@tauri-apps/plugin-updater");

  const currentVersion = await getCurrentVersion();
  const update = await check({ timeout: opts.timeout ?? 30000 } as any);

  if (!update) {
    return { status: "up-to-date" };
  }

  const info: UpdateInfo = {
    currentVersion,
    availableVersion: (update as any).version ?? "",
    notes: (update as any).body ?? (update as any).notes,
    pubDate: (update as any).date,
  };

  return { status: "available", info };
}
