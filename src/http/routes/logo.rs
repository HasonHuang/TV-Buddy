//! 台标库接口：公开取图与列表，面板上传与删除。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../../../docs/design/03-http-contract.md) §3。
//! 证据：F-011 §6、F-012。

use std::{path::Path as FsPath, sync::Arc};

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::{
    http::{error::ApiError, ext::PanelAuth},
    logo,
    state::AppState,
};

/// 列表响应：`{allowed_exts, count, logos}`。
fn list_body(cache: &str) -> Value {
    let logos = logo::list(cache);
    json!({
        "allowed_exts": logo::ALLOWED_EXTS,
        "count": logos.len(),
        "logos": logos,
    })
}

/// `GET /logo/api/list`——公开台标列表。
pub async fn public_list(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    Json(list_body(&config.cache.disk_path))
}

/// `GET /logo/<file>`——公开取图，`public, max-age=86400`。
pub async fn file(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    let Some(file) = logo::sanitize(&name) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let ext = file
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if !logo::ALLOWED_EXTS.contains(&ext.as_str()) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let config = state.snapshot();
    let path = logo::dir(&config.cache.disk_path).join(&file);
    let Ok(body) = tokio::fs::read(&path).await else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mime = mime_guess::from_path(FsPath::new(&file))
        .first_or_octet_stream()
        .to_string();
    let mut response = ([(header::CONTENT_TYPE, mime)], body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    response
}

/// `GET /panel/api/logo/list`。
pub async fn panel_list(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    Json(list_body(&config.cache.disk_path))
}

/// `POST /panel/api/logo/upload`——`{"items":[{"name","ext","data":"data:image/…;base64,…"}]}`。
pub async fn upload(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let items = body
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request("items 必须是数组"))?;
    let cache = state.snapshot().cache.disk_path.clone();

    let mut added: Vec<Value> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    for item in items {
        let name = item.get("name").and_then(Value::as_str).unwrap_or("");
        let mut ext = item
            .get("ext")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if ext.is_empty() {
            ext = name
                .rsplit_once('.')
                .map(|(_, ext)| ext.to_string())
                .unwrap_or_default();
        }
        let data = item.get("data").and_then(Value::as_str).unwrap_or("");
        let bytes = match logo::decode_data_url(data) {
            Ok((_, bytes)) => bytes,
            Err(error) => {
                failed.push(json!({ "name": name, "error": format!("{error:#}") }));
                continue;
            }
        };
        let base = name.rsplit_once('.').map(|(base, _)| base).unwrap_or(name);
        match logo::write(&cache, base, &ext, &bytes) {
            Ok(entry) => added.push(json!(entry)),
            Err(error) => failed.push(json!({ "name": name, "error": format!("{error:#}") })),
        }
    }
    Ok(Json(
        json!({ "ok": true, "added": added, "failed": failed }),
    ))
}

/// `POST /panel/api/logo/delete`——`{"file"}`。
pub async fn delete(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let file = body
        .get("file")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if file.trim().is_empty() {
        return Err(ApiError::bad_request("file 不能为空"));
    }
    let cache = state.snapshot().cache.disk_path.clone();
    match logo::remove(&cache, &file) {
        Ok(true) => Ok(Json(json!({ "ok": true, "removed": 1 }))),
        Ok(false) => Err(ApiError::not_found("文件不存在")),
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}
