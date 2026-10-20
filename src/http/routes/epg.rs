//! EPG 公开文档与面板接口。
//!
//! 公开面：`/epg.xml`、`/epg.xml.gz`、`/epg`。EPG 关闭时三者都 `404`（S-EP-05）。
//! 私有面：`/panel/api/epg/{status,refresh}`（S-EP-12 的 P1 子集）。

use std::{path::Path, sync::Arc};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::{
    epg,
    http::{error::ApiError, ext::PanelAuth, static_assets},
    state::AppState,
};

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

async fn document(state: &Arc<AppState>, gzip: bool) -> Response {
    let config = state.snapshot();
    if !config.epg.enabled {
        return not_found();
    }
    let name = if gzip { "epg.xml.gz" } else { "epg.xml" };
    let path = epg::output_dir(Path::new(&config.cache.disk_path)).join(name);
    let Ok(body) = tokio::fs::read(&path).await else {
        return not_found();
    };
    let mime = if gzip {
        "application/gzip"
    } else {
        "application/xml; charset=utf-8"
    };
    ([(header::CONTENT_TYPE, mime)], body).into_response()
}

/// `GET /epg.xml`。
pub async fn xml(State(state): State<Arc<AppState>>) -> Response {
    document(&state, false).await
}

/// `GET /epg.xml.gz`。
pub async fn xml_gz(State(state): State<Arc<AppState>>) -> Response {
    document(&state, true).await
}

/// `GET /epg`——公开浏览页。EPG 关闭时 `404`。
pub async fn viewer(State(state): State<Arc<AppState>>) -> Response {
    if !state.snapshot().epg.enabled {
        return not_found();
    }
    static_assets::epg_page().await
}

/// `GET /panel/api/epg/status`。
pub async fn status(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let last_success = *state
        .epg
        .last_success
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let last_error = state
        .epg
        .last_error
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    Json(json!({
        "enabled": config.epg.enabled,
        "running": state.epg.running.load(std::sync::atomic::Ordering::SeqCst),
        "sources": config.epg.sources.len(),
        "channels": state.epg.channels.load(std::sync::atomic::Ordering::SeqCst),
        "programmes": state.epg.programmes.load(std::sync::atomic::Ordering::SeqCst),
        "kept": state.epg.kept.load(std::sync::atomic::Ordering::SeqCst),
        "keep_days": config.epg.keep_days,
        "future_days": config.epg.future_days,
        "last_success": last_success,
        "last_error": last_error,
    }))
}

/// `POST /panel/api/epg/refresh`——同步跑一次采集，返回摘要。
pub async fn refresh(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    match epg::refresh(&state).await {
        Ok(summary) => Ok(Json(json!({
            "ok": true,
            "msg": "采集完成",
            "channels": summary.channels,
            "programmes": summary.kept,
        }))),
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}
