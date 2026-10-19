//! HTTP 层：路由、提取器、错误映射、静态资源。

pub mod error;
pub mod ext;
pub mod routes;
pub mod static_assets;

use std::sync::Arc;

use axum::{
    routing::{get, post, put},
    Router,
};

use crate::state::AppState;

/// 组装路由表。
///
/// M0：面板与公开页。M1：分组/频道接口与订阅导出。
/// 播放面（`/{gid}/{cid}.{ext}`、切片）在 M2 接入。
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // 公开面
        .route("/health", get(routes::public::health))
        .route("/", get(routes::public::home))
        .route("/playlist.m3u", get(routes::play::total_playlist))
        .route("/:gid/playlist.m3u", get(routes::play::group_playlist))
        // 播放链路（M2）
        .route("/:gid/:file", get(routes::stream::entry))
        .route("/:gid/:cid/*path", get(routes::stream::wildcard))
        // 内嵌页面（免登录）
        .route("/panel", get(static_assets::panel_page))
        .route("/panel/", get(static_assets::panel_page))
        .route("/panel/epg", get(static_assets::epg_page))
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
        .route("/panel/api/groups", post(routes::groups::create_group))
        .route("/panel/api/groups/export", get(routes::groups::export_all))
        .route(
            "/panel/api/groups/:gid",
            put(routes::groups::update_group).delete(routes::groups::delete_group),
        )
        .route(
            "/panel/api/groups/:gid/export",
            get(routes::groups::export_group),
        )
        .route(
            "/panel/api/groups/:gid/channels",
            post(routes::groups::create_channel),
        )
        .route(
            "/panel/api/groups/:gid/channels/:cid",
            put(routes::groups::update_channel).delete(routes::groups::delete_channel),
        )
        .fallback(routes::public::fallback)
        .with_state(state)
}
