//! 播放链路：入口清单 → 媒体清单 → 伪装切片（M2）与直连模式（M3）。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../../../docs/design/03-http-contract.md) §4。

use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{
    config::{model::Group, Config},
    proxy,
    state::AppState,
};

/// DASH/回退清单缓存头。
const ENTRY_CACHE: &str = "public, max-age=5, s-maxage=5";
/// Rewrite 模式缓存头。
const REWRITE_CACHE: &str = "no-cache, must-revalidate";
/// 主清单入口缓存头：变体清单每次都要重取（S-PROXY-08）。
const MASTER_CACHE: &str = "no-cache, no-store";
/// 清单缺 `#EXT-X-TARGETDURATION` 时的缓存窗口（秒）。
const DEFAULT_TARGET_DURATION: u64 = 6;
/// 切片首写缓存头。
const SEGMENT_FIRST: &str = "public, max-age=300";
/// 切片命中缓存头。
const SEGMENT_HIT: &str = "private, max-age=300";

/// `GET /{gid}/{cid}.m3u8` 与 `/{gid}/{cid}.mpd`。
pub async fn entry(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((gid, file)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let config = state.snapshot();
    let self_base = self_base(&headers);
    let Some((cid, ext)) = split_entry(&file) else {
        return placeholder(&config);
    };
    let Some(group) = config.group(&gid) else {
        return unknown_endpoint(&state, &config, &query);
    };
    // 归档组停播（F-009）。
    if group.access == "archived" {
        return placeholder(&config);
    }
    let Some(channel) = group.channels.iter().find(|channel| channel.id == cid) else {
        return unknown_endpoint(&state, &config, &query);
    };
    if !token_ok(&state, &config, group, &query) {
        return redirect(&config.auth.token_failure_url);
    }
    // 上游解析脚本：每次播放前算地址（S-RES-01）。
    let resolved = match crate::resolver::resolve(&state, &gid, &cid, channel).await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(gid = %gid, cid = %cid, "解析脚本失败: {error:#}");
            return redirect(&config.server.error_url);
        }
    };
    let base = resolved.unwrap_or_else(|| channel.url.clone());
    // 回看：只把 `playseek` 原样追加到上游（S-EP-04），其余回看参数丢弃。
    let mut upstream = with_playseek(&base, &query);
    // 本地回看：点亮 `livestream` 的频道优先读缓冲；未命中落回实时（S-TS-01）。
    if channel.livestream || channel.dash_timeshift {
        if let Some((start, end)) =
            crate::timeshift::range(&query, config.server.timezone_offset_hours as i64)
        {
            if let Some(body) = state.timeshift.playlist(&state, &gid, &cid, start, end) {
                return crate::timeshift::vad_response(body);
            }
            upstream = base.clone();
        }
    }
    // Passthrough 原样下发上游地址（S-MODE-11）。
    if group.mode == "passthrough" {
        return redirect(&upstream);
    }

    match group.mode.as_str() {
        // 302 直连上游；缓存解析出的地址（S-MODE-08/13）。
        "redirect" => redirect_entry(&state, &config, group, &cid, &upstream).await,
        // 清单绝对化，客户端直连上游切片（S-MODE-09/14）。
        "rewrite" => rewrite_entry(&state, &config, group, &cid, &upstream).await,
        // 连续流转封装：纯 Rust 切片；未产出前 503（S-MODE-16/18）。
        "remux" => {
            let session = state.remux.ensure(&state, &gid, &cid, &upstream);
            match session.playlist() {
                Some(body) => remux_playlist(body),
                None => warming_up(),
            }
        }
        // 就绪门：30 秒内拿到上游主清单，否则 503（S-MODE-15）。
        "dash" => dash_entry(&state, &config, group, &cid, &upstream).await,
        // 代理：上游是主清单则自造单变体主清单；上游是媒体清单则直接改写。
        _ => proxy_entry(&state, &config, &gid, &cid, &upstream, &self_base, &ext).await,
    }
}

/// 回看参数：只保留 `playseek`，按上游已有查询串拼接（S-EP-04）。
fn with_playseek(url: &str, query: &HashMap<String, String>) -> String {
    let Some(value) = query
        .get("playseek")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return url.to_string();
    };
    let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}playseek={encoded}")
}

/// 缓存键里的地址标签：同一频道不同回看区间不互相覆盖。
fn url_tag(url: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(url.as_bytes())[..4])
}

/// Redirect 模式：302 到上游；`解析最佳画质` 时先取主清单挑最高码率。
async fn redirect_entry(
    state: &Arc<AppState>,
    config: &Config,
    group: &Group,
    cid: &str,
    channel_url: &str,
) -> Response {
    let ttl = proxy::modes::effective_redirect_ttl(group.redirect_ttl);
    let key = format!("redirect:{}:{cid}:{}", group.id, url_tag(channel_url));
    if let Some(url) = state.modes.get(&key, ttl) {
        return redirect(&url);
    }
    let resolved = if proxy::modes::wants_best_quality(&group.redirect_mode) {
        match fetch(state, channel_url, &group.id, cid).await {
            Ok(text) => proxy::modes::best_variant(&text, channel_url)
                .unwrap_or_else(|| channel_url.to_string()),
            // 失败不缓存（S-MODE-08）。
            Err(err) => {
                tracing::warn!(url = %channel_url, error = %err, "解析上游主清单失败");
                return redirect(&config.server.error_url);
            }
        }
    } else {
        channel_url.to_string()
    };
    state.modes.put(&key, resolved.clone());
    redirect(&resolved)
}

/// Rewrite 模式：取一次清单，把相对地址改绝对，切片客户端直连（S-MODE-09）。
async fn rewrite_entry(
    state: &Arc<AppState>,
    config: &Config,
    group: &Group,
    cid: &str,
    channel_url: &str,
) -> Response {
    let key = format!("rw:{}:{cid}:{}", group.id, url_tag(channel_url));
    if let Some(body) = state.playlists.get(&key) {
        return rewrite_response(body);
    }
    let upstream = match fetch(state, channel_url, &group.id, cid).await {
        Ok(text) => text,
        Err(err) => {
            tracing::warn!(url = %channel_url, error = %err, "抓取上游清单失败");
            let _ = config;
            return (StatusCode::BAD_GATEWAY, "Upstream Error").into_response();
        }
    };
    let rendered = proxy::rewrite_media(&upstream, channel_url, &group.id, cid, &[], false, "");
    state.playlists.put(&key, rendered.body.clone());
    rewrite_response(rendered.body)
}

/// DASH 模式：30 秒就绪门。转封装需 ffmpeg（P1），当前透传上游正文。
async fn dash_entry(
    state: &Arc<AppState>,
    config: &Config,
    group: &Group,
    cid: &str,
    channel_url: &str,
) -> Response {
    let key = format!("dash:{}:{cid}", group.id);
    if state.watchdog.is_cooling(&key) {
        return not_ready();
    }
    let body = match state.modes.get(&key, 30) {
        Some(body) => body,
        None => match fetch(state, channel_url, &group.id, cid).await {
            Ok(body) => {
                state.watchdog.record_success(&key);
                state.modes.put(&key, body.clone());
                body
            }
            Err(err) => {
                tracing::warn!(url = %channel_url, error = %err, "等待上游主清单超时");
                state.stats.record_upstream_failure();
                if state.watchdog.record_failure(&key) {
                    tracing::warn!(group = %group.id, %cid, "连续失败 5 次，进入冷却 3600s");
                }
                let _ = config;
                return not_ready();
            }
        },
    };
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/dash+xml"),
    );
    insert_header(&mut response, header::CACHE_CONTROL, ENTRY_CACHE);
    response
}

/// Rewrite 响应：`text/plain` + `no-cache, must-revalidate`（S-MODE-09）。
fn rewrite_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    insert_header(&mut response, header::CACHE_CONTROL, REWRITE_CACHE);
    response
}

/// DASH 未就绪响应（S-MODE-15）。
fn not_ready() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "timeout waiting for master playlist",
    )
        .into_response()
}

/// Remux 清单响应：`text/plain`，禁止缓存。
fn remux_playlist(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    insert_header(
        &mut response,
        header::CACHE_CONTROL,
        "no-cache, must-revalidate",
    );
    response
}

/// Remux 预热响应（S-MODE-16）。
fn warming_up() -> Response {
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, "remux warming up").into_response();
    insert_header(&mut response, header::RETRY_AFTER, "2");
    insert_header(&mut response, header::CACHE_CONTROL, "no-store");
    response
}

/// `GET /{gid}/{cid}/*path`：`video/index.txt`、`index.m3u8` 或伪装切片。
pub async fn wildcard(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((gid, cid, rest)): Path<(String, String, String)>,
) -> Response {
    let rest = rest.trim_start_matches('/').to_string();
    if let Some(name) = rest.strip_prefix("timeshift/") {
        return state
            .timeshift
            .serve(&gid, &cid, name)
            .unwrap_or_else(|| (StatusCode::NOT_FOUND, "Segment not found").into_response());
    }
    if let Some(response) = state.remux.serve(&gid, &cid, &rest) {
        return response;
    }
    match rest.as_str() {
        // 变体媒体清单：只有上游是主清单时存在（S-PROXY-07）。
        "video/index.txt" => variant_playlist(&state, &gid, &cid, &self_base(&headers)).await,
        // 兼容别名：与入口同义。
        "index.m3u8" => {
            let config = state.snapshot();
            let Some((group, url)) = resolve(&config, &gid, &cid) else {
                return placeholder(&config);
            };
            if group.mode == "remux" {
                let session = state.remux.ensure(&state, &gid, &cid, &url);
                match session.playlist() {
                    Some(body) => remux_playlist(body),
                    None => warming_up(),
                }
            } else if group.mode == "proxy" {
                proxy_entry(
                    &state,
                    &config,
                    &gid,
                    &cid,
                    &url,
                    &self_base(&headers),
                    "m3u8",
                )
                .await
            } else {
                segment(&state, &gid, &cid, &rest).await
            }
        }
        _ => segment(&state, &gid, &cid, &rest).await,
    }
}

/// 代理模式入口。
///
/// - 上游是主清单 → 自造单变体主清单（S-PROXY-06）。
/// - 上游是媒体清单 → 直接改写为伪装切片地址（S-PROXY-01）。
async fn proxy_entry(
    state: &Arc<AppState>,
    config: &Config,
    gid: &str,
    cid: &str,
    channel_url: &str,
    self_base: &str,
    ext: &str,
) -> Response {
    let upstream = match upstream_playlist(state, channel_url, gid, cid).await {
        Ok(text) => text,
        Err(err) => {
            tracing::warn!(url = %channel_url, error = %err, "抓取上游清单失败");
            state.stats.record_upstream_failure();
            return (StatusCode::BAD_GATEWAY, "Upstream Error").into_response();
        }
    };
    // MPD/XML 原样透传，避免按行改写破坏正文（转封装属 P1）。
    if proxy::modes::looks_like_mpd(&upstream) {
        return dash_body(upstream);
    }
    if proxy::media::is_master(&upstream) {
        return master_entry(&upstream, channel_url, gid, cid, self_base, ext);
    }
    media_entry(state, config, gid, cid, channel_url, &upstream, self_base)
}

/// 自造主清单：保留最佳变体的属性，只指向本服务的变体媒体清单。
fn master_entry(
    upstream: &str,
    channel_url: &str,
    gid: &str,
    cid: &str,
    self_base: &str,
    ext: &str,
) -> Response {
    let attrs = proxy::media::best_variant(upstream, channel_url)
        .map(|(attrs, _)| attrs)
        .unwrap_or_else(|| "BANDWIDTH=1000000".to_string());
    let body = format!(
        "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-STREAM-INF:{attrs}\n{self_base}/{gid}/{cid}/video/index.txt\n"
    );
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(match ext {
            "mpd" => "application/dash+xml",
            _ => "application/vnd.apple.mpegurl",
        }),
    );
    insert_header(&mut response, header::CACHE_CONTROL, MASTER_CACHE);
    response
}

/// 改写媒体清单并登记切片。
fn media_entry(
    state: &Arc<AppState>,
    config: &Config,
    gid: &str,
    cid: &str,
    channel_url: &str,
    upstream: &str,
    self_base: &str,
) -> Response {
    let disguise = config.cache.disguise.enabled && config.cache.disguise.playlist;
    let rendered = proxy::rewrite_media(
        upstream,
        channel_url,
        gid,
        cid,
        &config.cache.disguise.formats,
        disguise,
        self_base,
    );
    state.segments.register(gid, cid, &rendered.segments);
    proxy::save_index(&state.cache_dir, gid, cid, &rendered.segments);
    state.stats.record_playlist();
    media_response(rendered.body, upstream)
}

/// 媒体清单响应：缓存窗口取上游 `#EXT-X-TARGETDURATION`（S-PROXY-03）。
fn media_response(body: String, upstream: &str) -> Response {
    let seconds = proxy::media::target_duration(upstream).unwrap_or(DEFAULT_TARGET_DURATION);
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    insert_header(
        &mut response,
        header::CACHE_CONTROL,
        &format!("public, max-age={seconds}, s-maxage={seconds}"),
    );
    response
}

/// 变体媒体清单：抓最佳变体的清单再改写。
async fn variant_playlist(
    state: &Arc<AppState>,
    gid: &str,
    cid: &str,
    self_base: &str,
) -> Response {
    let config = state.snapshot();
    let Some((group, channel_url)) = resolve(&config, gid, cid) else {
        return (StatusCode::NOT_FOUND, "Channel not found").into_response();
    };
    if group.access == "archived" {
        return placeholder(&config);
    }
    let upstream = match upstream_playlist(state, &channel_url, gid, cid).await {
        Ok(text) => text,
        Err(err) => {
            tracing::warn!(%gid, %cid, error = %err, "抓取上游清单失败");
            state.stats.record_upstream_failure();
            return (StatusCode::BAD_GATEWAY, "Upstream Error").into_response();
        }
    };
    // 上游是媒体清单时没有变体清单这条路（S-PROXY-07）。
    let Some((_attrs, variant_url)) = proxy::media::best_variant(&upstream, &channel_url) else {
        return redirect(&config.server.error_url);
    };
    let text = match fetch(state, &variant_url, gid, cid).await {
        Ok(text) => text,
        Err(err) => {
            tracing::warn!(url = %variant_url, error = %err, "抓取变体清单失败");
            state.stats.record_upstream_failure();
            return (StatusCode::BAD_GATEWAY, "Upstream Error").into_response();
        }
    };
    if proxy::modes::looks_like_mpd(&text) {
        return dash_body(text);
    }
    media_entry(state, &config, gid, cid, &variant_url, &text, self_base)
}

/// 上游清单：进程内短暂复用，减少突发请求（S-CACHE-02）。
async fn upstream_playlist(
    state: &AppState,
    url: &str,
    gid: &str,
    cid: &str,
) -> Result<String, reqwest::Error> {
    let key = format!("upstream:{gid}:{cid}");
    if let Some(text) = state.playlists.get(&key) {
        return Ok(text);
    }
    let text = fetch(state, url, gid, cid).await?;
    state.playlists.put(&key, text.clone());
    Ok(text)
}

/// MPD 正文透传（转封装属 P1）。
fn dash_body(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/dash+xml"),
    );
    insert_header(&mut response, header::CACHE_CONTROL, ENTRY_CACHE);
    response
}

/// 本服务对外基址：从 `Host` 头拼出绝对地址前缀。
fn self_base(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    match headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    {
        Some(host) if !host.is_empty() => format!("{scheme}://{host}"),
        _ => String::new(),
    }
}

/// 伪装切片：查索引、读缓存或抓上游、按命中改写缓存头。
async fn segment(state: &Arc<AppState>, gid: &str, cid: &str, name: &str) -> Response {
    let config = state.snapshot();
    let Some((group, _url)) = resolve(&config, gid, cid) else {
        // 子路径上的未知分组/频道等价于未知路径（对拍：目标 → `not_found_url`）。
        return placeholder(&config);
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
        state.stats.record_segment(true);
        return segment_response(bytes, &ext, SEGMENT_HIT);
    }
    state.stats.record_segment(false);
    match fetch_bytes(state, &entry.url, gid, cid).await {
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
            state.stats.record_upstream_failure();
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

/// 未知分组/频道：Token 合法（或未启用 Token）→ 占位视频；否则 → Token 失效视频。
fn unknown_endpoint(
    state: &AppState,
    config: &Config,
    query: &HashMap<String, String>,
) -> Response {
    let verified = !config.auth.token_enabled
        || query
            .get("token")
            .map(|token| state.play_tokens.verify(token))
            .unwrap_or(false);
    if verified {
        placeholder(config)
    } else {
        redirect(&config.auth.token_failure_url)
    }
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

/// 频道自定义上游头：`channel.headers` 中的 `"名称: 值"` 字符串。
///
/// 由 M3U 导入写入（`#EXTVLCOPT`、`#EXTHTTP` 与行内头）。
pub(crate) fn channel_headers(state: &AppState, gid: &str, cid: &str) -> Vec<(String, String)> {
    let config = state.snapshot();
    let Some((group, _)) = resolve(&config, gid, cid) else {
        return Vec::new();
    };
    let Some(channel) = group.channels.iter().find(|channel| channel.id == cid) else {
        return Vec::new();
    };
    channel
        .headers
        .iter()
        .filter_map(|value| {
            let text = value.as_str()?;
            let (name, value) = text.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// 抓上游文本。
pub(crate) async fn fetch(
    state: &AppState,
    url: &str,
    gid: &str,
    cid: &str,
) -> Result<String, reqwest::Error> {
    let mut request = state.http_for(gid, cid).get(url);
    for (name, value) in channel_headers(state, gid, cid) {
        request = request.header(name, value);
    }
    let response = request.send().await?.error_for_status()?;
    response.text().await
}

/// 抓上游字节。
async fn fetch_bytes(
    state: &AppState,
    url: &str,
    gid: &str,
    cid: &str,
) -> Result<Vec<u8>, reqwest::Error> {
    let mut request = state.http_for(gid, cid).get(url);
    for (name, value) in channel_headers(state, gid, cid) {
        request = request.header(name, value);
    }
    let response = request.send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}
