//! 公开面：健康检查、首页跳转、兜底。

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use crate::state::AppState;

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
pub async fn whoami(headers: axum::http::HeaderMap) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "ip": crate::http::ext::client_ip(&headers) }))
}
