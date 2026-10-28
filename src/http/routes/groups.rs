//! 分组与频道接口。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../../../docs/design/03-http-contract.md) §6.2。
//! 局部更新语义：把请求体字段合并到当前对象，再整体校验。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

use crate::{
    config::model::{Channel, Group},
    http::{error::ApiError, ext::PanelAuth},
    m3u,
    state::AppState,
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

/// `POST /panel/api/groups`。id 重复 → `409`（F-013 §5）。
pub async fn create_group(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let id = body_id(&body)?;
    if state.snapshot().group(&id).is_some() {
        return Err(ApiError::conflict("Group id already exists"));
    }
    let group: Group = merge_into(&Group::default(), &body)?;

    state
        .mutate(|config| {
            config.groups.push(group);
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    tracing::info!(group = %id, "新建分组");
    Ok(Json(json!({ "ok": true })))
}

/// `PUT /panel/api/groups/{gid}`——局部更新。
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
    let updated: Group = merge_into(group, &body)?;

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
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /panel/api/groups/{gid}`。
pub async fn delete_group(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(gid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state.snapshot().group(&gid).is_none() {
        return Err(ApiError::not_found("Group not found"));
    }
    state
        .mutate(|config| {
            config.groups.retain(|group| group.id != gid);
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    tracing::info!(group = %gid, "删除分组");
    Ok(Json(json!({ "ok": true })))
}

/// `GET /panel/api/groups/export`——顶层数组（F-013 §1）。
pub async fn export_all(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    Json(serde_json::to_value(&config.groups).unwrap_or(Value::Array(Vec::new())))
}

/// `GET /panel/api/groups/{gid}/export`——单个分组对象。
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

/// `POST /panel/api/groups/{gid}/channels`。
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
    let channel: Channel = merge_into(&Channel::default(), &body)?;

    state
        .mutate(|config| {
            let group = config
                .group_mut(&gid)
                .ok_or_else(|| anyhow::anyhow!("Group not found"))?;
            group.channels.push(channel);
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    tracing::info!(group = %gid, channel = %id, "新建频道");
    Ok(Json(json!({ "ok": true })))
}

/// `PUT /panel/api/groups/{gid}/channels/{cid}`——局部更新。
pub async fn update_channel(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let current = state.snapshot();
    let channel = current
        .group(&gid)
        .and_then(|group| group.channels.iter().find(|channel| channel.id == cid))
        .ok_or_else(|| ApiError::not_found("Channel not found"))?;
    let updated: Channel = merge_into(channel, &body)?;

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
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /panel/api/groups/{gid}/channels/{cid}`。
pub async fn delete_channel(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let exists = state
        .snapshot()
        .group(&gid)
        .map(|group| group.channels.iter().any(|channel| channel.id == cid))
        .unwrap_or(false);
    if !exists {
        return Err(ApiError::not_found("Channel not found"));
    }
    state
        .mutate(|config| {
            let group = config
                .group_mut(&gid)
                .ok_or_else(|| anyhow::anyhow!("Group not found"))?;
            group.channels.retain(|channel| channel.id != cid);
            Ok(())
        })
        .map_err(|err| ApiError::internal(err.to_string()))?;
    tracing::info!(group = %gid, channel = %cid, "删除频道");
    Ok(Json(json!({ "ok": true })))
}

/// 导入分组：`force = false` 跳过已存在的 id，`true` 覆盖。
fn import(state: Arc<AppState>, body: Value, force: bool) -> Result<Json<Value>, ApiError> {
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

    let mut imported = 0usize;
    let mut skipped = 0usize;
    state
        .mutate(|config| {
            for group in &groups {
                match config
                    .groups
                    .iter()
                    .position(|existing| existing.id == group.id)
                {
                    Some(index) if force => {
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
            Ok(())
        })
        .map_err(|err| ApiError::internal(format!("{err:#}")))?;

    Ok(Json(json!({
        "ok": true,
        "imported": imported,
        "skipped": skipped,
    })))
}

/// `POST /panel/api/groups/import`——导入分组数组，id 重复的跳过。
pub async fn import_groups(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    import(state, body, false)
}

/// `POST /panel/api/groups/import/force`——导入分组数组，id 重复的覆盖。
pub async fn import_groups_force(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    import(state, body, true)
}

/// 导入中的分组草稿。
struct Draft {
    id: String,
    name: String,
    has_dash: bool,
    channels: Vec<Channel>,
}

/// `POST /panel/api/groups/import/m3u`——按 M3U 文本导入分组与频道。
///
/// `mode`：`append`（默认，同名分组跳过）|`merge`（同名补频道）|`replace`（同名整体替换）。
/// `group_mode`：`auto`（默认）或目标的工作模式。
pub async fn import_m3u(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
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
        if let Some(value) = &entry.tvg_id {
            channel
                .extra
                .insert("tvg_id".into(), toml::Value::String(value.clone()));
        }
        if let Some(value) = &entry.tvg_name {
            channel
                .extra
                .insert("tvg_name".into(), toml::Value::String(value.clone()));
        }
        if let Some(value) = &entry.tvg_logo {
            channel
                .extra
                .insert("tvg_logo".into(), toml::Value::String(value.clone()));
        }
        if let Some(value) = &entry.group_title {
            channel
                .extra
                .insert("group_title".into(), toml::Value::String(value.clone()));
        }
        if let Some(value) = &entry.user_agent {
            channel
                .headers
                .push(toml::Value::String(format!("User-Agent: {value}")));
        }
        if let Some(value) = &entry.referer {
            channel
                .headers
                .push(toml::Value::String(format!("Referer: {value}")));
        }
        for (key, value) in &entry.headers {
            channel
                .headers
                .push(toml::Value::String(format!("{key}: {value}")));
        }
        draft.channels.push(channel);
    }
    let parsed_groups = drafts.len();

    let mut added_groups = 0usize;
    let mut merged_channels = 0usize;
    let mut replaced_groups = 0usize;
    let mut skipped_groups = 0usize;
    state
        .mutate(|config| {
            for draft in drafts {
                let (group_mode, format) = if group_mode == "auto" {
                    if draft.has_dash {
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
