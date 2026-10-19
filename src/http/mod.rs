//! HTTP 层：路由、提取器、错误映射、静态资源。

pub mod error;
pub mod ext;
pub mod routes;
pub mod static_assets;

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::state::AppState;

/// 组装路由表。M0 只含面板与公开页；播放面在 M2 接入。
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // 公开面
        .route("/health", get(routes::public::health))
        .route("/", get(routes::public::home))
        // 内嵌页面（免登录）
        .route("/panel", get(static_assets::panel_page))
        .route("/epg", get(static_assets::epg_page))
        .route("/logo", get(static_assets::logo_page))
        .route("/unban", get(static_assets::unban_page))
        .route("/__assets/*path", get(static_assets::asset))
        // 面板接口（需要会话）
        .route("/panel/api/login", post(routes::panel::login))
        .route("/panel/api/logout", post(routes::panel::logout))
        .route("/panel/api/status", get(routes::panel::status))
        .route(
            "/panel/api/config",
            get(routes::panel::get_config).put(routes::panel::put_config),
        )
        .route("/panel/api/reload", post(routes::panel::reload))
        .fallback(routes::public::fallback)
        .with_state(state)
}
