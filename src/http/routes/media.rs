//! 素材与脚本管理：视频、字幕、解析脚本；附带 ffmpeg 检测。
//!
//! 目标路由只登记了存在性（F-011、S-EP-16），响应形状为本实现自定。
//! 视频与字幕存在 `<cache>/{assets,subtitles}/`，解析脚本存在 `<cache>/resolvers/`。

use std::{path::Path as FsPath, sync::Arc};

use axum::{
    extract::{OriginalUri, Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::{
    assets::{self, Kind},
    http::{error::ApiError, ext::PanelAuth},
    state::AppState,
};

/// 由请求路径判定素材种类（路由静态注册，避免与 `/panel/api/*rest` 冲突）。
fn kind_of(uri: &axum::http::Uri) -> Result<Kind, ApiError> {
    let path = uri.path();
    if path.starts_with("/panel/api/assets/") {
        Ok(Kind::Video)
    } else if path.starts_with("/panel/api/subtitle/") {
        Ok(Kind::Subtitle)
    } else {
        Err(ApiError::not_found("未知素材类型"))
    }
}

/// `GET /panel/api/{assets,subtitles}/list`。
pub async fn list(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<Value>, ApiError> {
    let kind = kind_of(&uri)?;
    let entries = assets::list(&state.snapshot().cache.disk_path, kind);
    Ok(Json(json!({
        "kind": kind.dir_name(),
        "count": entries.len(),
        "files": entries,
    })))
}

/// `POST /panel/api/{assets,subtitles}/upload`——`{"items":[{"name","ext","data"}]}`。
pub async fn upload(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let kind = kind_of(&uri)?;
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
        let bytes = match crate::logo::decode_data_url(data) {
            Ok((_, bytes)) => bytes,
            Err(error) => {
                failed.push(json!({ "name": name, "error": format!("{error:#}") }));
                continue;
            }
        };
        let base = name.rsplit_once('.').map(|(base, _)| base).unwrap_or(name);
        match assets::write(&cache, kind, base, &ext, &bytes) {
            Ok(entry) => added.push(json!(entry)),
            Err(error) => failed.push(json!({ "name": name, "error": format!("{error:#}") })),
        }
    }
    Ok(Json(
        json!({ "ok": true, "added": added, "failed": failed }),
    ))
}

/// `POST /panel/api/{assets,subtitles}/remove`——`{"file"}`。
pub async fn remove(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let kind = kind_of(&uri)?;
    let file = body
        .get("file")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if file.trim().is_empty() {
        return Err(ApiError::bad_request("file 不能为空"));
    }
    let cache = state.snapshot().cache.disk_path.clone();
    match assets::remove(&cache, kind, &file) {
        Ok(true) => Ok(Json(json!({ "ok": true, "removed": 1 }))),
        Ok(false) => Err(ApiError::not_found("文件不存在")),
        Err(error) => Err(ApiError::internal(format!("{error:#}"))),
    }
}

/// `GET /panel/api/{assets,subtitles}/raw/<file>`——面板内下载。
pub async fn raw(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(file): Path<String>,
) -> Result<Response, ApiError> {
    let kind = kind_of(&uri)?;
    let cache = state.snapshot().cache.disk_path.clone();
    let bytes = assets::read(&cache, kind, &file).map_err(|error| {
        let message = format!("{error:#}");
        if message.contains("不存在") {
            ApiError::not_found("文件不存在")
        } else {
            ApiError::bad_request(message)
        }
    })?;
    let mime = mime_guess::from_path(FsPath::new(&file))
        .first_or_octet_stream()
        .to_string();
    let mut response = (StatusCode::OK, [("content-type", mime)], bytes).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0"),
    );
    Ok(response)
}

/// `GET /panel/api/assets/ffmpeg-status`——检测外部 `ffmpeg`。
pub async fn ffmpeg_status(PanelAuth(_): PanelAuth) -> Json<Value> {
    let output = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .ok();
    let installed = output
        .as_ref()
        .map(|out| out.status.success())
        .unwrap_or(false);
    let version = output
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_string()
        })
        .filter(|line| !line.is_empty());
    Json(json!({
        "installed": installed,
        "version": version,
        "remux_ready": installed,
        "note": if installed { "可转封装" } else { "未安装 ffmpeg；remux/dash 保持 503 预热" },
    }))
}

/// `GET /panel/api/resolver/scripts`。
pub async fn resolver_list(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let directory = std::path::Path::new(&state.snapshot().cache.disk_path).join("resolvers");
    let mut scripts: Vec<Value> = std::fs::read_dir(&directory)
        .map(|read| {
            read.filter_map(|item| item.ok())
                .filter_map(|item| {
                    let name = item.file_name().to_string_lossy().to_string();
                    if !name.ends_with(".py") {
                        return None;
                    }
                    let bytes = item.metadata().ok().map(|meta| meta.len()).unwrap_or(0);
                    Some(json!({ "name": name, "bytes": bytes }))
                })
                .collect()
        })
        .unwrap_or_default();
    scripts.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Json(json!({ "count": scripts.len(), "scripts": scripts }))
}

/// `POST /panel/api/resolver/script`——`{"name","content"}`；写 `<cache>/resolvers/<name>.py`。
pub async fn resolver_save(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let content = body
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base = name.trim_end_matches(".py");
    let safe = crate::logo::sanitize(base).ok_or_else(|| ApiError::bad_request("文件名不合法"))?;
    if content.is_empty() {
        return Err(ApiError::bad_request("content 不能为空"));
    }
    let directory = std::path::Path::new(&state.snapshot().cache.disk_path).join("resolvers");
    std::fs::create_dir_all(&directory)
        .map_err(|error| ApiError::internal(format!("建目录失败: {error}")))?;
    let file = format!("{safe}.py");
    std::fs::write(directory.join(&file), &content)
        .map_err(|error| ApiError::internal(format!("写入失败: {error}")))?;
    Ok(Json(
        json!({ "ok": true, "name": file, "bytes": content.len() }),
    ))
}

/// `POST /panel/api/resolver/remove`——`{"name"}`。
pub async fn resolver_remove(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let base = name.trim_end_matches(".py");
    let safe = crate::logo::sanitize(base).ok_or_else(|| ApiError::bad_request("文件名不合法"))?;
    let path = std::path::Path::new(&state.snapshot().cache.disk_path)
        .join("resolvers")
        .join(format!("{safe}.py"));
    if !path.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    std::fs::remove_file(&path)
        .map_err(|error| ApiError::internal(format!("删除失败: {error}")))?;
    Ok(Json(json!({ "ok": true, "removed": 1 })))
}

/// `POST /subtitle/mtran/test`——字幕机翻自检（S-SUB-03）。
pub async fn mtran_test(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("hello")
        .to_string();
    let config = state.snapshot();
    let base = config
        .extra
        .get("subtitle")
        .and_then(|table| table.get("translate"))
        .and_then(|table| table.get("url"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "请先填写 MTranServer 地址(字幕机翻只支持本地 MTranServer)"
            })),
        )
            .into_response();
    }
    let endpoint = format!("{base}/translate/batch");
    let result = state
        .http
        .post(&endpoint)
        .header(header::CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_vec(&json!({
                "texts": [text],
                "source_lang": "auto",
                "target_lang": "zh-cn",
            }))
            .unwrap_or_default(),
        )
        .send()
        .await;
    let (ok, error) = match result {
        Ok(response) if response.status().is_success() => (true, String::new()),
        Ok(response) => (false, format!("请求失败: HTTP {}", response.status())),
        Err(error) => (false, format!("请求失败: {error}")),
    };
    Json(json!({
        "base": base,
        "ok": ok,
        "targets": [{
            "code": "zh-Hans",
            "lang": "zh-cn",
            "ok": ok,
            "error": error,
        }],
        "version": null,
    }))
    .into_response()
}

/// `POST /subtitle/translate/clear`——清空字幕翻译缓存。
pub async fn translate_clear(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cleared = remove_files(&state.cache_dir.join("subtitles").join(".translate"));
    Json(json!({ "ok": true, "cleared": cleared }))
}

fn remove_files(dir: &FsPath) -> usize {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for item in read.flatten() {
        let path = item.path();
        if path.is_dir() {
            count += remove_files(&path);
            let _ = std::fs::remove_dir(&path);
        } else if std::fs::remove_file(&path).is_ok() {
            count += 1;
        }
    }
    count
}
