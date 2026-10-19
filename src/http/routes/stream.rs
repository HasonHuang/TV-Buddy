//! 播放链路：入口清单 → 媒体清单 → 伪装切片（M2）。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../../../docs/design/03-http-contract.md) §4。

use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{
    config::{model::Group, Config},
    proxy,
    state::AppState,
};

/// 入口清单缓存头（S-CACHE-01）。
const ENTRY_CACHE: &str = "public, max-age=5, s-maxage=5";
/// 切片首写缓存头。
const SEGMENT_FIRST: &str = "public, max-age=300";
/// 切片命中缓存头。
const SEGMENT_HIT: &str = "private, max-age=300";

/// `GET /{gid}/{cid}.m3u8` 与 `/{gid}/{cid}.mpd`。
pub async fn entry(
    State(state): State<Arc<AppState>>,
    Path((gid, file)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let config = state.snapshot();
    let Some((cid, ext)) = split_entry(&file) else {
        return placeholder(&config);
    };
    let Some(group) = config.group(&gid) else {
        return placeholder(&config);
    };
    // 归档组停播（F-009）。
    if group.access == "archived" {
        return placeholder(&config);
    }
    if !group.channels.iter().any(|channel| channel.id == cid) {
        return placeholder(&config);
    }
    if !token_ok(&state, &config, group, &query) {
        return redirect(&config.auth.token_failure_url);
    }

    // M2 覆盖 proxy/remux/dash；redirect/rewrite/passthrough 在 M3 补齐。
    let mut response = Response::new(Body::from(master_playlist(&gid, &cid)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(match ext.as_str() {
            "mpd" => "application/dash+xml",
            _ => "application/vnd.apple.mpegurl",
        }),
    );
    insert_header(&mut response, header::CACHE_CONTROL, ENTRY_CACHE);
    response
}

/// `GET /{gid}/{cid}/*path`：`video/index.txt` 或伪装切片。
pub async fn wildcard(
    State(state): State<Arc<AppState>>,
    Path((gid, cid, rest)): Path<(String, String, String)>,
) -> Response {
    let rest = rest.trim_start_matches('/').to_string();
    if rest == "video/index.txt" {
        media_playlist(&state, &gid, &cid).await
    } else {
        segment(&state, &gid, &cid, &rest).await
    }
}

/// 媒体清单：抓上游清单、改成伪装地址、登记切片索引。
async fn media_playlist(state: &Arc<AppState>, gid: &str, cid: &str) -> Response {
    let config = state.snapshot();
    let Some((group, channel_url)) = resolve(&config, gid, cid) else {
        return (StatusCode::NOT_FOUND, "Channel not found").into_response();
    };
    let cache_key = format!("media:{gid}:{cid}");
    let disguise = config.cache.disguise.enabled && config.cache.disguise.playlist;

    // 5 秒内复用同一份清单，避免突发请求打穿上游。
    if let Some(body) = state.playlists.get(&cache_key) {
        return playlist_response(body);
    }

    let upstream = match fetch(state, &channel_url).await {
        Ok(text) => text,
        Err(err) => {
            tracing::warn!(%gid, %cid, error = %err, "抓取上游清单失败");
            return (StatusCode::BAD_GATEWAY, "Upstream Error").into_response();
        }
    };

    let rendered = proxy::rewrite_media(
        &upstream,
        &channel_url,
        gid,
        cid,
        &config.cache.disguise.formats,
        disguise,
    );
    state.segments.register(gid, cid, &rendered.segments);
    proxy::save_index(&state.cache_dir, gid, cid, &rendered.segments);
    state.playlists.put(&cache_key, rendered.body.clone());
    let _ = group;

    playlist_response(rendered.body)
}

/// 媒体清单响应。
fn playlist_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    insert_header(&mut response, header::CACHE_CONTROL, ENTRY_CACHE);
    response
}

/// 伪装切片：查索引、读缓存或抓上游、按命中改写缓存头。
async fn segment(state: &Arc<AppState>, gid: &str, cid: &str, name: &str) -> Response {
    let config = state.snapshot();
    let Some((group, _url)) = resolve(&config, gid, cid) else {
        return (StatusCode::NOT_FOUND, "Channel not found").into_response();
    };
    if group.access == "archived" {
        return placeholder(&config);
    }
    let expected = proxy::prefix(&group.id, cid);
    let Some((suffix, ext)) = proxy::parse_segment(name) else {
        return placeholder(&config);
    };
    if !proxy::prefix_matches(&expected, name) {
        // 前缀错值 → 302 占位（S-DIS-17）。
        return placeholder(&config);
    }
    let Some(entry) = state.segments.get(&state.cache_dir, gid, cid, &suffix) else {
        return (StatusCode::NOT_FOUND, "Segment not found").into_response();
    };

    let path = proxy::segment_path(&state.cache_dir, gid, cid, &suffix, &ext);
    if let Ok(bytes) = std::fs::read(&path) {
        return segment_response(bytes, &ext, SEGMENT_HIT);
    }
    match fetch_bytes(state, &entry.url).await {
        Ok(bytes) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::write(&path, &bytes) {
                tracing::warn!(error = %err, "写切片缓存失败");
            }
            segment_response(bytes, &ext, SEGMENT_FIRST)
        }
        Err(err) => {
            tracing::warn!(url = %entry.url, error = %err, "抓取上游切片失败");
            (StatusCode::BAD_GATEWAY, "Upstream Error").into_response()
        }
    }
}

/// 解析分组与频道。
fn resolve(config: &Config, gid: &str, cid: &str) -> Option<(Group, String)> {
    let group = config.group(gid)?;
    let channel = group.channels.iter().find(|channel| channel.id == cid)?;
    Some((group.clone(), channel.url.clone()))
}

/// 拆入口文件名：`c1.m3u8` → `("c1", "m3u8")`。
fn split_entry(file: &str) -> Option<(String, String)> {
    let (cid, ext) = file.rsplit_once('.')?;
    if cid.is_empty() || !matches!(ext, "m3u8" | "mpd") {
        return None;
    }
    Some((cid.to_string(), ext.to_string()))
}

/// Token 仅校验入口；`open` 组免校验（F-007、F-009）。
fn token_ok(
    state: &AppState,
    config: &Config,
    group: &Group,
    query: &HashMap<String, String>,
) -> bool {
    if !config.auth.token_enabled || group.access == "open" {
        return true;
    }
    query
        .get("token")
        .map(|token| state.play_tokens.verify(token))
        .unwrap_or(false)
}

/// 入口清单正文。
fn master_playlist(gid: &str, cid: &str) -> String {
    format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:BANDWIDTH=2000000\n/{gid}/{cid}/video/index.txt\n"
    )
}

/// 302 占位视频（未知频道、归档组、错前缀）。
fn placeholder(config: &Config) -> Response {
    redirect(&config.server.not_found_url)
}

/// 手写 302（axum 的 `Redirect::temporary` 是 307）。
fn redirect(url: &str) -> Response {
    if url.trim().is_empty() {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    match HeaderValue::from_str(url) {
        Ok(value) => {
            let mut response = StatusCode::FOUND.into_response();
            response.headers_mut().insert(header::LOCATION, value);
            response
        }
        Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// 切片响应：按伪装扩展名给类型；忽略 `Range`，总是返回全量（S-DIS-07）。
fn segment_response(bytes: Vec<u8>, ext: &str, cache_control: &str) -> Response {
    let mime = mime_guess::from_ext(ext).first_or_octet_stream();
    let mut response = Response::new(Body::from(bytes));
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    insert_header(&mut response, header::CACHE_CONTROL, cache_control);
    response
}

fn insert_header(response: &mut Response, name: header::HeaderName, value: &str) {
    if let Ok(parsed) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, parsed);
    }
}

/// 抓上游文本。
async fn fetch(state: &AppState, url: &str) -> Result<String, reqwest::Error> {
    let response = state.http.get(url).send().await?.error_for_status()?;
    response.text().await
}

/// 抓上游字节。
async fn fetch_bytes(state: &AppState, url: &str) -> Result<Vec<u8>, reqwest::Error> {
    let response = state.http.get(url).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}
