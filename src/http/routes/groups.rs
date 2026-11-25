//! 分组与频道接口。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../../../docs/design/03-http-contract.md) §6.2。
//! 局部更新语义：把请求体字段合并到当前对象，再整体校验。

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

use crate::{
    config::model::{Channel, Group},
    http::{error::ApiError, ext::PanelAuth},
    m3u,
    state::AppState,
    subscription,
};

/// 把补丁对象的字段合并到基础对象。
fn merge_into<T>(base: &T, patch: &Value) -> Result<T, ApiError>
where
    T: Serialize + DeserializeOwned,
{
    let mut value =
        serde_json::to_value(base).map_err(|err| ApiError::internal(err.to_string()))?;
    match (&mut value, patch) {
        (Value::Object(map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                map.insert(key.clone(), patch_value.clone());
            }
        }
        _ => return Err(ApiError::bad_request("请求体必须是 JSON 对象")),
    }
    serde_json::from_value(value).map_err(|err| ApiError::bad_request(format!("字段不合法: {err}")))
}

fn validate_channel_mode(group: &Group, channel: &Channel) -> Result<(), ApiError> {
    if let Some(mode) = channel.mode.as_deref() {
        if !crate::config::model::valid_mode(mode) {
            return Err(ApiError::bad_request(format!("不支持的频道模式: {mode}")));
        }
        if mode == "passthrough" && !channel.source_id.is_empty() {
            return Err(ApiError::bad_request("同步频道不能使用 passthrough"));
        }
    }
    if !channel.source_id.is_empty() && channel.mode.is_none() && group.mode == "passthrough" {
        return Err(ApiError::bad_request(
            "分组使用 passthrough 时，同步频道必须设置显式模式",
        ));
    }
    Ok(())
}

fn validate_group_mode(group: &Group) -> Result<(), ApiError> {
    if !crate::config::model::valid_mode(&group.mode) {
        return Err(ApiError::bad_request(format!(
            "不支持的分组模式: {}",
            group.mode
        )));
    }
    if group.mode == "passthrough"
        && group
            .channels
            .iter()
            .any(|channel| !channel.source_id.is_empty() && channel.mode.is_none())
    {
        return Err(ApiError::bad_request(
            "组内有未设置模式的同步频道，不能使用 passthrough",
        ));
    }
    Ok(())
}

/// 校验分组请求体内嵌的所有频道。
fn validate_group_channels(group: &Group) -> Result<(), ApiError> {
    let mut seen = std::collections::HashSet::new();
    for channel in &group.channels {
        if channel.id.trim().is_empty() {
            return Err(ApiError::bad_request("频道 id 不能为空"));
        }
        if !seen.insert(&channel.id) {
            return Err(ApiError::bad_request(format!(
                "频道 id 重复: {}",
                channel.id
            )));
        }
        validate_channel_mode(group, channel)?;
    }
    Ok(())
}

/// 客户端不得写入来源标记。已存在频道恢复旧标记；新频道按是否显式设置模式标记。
fn restore_mode_source(channel: &mut Channel, old: Option<&Channel>) {
    match old {
        Some(old) => channel.set_mode_source(old.mode_source()),
        None => {
            channel.clear_mode_source();
            if channel.mode.is_some() {
                channel.set_mode_source(crate::config::model::MODE_SOURCE_MANUAL);
            }
        }
    }
}

/// 分组接口整组保存频道时，把真正改过的模式与固定频道标成手工。
fn mark_group_channels_manual(old: &Group, updated: &mut Group) {
    for channel in &mut updated.channels {
        let old_channel = old.channels.iter().find(|old| old.id == channel.id);
        restore_mode_source(channel, old_channel);
        if let Some(old_channel) = old_channel {
            if old_channel.mode != channel.mode
                || old_channel.url != channel.url
                || (old_channel.sync_follow && !channel.sync_follow)
            {
                channel.set_mode_source(crate::config::model::MODE_SOURCE_MANUAL);
            }
        }
    }
}

fn body_id(body: &Value) -> Result<String, ApiError> {
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(ApiError::bad_request("id 不能为空"));
    }
    Ok(id)
}

/// `POST /api/panel/groups`。id 重复 → `409`（F-013 §5）。
pub async fn create_group(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let id = body_id(&body)?;
    if state.snapshot().group(&id).is_some() {
        return Err(ApiError::conflict("Group id already exists"));
    }
    let mut group: Group = merge_into(&Group::default(), &body)?;
    validate_group_mode(&group)?;
    validate_group_channels(&group)?;
    for channel in &mut group.channels {
        restore_mode_source(channel, None);
    }

    state
        .mutate(|config| {
            config.groups.push(group);
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    tracing::info!(group = %id, "新建分组");
    Ok(Json(json!({ "ok": true })))
}

/// `PUT /api/panel/groups/{gid}`——局部更新。
pub async fn update_group(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(gid): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let current = state.snapshot();
    let group = current
        .group(&gid)
        .ok_or_else(|| ApiError::not_found("Group not found"))?;
    let mut updated: Group = merge_into(group, &body)?;
    validate_group_mode(&updated)?;
    validate_group_channels(&updated)?;
    mark_group_channels_manual(group, &mut updated);
    let offline_changed = group.offline_on_fail != updated.offline_on_fail;
    let group_playback_changed = group.mode != updated.mode || group.format != updated.format;
    let old_channels = group.channels.clone();
    let new_channels = updated.channels.clone();

    state
        .mutate(|config| {
            if let Some(slot) = config.group_mut(&gid) {
                *slot = updated;
            } else {
                anyhow::bail!("Group not found");
            }
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    if offline_changed || group_playback_changed {
        state.health.forget_group(&gid);
        state.quality.forget_group(&gid);
    }
    // 整组替换可能删除或改写频道。旧探测结果不再代表新配置。
    for old_channel in &old_channels {
        let changed = new_channels
            .iter()
            .find(|channel| channel.id == old_channel.id)
            .is_none_or(|channel| {
                old_channel.url != channel.url
                    || old_channel.mode != channel.mode
                    || crate::resolver::Setting::from_channel(old_channel)
                        != crate::resolver::Setting::from_channel(channel)
            });
        if changed {
            state.health.forget(&gid, &old_channel.id);
            state.quality.forget(&gid, &old_channel.id);
            forget_quality_results(&state, &gid, old_channel);
        }
    }
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/panel/groups/{gid}`。
pub async fn delete_group(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(gid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let group = state.snapshot().group(&gid).cloned();
    let Some(group) = group else {
        return Err(ApiError::not_found("Group not found"));
    };
    state
        .mutate(|config| {
            // 同步分组删除：把现有频道的键写入源排除。不写 `group:`，
            // 避免同组的新频道被永久排除。
            subscription::exclude_channels(config, &group.channels);
            config.groups.retain(|item| item.id != gid);
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    state.health.forget_group(&gid);
    state.quality.forget_group(&gid);
    for channel in &group.channels {
        forget_quality_results(&state, &gid, channel);
    }
    tracing::info!(group = %gid, "删除分组");
    Ok(Json(json!({ "ok": true })))
}

/// 删除频道持久化结果，避免重建同 id 命中旧失败状态。
fn forget_quality_results(state: &AppState, gid: &str, channel: &Channel) {
    let _ = state
        .quality_store
        .delete(&format!("channel:{gid}/{}", channel.id));
    if !channel.source_id.is_empty() {
        let _ = state.quality_store.delete(&format!(
            "sync:{}:{}",
            channel.source_id, channel.source_key
        ));
    }
}

/// `GET /api/panel/groups/export`——顶层数组（F-013 §1）。
pub async fn export_all(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    Json(serde_json::to_value(&config.groups).unwrap_or(Value::Array(Vec::new())))
}

/// `GET /api/panel/groups/{gid}/export`——单个分组对象。
pub async fn export_group(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(gid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let config = state.snapshot();
    let group = config
        .group(&gid)
        .ok_or_else(|| ApiError::not_found("Group not found"))?;
    Ok(Json(serde_json::to_value(group).unwrap_or(Value::Null)))
}

/// `POST /api/panel/groups/{gid}/channels`。
pub async fn create_channel(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(gid): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let id = body_id(&body)?;
    let current = state.snapshot();
    let group = current
        .group(&gid)
        .ok_or_else(|| ApiError::not_found("Group not found"))?;
    if group.channels.iter().any(|channel| channel.id == id) {
        return Err(ApiError::conflict("Channel id already exists"));
    }
    let mut channel: Channel = merge_into(&Channel::default(), &body)?;
    validate_channel_mode(group, &channel)?;
    if body.get("mode").is_some() {
        channel.set_mode_source(crate::config::model::MODE_SOURCE_MANUAL);
    } else {
        channel.clear_mode_source();
    }

    state
        .mutate(|config| {
            let group = config
                .group_mut(&gid)
                .ok_or_else(|| anyhow::anyhow!("Group not found"))?;
            group.channels.push(channel);
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    state.health.forget(&gid, &id);
    state.quality.forget(&gid, &id);
    tracing::info!(group = %gid, channel = %id, "新建频道");
    Ok(Json(json!({ "ok": true })))
}

/// `PUT /api/panel/groups/{gid}/channels/{cid}`——局部更新。
pub async fn update_channel(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let current = state.snapshot();
    let group = current
        .group(&gid)
        .ok_or_else(|| ApiError::not_found("Group not found"))?;
    let channel = group
        .channels
        .iter()
        .find(|channel| channel.id == cid)
        .ok_or_else(|| ApiError::not_found("Channel not found"))?;
    let mut updated: Channel = merge_into(channel, &body)?;
    // 来源标记由服务端维护。客户端带来的值一律丢弃。
    updated.set_mode_source(channel.mode_source());
    validate_channel_mode(group, &updated)?;
    if body.get("mode").is_some() {
        updated.set_mode_source(crate::config::model::MODE_SOURCE_MANUAL);
    }
    let upstream_changed = subscription::upstream_differs(channel, &updated);
    // 改上游字段后固定。请求体显式带 `sync_follow` 时以请求为准。
    if !channel.source_id.is_empty() && body.get("sync_follow").is_none() && upstream_changed {
        updated.sync_follow = false;
        updated.set_mode_source(crate::config::model::MODE_SOURCE_MANUAL);
    }
    let offline_changed = channel.offline_on_fail != updated.offline_on_fail;
    // 解析脚本或上游地址变化后，旧探测结果不再代表这个频道。
    let resolver_changed = crate::resolver::Setting::from_channel(channel)
        != crate::resolver::Setting::from_channel(&updated);

    state
        .mutate(|config| {
            let group = config
                .group_mut(&gid)
                .ok_or_else(|| anyhow::anyhow!("Group not found"))?;
            let slot = group
                .channels
                .iter_mut()
                .find(|channel| channel.id == cid)
                .ok_or_else(|| anyhow::anyhow!("Channel not found"))?;
            *slot = updated;
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    if offline_changed || upstream_changed || resolver_changed {
        state.health.forget(&gid, &cid);
        state.quality.forget(&gid, &cid);
    }
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/panel/groups/{gid}/channels/{cid}`。
pub async fn delete_channel(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let channel = state
        .snapshot()
        .group(&gid)
        .and_then(|group| group.channels.iter().find(|channel| channel.id == cid))
        .cloned();
    let Some(channel) = channel else {
        return Err(ApiError::not_found("Channel not found"));
    };
    state
        .mutate(|config| {
            // 删除同步频道时写入源排除，防止下次同步回流。
            subscription::exclude_channel(config, &channel);
            let group = config
                .group_mut(&gid)
                .ok_or_else(|| anyhow::anyhow!("Group not found"))?;
            group.channels.retain(|channel| channel.id != cid);
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    state.health.forget(&gid, &cid);
    state.quality.forget(&gid, &cid);
    forget_quality_results(&state, &gid, &channel);
    tracing::info!(group = %gid, channel = %cid, "删除频道");
    Ok(Json(json!({ "ok": true })))
}

/// `POST /api/panel/groups/:gid/detach`——分组转为手工。
pub async fn detach_group(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(gid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut found = false;
    state
        .mutate_admission_full(&[], |config| {
            let delete_keys = subscription::group_result_keys(config, &gid);
            found = subscription::detach_group(config, &gid);
            Ok((Vec::new(), delete_keys))
        })
        .map_err(|err| ApiError::internal(format!("{err:#}")))?;
    if !found {
        return Err(ApiError::not_found("Group not found"));
    }
    tracing::info!(group = %gid, "分组转为手工");
    Ok(Json(json!({ "ok": true })))
}

/// `POST /api/panel/groups/:gid/channels/:cid/detach`——频道转为手工。
pub async fn detach_channel(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let mut found = false;
    state
        .mutate_admission_full(&[], |config| {
            let delete_keys = subscription::channel_result_keys(config, &gid, &cid);
            found = subscription::detach_channel(config, &gid, &cid);
            Ok((Vec::new(), delete_keys))
        })
        .map_err(|err| ApiError::internal(format!("{err:#}")))?;
    if !found {
        return Err(ApiError::not_found("Channel not found"));
    }
    tracing::info!(group = %gid, channel = %cid, "频道转为手工");
    Ok(Json(json!({ "ok": true })))
}

/// 导入分组：`force = false` 跳过已存在的 id，`true` 覆盖。
fn import(
    state: Arc<AppState>,
    body: Value,
    force: bool,
    include_synced: bool,
) -> Result<Json<Value>, ApiError> {
    let items = body
        .as_array()
        .ok_or_else(|| ApiError::bad_request("请求体必须是分组数组"))?;
    let mut groups: Vec<Group> = Vec::with_capacity(items.len());
    for item in items {
        let group: Group = serde_json::from_value(item.clone())
            .map_err(|err| ApiError::bad_request(format!("分组字段不合法: {err}")))?;
        if group.id.trim().is_empty() || group.name.trim().is_empty() {
            return Err(ApiError::bad_request("分组 id 与名称不能为空"));
        }
        groups.push(group);
    }

    // 先判断哪些分组会被跳过，再校验。被跳过的旧同步分组不应让整批导入失败。
    let snapshot = state.snapshot();
    let mut to_import: Vec<Group> = Vec::with_capacity(groups.len());
    let mut skipped = 0usize;
    for mut group in groups {
        let existing = snapshot.group(&group.id);
        let skipped_synced = !include_synced && !group.source_id.is_empty();
        let skipped_duplicate = match existing {
            Some(_) if !force => true,
            Some(current) if !include_synced && !current.source_id.is_empty() => true,
            _ => false,
        };
        if skipped_synced || skipped_duplicate {
            skipped += 1;
            continue;
        }
        validate_group_mode(&group)?;
        validate_group_channels(&group)?;
        match existing {
            Some(current) => mark_group_channels_manual(current, &mut group),
            None => {
                for channel in &mut group.channels {
                    restore_mode_source(channel, None);
                }
            }
        }
        to_import.push(group);
    }

    let mut imported = 0usize;
    state
        .mutate_admission_full(&[], |config| {
            let mut delete_keys = Vec::new();
            for group in &to_import {
                match config
                    .groups
                    .iter()
                    .position(|existing| existing.id == group.id)
                {
                    Some(index) if force => {
                        if !include_synced && !config.groups[index].source_id.is_empty() {
                            skipped += 1;
                            continue;
                        }
                        delete_keys.extend(subscription::group_result_keys(config, &group.id));
                        config.groups[index] = group.clone();
                        imported += 1;
                    }
                    Some(_) => skipped += 1,
                    None => {
                        config.groups.push(group.clone());
                        imported += 1;
                    }
                }
            }
            Ok((Vec::new(), delete_keys))
        })
        .map_err(|err| ApiError::internal(format!("{err:#}")))?;

    // 覆盖导入可能换掉频道地址。丢掉旧探测结果，按在线处理到下一轮探测。
    for group in &to_import {
        state.health.forget_group(&group.id);
        state.quality.forget_group(&group.id);
    }

    Ok(Json(json!({
        "ok": true,
        "imported": imported,
        "skipped": skipped,
    })))
}

/// `POST /api/panel/groups/import`——导入分组数组，id 重复的跳过。
pub async fn import_groups(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    import(state, body, false, include_synced(&params))
}

/// `POST /api/panel/groups/import/force`——导入分组数组，id 重复的覆盖。
pub async fn import_groups_force(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    import(state, body, true, include_synced(&params))
}

fn include_synced(params: &HashMap<String, String>) -> bool {
    params
        .get("include_synced")
        .map(|value| value != "0" && value != "false")
        .unwrap_or(false)
}

/// 导入中的分组草稿。
struct Draft {
    id: String,
    name: String,
    has_dash: bool,
    channels: Vec<Channel>,
}

/// `POST /api/panel/groups/import/m3u`——按 M3U 文本导入分组与频道。
///
/// `mode`：`append`（默认，同名分组跳过）|`merge`（同名补频道）|`replace`（同名整体替换）。
/// `group_mode`：`auto`（默认）或目标的工作模式。
pub async fn import_m3u(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let include_synced = include_synced(&params);
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if text.trim().is_empty() {
        return Err(ApiError::bad_request("text 不能为空"));
    }
    let mode = body
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("append")
        .to_string();
    if !matches!(mode.as_str(), "append" | "merge" | "replace") {
        return Err(ApiError::bad_request(
            "mode 必须是 append、merge 或 replace",
        ));
    }
    let group_mode = body
        .get("group_mode")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_string();
    if !matches!(
        group_mode.as_str(),
        "auto" | "proxy" | "remux" | "dash" | "redirect" | "rewrite" | "passthrough"
    ) {
        return Err(ApiError::bad_request("group_mode 不合法"));
    }

    let entries = m3u::parse(&text);
    if entries.is_empty() {
        return Err(ApiError::bad_request("未解析到频道"));
    }
    let parsed_channels = entries.len();

    let mut drafts: Vec<Draft> = Vec::new();
    for entry in entries {
        let title = entry
            .group_title
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Default".to_string());
        let is_dash = entry
            .url
            .split('?')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .ends_with(".mpd");
        let index = match drafts.iter().position(|draft| draft.name == title) {
            Some(index) => index,
            None => {
                drafts.push(Draft {
                    id: m3u::group_id(&title),
                    name: title,
                    has_dash: false,
                    channels: Vec::new(),
                });
                drafts.len() - 1
            }
        };
        let draft = &mut drafts[index];
        draft.has_dash |= is_dash;

        // `random_id_enabled` 打开时忽略 `tvg-id`，每个新频道拿随机 id。
        // 目标开启该键后已有频道与切片前缀不变（F-010 §4），所以只影响新导入。
        let base = if state.snapshot().auth.random_id_enabled {
            m3u::random_channel_id()
        } else {
            m3u::channel_id(entry.tvg_id.as_deref())
        };
        let mut id = base.clone();
        let mut suffix = 2;
        while draft.channels.iter().any(|channel| channel.id == id) {
            id = format!("{base}-{suffix}");
            suffix += 1;
        }

        let mut channel = Channel {
            id,
            name: entry
                .tvg_name
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(entry.name),
            url: entry.url.clone(),
            ..Channel::default()
        };
        // `auto` 按每个频道的 URL 定模式。混合分组不能让 HLS 频道继承 DASH。
        if group_mode == "auto" {
            channel.mode = Some(if is_dash { "dash" } else { "rewrite" }.to_string());
        }
        if let Some(value) = &entry.tvg_id {
            channel
                .extra
                .insert("tvg_id".into(), serde_json::Value::String(value.clone()));
        }
        if let Some(value) = &entry.tvg_name {
            channel
                .extra
                .insert("tvg_name".into(), serde_json::Value::String(value.clone()));
        }
        if let Some(value) = &entry.tvg_logo {
            channel
                .extra
                .insert("tvg_logo".into(), serde_json::Value::String(value.clone()));
        }
        if let Some(value) = &entry.group_title {
            channel.extra.insert(
                "group_title".into(),
                serde_json::Value::String(value.clone()),
            );
        }
        if let Some(value) = &entry.user_agent {
            channel.headers.push(serde_json::json!({
                "name": "User-Agent",
                "value": value
            }));
        }
        if let Some(value) = &entry.referer {
            channel.headers.push(serde_json::json!({
                "name": "Referer",
                "value": value
            }));
        }
        for (key, value) in &entry.headers {
            channel
                .headers
                .push(serde_json::json!({ "name": key, "value": value }));
        }
        draft.channels.push(channel);
    }
    let parsed_groups = drafts.len();
    let draft_ids: Vec<String> = drafts.iter().map(|draft| draft.id.clone()).collect();

    let mut added_groups = 0usize;
    let mut merged_channels = 0usize;
    let mut replaced_groups = 0usize;
    let mut skipped_groups = 0usize;
    state
        .mutate(|config| {
            for draft in drafts {
                let has_hls = draft
                    .channels
                    .iter()
                    .any(|channel| channel.mode.as_deref() != Some("dash"));
                let (group_mode, format) = if group_mode == "auto" {
                    if draft.has_dash && has_hls {
                        // 混合分组用 HLS 清单承载 DASH 频道，避免 HLS 频道拿到 `.mpd` 入口。
                        ("rewrite", "hls")
                    } else if draft.has_dash {
                        ("dash", "mpd")
                    } else {
                        ("rewrite", "hls")
                    }
                } else if group_mode == "dash" {
                    (group_mode.as_str(), "mpd")
                } else {
                    (group_mode.as_str(), "hls")
                };
                let existing = config.groups.iter().position(|group| group.id == draft.id);
                match existing {
                    Some(index) if mode == "replace" => {
                        if !include_synced && !config.groups[index].source_id.is_empty() {
                            skipped_groups += 1;
                            continue;
                        }
                        config.groups[index] = Group {
                            id: draft.id,
                            name: draft.name,
                            mode: group_mode.to_string(),
                            format: format.to_string(),
                            channels: draft.channels,
                            ..Group::default()
                        };
                        replaced_groups += 1;
                    }
                    Some(index) if mode == "merge" => {
                        for channel in draft.channels {
                            if !config.groups[index]
                                .channels
                                .iter()
                                .any(|existing| existing.id == channel.id)
                            {
                                config.groups[index].channels.push(channel);
                                merged_channels += 1;
                            }
                        }
                    }
                    Some(_) => skipped_groups += 1,
                    None => {
                        config.groups.push(Group {
                            id: draft.id,
                            name: draft.name,
                            mode: group_mode.to_string(),
                            format: format.to_string(),
                            channels: draft.channels,
                            ..Group::default()
                        });
                        added_groups += 1;
                    }
                }
            }
            Ok(())
        })
        .map_err(|err| ApiError::internal(format!("{err:#}")))?;

    // 替换导入会换掉频道地址与设置。丢掉旧探测结果。
    for gid in &draft_ids {
        state.health.forget_group(gid);
        state.quality.forget_group(gid);
    }

    Ok(Json(json!({
        "ok": true,
        "parsed_groups": parsed_groups,
        "parsed_channels": parsed_channels,
        "added_groups": added_groups,
        "merged_channels": merged_channels,
        "replaced_groups": replaced_groups,
        "skipped_groups": skipped_groups,
        "group_mode": group_mode,
    })))
}
