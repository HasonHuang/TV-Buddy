//! 请求前置检查：IP 统计与封禁。
//!
//! 只作用于非 `/panel` 路径，避免管理员误封自己后无法解封。
//! 封禁期内返回 `403`（形状为本实现自定）。

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{http::ext::client_ip, state::AppState};

/// 记录请求并在封禁期内拒绝。
pub async fn ip_guard(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if !request.uri().path().starts_with("/panel") {
        let ip = client_ip(request.headers());
        state.ip_guard.record_request(&ip);
        if state.ip_guard.is_banned(&ip) {
            return (StatusCode::FORBIDDEN, "banned").into_response();
        }
    }
    next.run(request).await
}
