//! 定时录制与直播开关。
//!
//! 证据：F-009 §2。`{idx}` 是列表下标，不是任务 ID。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    http::{error::ApiError, ext::PanelAuth},
    schedule::Task,
    state::AppState,
};

/// `GET /panel/api/schedule`——任务数组。
pub async fn list(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!(state.schedule.list()))
}

/// `POST /panel/api/schedule`——新增。
pub async fn create(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(task): Json<Task>,
) -> Result<Json<Value>, ApiError> {
    let config = state.snapshot();
    let Some(group) = config.group(&task.group_id) else {
        return Err(ApiError::not_found("Group not found"));
    };
    if !group
        .channels
        .iter()
        .any(|channel| channel.id == task.channel_id)
    {
        return Err(ApiError::not_found("Channel not found"));
    }
    let id = state
        .schedule
        .add(task)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    Ok(Json(json!({ "ok": true, "id": id })))
}

/// `PUT /panel/api/schedule/:idx`。
pub async fn update(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(index): Path<usize>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let task = state
        .schedule
        .update(index, &body)
        .map_err(|_| ApiError::not_found("任务不存在"))?;
    Ok(Json(json!({ "ok": true, "task": task })))
}

/// `DELETE /panel/api/schedule/:idx`。
pub async fn remove(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(index): Path<usize>,
) -> Result<Json<Value>, ApiError> {
    let (_, was_running) = state
        .schedule
        .delete(index)
        .map_err(|_| ApiError::not_found("任务不存在"))?;
    Ok(Json(json!({ "ok": true, "was_running": was_running })))
}

/// `POST /panel/api/schedule/:idx/stop`。
pub async fn stop(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(index): Path<usize>,
) -> Result<Json<Value>, ApiError> {
    let was_running = state
        .schedule
        .stop(index)
        .map_err(|_| ApiError::not_found("任务不存在"))?;
    Ok(Json(json!({ "ok": true, "was_running": was_running })))
}

/// `GET /panel/api/schedule/download/:filename`。
pub async fn download(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let path = state
        .schedule
        .file(&name)
        .ok_or_else(|| ApiError::not_found("文件不存在"))?;
    let bytes = std::fs::read(&path).map_err(|error| ApiError::internal(format!("{error}")))?;
    let mime = if name.ends_with(".mkv") {
        "video/x-matroska"
    } else {
        "video/mp2t"
    };
    let mut response = (StatusCode::OK, bytes).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    Ok(response)
}

/// `POST /panel/api/schedule/from-epg`——按条目批量建任务。
///
/// 请求：`{"items":[{name,group_id,channel_id,start_time,end_time,format}]}`。
pub async fn from_epg(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let items = body
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request("缺少 items 数组"))?;
    let config = state.snapshot();
    let mut ids = Vec::new();
    let mut failed = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let task: Task = match serde_json::from_value(item.clone()) {
            Ok(task) => task,
            Err(error) => {
                failed.push(json!({ "index": index, "error": format!("字段不合法: {error}") }));
                continue;
            }
        };
        let valid = config
            .group(&task.group_id)
            .map(|group| {
                group
                    .channels
                    .iter()
                    .any(|channel| channel.id == task.channel_id)
            })
            .unwrap_or(false);
        if !valid {
            failed.push(json!({ "index": index, "error": "Channel not found" }));
            continue;
        }
        match state.schedule.add(task) {
            Ok(id) => ids.push(id),
            Err(error) => failed.push(json!({ "index": index, "error": format!("{error:#}") })),
        }
    }
    Ok(Json(json!({
        "ok": failed.is_empty(),
        "created": ids.len(),
        "ids": ids,
        "failed": failed,
    })))
}

/// `GET /panel/api/livestream`——已点亮的频道。
pub async fn livestream_list(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    let entries: Vec<Value> = config
        .groups
        .iter()
        .flat_map(|group| {
            group
                .channels
                .iter()
                .filter(|channel| channel.livestream)
                .map(|channel| {
                    json!({
                        "group_id": group.id,
                        "channel_id": channel.id,
                        "name": channel.name,
                        "timeshift_hours": channel.timeshift_hours,
                    })
                })
        })
        .collect();
    Json(json!({ "count": entries.len(), "channels": entries }))
}

#[derive(Debug, Deserialize)]
pub struct ActionPath {
    pub gid: String,
    pub cid: String,
    pub action: String,
}

/// `POST /panel/api/livestream/:gid/:cid/{start,stop}`。
pub async fn livestream_switch(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(ActionPath { gid, cid, action }): Path<ActionPath>,
) -> Result<Json<Value>, ApiError> {
    let enable = match action.as_str() {
        "start" => true,
        "stop" => false,
        _ => return Err(ApiError::not_found("未知操作")),
    };
    let mut found = false;
    state
        .mutate(|config| {
            let Some(group) = config.groups.iter_mut().find(|group| group.id == gid) else {
                return Ok(());
            };
            let Some(channel) = group.channels.iter_mut().find(|channel| channel.id == cid) else {
                return Ok(());
            };
            channel.livestream = enable;
            found = true;
            Ok(())
        })
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    if !found {
        return Err(ApiError::not_found("Channel not found"));
    }
    if enable {
        // 立即开始缓冲，用户随后即可回看。
        state.timeshift.ensure(&state, &gid, &cid);
    }
    Ok(Json(json!({ "ok": true, "livestream": enable })))
}
