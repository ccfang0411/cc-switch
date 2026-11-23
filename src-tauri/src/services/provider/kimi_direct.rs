//! 写 Kimi Code 的 `config.toml` 里模式机负责的部分：进入代理写保留的代理表，
//! 退出代理删代理表、把 `default_model` 指回直连供应商。直连切换也走这里：供应商的
//! 表和直连指针在同一个操作里提交。代理模式下供应商的编辑和成员变更由
//! `services::provider` 协调，不经过这里。

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::live::engine::LiveFile;
use crate::live::project::kimi::KimiProjection;
use crate::mode::operation::{AppWrite, FileChange, OperationReport};
use crate::mode::state::PendingTarget;

fn app() -> &'static str {
    AppType::Kimi.as_str()
}

/// `config.toml` 里有 Key，按 0600 写。
pub(crate) fn config_file() -> LiveFile {
    LiveFile::private(crate::kimi_config::config_path())
}

pub(crate) fn switch_to(
    db: &Database,
    provider: &crate::provider::Provider,
) -> Result<(), AppError> {
    crate::kimi_config::with_write_lock(|| {
        let patch = crate::kimi_config::SelectProvider {
            id: &provider.id,
            config: &provider.settings_config,
        };
        let write = AppWrite::begin(db, app())?;
        write.run(
            "switch-kimi-direct",
            &[FileChange {
                file: config_file(),
                patch: &patch,
            }],
            PendingTarget::pointer(Some(provider.id.clone())),
        )?;
        Ok(())
    })
}

/// 把 `projection` 写进 `config.toml`，和 `target` 在同一个操作里提交；`projection`
/// 为空时只落定状态、不读也不写文件。
pub(crate) fn run(
    db: &Database,
    op: &str,
    projection: Option<&KimiProjection>,
    target: PendingTarget,
) -> Result<OperationReport, AppError> {
    // 先补完上一次没做完的操作：它可能改了 `default_model` 和模式状态。
    crate::kimi_config::with_write_lock(|| {
        let app_write = AppWrite::begin(db, app())?;
        let changes: Vec<FileChange<'_>> = projection
            .map(|projection| FileChange {
                file: config_file(),
                patch: projection,
            })
            .into_iter()
            .collect();
        app_write.run(op, &changes, target)
    })
}
