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
    let local = playlist::render_total(&config, &base_url(&headers), token.as_deref());
    let extra = aggregate_sources(&state, &config).await;
    let body = playlist::compose(local, extra, &config.playlist.merge_position);
    m3u_response(body, "playlist.m3u")
}

/// 抓取并合并聚合源（F-013 §5）。
///
/// 单个源失败只记日志；本地订阅照常返回。
async fn aggregate_sources(state: &Arc<AppState>, config: &crate::config::Config) -> String {
    let mut extra = String::new();
    let ttl = config.playlist.merge_ttl_secs.max(1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let bucket = now / ttl;
    for source in config
        .playlist
        .sources
        .iter()
        .filter(|source| source.enabled && !source.url.trim().is_empty())
    {
        let key = format!("sub:{}:{bucket}", source.url);
        let body = match state.sources.get(&key) {
            Some(body) => body,
            None => match state.http.get(&source.url).send().await {
                Ok(response) if response.status().is_success() => match response.text().await {
                    Ok(text) => {
                        state.sources.put(&key, text.clone());
                        text
                    }
                    Err(error) => {
                        tracing::warn!(url = %source.url, error = %error, "读取聚合源失败");
                        continue;
                    }
                },
                Ok(response) => {
                    tracing::warn!(url = %source.url, status = %response.status(), "聚合源返回非成功状态");
                    continue;
                }
                Err(error) => {
                    tracing::warn!(url = %source.url, error = %error, "抓取聚合源失败");
                    continue;
                }
            },
        };
        extra.push_str(&playlist::aggregate(
            source,
            &config.playlist.global_exclude,
            &body,
        ));
    }
    extra
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
