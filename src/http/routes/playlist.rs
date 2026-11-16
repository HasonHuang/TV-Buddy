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
        .mutate_admission_full(&[], |config| {
            let delete_keys = if leaves_sync {
                subscription::source_result_keys(config, &id)
            } else {
                Vec::new()
            };
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
            Ok((Vec::new(), delete_keys))
        })
        .map_err(map_internal)?;
    // 切换模式后，旧状态只属于旧模式。清空计数与提醒，避免聚合行显示托管状态。
    if existing.mode != source.mode {
        let mode = source.mode.clone();
        state.sources.update_status(&id, |status| {
            *status = crate::subs::SourceStatus {
                mode,
                ..Default::default()
            };
        });
        // 首次进入托管：清掉旧计时，调度器下一轮立即补跑。
        if source.mode == "sync" {
            state
                .sources
                .update_status(&id, |status| status.last_try = 0);
        }
    }
    Ok(Json(json!({
        "ok": true,
        "source": source,
        "detached": detached,
    })))
}

/// `POST /panel/api/playlist/sources/:id/disable`——停用源。
pub async fn disable_source(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let keep_channels = body
        .get("keep_channels")
        .and_then(Value::as_bool)
        .ok_or_else(|| ApiError::bad_request("keep_channels 必须是布尔值"))?;
    let snapshot = state.snapshot();
    if !snapshot
        .playlist
        .sources
        .iter()
        .any(|source| source.id == id)
    {
        return Err(ApiError::not_found("订阅源不存在"));
    }
    let removed: Vec<(String, String, String, String, String, String)> = snapshot
        .groups
        .iter()
        .flat_map(|group| {
            group
                .channels
                .iter()
                .filter(|channel| channel.source_id == id)
                .map(|channel| {
                    (
                        group.id.clone(),
                        group.name.clone(),
                        channel.id.clone(),
                        channel.name.clone(),
                        channel.source_key.clone(),
                        crate::quality::admission::url_hash_hex(&channel.url),
                    )
                })
        })
        .collect();
    let results: Vec<_> = if keep_channels {
        Vec::new()
    } else {
        removed
            .iter()
            .map(|(gid, group_name, cid, name, source_key, url_hash)| {
                crate::quality::store::QualityResult {
                    key: format!("sync:{id}:{source_key}"),
                    scope: "sync".into(),
                    source_id: id.clone(),
                    source_key: source_key.clone(),
                    gid: gid.clone(),
                    cid: cid.clone(),
                    name: name.clone(),
                    target_group: group_name.clone(),
                    url_hash: url_hash.clone(),
                    action: "removed".into(),
                    reason: "source_disabled".into(),
                    ok: false,
                    checked_at: crate::schedule::now_epoch(),
                    ..Default::default()
                }
            })
            .collect()
    };
    state
        .mutate_admission(&[], |config| {
            let source = config
                .playlist
                .sources
                .iter_mut()
                .find(|source| source.id == id)
                .ok_or_else(|| anyhow::anyhow!("订阅源不存在"))?;
            source.enabled = false;
            if !keep_channels {
                subscription::remove_source_content(config, &id);
            }
            Ok(results)
        })
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    if !keep_channels {
        for (gid, _, cid, _, _, _) in removed {
            state.quality.forget(&gid, &cid);
        }
    }
    state
        .sources
        .update_status(&id, |status| clear_gate(status));
    Ok(Json(json!({ "ok": true, "keep_channels": keep_channels })))
}

/// `POST /panel/api/playlist/sources/:id/enable`——启用源，sync 源会排入同步。
pub async fn enable_source(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let snapshot = state.snapshot();
    let source = snapshot
        .playlist
        .sources
        .iter()
        .find(|source| source.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("订阅源不存在"))?;
    state
        .mutate(|config| {
            let source = config
                .playlist
                .sources
                .iter_mut()
                .find(|source| source.id == id)
                .ok_or_else(|| anyhow::anyhow!("订阅源不存在"))?;
            source.enabled = true;
            Ok(())
        })
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    let queued = source.mode == "sync";
    if queued {
        let state = state.clone();
        let id = id.clone();
        tokio::spawn(async move {
            if let Err(error) = subscription::sync_source(&state, &id, false).await {
                tracing::warn!(source = %id, %error, "启用源后同步失败");
            }
        });
    }
    Ok(Json(json!({ "ok": true, "queued": queued })))
}

fn clear_gate(status: &mut crate::subs::SourceStatus) {
    status.gate_phase.clear();
    status.gate_cursor.clear();
    status.gate_remaining = 0;
    status.gate_paused = false;
    status.gate_pause_reason.clear();
    status.gate_truncated = false;
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
        .mutate_admission_full(&[], |config| {
            let now = crate::schedule::now_epoch();
            let channels: Vec<(String, String, String, String, String, String)> = config
                .groups
                .iter()
                .flat_map(|group| {
                    group
                        .channels
                        .iter()
                        .filter(|channel| channel.source_id == id)
                        .map(|channel| {
                            (
                                group.id.clone(),
                                group.name.clone(),
                                channel.id.clone(),
                                channel.name.clone(),
                                channel.source_key.clone(),
                                crate::quality::admission::url_hash_hex(&channel.url),
                            )
                        })
                })
                .collect();
            config.playlist.sources.retain(|item| item.id != id);
            if content > 0 {
                detached = if to_manual {
                    subscription::detach_source(config, &id)
                } else {
                    subscription::remove_source_content(config, &id)
                };
            }
            let mut results = Vec::new();
            let mut delete_keys = Vec::new();
            for (gid, group_name, cid, name, source_key, url_hash) in channels {
                let sync_key = format!("sync:{id}:{source_key}");
                if to_manual {
                    delete_keys.push(sync_key);
                    results.push(crate::quality::store::QualityResult {
                        key: format!("channel:{gid}/{cid}"),
                        scope: "manual".into(),
                        gid,
                        cid,
                        name,
                        target_group: group_name,
                        url_hash,
                        action: "kept".into(),
                        reason: "source_removed".into(),
                        ok: true,
                        checked_at: now,
                        ..Default::default()
                    });
                } else {
                    results.push(crate::quality::store::QualityResult {
                        key: sync_key,
                        scope: "sync".into(),
                        source_id: id.clone(),
                        source_key,
                        gid,
                        cid,
                        name,
                        target_group: group_name,
                        url_hash,
                        action: "source_removed".into(),
                        reason: "source_removed".into(),
                        ok: false,
                        checked_at: now,
                        ..Default::default()
                    });
                }
            }
            Ok((results, delete_keys))
        })
        .map_err(map_internal)?;
    state
        .sources
        .update_status(&id, |status| clear_gate(status));
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
