//! HTTP 层：路由、提取器、错误映射、静态资源。

pub mod error;
pub mod ext;
pub mod guard;
mod legacy;
pub mod routes;
pub mod static_assets;

use std::sync::Arc;

use axum::{
    routing::{any, delete, get, post, put},
    Router,
};

use crate::{paths, state::AppState};

/// 组装路由表。
///
/// M0：面板与公开页。M1：分组/频道接口与订阅导出。
/// 播放面（`/api/play` 清单、`/media` 切片）在 M2 接入。
/// 旧路径改写层在内层路由之外。它先运行，然后内层做路由匹配。
pub fn router(state: Arc<AppState>) -> Router {
    let inner = Router::new()
        // 公开面
        .route("/api/health", get(routes::public::health))
        .route("/", get(routes::public::home))
        .route("/api/unban/whoami", get(routes::public::whoami))
        .route("/api/unban", post(routes::public::unban))
        .route("/api/share/pull", get(routes::share::pull))
        // 全局订阅已停用。保留路由是为了给出明确原因，而不是落进兜底。
        .route("/playlist.m3u", get(routes::sub::gone))
        .route("/:gid/playlist.m3u", get(routes::sub::gone))
        // 用户订阅入口。令牌见 `tv-buddy.db` 的 access_tokens 表。
        .route("/api/sub/:token/playlist.m3u", get(routes::sub::playlist))
        // 单个分组的订阅。分组范围仍受令牌所属用户的可见性限制。
        .route(
            "/api/sub/:token/:gid/playlist.m3u",
            get(routes::sub::group_playlist),
        )
        .route("/api/sub/:token/epg.xml", get(routes::sub::epg))
        .route("/api/sub/:token/epg.xml.gz", get(routes::sub::epg_gz))
        // 面板接口挂载在新前缀下。旧 `/panel/api` 由改写层提供。
        .nest(paths::PANEL_API, panel_api_routes())
        // 播放链路（M2）。清单只走 `/api/play`，媒体数据只走 `/media`。
        .route("/api/play/:gid/:file", get(routes::stream::entry))
        .route(
            "/api/play/:gid/:cid/*path",
            get(routes::stream::play_wildcard),
        )
        .route(
            "/api/play/:gid/:cid/l/:exp/:sig/*path",
            get(routes::stream::signed_play),
        )
        .route(
            "/api/play/token/:token/:gid/:file",
            get(routes::stream::token_entry),
        )
        .route(
            "/api/play/token/:token/:gid/:cid/*path",
            get(routes::stream::token_alias),
        )
        .route(
            "/media/:gid/:cid/*path",
            get(routes::stream::media_wildcard),
        )
        .route(
            "/media/:gid/:cid/l/:exp/:sig/*path",
            get(routes::stream::signed_media),
        )
        // 内嵌页面（免登录）
        .route("/page/panel", get(static_assets::panel_page))
        .route("/page/panel/", get(static_assets::panel_page))
        .route("/page/unban", get(static_assets::unban_page))
        .route("/static/panel/*path", get(static_assets::asset))
        .route("/static/logo/:file", get(routes::logo::file))
        .route("/static/assets/*path", get(routes::media::served))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            routes::admin::count_request,
        ))
        .fallback(routes::public::fallback)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::ip_guard,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::feature_gate,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::admin_gate,
        ))
        .with_state(state);
    legacy::wrap(inner)
}

/// 面板 API 路由。挂载在 `/api/panel`。旧 `/panel/api` 由改写层提供。
///
/// 路径都是相对子路径。未知路径返回 JSON 404。兜底同时登记为通配路由与
/// 嵌套 fallback：外层兜底可能先于嵌套 fallback 生效，通配路由保证 JSON 404。
fn panel_api_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", post(routes::panel::login))
        // 用户与设备（对齐 mpd-hls 的 /users）
        .route(
            "/users",
            get(routes::users::list).post(routes::users::create),
        )
        .route(
            "/users/channel-options",
            get(routes::users::channel_options),
        )
        .route(
            "/users/:id",
            put(routes::users::update).delete(routes::users::remove),
        )
        .route("/users/:id/password", put(routes::users::set_password))
        .route(
            "/users/:id/tokens",
            get(routes::users::list_tokens).post(routes::users::create_token),
        )
        .route(
            "/users/:id/tokens/:token",
            put(routes::users::update_token).delete(routes::users::revoke_token),
        )
        .route("/me", get(routes::users::me))
        .route("/logout", post(routes::panel::logout))
        .route("/status", get(routes::panel::status))
        .route("/health", get(routes::dash::health))
        .route("/quality/status", get(routes::quality::status))
        .route("/quality/results", get(routes::quality::results))
        .route("/quality/sources", get(routes::quality::sources))
        .route("/quality/effective", get(routes::quality::effective))
        .route(
            "/quality/schedule/preview",
            post(routes::quality::schedule_preview),
        )
        .route("/quality/run", post(routes::quality::run))
        .route("/quality/runs/:run_id", get(routes::quality::run_status))
        .route("/quality/cancel", post(routes::quality::cancel))
        .route("/quality/preview", post(routes::quality::preview))
        .route("/subs/status", get(routes::subs::panel_status))
        .route("/subs/groups", post(routes::subs::panel_groups))
        .route("/subs/preview", post(routes::subs::panel_preview))
        .route("/subs/sync", post(routes::subs::panel_sync))
        .route("/subs/:id/content", delete(routes::subs::panel_content))
        .route(
            "/subs/:id/backfill-mode",
            post(routes::subs::panel_backfill_mode),
        )
        .route(
            "/schedule",
            get(routes::schedule::list).post(routes::schedule::create),
        )
        .route("/schedule/from-epg", post(routes::schedule::from_epg))
        .route(
            "/schedule/download/*filename",
            get(routes::schedule::download),
        )
        .route(
            "/schedule/:idx",
            put(routes::schedule::update).delete(routes::schedule::remove),
        )
        .route("/schedule/:idx/stop", post(routes::schedule::stop))
        .route("/livestream", get(routes::schedule::livestream_list))
        .route(
            "/livestream/:gid/:cid/:action",
            post(routes::schedule::livestream_switch),
        )
        .route(
            "/playlist",
            get(routes::playlist::get).put(routes::playlist::update),
        )
        .route("/playlist/settings", put(routes::playlist::update_settings))
        .route("/playlist/sources", post(routes::playlist::create_source))
        .route(
            "/playlist/sources/:id",
            put(routes::playlist::update_source).delete(routes::playlist::delete_source),
        )
        .route(
            "/playlist/sources/:id/disable",
            post(routes::playlist::disable_source),
        )
        .route(
            "/playlist/sources/:id/enable",
            post(routes::playlist::enable_source),
        )
        .route("/dash/status", get(routes::dash::status))
        .route("/dash/tracks/:gid/:cid", get(routes::dash::tracks))
        .route("/dash/scan-url", post(routes::dash::scan_url))
        .route(
            "/dash-timeshift/status",
            get(routes::dash::timeshift_status),
        )
        .route(
            "/config",
            get(routes::panel::get_config).put(routes::panel::put_config),
        )
        .route("/reload", post(routes::panel::reload))
        .route("/cache/stats", get(routes::admin::cache_stats))
        .route("/cache/flush", post(routes::admin::cache_flush))
        .route("/cdn/estimate", post(routes::cdn::estimate))
        .route("/stats", get(routes::admin::stats))
        .route("/metrics/history", get(routes::admin::metrics_history))
        .route("/logs", get(routes::admin::logs))
        .route("/link-info", get(routes::security::link_info))
        .route("/security/ip/stats", get(routes::security::ip_stats))
        .route("/security/ip/ban", post(routes::security::ip_ban))
        .route("/security/ip/unban", post(routes::security::ip_unban))
        .route("/security/ip/unban-batch", post(routes::security::ip_unban))
        .route("/backup", get(routes::backup::backup))
        .route(
            "/restore",
            post(routes::backup::restore)
                .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route("/assets/ffmpeg-status", get(routes::media::ffmpeg_status))
        .route("/tools/ffmpeg/install", post(routes::media::ffmpeg_install))
        .route("/tools/ffmpeg/remove", post(routes::media::ffmpeg_remove))
        .route("/assets/list", get(routes::media::list))
        .route("/assets/convert", post(routes::media::convert))
        .route("/assets/candidates", get(routes::media::candidates))
        .route("/assets/upload", post(routes::media::upload))
        .route("/assets/remove", post(routes::media::remove))
        .route("/assets/raw/:file", get(routes::media::raw))
        .route("/subtitle/list", get(routes::media::list))
        .route("/subtitle/upload", post(routes::media::upload))
        .route("/subtitle/remove", post(routes::media::remove))
        .route("/subtitle/raw/:file", get(routes::media::raw))
        .route(
            "/subtitle/translate/status",
            get(routes::media::translate_status),
        )
        .route(
            "/subtitle/translate",
            post(routes::media::translate_subtitle),
        )
        .route(
            "/subtitle/translate/raw/:name",
            get(routes::media::translate_raw),
        )
        .route("/subtitle/mtran/test", post(routes::media::mtran_test))
        .route(
            "/subtitle/translate/clear",
            post(routes::media::translate_clear),
        )
        .route("/resolver/scripts", get(routes::media::resolver_list))
        .route("/resolver/script", post(routes::media::resolver_save))
        .route("/resolver/remove", post(routes::media::resolver_remove))
        .route("/logo/list", get(routes::logo::panel_list))
        .route("/logo/upload", post(routes::logo::upload))
        .route("/logo/delete", post(routes::logo::delete))
        .route("/logo/rename", post(routes::logo::rename))
        .route("/logo/restore", post(routes::logo::restore))
        .route("/logo/animate", post(routes::logo::animate))
        .route("/logo/match-report", get(routes::logo::match_report))
        .route("/logo/apply", post(routes::logo::apply))
        .route("/logo/suggest", post(routes::logo::suggest))
        // EPG 翻译端点也要有面板路径。
        .route("/epg/translate/status", get(routes::epg::translate_status))
        .route("/epg/translate/models", get(routes::epg::translate_models))
        .route("/epg/translate/run", post(routes::epg::translate_run))
        .route("/epg/translate/probe", post(routes::epg::translate_probe))
        .route(
            "/epg/translate/mtran/test",
            post(routes::epg::translate_mtran),
        )
        .route(
            "/epg/translate/clear",
            post(routes::epg::translate_clear_results),
        )
        .route(
            "/epg/translate/clear-engine",
            post(routes::epg::translate_clear),
        )
        .route("/epg/status", get(routes::epg::status))
        .route("/epg/refresh", post(routes::epg::refresh))
        .route("/epg/regenerate", post(routes::epg::regenerate))
        .route("/epg/mappings", get(routes::epg::mappings))
        .route("/epg/mapping", post(routes::epg::mapping))
        .route(
            "/epg/sources",
            get(routes::epg::sources).post(routes::epg::source_save),
        )
        .route("/epg/sources/:id", delete(routes::epg::source_delete))
        .route(
            "/epg/sources/:id/refresh",
            post(routes::epg::source_refresh),
        )
        .route("/epg/bindings", get(routes::epg::bindings))
        .route(
            "/epg/bindings/:gid/:cid",
            put(routes::epg::binding_set).delete(routes::epg::binding_delete),
        )
        .route(
            "/epg/bindings/:gid/:cid/copy",
            post(routes::epg::binding_copy),
        )
        .route(
            "/epg/rules",
            get(routes::epg::rules).post(routes::epg::rule_save),
        )
        .route("/epg/rules/preview", post(routes::epg::rule_preview))
        .route("/epg/rules/:id", delete(routes::epg::rule_delete))
        .route("/epg/extra", post(routes::epg::extra))
        .route("/epg/scripts", get(routes::epg::scripts_list))
        .route("/epg/script", post(routes::epg::script_save))
        .route("/epg/script-delete", post(routes::epg::script_delete))
        .route("/epg/cron-preview", post(routes::epg::cron_preview))
        .route("/epg/programmes/:channel", get(routes::epg::programmes))
        .route("/epg/suggest/:gid/:cid", get(routes::epg::suggest))
        .route("/epg/search", get(routes::epg::search))
        .route("/epg/viewer-channels", get(routes::epg::viewer_channels))
        .route(
            "/epg/source-channels/:id",
            get(routes::epg::source_channels),
        )
        .route("/epg/tz-preview/:id", get(routes::epg::tz_preview))
        .route("/epg/match-report", get(routes::epg::match_report))
        .route("/groups", post(routes::groups::create_group))
        .route("/groups/export", get(routes::groups::export_all))
        .route("/groups/import", post(routes::groups::import_groups))
        .route(
            "/groups/import/force",
            post(routes::groups::import_groups_force),
        )
        .route("/groups/import/m3u", post(routes::groups::import_m3u))
        .route(
            "/groups/:gid",
            put(routes::groups::update_group).delete(routes::groups::delete_group),
        )
        .route("/groups/:gid/export", get(routes::groups::export_group))
        .route(
            "/groups/:gid/channels",
            post(routes::groups::create_channel),
        )
        .route(
            "/groups/:gid/channels/:cid",
            put(routes::groups::update_channel).delete(routes::groups::delete_channel),
        )
        .route("/groups/:gid/detach", post(routes::groups::detach_group))
        .route(
            "/groups/:gid/channels/:cid/detach",
            post(routes::groups::detach_channel),
        )
        .route("/*rest", any(routes::panel::api_missing))
        .fallback(routes::panel::api_missing)
}
