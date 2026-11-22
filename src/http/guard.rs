//! 请求前置检查：IP 统计与封禁。
//!
//! 面板、页面、静态面板资源与解封路径不受拦截，避免管理员或用户无法解封。
//! 封禁期内其他路径返回 `403`（形状为本实现自定）。

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{http::ext::client_ip_with_peer, state::AppState};

/// 记录请求并在封禁期内拒绝。
pub async fn ip_guard(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if !ip_guard_exempt(request.uri().path()) {
        let peer = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0);
        let ip = client_ip_with_peer(request.headers(), peer);
        state.ip_guard.record_request(&ip);
        // 白名单豁免封禁。只在 `ip_guard_enabled` 打开时生效。
        // 关闭时不改变既有封禁行为。
        let config = state.snapshot();
        let exempt =
            config.auth.ip_guard_enabled && crate::auth::whitelisted(&config.auth.whitelist, &ip);
        if !exempt && state.ip_guard.is_banned(&ip) {
            return (StatusCode::FORBIDDEN, "banned").into_response();
        }
    }
    next.run(request).await
}

/// 该路径是否跳过 IP 统计与封禁。
///
/// 面板与解封路径必须可用。`/api/health` 不在豁免内。
fn ip_guard_exempt(path: &str) -> bool {
    path.starts_with("/page")
        || path.starts_with("/static/panel")
        || path.starts_with("/api/panel")
        || path.starts_with("/api/unban")
        || path.starts_with("/panel")
        || path.starts_with("/unban")
}

/// 去掉面板 API 前缀，返回相对子路径。
///
/// 同时识别旧前缀 `/panel/api` 与新前缀 `/api/panel`。
fn panel_relative(path: &str) -> Option<&str> {
    path.strip_prefix("/panel/api")
        .or_else(|| path.strip_prefix("/api/panel"))
}

/// 管理员校验：未登录返回 `401`，非管理员返回 `403`。
///
/// 只有「我的订阅」相关的几个端点对普通用户开放：
/// `login`、`logout`、`me`、`my/*`。
///
/// 公开管理路径也在管理员范围内。见 [`needs_admin`]。
pub async fn admin_gate(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !needs_admin(path) {
        return next.run(request).await;
    }
    let Some(token) = session_token(request.headers()) else {
        return need_login();
    };
    let Some(info) = state.sessions.lookup(&token) else {
        return need_login();
    };
    // 角色从库里重读，不信任登录快照：降级管理员立即失去权限。
    let admin = state
        .users
        .find_user(&info.user_id)
        .is_some_and(|user| user.role.is_admin());
    if admin {
        return next.run(request).await;
    }
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": "需要管理员权限" })),
    )
        .into_response()
}

fn need_login() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({ "error": "未登录或登录已失效" })),
    )
        .into_response()
}

/// 该路径是否只允许管理员。
fn needs_admin(path: &str) -> bool {
    // 非面板路径里的公开管理端点。旧路由保留到 Task 10。
    const PUBLIC_ADMIN: [&str; 5] = [
        "/subs/status",
        "/subs/groups",
        "/subs/preview",
        "/subtitle/mtran/test",
        "/subtitle/translate/clear",
    ];
    if PUBLIC_ADMIN.contains(&path) || path.starts_with("/epg/translate/") {
        return true;
    }
    let Some(relative) = panel_relative(path) else {
        return false;
    };
    const OPEN: [&str; 3] = ["/login", "/logout", "/me"];
    if OPEN.contains(&relative) {
        return false;
    }
    // 「我的订阅」相关接口对普通用户开放。
    !relative.starts_with("/my")
}

/// 取会话令牌。与 [`crate::http::ext::PanelAuth`] 同一套头。
fn session_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| value.trim().to_string())
        .or_else(|| {
            headers
                .get("x-token")
                .and_then(|value| value.to_str().ok())
                .map(|value| value.trim().to_string())
        })
        .filter(|token| !token.is_empty())
}

/// 功能开关：关掉的功能，其面板接口返回 `403`。
///
/// 语义与部署规格无关。高配与低配行为一致：
/// 入口隐藏与接口拒绝同时生效。
///
/// 恢复路径不受拦截：`/panel/api/config` 可以重新打开开关，
/// `/panel/api/login` 与 `/panel/api/me` 也不在拦截范围。
pub async fn feature_gate(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let config = state.snapshot();
    if let Some(enabled) = feature_of(path, &config) {
        if !enabled {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({ "error": "该功能已在系统设置中关闭" })),
            )
                .into_response();
        }
    }
    next.run(request).await
}

/// 路径 → 对应的功能开关。`None` 表示不受功能开关控制。
///
/// 面板路径先去掉前缀再匹配，新旧前缀结果一致。非面板的旧路径
/// （`/subs/`、`/subtitle/`、`/epg/translate/`）保留到 Task 10。
fn feature_of(path: &str, config: &crate::config::Config) -> Option<bool> {
    if path.starts_with("/subtitle/") {
        return Some(config.features.assets);
    }
    if path.starts_with("/subs/") {
        return Some(config.features.playlist);
    }
    if path.starts_with("/epg/translate/") {
        return Some(config.epg.enabled);
    }
    let relative = panel_relative(path)?;
    if relative.starts_with("/schedule") {
        return Some(config.features.recording);
    }
    if relative.starts_with("/livestream") {
        return Some(config.features.livestream);
    }
    if relative.starts_with("/logo") {
        return Some(config.features.logo);
    }
    if relative.starts_with("/assets") || relative.starts_with("/subtitle") {
        return Some(config.features.assets);
    }
    if relative.starts_with("/playlist") || relative.starts_with("/subs") {
        return Some(config.features.playlist);
    }
    if relative.starts_with("/epg/translate") {
        return Some(config.epg.enabled);
    }
    if relative.starts_with("/users") {
        return Some(config.features.users);
    }
    if relative == "/stats" {
        return Some(config.stats.enabled);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn feature_gate_maps_paths_to_switches() {
        let mut config = Config::default();
        assert_eq!(
            feature_of("/panel/api/schedule", &config),
            Some(true),
            "默认开启"
        );
        config.features.recording = false;
        assert_eq!(
            feature_of("/panel/api/schedule/3/stop", &config),
            Some(false)
        );
        assert_eq!(
            feature_of("/panel/api/livestream/a/b/start", &config),
            Some(true)
        );
        assert_eq!(feature_of("/panel/api/login", &config), None, "登录不受限");
        assert_eq!(feature_of("/panel/api/config", &config), None, "配置可恢复");
        assert_eq!(feature_of("/panel/api/me", &config), None);
        // 新前缀与旧前缀结果一致。
        assert_eq!(feature_of("/api/panel/schedule", &config), Some(false));
        assert_eq!(
            feature_of("/api/panel/livestream/a/b/start", &config),
            Some(true)
        );
        assert_eq!(feature_of("/api/panel/login", &config), None);
        assert_eq!(feature_of("/api/panel/config", &config), None);
        assert_eq!(feature_of("/api/panel/me", &config), None);
    }

    #[test]
    fn new_panel_prefix_maps_same_features_as_old() {
        let mut config = Config::default();
        for (old, new) in [
            ("/panel/api/schedule", "/api/panel/schedule"),
            ("/panel/api/livestream", "/api/panel/livestream"),
            ("/panel/api/logo/list", "/api/panel/logo/list"),
            ("/panel/api/assets/list", "/api/panel/assets/list"),
            ("/panel/api/subtitle/list", "/api/panel/subtitle/list"),
            ("/panel/api/playlist", "/api/panel/playlist"),
            ("/panel/api/subs/status", "/api/panel/subs/status"),
            (
                "/panel/api/epg/translate/status",
                "/api/panel/epg/translate/status",
            ),
            ("/panel/api/users", "/api/panel/users"),
            ("/panel/api/stats", "/api/panel/stats"),
        ] {
            assert_eq!(feature_of(old, &config), feature_of(new, &config), "{new}");
        }
        config.features.assets = false;
        assert_eq!(feature_of("/api/panel/subtitle/list", &config), Some(false));
        config.features.playlist = false;
        assert_eq!(feature_of("/api/panel/subs/status", &config), Some(false));
        config.epg.enabled = false;
        assert_eq!(
            feature_of("/api/panel/epg/translate/status", &config),
            Some(false)
        );
    }

    #[test]
    fn admin_gate_maps_paths() {
        assert!(needs_admin("/panel/api/config"), "配置只给管理员");
        assert!(needs_admin("/panel/api/groups"), "分组只给管理员");
        assert!(needs_admin("/panel/api/users"), "用户管理只给管理员");
        assert!(!needs_admin("/panel/api/login"));
        assert!(!needs_admin("/panel/api/logout"));
        assert!(!needs_admin("/panel/api/me"));
        assert!(!needs_admin("/panel/api/my/channels"));
        assert!(!needs_admin("/health"), "非面板路径不管");
        assert!(!needs_admin("/sub/abc/playlist.m3u"));
        // 新前缀与旧前缀结果一致。
        assert!(!needs_admin("/api/panel/login"));
        assert!(!needs_admin("/api/panel/logout"));
        assert!(!needs_admin("/api/panel/me"));
        assert!(!needs_admin("/api/panel/my/channels"));
        assert!(needs_admin("/api/panel/config"), "配置只给管理员");
        assert!(needs_admin("/api/panel/groups"), "分组只给管理员");
        assert!(needs_admin("/api/panel/users"), "用户管理只给管理员");
        assert!(needs_admin("/api/panel/subs/status"), "订阅管理只给管理员");
        assert!(
            needs_admin("/api/panel/subtitle/list"),
            "字幕管理只给管理员"
        );
        assert!(
            needs_admin("/api/panel/epg/translate/status"),
            "EPG 翻译只给管理员"
        );
    }

    #[test]
    fn page_and_auth_recovery_paths_are_ip_guard_exempt() {
        for path in [
            "/page/panel",
            "/static/panel/a.js",
            "/api/panel/status",
            "/api/unban/whoami",
        ] {
            assert!(ip_guard_exempt(path), "{path}");
        }
        for path in ["/api/health", "/media/g/c/seg.png", "/api/play/g/c.m3u8"] {
            assert!(!ip_guard_exempt(path), "{path}");
        }
        // 旧前缀迁移期内继续豁免。
        for path in ["/panel/api/status", "/unban/whoami"] {
            assert!(ip_guard_exempt(path), "{path}");
        }
    }

    #[test]
    fn panel_relative_accepts_both_prefixes() {
        assert_eq!(panel_relative("/panel/api/login"), Some("/login"));
        assert_eq!(panel_relative("/api/panel/login"), Some("/login"));
        assert_eq!(panel_relative("/api/panel"), Some(""));
        assert_eq!(panel_relative("/health"), None);
    }

    #[test]
    fn stats_switch_follows_stats_config() {
        let mut config = Config::default();
        assert_eq!(feature_of("/panel/api/stats", &config), Some(true));
        config.stats.enabled = false;
        assert_eq!(feature_of("/panel/api/stats", &config), Some(false));
    }
}
