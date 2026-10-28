//! 运维接口：缓存状态与清空、运行指标、运行日志（M4）。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../../../docs/design/03-http-contract.md) §6.4。
//! 响应形状为本实现自定（目标未逐字段采证）。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::{
    cache_gc::dir_usage,
    http::{error::ApiError, ext::PanelAuth},
    state::AppState,
};

/// 清空缓存时清理的目录（`dash/` 保留，S-CACHE-06）。
const FLUSH_DIRS: [&str; 4] = ["m3u8", "seg_url", "seg_bytes", "sub_url"];

/// `GET /panel/api/cache/stats`。
///
/// 字段面按目标 v0.8.2 实测（F-021 §6）。附加字段 `entries`、`recommend`
/// 供本实现的面板使用。
pub async fn cache_stats(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let mut entries = serde_json::Map::new();
    let mut disk_bytes = 0u64;
    let mut counts: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for name in [
        "m3u8",
        "seg_url",
        "seg_bytes",
        "sub_url",
        "dash",
        "timeshift",
        "dash_timeshift",
        "recordings",
        "livestream",
    ] {
        let (files, size) = dir_usage(&state.cache_dir.join(name));
        counts.insert(name, files);
        disk_bytes += size;
        if matches!(name, "m3u8" | "seg_url" | "seg_bytes" | "sub_url" | "dash") {
            entries.insert(name.to_string(), json!(files));
        }
    }
    let dir_bytes = |name: &str| dir_usage(&state.cache_dir.join(name)).1;
    Json(json!({
        // 目标字段
        "backend": state.snapshot().cache.backend,
        "disk_bytes": disk_bytes,
        "m3u8_entries": counts.get("m3u8").copied().unwrap_or(0),
        "segment_entries": counts.get("seg_bytes").copied().unwrap_or(0),
        "segment_url_entries": counts.get("seg_url").copied().unwrap_or(0),
        "sub_url_entries": counts.get("sub_url").copied().unwrap_or(0),
        "dash_disk_bytes": dir_bytes("dash"),
        "timeshift_disk_bytes": dir_bytes("timeshift") + dir_bytes("dash_timeshift"),
        "recording_disk_bytes": dir_bytes("recordings"),
        "livestream_disk_bytes": dir_bytes("livestream"),
        // 本实现附加字段
        "disk_path": state.cache_dir.display().to_string(),
        "entries": Value::Object(entries),
        "bytes": disk_bytes,
        "playlist_cache_entries": state.playlist_cache_entries(),
        // 推荐 TTL / 磁盘上限（被动实测，见 `docs/design/06-low-spec-optimization.md` §3）。
        "recommend": crate::cache_gc::recommendation(&state),
    }))
}

/// `POST /panel/api/cache/flush`。
pub async fn cache_flush(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    // 内存后端：先清进程内的切片字典，再清磁盘目录。
    let memory_bytes = state.memory.bytes();
    if memory_bytes > 0 || !state.memory.is_empty() {
        state.memory.clear();
    }
    let mut removed = memory_bytes;
    for name in FLUSH_DIRS {
        removed += dir_usage(&state.cache_dir.join(name)).0;
        let dir = state.cache_dir.join(name);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(ApiError::internal(format!(
                    "清理 {} 失败: {err}",
                    dir.display()
                )))
            }
        }
        if let Err(err) = std::fs::create_dir_all(&dir) {
            return Err(ApiError::internal(format!(
                "重建 {} 失败: {err}",
                dir.display()
            )));
        }
    }
    // 同步丢弃内存索引与清单缓存，避免清空后仍命中旧数据。
    state.segments.clear();
    state.playlists.clear();
    state.sources.clear();
    tracing::info!(removed, "缓存已清空");
    Ok(Json(json!({ "ok": true, "removed": removed })))
}

/// `GET /panel/api/stats`。
pub async fn stats(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let snapshot = state.stats.snapshot();
    let config = state.snapshot();
    Json(json!({
        "version": state.version(),
        "uptime_secs": state.uptime_secs(),
        "requests": snapshot.requests,
        "playlists": snapshot.playlists,
        "segments": snapshot.segments,
        "segment_hits": snapshot.segment_hits,
        "upstream_failures": snapshot.upstream_failures,
        "online_window_secs": config.stats.online_window_secs,
        "groups": snapshot
            .groups
            .iter()
            .map(|(id, requests)| json!({ "id": id, "requests": requests }))
            .collect::<Vec<_>>(),
    }))
}

/// `GET /panel/api/logs?limit=n`。
pub async fn logs(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(200)
        .min(500);
    let lines = state.logs.tail(limit);
    Json(json!({
        "total": state.logs.len(),
        "lines": lines,
    }))
}

/// 统计中间件：记总请求数，并按首个路径段记分组。
pub async fn count_request(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let gid = group_of(request.uri().path()).unwrap_or_default();
    state.stats.record_request(&gid);
    let response = next.run(request).await;
    count_out(state, response)
}

/// 把响应体包一层，边发边累加出网字节。
///
/// 目标 `traffic_out_bytes` 随响应增长（F-021 §6）。不缓冲正文。
fn count_out(state: Arc<AppState>, response: axum::response::Response) -> axum::response::Response {
    use axum::body::Body;
    use http_body_util::BodyExt;
    let (parts, body) = response.into_parts();
    let metrics = state.metrics.clone();
    let counted = body.map_frame(move |frame| {
        if let Some(data) = frame.data_ref() {
            metrics.add_out(data.len() as u64);
        }
        frame
    });
    axum::response::Response::from_parts(parts, Body::new(counted))
}

/// 从路径取分组 id。面板、静态资源与公开文件不算。
fn group_of(path: &str) -> Option<String> {
    let first = path.trim_start_matches('/').split('/').next()?;
    if first.is_empty() || first.contains('.') {
        return None;
    }
    if matches!(
        first,
        "panel" | "__assets" | "epg" | "logo" | "unban" | "api" | "health"
    ) {
        return None;
    }
    Some(first.to_string())
}
