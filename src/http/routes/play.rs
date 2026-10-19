//! 订阅端点：总订阅与分组订阅。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../../../docs/design/03-http-contract.md) §5。

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{playlist, state::AppState};

/// `GET /playlist.m3u`。
pub async fn total_playlist(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let config = state.snapshot();
    let token = token_for(&state, &config);
    let body = playlist::render_total(&config, &base_url(&headers), token.as_deref());
    m3u_response(body, "playlist.m3u")
}

/// `GET /{gid}/playlist.m3u`。
pub async fn group_playlist(
    State(state): State<Arc<AppState>>,
    Path(gid): Path<String>,
    headers: HeaderMap,
) -> Response {
    let config = state.snapshot();
    let Some(group) = config.group(&gid) else {
        return (StatusCode::NOT_FOUND, "Group not found").into_response();
    };
    let token = token_for(&state, &config);
    match playlist::render_group(&config, group, &base_url(&headers), token.as_deref()) {
        Ok(body) => m3u_response(body, &format!("{gid}.m3u")),
        Err(error) => (StatusCode::NOT_FOUND, error.message()).into_response(),
    }
}

/// Token 仅在校验开启时随订阅下发（S-URL-04）。
fn token_for(state: &AppState, config: &crate::config::Config) -> Option<String> {
    if config.auth.token_enabled {
        Some(state.play_tokens.current())
    } else {
        None
    }
}

/// 由请求推导外部基地址。
fn base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1:19899");
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    format!("{scheme}://{host}")
}

fn m3u_response(body: String, filename: &str) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-mpegurl; charset=utf-8"),
    );
    let disposition = format!("attachment; filename=\"{filename}\"");
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}
