//! 配置重载。
//!
//! 配置存在数据库里，没有文件可监视。改库的路径有两条：
//! 面板保存（直接替换快照）与管理 CLI（之后调重载接口）。
//! 两条都落到这里。

use crate::state::AppState;

/// 从数据库重读配置并替换快照。失败保留旧快照。
///
/// 返回 `Ok(true)` 表示已替换；`Ok(false)` 表示库里没有配置；
/// `Err` 表示读取、补密或写回失败。调用方据此返回真实结果。
pub fn reload(state: &AppState) -> anyhow::Result<bool> {
    match state.reload_from_db() {
        Ok(true) => {
            tracing::info!("配置已重载");
            Ok(true)
        }
        Ok(false) => {
            tracing::error!("数据库里没有配置，保留旧配置");
            Ok(false)
        }
        Err(err) => {
            tracing::error!(error = %err, "配置重载失败，保留旧配置");
            Err(err)
        }
    }
}
