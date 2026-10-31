//! 请求前置检查：IP 统计与封禁。
//!
//! 只作用于非 `/panel` 与非 `/unban` 路径，避免管理员或用户无法解封。
//! 封禁期内返回 `403`（形状为本实现自定）。

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
    if !request.uri().path().starts_with("/panel") && !request.uri().path().starts_with("/unban") {
        let peer = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0);
        let ip = client_ip_with_peer(request.headers(), peer);
        state.ip_guard.record_request(&ip);
        // 白名单豁免封禁。只在 `ip_guard_enabled` 打开时生效，
        // 否则默认配置下白名单会改变既有封禁行为。
        let config = state.snapshot();
        let exempt =
            config.auth.ip_guard_enabled && crate::auth::whitelisted(&config.auth.whitelist, &ip);
        if !exempt && state.ip_guard.is_banned(&ip) {
            return (StatusCode::FORBIDDEN, "banned").into_response();
        }
    }
    next.run(request).await
}

/// 管理员校验：非管理员访问管理接口时返回 `403`。
///
/// 只有「我的订阅」相关的几个端点对普通用户开放：
/// `login`、`logout`、`me`、`my/*`。
///
/// 未登录的请求放行，让处理器给出 `401`。这样错误语义不变。
pub async fn admin_gate(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !needs_admin(path) {
        return next.run(request).await;
    }
    let token = session_token(request.headers());
    let Some(token) = token else {
        return next.run(request).await;
    };
    let Some(info) = state.sessions.lookup(&token) else {
        return next.run(request).await;
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

/// 该路径是否只允许管理员。
fn needs_admin(path: &str) -> bool {
    if !path.starts_with("/panel/api") {
        return false;
    }
    const OPEN: [&str; 3] = ["/panel/api/login", "/panel/api/logout", "/panel/api/me"];
    if OPEN.contains(&path) {
        return false;
    }
    // 「我的订阅」相关接口对普通用户开放。
    !path.starts_with("/panel/api/my")
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
fn feature_of(path: &str, config: &crate::config::Config) -> Option<bool> {
    if path.starts_with("/panel/api/schedule") {
        return Some(config.features.recording);
    }
    if path.starts_with("/panel/api/livestream") {
        return Some(config.features.livestream);
    }
    if path.starts_with("/panel/api/logo") {
        return Some(config.features.logo);
    }
    if path.starts_with("/panel/api/assets") || path.starts_with("/panel/api/subtitle") {
        return Some(config.features.assets);
    }
    if path.starts_with("/panel/api/playlist") || path.starts_with("/panel/api/subs") {
        return Some(config.features.playlist);
    }
    if path.starts_with("/panel/api/users") {
        return Some(config.features.users);
    }
    if path == "/panel/api/stats" {
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
    }

    #[test]
    fn stats_switch_follows_stats_config() {
        let mut config = Config::default();
        assert_eq!(feature_of("/panel/api/stats", &config), Some(true));
        config.stats.enabled = false;
        assert_eq!(feature_of("/panel/api/stats", &config), Some(false));
    }
}
