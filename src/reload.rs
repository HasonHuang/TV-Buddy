//! 配置重载。
//!
//! 配置存在数据库里，没有文件可监视。改库的路径有两条：
//! 面板保存（直接替换快照）与管理 CLI（之后调重载接口）。
//! 两条都落到这里。

use crate::state::AppState;

/// 从数据库重读配置并替换快照。失败保留旧快照。
pub fn reload(state: &AppState) {
    match state.reload_from_db() {
        Ok(true) => tracing::info!("配置已重载"),
        Ok(false) => tracing::error!("数据库里没有配置，保留旧配置"),
        Err(err) => tracing::error!(error = %err, "配置解析失败，保留旧配置"),
    }
}
