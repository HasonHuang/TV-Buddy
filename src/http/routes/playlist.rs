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
    // 先校验候选配置。失败返回 400，不能走到保存阶段变成 500。
    // 与 `mutate`/`store.save` 一致：先补默认值再校验，空字符串不算错误。
    let mut candidate = (*state.snapshot()).clone();
    candidate.playlist = cfg.clone();
    candidate.normalize();
    crate::config::validate(&candidate)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    // 全量替换同样遵守「进入托管后立即同步」：清掉旧计时。
    let previous: HashMap<String, String> = state
        .snapshot()
        .playlist
        .sources
        .iter()
        .map(|source| (source.id.clone(), source.mode.clone()))
        .collect();
    let entered_sync: Vec<String> = candidate
        .playlist
        .sources
        .iter()
        .filter(|source| source.mode == "sync")
        .filter(|source| previous.get(&source.id).map(String::as_str) != Some("sync"))
        .map(|source| source.id.clone())
        .collect();
    state
        .mutate(|config| {
            config.playlist = cfg.clone();
            Ok(())
        })
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    for id in entered_sync {
        state.sources.update_status(&id, |status| {
            status.last_try = 0;
            status.error = None;
        });
    }
    Ok(Json(json!({ "ok": true })))
}

/// `PUT /panel/api/playlist/settings`——只保存全局设置，保留源列表。
pub async fn update_settings(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(cfg): Json<PlaylistCfg>,
) -> Result<Json<Value>, ApiError> {
    if cfg.merge_position != "top" && cfg.merge_position != "bottom" {
        return Err(ApiError::bad_request("merge_position 只能是 top 或 bottom"));
    }
    // 先校验候选配置。失败返回 400，不能走到保存阶段变成 500。
    let mut candidate = (*state.snapshot()).clone();
    apply_global_settings(&mut candidate.playlist, &cfg);
    candidate.normalize();
    crate::config::validate(&candidate)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;

    state
        .mutate(|config| {
            // 只改全局字段。并发修改的其他字段（如源列表）保持不动。
            apply_global_settings(&mut config.playlist, &cfg);
            Ok(())
        })
        .map_err(map_internal)?;
    Ok(Json(json!({ "ok": true })))
}

/// 把全局同步与聚合设置写入目标。源列表不动。
fn apply_global_settings(target: &mut PlaylistCfg, cfg: &PlaylistCfg) {
    target.merge_ttl_secs = cfg.merge_ttl_secs.clamp(30, 86_400);
    target.merge_position = cfg.merge_position.clone();
    target.global_exclude = cfg.global_exclude.clone();
    target.sync_enabled = cfg.sync_enabled;
    target.sync_schedule = cfg.sync_schedule.clone();
    target.sync_interval_secs = cfg.sync_interval_secs;
    target.sync_daily_at = cfg.sync_daily_at.clone();
    target.sync_allow_empty = cfg.sync_allow_empty;
    target.sync_warn_removal_ratio = cfg.sync_warn_removal_ratio;
    target.sync_max_channels = cfg.sync_max_channels;
    target.sync_total_warn = cfg.sync_total_warn;
    target.sync_concurrency = cfg.sync_concurrency;
    target.sync_fetch_timeout_secs = cfg.sync_fetch_timeout_secs;
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
    candidate.normalize();
    crate::config::validate(&candidate)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    let source = candidate
        .playlist
        .sources
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .unwrap_or(source);
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
    candidate.normalize();
    crate::config::validate(&candidate)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    let source = candidate
        .playlist
        .sources
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .unwrap_or(source);

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
    // 首次进入托管：清掉旧计时，调度器下一轮立即补跑。
    if existing.mode != "sync" && source.mode == "sync" {
        state.sources.update_status(&id, |status| {
            status.last_try = 0;
            status.error = None;
        });
    }
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
    // 空字符串按默认值处理，与整份配置保存一致。
    source.normalize();
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
