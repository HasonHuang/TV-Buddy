//! 配置重载。
//!
//! 配置存在数据库里，没有文件可监视。改库的路径有两条：
//! 面板保存（直接替换快照）与管理 CLI（之后调重载接口）。
//! 两条都落到这里。

use crate::state::AppState;

/// 从数据库重读配置并替换快照。失败保留旧快照。
pub fn reload(state: &AppState) {
    match state.config_store.load() {
        Ok(Some(mut config)) => {
            if crate::sign::needs_secret(&config) {
                crate::sign::ensure_secret(&mut config);
                if let Err(error) = state.config_store.save(&config) {
                    tracing::warn!(%error, "补写签名密钥失败");
                }
            }
            state.replace(config);
            tracing::info!("配置已重载");
        }
        Ok(None) => tracing::error!("数据库里没有配置，保留旧配置"),
        Err(err) => tracing::error!(error = %err, "配置解析失败，保留旧配置"),
    }
}
