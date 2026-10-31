//! 素材与脚本管理：视频、字幕、解析脚本；附带 ffmpeg 检测。
//!
//! 目标路由只登记了存在性（F-011、S-EP-16），响应形状为本实现自定。
//! 视频与字幕存在 `<cache>/{assets,subtitles}/`，解析脚本存在 `<cache>/resolvers/`。

use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};

use axum::{
    extract::{OriginalUri, Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    assets::{self, Kind},
    http::{error::ApiError, ext::PanelAuth},
    state::AppState,
    subtitle, translate,
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
    let config = state.snapshot();
    let entries = assets::list(&config.cache.disk_path, kind);
    if kind == Kind::Subtitle {
        return Ok(Json(json!({
            "kind": kind.dir_name(),
            "count": entries.len(),
            "files": entries,
        })));
    }
    let map = config.asset_map();
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for url in asset_urls(&config) {
        if !seen.insert(url.clone()) {
            continue;
        }
        let local = map.get(&url);
        items.push(json!({
            "url": url,
            "converted": local.is_some(),
            "local_path": local,
        }));
    }
    let ffmpeg = ffmpeg_probe().await;
    Ok(Json(json!({
        "kind": kind.dir_name(),
        "count": entries.len(),
        "files": entries,
        "items": items,
        "ffmpeg_installed": ffmpeg.is_some(),
        "ffmpeg_path": ffmpeg,
        "orphans": [],
    })))
}

/// 从配置收集可能需要转换的视频 URL。
fn asset_urls(config: &crate::config::Config) -> Vec<String> {
    let mut urls = Vec::new();
    let mut push = |url: &str| {
        let url = url.trim();
        if url.starts_with("http://") || url.starts_with("https://") {
            let path = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
            if [".mp4", ".mkv", ".mov", ".webm", ".m4v", ".flv"]
                .iter()
                .any(|ext| path.ends_with(ext))
            {
                urls.push(url.to_string());
            }
        }
    };
    push(&config.server.not_found_url);
    push(&config.server.error_url);
    push(&config.server.home_url);
    push(&config.auth.token_failure_url);
    push(&config.auth.limit_url);
    push(&config.auth.ban_url);
    for group in &config.groups {
        for channel in &group.channels {
            push(&channel.url);
        }
    }
    urls
}

async fn ffmpeg_probe() -> Option<String> {
    let output = tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .await
        .ok()?;
    output.status.success().then(|| "ffmpeg".to_string())
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
    if kind == Kind::Video {
        if let Some(url) = body
            .get("url")
            .and_then(Value::as_str)
            .filter(|url| !url.trim().is_empty())
        {
            return remove_asset_url(&state, url.trim()).await;
        }
    }
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

/// 字幕翻译配置（读 `[subtitle.translate]`）。
#[derive(Debug, Clone)]
struct SubtitleTranslate {
    enabled: bool,
    url: String,
    token: String,
    source: String,
    targets: Vec<String>,
    translation_first: bool,
    batch: usize,
}

impl SubtitleTranslate {
    /// 请求指定的目标；缺省取第一项目标语言。
    fn target(&self, requested: Option<&str>) -> String {
        requested
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| self.targets.first().cloned())
            .unwrap_or_else(|| "zh-cn".to_string())
    }
}

fn subtitle_translate(config: &crate::config::Config) -> SubtitleTranslate {
    let cfg = &config.subtitle.translate;
    let enabled = cfg.enabled;
    let url = cfg.url.trim().trim_end_matches('/').to_string();
    let token = cfg.token.trim().to_string();
    let source = cfg.source.clone();
    let translation_first = cfg.translation_first;
    let mut targets: Vec<String> = cfg
        .targets
        .iter()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect();
    if targets.is_empty() {
        let target = cfg.target.trim();
        if !target.is_empty() {
            targets.push(target.to_string());
        }
    }
    if targets.is_empty() {
        targets.push("zh-cn".to_string());
    }
    let batch = cfg.batch.clamp(1, 500) as usize;
    SubtitleTranslate {
        enabled,
        url,
        token,
        source,
        targets,
        translation_first,
        batch,
    }
}

fn subtitle_cache_dir(state: &AppState) -> PathBuf {
    state.cache_dir.join("subtitles").join(".translate")
}

/// 缓存文件名：源文件 + 目标 + 排列 + 格式的哈希。
fn subtitle_cache_name(
    file: &str,
    target: &str,
    translation_first: bool,
    format: subtitle::Format,
) -> String {
    let digest =
        Sha256::digest(format!("{file}\u{1f}{target}\u{1f}{translation_first}").as_bytes());
    format!("{}.{}", &hex::encode(digest)[..32], format.ext())
}

/// `GET /panel/api/subtitle/translate/status`——字幕翻译能力。
pub async fn translate_status(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    let cfg = subtitle_translate(&config);
    let cached = std::fs::read_dir(subtitle_cache_dir(&state))
        .map(|read| {
            read.filter_map(|item| item.ok())
                .filter(|item| item.path().is_file())
                .count()
        })
        .unwrap_or(0);
    Json(json!({
        "enabled": cfg.enabled,
        "engine": if cfg.url.is_empty() { "none" } else { "mtran" },
        "url": cfg.url,
        "source": cfg.source,
        "targets": cfg.targets,
        "translation_first": cfg.translation_first,
        "formats": ["srt", "vtt", "ass", "ssa"],
        "converts_ass": true,
        "cached": cached,
    }))
}

/// `POST /panel/api/subtitle/translate`——翻译一个已上传的字幕文件。
pub async fn translate_subtitle(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let file = body
        .get("file")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if file.is_empty() {
        return Err(ApiError::bad_request("file 不能为空"));
    }
    let config = state.snapshot();
    let cfg = subtitle_translate(&config);
    if !cfg.enabled {
        return Err(ApiError::bad_request("字幕翻译未启用"));
    }
    if cfg.url.is_empty() {
        return Err(ApiError::bad_request(
            "请先填写 MTranServer 地址(字幕机翻只支持本地 MTranServer)",
        ));
    }
    let target = cfg.target(body.get("target").and_then(Value::as_str));
    let bytes = assets::read(&config.cache.disk_path, Kind::Subtitle, &file).map_err(|error| {
        let message = format!("{error:#}");
        if message.contains("不存在") {
            ApiError::not_found("文件不存在")
        } else {
            ApiError::bad_request(message)
        }
    })?;
    let text =
        String::from_utf8(bytes).map_err(|_| ApiError::bad_request("字幕不是 UTF-8 文本"))?;
    let detected =
        subtitle::detect(&text).map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    let (text, format) = if detected == subtitle::Format::Ass {
        let converted = subtitle::convert_ass_to_vtt(&text)
            .map_err(|error| ApiError::bad_request(format!("ASS 转换失败: {error:#}")))?;
        (converted, subtitle::Format::Vtt)
    } else {
        (text, detected)
    };

    let directory = subtitle_cache_dir(&state);
    let name = subtitle_cache_name(&file, &target, cfg.translation_first, format);
    let path = directory.join(&name);
    let download = format!("/panel/api/subtitle/translate/raw/{name}");
    if path.is_file() {
        let bytes = std::fs::read(&path)
            .map_err(|error| ApiError::internal(format!("读缓存失败: {error}")))?;
        return Ok(Json(json!({
            "ok": true,
            "file": file,
            "target": target,
            "format": format.ext(),
            "cached": true,
            "bytes": bytes.len(),
            "download": download,
        })));
    }

    let lines = subtitle::text_lines(&text);
    if lines.is_empty() {
        return Err(ApiError::bad_request("没有可翻译的字幕文本"));
    }
    let mut translations: HashMap<String, String> = HashMap::new();
    for chunk in lines.chunks(cfg.batch) {
        let values = translate::batch_with_token(
            &state.http,
            &cfg.url,
            &cfg.source,
            &target,
            chunk,
            Some(cfg.token.as_str()),
        )
        .await
        .map_err(|error| ApiError::internal(format!("翻译失败: {error:#}")))?;
        for (line, value) in chunk.iter().zip(values) {
            translations.insert(line.clone(), value);
        }
    }
    let output = subtitle::render(&text, &translations, cfg.translation_first);
    std::fs::create_dir_all(&directory)
        .map_err(|error| ApiError::internal(format!("建目录失败: {error}")))?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, output.as_bytes())
        .map_err(|error| ApiError::internal(format!("写缓存失败: {error}")))?;
    std::fs::rename(&temp, &path)
        .map_err(|error| ApiError::internal(format!("保存缓存失败: {error}")))?;
    Ok(Json(json!({
        "ok": true,
        "file": file,
        "target": target,
        "format": format.ext(),
        "cached": false,
        "lines": lines.len(),
        "bytes": output.len(),
        "download": download,
    })))
}

/// `GET /panel/api/subtitle/translate/raw/<name>`——下载翻译结果。
pub async fn translate_raw(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return Err(ApiError::bad_request("文件名不合法"));
    };
    if stem.len() != 32 || !stem.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("文件名不合法"));
    }
    if !matches!(ext, "srt" | "vtt") {
        return Err(ApiError::bad_request("文件名不合法"));
    }
    let path = subtitle_cache_dir(&state).join(&name);
    let bytes = std::fs::read(&path).map_err(|_| ApiError::not_found("文件不存在"))?;
    let mime = if ext == "vtt" {
        "text/vtt"
    } else {
        "application/x-subrip"
    };
    let mut response = (StatusCode::OK, [("content-type", mime)], bytes).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0"),
    );
    Ok(response)
}

/// `POST /subtitle/mtran/test`——字幕机翻自检（S-SUB-03）。
pub async fn mtran_test(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("hello")
        .to_string();
    let config = state.snapshot();
    let cfg = subtitle_translate(&config);
    if cfg.url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "请先填写 MTranServer 地址(字幕机翻只支持本地 MTranServer)"
            })),
        )
            .into_response();
    }
    let target = cfg.target(None);
    let translated = translate::batch_with_token(
        &state.http,
        &cfg.url,
        "auto",
        &target,
        &[text],
        Some(cfg.token.as_str()),
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
        "base": cfg.url,
        "ok": ok,
        "result": result,
        "targets": [{
            "code": "zh-Hans",
            "lang": target,
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

/// 把一个上游 URL 转成本地 HLS，并写入 `[assets].map`。
pub async fn convert(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let url = body
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ApiError::bad_request("url 必须是 http(s) 地址"));
    }
    if ffmpeg_probe().await.is_none() {
        return Err(ApiError::internal("未检测到 ffmpeg"));
    }
    let config = state.snapshot();
    let digest = hex::encode(Sha256::digest(url.as_bytes()));
    let digest = &digest[..12];
    let root = FsPath::new(&config.cache.disk_path)
        .join("assets")
        .join("convert")
        .join(digest);
    let _ = tokio::fs::remove_dir_all(&root).await;
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|error| ApiError::internal(format!("建目录失败: {error}")))?;
    let input = root.join("input.mp4");
    let response = state
        .http
        .get(&url)
        .send()
        .await
        .map_err(|error| ApiError::internal(format!("下载失败: {error}")))?
        .error_for_status()
        .map_err(|error| ApiError::internal(format!("上游状态异常: {error}")))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ApiError::internal(format!("读取正文失败: {error}")))?;
    if bytes.is_empty() {
        return Err(ApiError::internal("上游正文为空"));
    }
    tokio::fs::write(&input, &bytes)
        .await
        .map_err(|error| ApiError::internal(format!("写临时文件失败: {error}")))?;

    let playlist = root.join("index.m3u8");
    let segments = root.join("seg_%05d.ts");
    let mut output = run_ffmpeg(&input, &playlist, &segments, true).await;
    if output
        .as_ref()
        .map(|out| !out.status.success())
        .unwrap_or(true)
    {
        output = run_ffmpeg(&input, &playlist, &segments, false).await;
    }
    let _ = tokio::fs::remove_file(&input).await;
    let output =
        output.map_err(|error| ApiError::internal(format!("执行 ffmpeg 失败: {error}")))?;
    if !output.status.success() || !playlist.is_file() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ApiError::internal(format!(
            "ffmpeg 转换失败: {}",
            stderr.trim().chars().take(500).collect::<String>()
        )));
    }
    let local = format!("/assets/convert/{digest}/index.m3u8");
    set_asset_map(&state, &url, Some(&local))
        .map_err(|error| ApiError::internal(format!("写配置失败: {error:#}")))?;
    Ok(Json(json!({
        "ok": true,
        "url": url,
        "local_path": local,
        "bytes": bytes.len(),
    })))
}

/// 配置里 5 个占位跳转 URL 的清单。
///
/// 这些地址常指向 MP4。部分播放器不显示 MP4，所以要能逐个转成本地 HLS。
const PLACEHOLDER_KEYS: [(&str, &str); 5] = [
    ("server.not_found_url", "404 跳转（频道不存在）"),
    ("server.error_url", "上游错误跳转"),
    ("auth.token_failure_url", "Token 失效跳转"),
    ("auth.limit_url", "IP 限流跳转"),
    ("auth.ban_url", "IP 封禁跳转"),
];

/// `GET /panel/api/assets/candidates`——列出配置里可转 HLS 的占位 URL。
pub async fn candidates(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    let map = asset_map(&config);
    let urls: [&str; 5] = [
        &config.server.not_found_url,
        &config.server.error_url,
        &config.auth.token_failure_url,
        &config.auth.limit_url,
        &config.auth.ban_url,
    ];
    let items: Vec<Value> = PLACEHOLDER_KEYS
        .iter()
        .zip(urls)
        .map(|((key, label), url)| {
            let local = map.get(url).cloned().unwrap_or_default();
            json!({
                "key": key,
                "label": label,
                "url": url,
                "local_path": local,
                "converted": !local.is_empty(),
            })
        })
        .collect();
    Json(json!({ "items": items }))
}

/// 读取 `[assets].map`：上游 URL → 本地路径。
fn asset_map(config: &crate::config::model::Config) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(table) = config
        .extra
        .get("assets")
        .and_then(serde_json::Value::as_object)
    else {
        return out;
    };
    let Some(map) = table.get("map").and_then(serde_json::Value::as_object) else {
        return out;
    };
    for (url, local) in map {
        if let Some(local) = local.as_str() {
            if !local.is_empty() {
                out.insert(url.clone(), local.to_string());
            }
        }
    }
    out
}

async fn run_ffmpeg(
    input: &FsPath,
    playlist: &FsPath,
    segments: &FsPath,
    copy: bool,
) -> std::io::Result<std::process::Output> {
    let mut command = tokio::process::Command::new("ffmpeg");
    command.arg("-y").arg("-i").arg(input);
    if copy {
        command.args(["-c", "copy"]);
    } else {
        command.args(["-c:v", "libx264", "-c:a", "aac"]);
    }
    command
        .args(["-f", "hls", "-hls_time", "6", "-hls_playlist_type", "vod"])
        .arg("-hls_segment_filename")
        .arg(segments)
        .arg(playlist);
    match tokio::time::timeout(std::time::Duration::from_secs(1_800), command.output()).await {
        Ok(output) => output,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "ffmpeg 超时",
        )),
    }
}

fn set_asset_map(state: &Arc<AppState>, url: &str, local: Option<&str>) -> anyhow::Result<()> {
    state.mutate(|config| {
        let assets = config
            .extra
            .entry("assets".to_string())
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if !assets.is_object() {
            *assets = serde_json::Value::Object(Default::default());
        }
        let table = assets.as_object_mut().expect("assets 表");
        let map = table
            .entry("map".to_string())
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if !map.is_object() {
            *map = serde_json::Value::Object(Default::default());
        }
        let map = map.as_object_mut().expect("map 表");
        match local {
            Some(local) => {
                map.insert(
                    url.to_string(),
                    serde_json::Value::String(local.to_string()),
                );
            }
            None => {
                map.remove(url);
            }
        }
        Ok(())
    })
}

async fn remove_asset_url(state: &Arc<AppState>, url: &str) -> Result<Json<Value>, ApiError> {
    let config = state.snapshot();
    let local = config.asset_local(url).map(str::to_string);
    set_asset_map(state, url, None)
        .map_err(|error| ApiError::internal(format!("写配置失败: {error:#}")))?;
    if let Some(local) = local {
        if let Some(relative) = local.strip_prefix("/assets/") {
            let path = FsPath::new(&config.cache.disk_path)
                .join("assets")
                .join(relative);
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::remove_dir_all(parent).await;
            }
        }
    }
    Ok(Json(json!({ "ok": true, "removed": 1 })))
}

/// `GET /assets/*path`——公开读取本地 HLS 入口与切片。
pub async fn served(State(state): State<Arc<AppState>>, Path(path): Path<String>) -> Response {
    let cache = state.snapshot().cache.disk_path.clone();
    let mut full = FsPath::new(&cache).join("assets");
    for part in path.split('/') {
        let Some(part) = crate::logo::sanitize(part) else {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        };
        if part == "." || part == ".." {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
        full.push(part);
    }
    let Ok(bytes) = tokio::fs::read(&full).await else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mime = match full.extension().and_then(|ext| ext.to_str()) {
        Some("m3u8") => "application/vnd.apple.mpegurl",
        Some("ts") => "video/mp2t",
        Some("m4s") => "video/iso.segment",
        Some("mp4") => "video/mp4",
        Some("vtt") => "text/vtt",
        _ => "application/octet-stream",
    };
    let mut response = ([(header::CONTENT_TYPE, mime)], bytes).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}
