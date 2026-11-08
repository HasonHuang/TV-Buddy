//! EPG 公开文档与面板接口。
//!
//! 公开面：`/epg.xml`、`/epg.xml.gz`、`/epg`。EPG 关闭时三者都 `404`（S-EP-05）。
//! 私有面：`/panel/api/epg/{status,refresh,mappings,mapping,match-report}`。

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::{
    epg, epg_rules,
    epg_store::{Binding, Rule, Source},
    http::{error::ApiError, ext::PanelAuth},
    state::AppState,
};

/// EPG 关闭时公开接口的统一错误。
fn epg_closed() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "EPG 功能已关闭" })),
    )
        .into_response()
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
        "sources": state.epg_store.sources().len(),
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
        "extra": epg::extras(&state),
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
        Ok(Some(mapping)) => {
            spawn_regenerate(&state);
            Ok(Json(json!({ "ok": true, "mapping": mapping })))
        }
        Ok(None) => {
            spawn_regenerate(&state);
            Ok(Json(json!({ "ok": true, "cleared": true })))
        }
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
    let extra_count = epg::extras(&state).len();
    let mut body =
        format!("matched_count={matched} empty_count={empty} extra_count={extra_count}\n");
    body.push_str("gid    cid  name                   out_id       via       src_id     pc\n");
    for row in &rows {
        body.push_str(&format!(
            "{:<6} {:<4} {:<22} {:<12} {:<9} {:<9} {}\n",
            row.gid, row.cid, row.name, row.out_id, row.via, row.source_id, row.programmes
        ));
    }
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

/// `GET /panel/api/epg/scripts`——脚本列表与 Python 版本。
pub async fn scripts_list(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    Json(json!({
        "python": epg::python_version().await,
        "scripts": epg::list_scripts(&config),
    }))
}

/// `POST /panel/api/epg/script`——上传或覆盖脚本。
pub async fn script_save(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let content = body.get("content").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        return Err(ApiError::bad_request("缺少 name"));
    }
    match epg::save_script(&state.snapshot(), name, content) {
        Ok(info) => Ok(Json(json!({ "ok": true, "script": info }))),
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `POST /panel/api/epg/script-delete`——删除脚本。
pub async fn script_delete(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("缺少 name"));
    }
    match epg::delete_script(&state.snapshot(), name) {
        Ok(removed) => Ok(Json(json!({ "ok": true, "removed": removed }))),
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `POST /panel/api/epg/extra`——额外频道的增删。
pub async fn extra(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let action = body
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("add")
        .trim();
    match action {
        "add" => {
            let text = |key: &str| {
                body.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            match epg::add_extra(
                &state,
                &text("source_id"),
                &text("source_channel_id"),
                &text("out_id"),
                &text("out_name"),
            ) {
                Ok(extra) => {
                    spawn_regenerate(&state);
                    Ok(Json(json!({ "ok": true, "extra": extra })))
                }
                Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
            }
        }
        "remove" => {
            let keys: Vec<(String, String)> = match body.get("items").and_then(Value::as_array) {
                Some(items) => items
                    .iter()
                    .filter_map(|item| {
                        let source_id = item.get("source_id")?.as_str()?.trim().to_string();
                        let source_channel_id =
                            item.get("source_channel_id")?.as_str()?.trim().to_string();
                        Some((source_id, source_channel_id))
                    })
                    .collect(),
                None => {
                    let source_id = body
                        .get("source_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let source_channel_id = body
                        .get("source_channel_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if source_id.is_empty() || source_channel_id.is_empty() {
                        return Err(ApiError::bad_request("缺少 source_id/source_channel_id"));
                    }
                    vec![(source_id, source_channel_id)]
                }
            };
            match epg::remove_extras(&state, &keys) {
                Ok(removed) => {
                    spawn_regenerate(&state);
                    Ok(Json(json!({ "ok": true, "removed": removed })))
                }
                Err(error) => Err(ApiError::internal(format!("{error:#}"))),
            }
        }
        _ => Err(ApiError::bad_request("未知 action")),
    }
}

/// 后台按 raw 缓存重新生成，不阻塞面板请求。
fn spawn_regenerate(state: &Arc<AppState>) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = epg::regenerate(&state).await {
            tracing::warn!(%error, "EPG 重新生成失败");
        }
    });
}

/// `POST /panel/api/epg/regenerate`——按 raw 缓存重新匹配并生成。
pub async fn regenerate(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    match epg::regenerate(&state).await {
        Ok(summary) => Ok(Json(json!({
            "ok": true,
            "channels": summary.channels,
            "programmes": summary.kept,
        }))),
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}

/// `POST /panel/api/epg/cron-preview`——校验表达式并返回下次触发时间。
pub async fn cron_preview(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let expr = body
        .get("expr")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let config = state.snapshot();
    let offset = config.server.timezone_offset_hours as i64;
    match epg::cron_preview(expr, epg::now_secs() as i64, offset, 3) {
        Ok(next) => Json(json!({ "ok": true, "next": next })),
        Err(error) => Json(json!({ "ok": false, "error": format!("{error:#}") })),
    }
}

/// `GET /panel/api/epg/programmes/{channel}`——频道节目表与录制缓冲。
pub async fn programmes(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
) -> Response {
    let config = state.snapshot();
    match epg::programmes_for(&config, &channel) {
        Ok(value) => Json(value).into_response(),
        Err(error) => Json(json!({
            "now": epg::now_secs(),
            "channel": channel,
            "programmes": [],
            "note": format!("{error:#}"),
        }))
        .into_response(),
    }
}

/// `GET /panel/api/epg/suggest/{gid}/{cid}`——按名称推荐源频道。
pub async fn suggest(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    match epg::suggest(&state, &gid, &cid, 10) {
        Ok(value) => Ok(Json(value)),
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `GET /panel/api/epg/source-channels/{id}`——源频道搜索与分页。
pub async fn source_channels(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(source_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let query = params.get("q").map(String::as_str).unwrap_or("");
    let offset = params
        .get("offset")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(60);
    Json(epg::source_channels(
        &state, &source_id, query, offset, limit,
    ))
}

/// `GET /panel/api/epg/tz-preview/{id}?shift=`——时间偏移预览。
pub async fn tz_preview(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(source_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let shift_mins = params
        .get("shift")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    Json(epg::tz_preview(&state, &source_id, shift_mins * 60))
}

fn title_of(programme: &epg::Programme) -> String {
    programme
        .texts
        .iter()
        .find(|(name, _, _)| name == "title")
        .map(|(_, _, text)| text.clone())
        .unwrap_or_default()
}

fn time_of(value: &str) -> i64 {
    epg::parse_time(value).unwrap_or(0)
}

fn search_programme_json(programme: &epg::Programme, channel_name: &str) -> Value {
    json!({
        "channel": programme.channel,
        "channel_name": channel_name,
        "start": time_of(&programme.start),
        "stop": time_of(&programme.stop),
        "title": title_of(programme),
    })
}

/// `GET /panel/api/epg/search?q=`——面板侧标题搜索。
pub async fn search(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let config = state.snapshot();
    if !config.epg.enabled {
        return epg_closed();
    }
    let viewer = epg::load_viewer(&config).unwrap_or_default();
    let data = epg::load_generated(&config).unwrap_or_default();
    let query = params
        .get("q")
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    let channels: Vec<Value> = if query.is_empty() {
        Vec::new()
    } else {
        viewer
            .channels
            .iter()
            .filter(|channel| {
                channel.id.to_lowercase().contains(&query)
                    || channel.name.to_lowercase().contains(&query)
            })
            .map(viewer_channel_json)
            .collect()
    };
    let names: HashMap<String, String> = viewer
        .channels
        .iter()
        .map(|channel| (channel.id.to_ascii_lowercase(), channel.name.clone()))
        .collect();
    let mut programmes: Vec<&epg::Programme> = if query.is_empty() {
        Vec::new()
    } else {
        data.programmes
            .iter()
            .filter(|programme| title_of(programme).to_lowercase().contains(&query))
            .collect()
    };
    programmes.sort_by_key(|programme| time_of(&programme.start));
    let programmes: Vec<Value> = programmes
        .into_iter()
        .take(200)
        .map(|programme| {
            let name = names
                .get(&programme.channel.to_ascii_lowercase())
                .map(String::as_str)
                .unwrap_or(&programme.channel);
            search_programme_json(programme, name)
        })
        .collect();
    Json(json!({
        "channels": channels,
        "programmes": programmes,
        "tz_offset_hours": config.server.timezone_offset_hours,
    }))
    .into_response()
}

/// `GET /panel/api/epg/viewer-channels`——公开浏览页可见频道。
pub async fn viewer_channels(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Response {
    let config = state.snapshot();
    if !config.epg.enabled {
        return epg_closed();
    }
    let viewer = epg::load_viewer(&config).unwrap_or_default();
    let channels: Vec<Value> = viewer.channels.iter().map(viewer_channel_json).collect();
    Json(json!({ "channels": channels })).into_response()
}

fn viewer_channel_json(channel: &epg::ViewerChannel) -> Value {
    json!({
        "icon": channel.icon,
        "id": channel.id,
        "kind": channel.kind,
        "last_stop": channel.last_stop,
        "name": channel.name,
        "programme_count": channel.programme_count,
        "source_id": channel.source_id,
        "source_name": channel.source_name,
    })
}

/// `GET /epg/translate/status`——翻译能力与缓存状态。
pub async fn translate_status(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
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
pub async fn translate_models(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
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
    PanelAuth(_): PanelAuth,
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
    let subtitle = &config.subtitle.translate;
    (
        subtitle.url.trim().trim_end_matches('/').to_string(),
        subtitle.token.trim().to_string(),
    )
}

/// `POST /epg/translate/clear`——清空翻译结果缓存。
pub async fn translate_clear_results(
    PanelAuth(token): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    translate_clear(PanelAuth(token), State(state)).await
}

/// `POST /epg/translate/clear-engine`——清空翻译缓存。
pub async fn translate_clear(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
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

// ---- EPG 配置：源、绑定、关键字规则 ----

/// `GET /panel/api/epg/sources`——源列表与运行时状态。
pub async fn sources(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let statuses = state
        .epg
        .source_status
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let items: Vec<Value> = state
        .epg_store
        .sources()
        .into_iter()
        .map(|source| {
            let status = statuses.get(&source.id).cloned().unwrap_or_default();
            let mut value = serde_json::to_value(&source).unwrap_or_else(|_| json!({}));
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "last_fetch_started_at_ms".into(),
                    json!(status.last_fetch_started_at_ms),
                );
                object.insert(
                    "last_fetch_finished_at_ms".into(),
                    json!(status.last_fetch_finished_at_ms),
                );
                object.insert("last_fetch_ok".into(), json!(status.last_fetch_ok));
                object.insert("last_error".into(), json!(status.last_error));
                object.insert("channels".into(), json!(status.channels));
                object.insert("programmes".into(), json!(status.programmes));
                object.insert("coverage_start_ms".into(), json!(status.coverage_start_ms));
                object.insert("coverage_end_ms".into(), json!(status.coverage_end_ms));
                object.insert("url_statuses".into(), json!(status.urls));
            }
            value
        })
        .collect();
    Json(json!(items))
}

/// `POST /panel/api/epg/sources`——新增或更新一个源。
pub async fn source_save(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(source): Json<Source>,
) -> Result<Json<Value>, ApiError> {
    match state.epg_store.save_source(source) {
        Ok(source) => Ok(Json(json!({ "ok": true, "source": source }))),
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `DELETE /panel/api/epg/sources/{id}`——删除源及其绑定。
pub async fn source_delete(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    match state.epg_store.delete_source(&id) {
        Ok(true) => Ok(Json(json!({ "ok": true }))),
        Ok(false) => Err(ApiError::not_found("EPG 源不存在")),
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}

/// `POST /panel/api/epg/sources/{id}/refresh`——只采集一个源。
pub async fn source_refresh(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    match epg::refresh_source(&state, &id).await {
        Ok(summary) => Ok(Json(json!({
            "ok": true,
            "channels": summary.channels,
            "programmes": summary.kept,
        }))),
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `GET /panel/api/epg/bindings`——全部本地频道与绑定状态。
pub async fn bindings(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let bindings: HashMap<String, Binding> = state
        .epg_store
        .bindings()
        .into_iter()
        .map(|binding| (binding.key(), binding))
        .collect();
    let sources = state.epg_store.source_names();
    let matches: HashMap<String, epg::MatchRow> = state
        .epg
        .match_rows
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .map(|row| (format!("{}/{}", row.gid, row.cid), row.clone()))
        .collect();
    let mut items = Vec::new();
    for group in &config.groups {
        for channel in &group.channels {
            let key = format!("{}/{}", group.id, channel.id);
            let binding = bindings.get(&key);
            let matched = matches.get(&key);
            items.push(json!({
                "gid": group.id,
                "cid": channel.id,
                "name": channel.name,
                "group_id": group.id,
                "group_name": group.name,
                "out_id": epg::channel_out_id(channel),
                "source_id": binding.map(|value| value.source_id.clone()).unwrap_or_default(),
                "source_name": binding
                    .and_then(|value| sources.get(&value.source_id).cloned())
                    .unwrap_or_default(),
                "raw_channel_id": binding.map(|value| value.raw_channel_id.clone()).unwrap_or_default(),
                "upstream_display_name": binding
                    .map(|value| value.upstream_display_name.clone())
                    .unwrap_or_default(),
                "origin": binding.map(|value| value.origin.clone()).unwrap_or_else(|| "none".into()),
                "updated_at_ms": binding.map(|value| value.updated_at_ms).unwrap_or(0),
                "match_via": matched.map(|row| row.via.clone()).unwrap_or_default(),
                "programmes": matched.map(|row| row.programmes).unwrap_or(0),
            }));
        }
    }
    let bound = items
        .iter()
        .filter(|item| {
            item["origin"]
                .as_str()
                .is_some_and(|origin| origin != "none")
        })
        .count();
    let total = items.len();
    Json(json!({ "items": items, "bound": bound, "total": total }))
}

/// `PUT /panel/api/epg/bindings/{gid}/{cid}`——手工绑定或清除。
pub async fn binding_set(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let text = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    match epg::set_mapping(
        &state,
        &gid,
        &cid,
        &text("source_id"),
        &text("raw_channel_id"),
    ) {
        Ok(Some(mapping)) => {
            spawn_regenerate(&state);
            Ok(Json(json!({ "ok": true, "mapping": mapping })))
        }
        Ok(None) => {
            spawn_regenerate(&state);
            Ok(Json(json!({ "ok": true, "cleared": true })))
        }
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `DELETE /panel/api/epg/bindings/{gid}/{cid}`——清除绑定。
pub async fn binding_delete(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    match state.epg_store.remove_binding(&gid, &cid) {
        Ok(removed) => {
            if removed {
                spawn_regenerate(&state);
            }
            Ok(Json(json!({ "ok": true, "removed": removed })))
        }
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}

/// `POST /panel/api/epg/bindings/{gid}/{cid}/copy`——复用另一个频道的绑定。
pub async fn binding_copy(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path((gid, cid)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let from_gid = body
        .get("from_gid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let from_cid = body
        .get("from_cid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let Some(binding) = state.epg_store.binding(from_gid, from_cid) else {
        return Err(ApiError::not_found("源频道没有绑定"));
    };
    match epg::set_mapping(
        &state,
        &gid,
        &cid,
        &binding.source_id,
        &binding.raw_channel_id,
    ) {
        Ok(_) => {
            spawn_regenerate(&state);
            Ok(Json(json!({ "ok": true })))
        }
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `GET /panel/api/epg/rules`——关键字规则列表。
pub async fn rules(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let items: Vec<Value> = state
        .epg_store
        .rules()
        .into_iter()
        .map(|rule| {
            let group = config.group(&rule.gid);
            let channel = group
                .and_then(|group| group.channels.iter().find(|channel| channel.id == rule.cid));
            json!({
                "rule": rule,
                "group_name": group.map(|group| group.name.clone()).unwrap_or_default(),
                "channel_name": channel.map(|channel| channel.name.clone()).unwrap_or_default(),
            })
        })
        .collect();
    Json(json!(items))
}

/// `POST /panel/api/epg/rules`——新增或更新规则。
pub async fn rule_save(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(rule): Json<Rule>,
) -> Result<Json<Value>, ApiError> {
    if let Err(error) = epg_rules::validate(&state, &rule) {
        return Err(ApiError::bad_request(format!("{error:#}")));
    }
    match state.epg_store.save_rule(rule) {
        Ok(rule) => Ok(Json(json!({ "ok": true, "rule": rule }))),
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}

/// `DELETE /panel/api/epg/rules/{id}`——删除规则与待执行任务。
pub async fn rule_delete(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    match state.epg_store.delete_rule(&id) {
        Ok(true) => {
            let removed = state.schedule.remove_rule_tasks(&id);
            Ok(Json(json!({ "ok": true, "removed_tasks": removed })))
        }
        Ok(false) => Err(ApiError::not_found("规则不存在")),
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}

/// `POST /panel/api/epg/rules/preview`——预览未来 48 小时的命中。
pub async fn rule_preview(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(rule): Json<Rule>,
) -> Result<Json<Value>, ApiError> {
    if let Err(error) = epg_rules::validate(&state, &rule) {
        return Err(ApiError::bad_request(format!("{error:#}")));
    }
    match epg_rules::preview(&state, &rule) {
        Ok(value) => Ok(Json(value)),
        Err(error) => Err(ApiError::bad_request(format!("{error:#}"))),
    }
}
