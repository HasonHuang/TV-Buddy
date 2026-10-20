//! HTTP 层：路由、提取器、错误映射、静态资源。

pub mod error;
pub mod ext;
pub mod routes;
pub mod static_assets;

use std::sync::Arc;

use axum::{
    routing::{any, get, post, put},
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
        .route("/epg.xml", get(routes::epg::xml))
        .route("/epg.xml.gz", get(routes::epg::xml_gz))
        .route("/epg/api/channels", get(routes::epg::api_channels))
        .route(
            "/epg/api/programmes/:channel",
            get(routes::epg::api_programmes),
        )
        .route("/epg/api/search", get(routes::epg::api_search))
        .route("/logo/api/list", get(routes::logo::public_list))
        .route("/logo/:file", get(routes::logo::file))
        .route("/whoami", get(routes::public::whoami))
        .route("/unban/whoami", get(routes::public::whoami))
        .route("/playlist.m3u", get(routes::play::total_playlist))
        .route("/:gid/playlist.m3u", get(routes::play::group_playlist))
        // 面板接口：未知路径固定 404 JSON，先于播放通配路由匹配。
        .route("/panel/api/*rest", any(routes::panel::api_missing))
        // 播放链路（M2）
        .route("/:gid/:file", get(routes::stream::entry))
        .route("/:gid/:cid/*path", get(routes::stream::wildcard))
        // 内嵌页面（免登录）
        .route("/panel", get(static_assets::panel_page))
        .route("/panel/", get(static_assets::panel_page))
        .route("/panel/epg", get(routes::epg::viewer))
        .route("/epg", get(routes::epg::viewer))
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
        .route("/panel/api/cache/stats", get(routes::admin::cache_stats))
        .route("/panel/api/cache/flush", post(routes::admin::cache_flush))
        .route("/panel/api/stats", get(routes::admin::stats))
        .route("/panel/api/logs", get(routes::admin::logs))
        .route("/panel/api/logo/list", get(routes::logo::panel_list))
        .route("/panel/api/logo/upload", post(routes::logo::upload))
        .route("/panel/api/logo/delete", post(routes::logo::delete))
        .route("/panel/api/epg/status", get(routes::epg::status))
        .route("/panel/api/epg/refresh", post(routes::epg::refresh))
        .route("/panel/api/groups", post(routes::groups::create_group))
        .route("/panel/api/groups/export", get(routes::groups::export_all))
        .route(
            "/panel/api/groups/import",
            post(routes::groups::import_groups),
        )
        .route(
            "/panel/api/groups/import/force",
            post(routes::groups::import_groups_force),
        )
        .route(
            "/panel/api/groups/import/m3u",
            post(routes::groups::import_m3u),
        )
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
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            routes::admin::count_request,
        ))
        .fallback(routes::public::fallback)
        .with_state(state)
}
