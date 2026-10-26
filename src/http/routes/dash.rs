//! DASH 面板接口：轨道列表、URL 探测、回看状态与本机健康摘要。
//!
//! 目标只登记了路由存在性（F-011、F-013）；响应形状为本实现自定。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};

use crate::{
    dash,
    http::{error::ApiError, ext::PanelAuth},
    state::AppState,
};

/// 在配置中查找频道 URL。
fn channel_url(state: &AppState, gid: &str, cid: &str) -> Option<String> {
    let config = state.snapshot();
    let group = config.groups.iter().find(|group| group.id == gid)?;
    let channel = group.channels.iter().find(|channel| channel.id == cid)?;
    Some(channel.url.clone())
}

async fn inspect(state: &AppState, url: &str, gid: &str, cid: &str) -> Result<Value, ApiError> {
    let xml = super::stream::fetch(state, url, gid, cid)
        .await
        .map_err(|error| ApiError::internal(format!("Upstream Error: {error}")))?;
    let manifest = dash::parse(&xml)
        .map_err(|error| ApiError::internal(format!("MPD 解析失败: {error:#}")))?;
    Ok(json!({
        "ok": true,
        "url": url,
        "kind": manifest.kind,
        "duration": manifest.duration,
        "count": manifest.tracks.len(),
        "tracks": manifest.tracks,
    }))
}

/// `GET /panel/api/dash/status`——运行中的 DASH 会话。
pub async fn status(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(Value::Array(state.dash_hls.status()))
}

/// `GET /panel/api/dash/tracks/:gid/:cid`。
pub async fn tracks(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let url =
        channel_url(&state, &gid, &cid).ok_or_else(|| ApiError::not_found("Channel not found"))?;
    inspect(&state, &url, &gid, &cid).await.map(Json)
}

/// `POST /panel/api/dash/scan-url`——`{"url"}`；只解析，不改配置。
pub async fn scan_url(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let url = body.get("url").and_then(Value::as_str).unwrap_or("").trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ApiError::bad_request("url 必须是 HTTP(S)"));
    }
    inspect(&state, url, "", "").await.map(Json)
}

/// `GET /panel/api/dash-timeshift/status`——回看能力与已录制片数。
pub async fn timeshift_status(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    let channels: Vec<Value> = config
        .groups
        .iter()
        .flat_map(|group| {
            group
                .channels
                .iter()
                .filter(|channel| channel.livestream || channel.dash_timeshift)
                .map(|channel| {
                    let dash = group.mode == "dash" && channel.dash_timeshift;
                    let info = if dash {
                        state.dash_timeshift.info(&group.id, &channel.id)
                    } else {
                        None
                    };
                    let hours = if dash {
                        channel.dash_timeshift_hours
                    } else {
                        channel.timeshift_hours
                    };
                    let segments = if dash {
                        info.as_ref().map(|info| info.segments).unwrap_or(0)
                    } else {
                        state.timeshift.count(&group.id, &channel.id)
                    };
                    let mut value = json!({
                        "group_id": group.id,
                        "channel_id": channel.id,
                        "hours": hours,
                        "segments": segments,
                        "mode": group.mode,
                    });
                    if let Some(info) = info {
                        value["tracks"] = json!(info.tracks);
                        value["bytes"] = json!(info.bytes);
                        value["oldest_age_secs"] = json!(info.oldest_age_secs);
                        value["running"] = json!(info.running);
                        value["last_error"] = json!(info.last_error);
                        value["max_gb"] = json!(info.max_gb);
                        value["target_duration"] = json!(info.target_duration);
                    }
                    value
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let ready = channels
        .iter()
        .any(|channel| channel["segments"].as_u64().unwrap_or(0) > 0);
    Json(json!({
        "ok": true,
        "enabled": !channels.is_empty(),
        "ready": ready,
        "channels": channels,
        "note": "代理频道直接缓冲；DASH 频道按轨道录制 MPD 切片",
    }))
}

/// `GET /panel/api/health`——本机健康摘要。
pub async fn health(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let channels: usize = config.groups.iter().map(|group| group.channels.len()).sum();
    let (ips, banned) = state.ip_guard.summary();
    let unix_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    Json(json!({
        "ok": true,
        "version": state.version(),
        "uptime_secs": state.uptime_secs(),
        "time": unix_now,
        "bind": config.server.bind,
        "groups": config.groups.len(),
        "channels": channels,
        "sessions": state.sessions.len(),
        "cache": {
            "backend": config.cache.backend,
            "disk_path": config.cache.disk_path,
        },
        "epg": {
            "channels": state.epg.channels.load(std::sync::atomic::Ordering::Relaxed),
            "programmes": state.epg.programmes.load(std::sync::atomic::Ordering::Relaxed),
            "last_success": *state.epg.last_success.lock().unwrap_or_else(|e| e.into_inner()),
            "last_error": *state.epg.last_error.lock().unwrap_or_else(|e| e.into_inner()),
        },
        "security": { "ips": ips, "banned": banned },
        "share": { "enabled": config.share.enabled, "role": config.share.role },
    }))
}
