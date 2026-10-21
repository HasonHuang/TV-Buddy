//! 订阅源预检端点：`/subs/status`、`/subs/groups`、`/subs/preview`。
//!
//! 证据：S-GAP-07。响应形状自定，字段保留目标的 `trace` 结构。
//! 这组端点公开；只接受 `http`/`https` 源地址。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::{
    http::error::ApiError,
    m3u,
    state::AppState,
    subs::{self, PreviewSource},
};

/// `GET /subs/status`——列出已配置的聚合源。
pub async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let sources: Vec<Value> = config
        .playlist
        .sources
        .iter()
        .map(|source| {
            json!({
                "id": source.id,
                "name": source.name,
                "url": source.url,
                "enabled": source.enabled,
                "include": source.include,
                "exclude": source.exclude,
            })
        })
        .collect();
    Json(json!({ "sources": sources }))
}

/// `POST /subs/groups`——抓取源并列出分组。
pub async fn groups(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let url = body
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| ApiError::bad_request("缺少 url"))?;
    validate_url(url)?;
    let text = fetch(&state, url).await?;
    let entries = m3u::parse(&text);
    let total = entries.len();
    let groups: Vec<Value> = subs::group_counts(&entries)
        .into_iter()
        .map(|count| json!({ "name": count.name, "count": count.count }))
        .collect();
    Ok(Json(json!({ "groups": groups, "total": total })))
}

/// `POST /subs/preview`——按映射与过滤规则预检源。
pub async fn preview(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
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

    let mut source = PreviewSource {
        unlisted_groups: input
            .get("unlisted_groups")
            .and_then(Value::as_str)
            .unwrap_or("drop")
            .to_string(),
        include: strings(&input, "include"),
        exclude: strings(&input, "exclude"),
        ..PreviewSource::default()
    };
    if source.include.is_empty() {
        if let Some(configured) = configured {
            source.include = configured.include.clone();
            source.exclude = configured.exclude.clone();
        }
    }
    if let Some(map) = input.get("group_map").and_then(Value::as_object) {
        for (from, value) in map {
            let to = value.as_str().map(str::trim).filter(|to| !to.is_empty());
            source
                .group_map
                .push((from.clone(), to.map(str::to_string)));
        }
    }

    let text = fetch(&state, url).await?;
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
                "url": item.url,
            })
        })
        .collect();
    let out_groups: Vec<Value> = result
        .out_groups
        .iter()
        .map(|count| json!({ "name": count.name, "count": count.count }))
        .collect();

    Ok(Json(json!({
        "kept": result.kept,
        "sample": sample,
        "trace": {
            "total": result.trace.total,
            "after_group": result.trace.after_group,
            "after_global_exclude": result.trace.after_global_exclude,
            "after_include": result.trace.after_include,
        },
        "dropped_groups": result.dropped_groups,
        "out_groups": out_groups,
    })))
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

async fn fetch(state: &Arc<AppState>, url: &str) -> Result<String, ApiError> {
    let response = state
        .http
        .get(url)
        .send()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("抓取失败: {error}")))?;
    if !response.status().is_success() {
        return Err(ApiError::bad_gateway(format!(
            "上游返回 {}",
            response.status()
        )));
    }
    response
        .text()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("读取失败: {error}")))
}
