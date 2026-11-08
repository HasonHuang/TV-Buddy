//! 订阅源端点：状态、分组、预检、同步与内容清理。
//!
//! 证据：S-GAP-07。响应形状自定，字段保留目标的 `trace` 结构。
//! 所有端点都需要管理员会话；只接受 `http`/`https` 源地址。

use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};

use crate::{
    http::{error::ApiError, ext::PanelAuth},
    m3u,
    state::AppState,
    subs::{self, PreviewSource},
    subscription,
};

/// `GET /subs/status`——列出源最近一次抓取与同步状态。需要管理员会话。
pub async fn status(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    status_body(state).await
}

async fn status_body(state: Arc<AppState>) -> Json<Value> {
    let config = state.snapshot();
    let statuses = state.sources.statuses();
    let sources: Vec<Value> = config
        .playlist
        .sources
        .iter()
        .map(|source| {
            let status = subscription::status_of(&statuses, &source.id, &source.mode);
            let next_sync = if source.mode == "sync" {
                subscription::next_run_at(
                    &config.playlist,
                    source,
                    status.last_try,
                    config.server.timezone_offset_hours,
                )
            } else {
                0
            };
            json!({
                "id": source.id,
                "name": source.name,
                "url": source.url,
                "enabled": source.enabled,
                "mode": status.mode,
                "error": status.error,
                "channels": status.channels,
                "groups": status.groups,
                "last_ok": status.last_ok,
                "last_try": status.last_try,
                "last_sync": status.last_sync,
                "added": status.added,
                "updated": status.updated,
                "removed": status.removed,
                "skipped_removals": status.skipped_removals,
                "needs_review": status.needs_review,
                "zero_blocked": status.zero_blocked,
                "shrink_permille": status.shrink_permille,
                "duration_ms": status.duration_ms,
                "syncing": status.syncing,
                "next_sync": next_sync,
                "ttl": crate::subs::effective_ttl_secs(source.ttl_secs),
            })
        })
        .collect();
    Json(json!({
        "sources": sources,
        "sync": {
            "enabled": config.playlist.sync_enabled,
            "schedule": config.playlist.sync_schedule,
            "interval_secs": config.playlist.sync_interval_secs,
            "daily_at": config.playlist.sync_daily_at,
        },
    }))
}

/// `POST /subs/groups`——抓取源并列出分组。需要管理员会话。
pub async fn groups(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    groups_body(state, body).await
}

async fn groups_body(state: Arc<AppState>, body: Value) -> Result<Json<Value>, ApiError> {
    let config = state.snapshot();
    let url = body
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| ApiError::bad_request("缺少 url"))?;
    validate_url(url)?;
    let configured = body
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| config.playlist.sources.iter().find(|item| item.id == id));
    // 草稿可带请求头与代理。读取分组要和同步使用同一套抓取参数。
    let mut draft = configured.cloned().unwrap_or_default();
    draft.url = url.to_string();
    if let Some(headers) = body.get("headers") {
        draft.headers = headers
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
    }
    if let Some(proxy) = body.get("proxy") {
        if let Ok(parsed) =
            serde_json::from_value::<Option<crate::config::model::ProxyCfg>>(proxy.clone())
        {
            draft.proxy = parsed;
        }
    }
    if let Some(mode) = body.get("mode").and_then(Value::as_str) {
        draft.mode = mode.to_string();
    }
    draft.normalize();
    let text = fetch_for(&state, &draft, &config.playlist).await?;
    let entries = m3u::parse(&text);
    let total = entries.len();
    let groups: Vec<Value> = subs::group_counts(&entries)
        .into_iter()
        .map(|count| json!({ "name": count.name, "count": count.count }))
        .collect();
    Ok(Json(json!({ "groups": groups, "total": total })))
}

/// `POST /subs/preview`——按映射与过滤规则预检源。需要管理员会话。
pub async fn preview(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    preview_body(state, body).await
}

async fn preview_body(state: Arc<AppState>, body: Value) -> Result<Json<Value>, ApiError> {
    let input = body.get("source").cloned().unwrap_or_else(|| body.clone());
    let config = state.snapshot();

    // `id` 命中已配置源时，用配置补齐空字段。
    let configured = input
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| config.playlist.sources.iter().find(|item| item.id == id));

    let url = input
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .or_else(|| configured.map(|source| source.url.trim()))
        .filter(|url| !url.is_empty())
        .ok_or_else(|| ApiError::bad_request("缺少 url"))?;
    validate_url(url)?;

    let mut source = configured
        .map(|configured| PreviewSource {
            group_map: configured
                .group_map
                .iter()
                .map(|(from, to)| (from.clone(), to.clone()))
                .collect(),
            group_mode: configured.group_mode.clone(),
            merge_group: configured.merge_group.clone(),
            merge_into: configured.merge_into.clone(),
            merge_new: configured.merge_new.clone(),
            unlisted_groups: configured.unlisted_groups.clone(),
            include: configured.include.clone(),
            exclude: configured.exclude.clone(),
        })
        .unwrap_or_else(|| PreviewSource {
            group_mode: "keep".into(),
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        });

    if let Some(value) = input.get("unlisted_groups").and_then(Value::as_str) {
        source.unlisted_groups = value.to_string();
    }
    if let Some(value) = input.get("group_mode").and_then(Value::as_str) {
        source.group_mode = value.to_string();
    }
    if let Some(value) = input.get("merge_group").and_then(Value::as_str) {
        source.merge_group = value.to_string();
    }
    if input.get("include").is_some() {
        source.include = strings(&input, "include");
    }
    if input.get("exclude").is_some() {
        source.exclude = strings(&input, "exclude");
    }
    if let Some(map) = input.get("group_map") {
        let map = map
            .as_object()
            .ok_or_else(|| ApiError::bad_request("source.group_map 必须是对象"))?;
        source.group_map.clear();
        for (from, value) in map {
            let to = value
                .as_str()
                .ok_or_else(|| ApiError::bad_request("source.group_map 的值必须是字符串"))?;
            source.group_map.insert(from.clone(), to.to_string());
        }
    }
    if let Some(map) = input.get("merge_into") {
        let map = map
            .as_object()
            .ok_or_else(|| ApiError::bad_request("source.merge_into 必须是对象"))?;
        source.merge_into.clear();
        for (from, value) in map {
            let to = value
                .as_str()
                .ok_or_else(|| ApiError::bad_request("source.merge_into 的值必须是字符串"))?;
            source.merge_into.insert(from.clone(), to.to_string());
        }
    }
    if let Some(map) = input.get("merge_new") {
        let map = map
            .as_object()
            .ok_or_else(|| ApiError::bad_request("source.merge_new 必须是对象"))?;
        source.merge_new.clear();
        for (from, value) in map {
            let to = value
                .as_str()
                .ok_or_else(|| ApiError::bad_request("source.merge_new 的值必须是字符串"))?;
            source.merge_new.insert(from.clone(), to.to_string());
        }
    }

    // 与保存时一致：空值按 keep；空映射、非 map 模式的 drop 归一为 keep。
    source.normalize();

    // 托管源使用草稿源抓取，应用请求头、代理与超时。聚合源沿用轻量抓取。
    let mut candidate = configured.cloned().unwrap_or_default();
    if let Some(object) = input.as_object() {
        let mut base = serde_json::to_value(&candidate).unwrap_or_else(|_| json!({}));
        if let Some(map) = base.as_object_mut() {
            for (key, value) in object {
                if key != "id" {
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        candidate = serde_json::from_value(base).unwrap_or(candidate);
    }
    candidate.url = url.to_string();
    candidate.group_map = source.group_map.clone();
    candidate.group_mode = source.group_mode.clone();
    candidate.merge_group = source.merge_group.clone();
    candidate.merge_into = source.merge_into.clone();
    candidate.merge_new = source.merge_new.clone();
    candidate.unlisted_groups = source.unlisted_groups.clone();
    candidate.include = source.include.clone();
    candidate.exclude = source.exclude.clone();
    candidate.normalize();

    // 聚合输出不读取显式目标。预检与聚合清单保持一致。
    if candidate.mode != "sync" {
        source.merge_into.clear();
        source.merge_new.clear();
        candidate.merge_into.clear();
        candidate.merge_new.clear();
    }

    // 与「读取分组」同一套抓取参数：请求头、代理、按模式选超时。
    let text = fetch_for(&state, &candidate, &config.playlist).await?;
    let entries = m3u::parse(&text);
    let result = subs::preview(
        &entries,
        &source,
        &config.playlist.global_exclude,
        SAMPLE_LIMIT,
    );

    let sample: Vec<Value> = result
        .sample
        .iter()
        .map(|item| {
            json!({
                "name": item.name,
                "group": item.group,
                "tvg_id": item.tvg_id,
            })
        })
        .collect();
    let out_groups: Vec<Value> = result
        .out_groups
        .iter()
        .map(|count| json!({ "name": count.name, "count": count.count }))
        .collect();

    // 托管源：用同一份抓取结果给出同步差异。不写配置，不写状态。
    let sync = if candidate.mode == "sync" {
        let plan = subscription::plan(&config, &candidate, &entries);
        let diff = subscription::diff(&config, &candidate, &plan);
        let max_channels = if candidate.max_channels > 0 {
            candidate.max_channels
        } else {
            config.playlist.sync_max_channels
        };
        let existing = subscription::content_count(&config, &candidate.id);
        Some(json!({
            "add": diff.add,
            "update": diff.update,
            "remove": diff.remove,
            "groups_add": diff.groups_add,
            "groups_remove": diff.groups_remove,
            "groups_rename": diff.groups_rename,
            "skipped_duplicates": diff.skipped_duplicates,
            "guard": {
                "over_limit": max_channels > 0 && plan.channels.len() as u64 > max_channels,
                "empty": plan.channels.is_empty()
                    && existing > 0
                    && !config.playlist.sync_allow_empty,
                "max_channels": max_channels,
                "allow_empty": config.playlist.sync_allow_empty,
            },
        }))
    } else {
        None
    };

    Ok(Json(json!({
        "kept": result.kept,
        "sample": sample,
        "sync": sync,
        "trace": {
            "total": result.trace.total,
            "after_group": result.trace.after_group,
            "after_global_exclude": result.trace.after_global_exclude,
            "after_include": result.trace.after_include,
            "after_exclude": result.trace.after_exclude,
            "dropped_groups": result.dropped_groups,
            "out_groups": out_groups,
        },
    })))
}

/// `GET /panel/api/subs/status`——目标面板路径，需要会话。
pub async fn panel_status(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    status_body(state).await
}

/// `POST /panel/api/subs/groups`——目标面板路径，需要会话。
pub async fn panel_groups(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    groups_body(state, body).await
}

/// `POST /panel/api/subs/preview`——目标面板路径，需要会话。
pub async fn panel_preview(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    preview_body(state, body).await
}

/// `POST /panel/api/subs/sync`——异步触发同步。返回 202。
///
/// `{ "id": "可选", "force": false }`。省略 `id` 时同步全部启用的托管源。
pub async fn panel_sync(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let force = body.get("force").and_then(Value::as_bool).unwrap_or(false);
    let requested = body
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let config = state.snapshot();
    let ids: Vec<String> = match requested {
        Some(id) => {
            let Some(source) = config
                .playlist
                .sources
                .iter()
                .find(|source| source.id == id)
            else {
                return Err(ApiError::not_found("订阅源不存在"));
            };
            if source.mode != "sync" {
                return Err(ApiError::bad_request("该源不是托管模式"));
            }
            vec![id]
        }
        None => config
            .playlist
            .sources
            .iter()
            .filter(|source| source.mode == "sync" && source.enabled)
            .map(|source| source.id.clone())
            .collect(),
    };
    if ids.is_empty() {
        return Err(ApiError::bad_request("没有可同步的托管源"));
    }
    throttle(&state, &ids)?;

    let guard = state
        .sync_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| ApiError::conflict_code("sync_running", "已有同步任务在运行"))?;
    let queued = ids.len();
    let st = state.clone();
    tokio::spawn(async move {
        let _guard = guard;
        for id in ids {
            if let Err(error) = subscription::sync_locked(&st, &id, force).await {
                tracing::warn!(source = %id, %error, "手动同步失败");
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "ok": true, "queued": queued })),
    ))
}

/// `DELETE /panel/api/subs/:id/content?to_manual=1`——清理物化内容。
///
/// 默认转为手工；`to_manual=0` 删除。
pub async fn panel_content(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let to_manual = params
        .get("to_manual")
        .map(|value| value != "0" && value != "false")
        .unwrap_or(true);
    let mut count = 0;
    state
        .mutate(|config| {
            count = if to_manual {
                subscription::detach_source(config, &id)
            } else {
                subscription::remove_source_content(config, &id)
            };
            Ok(())
        })
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    tracing::info!(source = %id, channels = count, to_manual, "订阅内容已清理");
    Ok(Json(json!({
        "ok": true,
        "channels": count,
        "to_manual": to_manual,
    })))
}

/// 单源 10 秒节流。
fn throttle(state: &AppState, ids: &[String]) -> Result<(), ApiError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let mut triggers = state
        .sync_triggers
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    triggers.retain(|_, at| now.saturating_sub(*at) < 3600);
    for id in ids {
        if let Some(at) = triggers.get(id) {
            let elapsed = now.saturating_sub(*at);
            if elapsed < subscription::MANUAL_MIN_INTERVAL_SECS {
                return Err(ApiError::too_many(json!({
                    "error": "同步过于频繁，请稍后再试",
                    "retry_after": subscription::MANUAL_MIN_INTERVAL_SECS - elapsed,
                })));
            }
        }
    }
    for id in ids {
        triggers.insert(id.clone(), now);
    }
    Ok(())
}

/// 抽样条数上限。
const SAMPLE_LIMIT: usize = 5;

fn strings(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn validate_url(url: &str) -> Result<(), ApiError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(ApiError::bad_request("url 必须是 http(s) 地址"))
    }
}

/// 按源模式抓取：托管源用同步超时，聚合源用固定短超时。
///
/// 请求头与代理都生效。「读取分组」与「预检」共用，避免同一源两个按钮
/// 拿到不同结果。
async fn fetch_for(
    state: &Arc<AppState>,
    source: &crate::config::model::PlaylistSource,
    playlist: &crate::config::model::PlaylistCfg,
) -> Result<String, ApiError> {
    let result = if source.mode == "sync" {
        subscription::fetch(state, source, playlist).await
    } else {
        subscription::fetch_bounded(
            state,
            source,
            Duration::from_secs(subscription::AGGREGATE_FETCH_TIMEOUT_SECS),
            subscription::MAX_BODY_BYTES,
        )
        .await
    };
    result.map_err(|error| ApiError::bad_gateway(format!("{error:#}")))
}
