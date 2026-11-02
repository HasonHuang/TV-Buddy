//! 订阅聚合配置：读写 `[playlist]`。
//!
//! 聚合清单本身由 `GET /playlist.m3u` 提供；本模块只管理源与过滤规则。
//! 单源增删改只改 `config.playlist`，不读取或回写整份 `config.groups`。

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde_json::{json, Value};

use crate::{
    config::model::{PlaylistCfg, PlaylistSource},
    http::{error::ApiError, ext::PanelAuth},
    state::AppState,
    subscription,
};

/// `GET /panel/api/playlist`。
pub async fn get(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!(state.snapshot().playlist))
}

/// `PUT /panel/api/playlist`——全量替换。
pub async fn update(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(mut cfg): Json<PlaylistCfg>,
) -> Result<Json<Value>, ApiError> {
    cfg.merge_ttl_secs = cfg.merge_ttl_secs.clamp(30, 86_400);
    if cfg.merge_position != "top" && cfg.merge_position != "bottom" {
        return Err(ApiError::bad_request("merge_position 只能是 top 或 bottom"));
    }
    for source in &mut cfg.sources {
        normalize_source(source)?;
    }
    state
        .mutate(|config| {
            config.playlist = cfg.clone();
            Ok(())
        })
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    Ok(Json(json!({ "ok": true })))
}

/// `PUT /panel/api/playlist/settings`——只保存全局设置，保留源列表。
pub async fn update_settings(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(mut cfg): Json<PlaylistCfg>,
) -> Result<Json<Value>, ApiError> {
    cfg.merge_ttl_secs = cfg.merge_ttl_secs.clamp(30, 86_400);
    if cfg.merge_position != "top" && cfg.merge_position != "bottom" {
        return Err(ApiError::bad_request("merge_position 只能是 top 或 bottom"));
    }
    cfg.sources.clear();
    state
        .mutate(|config| {
            config.playlist.merge_ttl_secs = cfg.merge_ttl_secs;
            config.playlist.merge_position = cfg.merge_position.clone();
            config.playlist.global_exclude = cfg.global_exclude.clone();
            config.playlist.sync_enabled = cfg.sync_enabled;
            config.playlist.sync_schedule = cfg.sync_schedule.clone();
            config.playlist.sync_interval_secs = cfg.sync_interval_secs;
            config.playlist.sync_daily_at = cfg.sync_daily_at.clone();
            config.playlist.sync_allow_empty = cfg.sync_allow_empty;
            config.playlist.sync_warn_removal_ratio = cfg.sync_warn_removal_ratio;
            config.playlist.sync_max_channels = cfg.sync_max_channels;
            config.playlist.sync_total_warn = cfg.sync_total_warn;
            config.playlist.sync_concurrency = cfg.sync_concurrency;
            config.playlist.sync_fetch_timeout_secs = cfg.sync_fetch_timeout_secs;
            Ok(())
        })
        .map_err(map_internal)?;
    Ok(Json(json!({ "ok": true })))
}

/// `POST /panel/api/playlist/sources`——新建一个源。
pub async fn create_source(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(mut source): Json<PlaylistSource>,
) -> Result<Json<Value>, ApiError> {
    if source.id.trim().is_empty() {
        source.id = uuid::Uuid::new_v4().to_string();
    }
    normalize_source(&mut source)?;
    let id = source.id.clone();
    let mut candidate = (*state.snapshot()).clone();
    if candidate.playlist.sources.iter().any(|item| item.id == id) {
        return Err(ApiError::bad_request(format!("订阅源 id 重复: {id}")));
    }
    candidate.playlist.sources.push(source.clone());
    crate::config::validate(&candidate)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    state
        .mutate(|config| {
            config.playlist.sources.push(source.clone());
            Ok(())
        })
        .map_err(map_internal)?;
    Ok(Json(json!({ "ok": true, "source": source })))
}

/// `PUT /panel/api/playlist/sources/:id`——保存一个源。
///
/// `?keep_content=1` 保留为手工；`0` 一并删除。源不再托管且已有内容时，
/// 缺少该参数返回 `409`，`code=content_exists`。
pub async fn update_source(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(mut source): Json<PlaylistSource>,
) -> Result<Json<Value>, ApiError> {
    source.id = id.clone();
    normalize_source(&mut source)?;
    let keep = params
        .get("keep_content")
        .map(|value| !(value == "0" || value == "false"));
    let snapshot = state.snapshot();
    let Some(existing) = snapshot
        .playlist
        .sources
        .iter()
        .find(|item| item.id == id)
        .cloned()
    else {
        return Err(ApiError::not_found("订阅源不存在"));
    };
    let content = subscription::content_count(&snapshot, &id);
    let leaves_sync = existing.mode == "sync" && source.mode != "sync";
    if leaves_sync && content > 0 && keep.is_none() {
        return Err(content_exists(content));
    }

    let mut candidate = (*snapshot).clone();
    if let Some(slot) = candidate
        .playlist
        .sources
        .iter_mut()
        .find(|item| item.id == id)
    {
        *slot = source.clone();
    }
    crate::config::validate(&candidate)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;

    let mut detached = 0;
    state
        .mutate(|config| {
            let Some(slot) = config
                .playlist
                .sources
                .iter_mut()
                .find(|item| item.id == id)
            else {
                anyhow::bail!("订阅源不存在");
            };
            *slot = source.clone();
            if leaves_sync && content > 0 {
                detached = if keep == Some(false) {
                    subscription::remove_source_content(config, &id)
                } else {
                    subscription::detach_source(config, &id)
                };
            }
            Ok(())
        })
        .map_err(map_internal)?;
    Ok(Json(json!({
        "ok": true,
        "source": source,
        "detached": detached,
    })))
}

/// `DELETE /panel/api/playlist/sources/:id`——删除源。
///
/// 默认保留内容为手工。`?keep_content=0` 一并删除。已有内容且未给参数时
/// 返回 `409`，`code=content_exists`。
pub async fn delete_source(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let keep = params
        .get("keep_content")
        .map(|value| !(value == "0" || value == "false"));
    let snapshot = state.snapshot();
    if !snapshot.playlist.sources.iter().any(|item| item.id == id) {
        return Err(ApiError::not_found("订阅源不存在"));
    }
    let content = subscription::content_count(&snapshot, &id);
    if content > 0 && keep.is_none() {
        return Err(content_exists(content));
    }
    let to_manual = keep.unwrap_or(true);
    let mut detached = 0;
    state
        .mutate(|config| {
            config.playlist.sources.retain(|item| item.id != id);
            if content > 0 {
                detached = if to_manual {
                    subscription::detach_source(config, &id)
                } else {
                    subscription::remove_source_content(config, &id)
                };
            }
            Ok(())
        })
        .map_err(map_internal)?;
    Ok(Json(json!({
        "ok": true,
        "channels": detached,
        "to_manual": to_manual,
    })))
}

fn content_exists(channels: usize) -> ApiError {
    ApiError::conflict_body(json!({
        "error": format!("该源已有 {channels} 个频道。请选择保留为手工或一并删除"),
        "code": "content_exists",
        "channels": channels,
    }))
}

fn normalize_source(source: &mut PlaylistSource) -> Result<(), ApiError> {
    source.name = source.name.trim().to_string();
    source.url = source.url.trim().to_string();
    if source.enabled && !(source.url.starts_with("http://") || source.url.starts_with("https://"))
    {
        return Err(ApiError::bad_request(format!(
            "源 {} 的 url 必须是 HTTP(S)",
            source.name
        )));
    }
    if source.id.trim().is_empty() {
        source.id = uuid::Uuid::new_v4().to_string();
    }
    if source.mode == "sync" && source.playback_mode == "passthrough" {
        return Err(ApiError::bad_request("托管源禁止 passthrough"));
    }
    Ok(())
}

fn map_internal(error: anyhow::Error) -> ApiError {
    ApiError::internal(format!("{error:#}"))
}
