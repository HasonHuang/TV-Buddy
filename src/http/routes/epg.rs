//! EPG 公开文档与面板接口。
//!
//! 公开面：`/epg.xml`、`/epg.xml.gz`、`/epg`。EPG 关闭时三者都 `404`（S-EP-05）。
//! 私有面：`/panel/api/epg/{status,refresh,mappings,mapping,match-report}`。

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
            "translated": summary.translated,
        }))),
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}

/// `GET /panel/api/epg/mappings`——手工映射表。
pub async fn mappings(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "mappings": epg::mappings(&state),
        "extra": [],
    }))
}

/// `POST /panel/api/epg/mapping`——设置或清除手工映射。
pub async fn mapping(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let text = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let gid = text("gid");
    let cid = text("cid");
    if gid.is_empty() || cid.is_empty() {
        return Err(ApiError::bad_request("缺少 gid/cid"));
    }
    match epg::set_mapping(
        &state,
        &gid,
        &cid,
        &text("source_id"),
        &text("source_channel_id"),
    ) {
        Ok(Some(mapping)) => Ok(Json(json!({ "ok": true, "mapping": mapping }))),
        Ok(None) => Ok(Json(json!({ "ok": true, "cleared": true }))),
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `GET /panel/api/epg/match-report`——目标式文本报告。
pub async fn match_report(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Response {
    let rows = state
        .epg
        .match_rows
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let matched = rows
        .iter()
        .filter(|row| !row.source_id.is_empty() && row.via != "none" && row.via != "duplicate")
        .count();
    let empty = rows.iter().filter(|row| row.via == "none").count();
    let mut body = format!("matched_count={matched} empty_count={empty} extra_count=0\n");
    body.push_str("gid    cid  name                   out_id       via       src_id     pc\n");
    for row in &rows {
        body.push_str(&format!(
            "{:<6} {:<4} {:<22} {:<12} {:<9} {:<9} {}\n",
            row.gid, row.cid, row.name, row.out_id, row.via, row.source_id, row.programmes
        ));
    }
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
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

/// `GET /epg/translate/status`——翻译能力与缓存状态。
pub async fn translate_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let translate = epg::translate_config(&config);
    Json(json!({
        "enabled": translate.enabled,
        "engine": if translate.url.is_empty() { "none" } else { "http" },
        "url": translate.url,
        "source": translate.source,
        "targets": translate.targets,
        "translation_first": translate.translation_first,
        "models": translate.targets,
        "cache": epg::translate_cache_len(&config),
    }))
}

/// `GET /epg/translate/models`——可用翻译目标语言。
pub async fn translate_models(State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let translate = epg::translate_config(&config);
    Json(json!({
        "enabled": translate.enabled,
        "engine": if translate.url.is_empty() { "none" } else { "http" },
        "models": translate.targets,
    }))
}

/// `POST /epg/translate/run`——立即采集并翻译。
pub async fn translate_run(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let config = state.snapshot();
    let translate = epg::translate_config(&config);
    if !translate.enabled {
        return Err(ApiError::bad_request("EPG 翻译未启用"));
    }
    if translate.url.is_empty() {
        return Err(ApiError::bad_request("请先填写翻译服务地址"));
    }
    match epg::refresh(&state).await {
        Ok(summary) => Ok(Json(json!({
            "ok": true,
            "channels": summary.channels,
            "programmes": summary.kept,
            "translated": summary.translated,
        }))),
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}

/// `POST /epg/translate/probe`——探测翻译服务。
pub async fn translate_probe(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let config = state.snapshot();
    let translate = epg::translate_config(&config);
    let endpoint = epg::translate_endpoint(&config);
    if endpoint.is_empty() {
        return Err(ApiError::bad_request("请先填写翻译服务地址"));
    }
    let target = translate
        .targets
        .first()
        .cloned()
        .unwrap_or_else(|| "zh-cn".to_string());
    let result = crate::translate::batch_with_token(
        &state.http,
        &translate.url,
        &translate.source,
        &target,
        &["hello".to_string()],
        Some(translate.token.as_str()),
    )
    .await;
    Ok(Json(match result {
        Ok(values) => json!({
            "ok": true,
            "endpoint": endpoint,
            "status": 200,
            "result": values.first().cloned().unwrap_or_default(),
        }),
        Err(error) => json!({
            "ok": false,
            "endpoint": endpoint,
            "status": Value::Null,
            "error": format!("请求失败: {error:#}"),
        }),
    }))
}

/// `POST /epg/translate/mtran/test`——机翻自检。
pub async fn translate_mtran(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Response {
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("hello")
        .to_string();
    let (base, token) = translate_credentials(&state.snapshot());
    if base.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "请先填写 MTranServer 地址(字幕机翻只支持本地 MTranServer)"
            })),
        )
            .into_response();
    }
    let translated = crate::translate::batch_with_token(
        &state.http,
        &base,
        "auto",
        "zh-cn",
        &[text],
        Some(token.as_str()),
    )
    .await;
    let (ok, error, result) = match translated {
        Ok(values) => (
            true,
            String::new(),
            values.first().cloned().unwrap_or_default(),
        ),
        Err(error) => (false, format!("请求失败: {error:#}"), String::new()),
    };
    Json(json!({
        "base": base,
        "ok": ok,
        "result": result,
        "targets": [{ "code": "zh-Hans", "lang": "zh-cn", "ok": ok, "error": error }],
        "version": null,
    }))
    .into_response()
}

/// EPG 翻译优先；未配置时回退到字幕翻译配置。
fn translate_credentials(config: &crate::config::Config) -> (String, String) {
    let epg = epg::translate_config(config);
    if !epg.url.is_empty() {
        return (epg.url, epg.token);
    }
    let table = config
        .extra
        .get("subtitle")
        .and_then(|table| table.get("translate"));
    let get = |key: &str| table.and_then(|table| table.get(key));
    let url = get("url")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string();
    let token = get("token")
        .or_else(|| get("mtran_token"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    (url, token)
}

/// `POST /epg/translate/clear`——清空翻译结果缓存。
pub async fn translate_clear_results(State(state): State<Arc<AppState>>) -> Json<Value> {
    translate_clear(State(state)).await
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
