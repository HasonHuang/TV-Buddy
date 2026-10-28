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
