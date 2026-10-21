//! 公开面：健康检查、首页跳转、自助解封、兜底。

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::{
    auth::unban::WINDOW_SECS,
    http::{error::ApiError, ext::client_ip_with_peer},
    state::AppState,
};

/// `302 Found`。目标用 302 做占位跳转（S-DIS-15）。
fn found(url: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    match HeaderValue::from_str(url) {
        Ok(location) => {
            response.headers_mut().insert(header::LOCATION, location);
        }
        Err(_) => {
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        }
    }
    response
}

/// `GET /health` → `200` `ok`（`text/plain`，S-EP-10）。
pub async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=utf-8")],
        "ok",
    )
}

/// `GET /` → 跳首页（`server.home_url`）。
pub async fn home(State(state): State<Arc<AppState>>) -> Response {
    let cfg = state.snapshot();
    found(&cfg.server.home_url)
}

/// 兜底：面板接口返回 404 JSON；其余路径返回 404 正文。
///
/// 目标把 `/status` 一类单段未知路径判为 404（S-EP-08）。
/// 两段及以上的播放形状由 `/{gid}/{file}` 与通配路由处理。
pub async fn fallback(State(_): State<Arc<AppState>>, uri: Uri) -> Response {
    if uri.path().starts_with("/panel/api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "接口不存在" })),
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// `GET /whoami`、`GET /unban/whoami`——返回调用方地址。
///
/// 响应形状为本实现自定。目标只登记了路由存在性（S-EP-15）。
pub async fn whoami(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Json<Value> {
    Json(json!({ "ip": client_ip_with_peer(&headers, Some(peer)) }))
}

/// `POST /unban`——调用方自助解封。
///
/// 免登录（S-EP-07）。每个 IP 30 天最多 3 次（S-AUTH-07）。
/// 只解封调用方地址；不提供解封他人地址的能力。
pub async fn unban(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let ip = client_ip_with_peer(&headers, Some(peer));
    if !state.ip_guard.is_banned(&ip) {
        return Ok(Json(json!({
            "ok": false,
            "ip": ip,
            "reason": "not_banned",
            "remaining": state.unban_quota.remaining(&ip),
            "window_secs": WINDOW_SECS,
        })));
    }
    if !state.unban_quota.consume(&ip) {
        return Err(ApiError::too_many(json!({
            "error": "本月解封次数已用完",
            "remaining": 0,
            "window_secs": WINDOW_SECS,
        })));
    }
    state.ip_guard.unban(&ip);
    Ok(Json(json!({
        "ok": true,
        "ip": ip,
        "remaining": state.unban_quota.remaining(&ip),
        "window_secs": WINDOW_SECS,
    })))
}
