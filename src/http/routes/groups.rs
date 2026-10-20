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
