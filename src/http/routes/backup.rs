//! 备份与恢复接口。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../../../docs/design/03-http-contract.md) §6.5。
//! 证据：F-013 §8（备份范围）与 §9（恢复语义）。

use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::{backup, http::error::ApiError, http::ext::PanelAuth, state::AppState};

/// `GET /panel/api/backup?include=all|epg-scripts|resolver`。
pub async fn backup(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let config = state.snapshot();
    let include = params.get("include").map(String::as_str).unwrap_or("all");
    let data = backup::create(
        &config,
        &config.cache.disk_path,
        include,
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    let name = backup::file_name();
    let mut response = ([(header::CONTENT_TYPE, "application/gzip")], data).into_response();
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition);
    Ok(response)
}

/// `POST /panel/api/restore`——请求体是 gzip(tar) 原文。
pub async fn restore(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    if body.is_empty() {
        return Err(ApiError::bad_request("备份内容为空"));
    }
    let current = state.snapshot();
    let (restored, report) = backup::restore(&current.cache.disk_path, &body)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;

    // 覆盖前把当前配置另存到库文件旁，便于手工回退。
    if let Ok(text) = serde_json::to_string_pretty(&*current) {
        let path = state.db_path.with_extension("db.bak.json");
        if let Err(error) = std::fs::write(&path, text) {
            tracing::warn!(%error, path = %path.display(), "写配置备份失败");
        }
    }
    state
        .save_config(restored.clone())
        .map_err(|error| ApiError::internal(format!("写入配置失败: {error:#}")))?;
    Ok(Json(json!({
        "ok": true,
        "config_reloaded": true,
        "report": {
            "files": report.files,
            "skipped": report.skipped,
        },
        "groups": restored.groups.len(),
    })))
}
