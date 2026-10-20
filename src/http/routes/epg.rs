//! EPG 公开文档与面板接口。
//!
//! 公开面：`/epg.xml`、`/epg.xml.gz`、`/epg`。EPG 关闭时三者都 `404`（S-EP-05）。
//! 私有面：`/panel/api/epg/{status,refresh}`（S-EP-12 的 P1 子集）。

use std::{collections::HashMap, path::Path as FsPath, sync::Arc};

use axum::{
    extract::{Path, Query, State},
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
    let path = epg::output_dir(FsPath::new(&config.cache.disk_path)).join(name);
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

/// 读取并解析生成的节目单；关闭或缺失时返回 `None`。
fn load(state: &Arc<AppState>) -> Option<epg::DocumentData> {
    let config = state.snapshot();
    if !config.epg.enabled {
        return None;
    }
    epg::load_generated(&config).ok()
}

fn title_of(programme: &epg::Programme) -> String {
    programme
        .texts
        .iter()
        .find(|(name, _, _)| name == "title")
        .map(|(_, _, text)| text.clone())
        .unwrap_or_default()
}

fn desc_of(programme: &epg::Programme) -> String {
    programme
        .texts
        .iter()
        .find(|(name, _, _)| name == "desc")
        .map(|(_, _, text)| text.clone())
        .unwrap_or_default()
}

fn programme_json(programme: &epg::Programme) -> Value {
    json!({
        "channel": programme.channel,
        "start": programme.start,
        "stop": programme.stop,
        "title": title_of(programme),
        "desc": desc_of(programme),
        "categories": programme
            .texts
            .iter()
            .filter(|(name, _, _)| name == "category")
            .map(|(_, _, text)| text.clone())
            .collect::<Vec<_>>(),
    })
}

/// `GET /epg/api/channels`——频道表（形状为本实现自定）。
pub async fn api_channels(State(state): State<Arc<AppState>>) -> Response {
    let Some(data) = load(&state) else {
        return not_found();
    };
    let channels: Vec<Value> = data
        .channels
        .iter()
        .map(|channel| {
            json!({
                "id": channel.id,
                "names": channel
                    .names
                    .iter()
                    .map(|(lang, name)| json!({ "lang": lang, "name": name }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(json!({ "count": channels.len(), "channels": channels })).into_response()
}

/// `GET /epg/api/programmes/{channel}`——单个频道的节目。
pub async fn api_programmes(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
) -> Response {
    let Some(data) = load(&state) else {
        return not_found();
    };
    let programmes: Vec<Value> = data
        .programmes
        .iter()
        .filter(|programme| programme.channel == channel)
        .map(programme_json)
        .collect();
    Json(json!({ "channel": channel, "count": programmes.len(), "programmes": programmes }))
        .into_response()
}

/// `GET /epg/api/search?q=`——按标题/简介搜索，最多 200 条。
pub async fn api_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(data) = load(&state) else {
        return not_found();
    };
    let query = params
        .get("q")
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    if query.is_empty() {
        return Json(json!({ "q": "", "count": 0, "programmes": Vec::<Value>::new() }))
            .into_response();
    }
    let programmes: Vec<Value> = data
        .programmes
        .iter()
        .filter(|programme| {
            title_of(programme).to_lowercase().contains(&query)
                || desc_of(programme).to_lowercase().contains(&query)
        })
        .take(200)
        .map(programme_json)
        .collect();
    Json(json!({ "q": query, "count": programmes.len(), "programmes": programmes })).into_response()
}

/// `GET /epg/translate/status`——翻译能力（未启用引擎）。
pub async fn translate_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let translate = config
        .extra
        .get("epg")
        .and_then(|table| table.get("translate"))
        .cloned();
    let enabled = translate
        .as_ref()
        .and_then(|table| table.get("enabled"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    Json(json!({
        "enabled": enabled,
        "engine": "none",
        "models": [],
        "note": "EPG 翻译引擎尚未实现",
    }))
}

/// `POST /epg/translate/clear-engine`——清空翻译缓存。
pub async fn translate_clear(State(state): State<Arc<AppState>>) -> Json<Value> {
    let dir = state.cache_dir.join("epg").join(".translate");
    let mut cleared = 0usize;
    if let Ok(read) = std::fs::read_dir(&dir) {
        for item in read.flatten() {
            if std::fs::remove_file(item.path()).is_ok() {
                cleared += 1;
            }
        }
    }
    Json(json!({ "ok": true, "cleared": cleared }))
}
