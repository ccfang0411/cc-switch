//! 应用私有更新的安装锁与已下载包缓存。
//!
//! 用户手动下载的更新包只缓存在进程内，安装失败或临时离线时可重试。
//! 私有分支不启动检查或下载循环，也不向托盘提供自动更新入口。

use crate::sync_utils::MutexExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

/// 待安装的版本变了就发给前端：载荷是 `Option<ReadyUpdate>`，`null` 表示缓存作废了
/// （版本被撤回），提示要收起。
pub const UPDATE_READY_EVENT: &str = "app-update-ready";

struct PendingUpdate {
    update: Update,
    bytes: Vec<u8>,
}

static PENDING: Mutex<Option<PendingUpdate>> = Mutex::new(None);
/// 弹窗、关于页、托盘三个入口共用：同一时间只能有一次安装（安装会替换应用、做退出清理）。
static INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReadyUpdate {
    pub version: String,
    pub notes: Option<String>,
}

/// 便携版：可执行文件旁边有 `portable.ini`。便携版没有安装器，只能去下载页手动换。
pub fn is_portable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("portable.ini").is_file()))
        .unwrap_or(false)
}

fn pending_guard() -> std::sync::MutexGuard<'static, Option<PendingUpdate>> {
    PENDING.lock_recover()
}

/// 已下载、待安装的版本；开关关着时视为没有。
pub fn ready() -> Option<ReadyUpdate> {
    None
}

pub fn cache_download(update: Update, bytes: Vec<u8>) {
    *pending_guard() = Some(PendingUpdate { update, bytes });
}

/// 缓存里这个版本的安装包（复制一份）。不从缓存取走：安装失败时还能再装，离线也能重试；
/// 安装成功进程就退出了，缓存不用清。
pub fn pending_bytes_for(version: &str) -> Option<Vec<u8>> {
    pending_guard()
        .as_ref()
        .filter(|pending| pending.update.version == version)
        .map(|pending| pending.bytes.clone())
}

/// 离线时的退路：检查不了更新，就装已经下好的那个版本（同样只复制、不取走）。
pub fn any_pending() -> Option<(Update, Vec<u8>)> {
    pending_guard()
        .as_ref()
        .map(|pending| (pending.update.clone(), pending.bytes.clone()))
}

/// 缓存作废（服务器上已经没有这个版本了）：清掉，并让前端提示和托盘入口一起收起。
pub fn discard_pending(app: &AppHandle) {
    if pending_guard().take().is_some() {
        log::info!("已下载的应用更新不再可用，已丢弃");
        let _ = app.emit(UPDATE_READY_EVENT, None::<ReadyUpdate>);
        crate::tray::refresh_tray_menu(app);
    }
}

/// 安装期间持有；安装失败返回时随 drop 释放，成功时进程直接退出 / 重启。
pub struct InstallGuard;

impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

pub fn is_installing() -> bool {
    INSTALLING.load(Ordering::SeqCst)
}

/// 拿到安装权；已经有一次安装在跑就返回 `None`。
pub fn begin_install() -> Option<InstallGuard> {
    // 不能写 `then_some(InstallGuard)`：守卫会先构造出来，拿不到安装权时随即 drop，
    // 反而把别人持有的锁清掉。
    if INSTALLING.swap(true, Ordering::SeqCst) {
        None
    } else {
        Some(InstallGuard)
    }
}

pub fn build_updater(app: &AppHandle) -> Result<Updater, String> {
    let builder = app.updater_builder();
    // Windows 上 updater 以 `msiexec /passive /promptrestart` 安装 MSI。passive 没有模态框、
    // 未指定重启选项时，安装目录里有文件被占用，Windows Installer 会不经确认直接重启系统
    // （#8534）。`/promptrestart` 只对应 REBOOTPROMPT，挡不住这一点，因此显式追加
    // REBOOT=ReallySuppress（即 `/norestart`）。旧版本的 updater 不带这个参数，
    // 由 MSI 模板自身的 REBOOT 属性兜底。
    #[cfg(target_os = "windows")]
    let builder = builder.installer_arg("REBOOT=ReallySuppress");
    builder
        .build()
        .map_err(|e| format!("初始化更新器失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_branch_does_not_offer_an_automatic_update() {
        assert!(ready().is_none());
    }

    #[test]
    fn only_one_install_runs_at_a_time_and_failure_releases_it() {
        let first = begin_install().expect("first install gets the slot");
        assert!(is_installing());
        assert!(begin_install().is_none(), "second entry point is refused");
        // 被拒的那次不能顺手把锁放掉，第三个入口照样要被拒
        assert!(is_installing());
        assert!(
            begin_install().is_none(),
            "third entry point is refused too"
        );
        drop(first); // 安装失败返回
        assert!(!is_installing());
        let again = begin_install();
        assert!(again.is_some());
    }
}
