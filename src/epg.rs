//! 节目单（EPG）：拉取 XMLTV、按时间窗裁剪、匹配本服务频道、生成 `/epg.xml`。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §6.5。
//! 证据：F-008（S-EP-05、S-EP-12）。
//!
//! 输出只保留能匹配到本服务频道的频道与节目。生成文件放在
//! `<cache>/epg/epg.xml` 与 `epg.xml.gz`，由公开路由直接下发。

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use roxmltree::Document;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{config::Config, state::AppState};

/// EPG 运行状态（指标与最近一次结果）。
#[derive(Default)]
pub struct EpgState {
    /// 采集进行中。
    pub running: AtomicBool,
    /// 最近一次采集的频道数与节目数。
    pub channels: AtomicUsize,
    pub programmes: AtomicUsize,
    pub kept: AtomicUsize,
    /// 最近一次成功时间（Unix 秒）。
    pub last_success: Mutex<Option<u64>>,
    /// 最近一次错误。
    pub last_error: Mutex<Option<String>>,
    /// 额外频道：不进本地频道表，但写入节目单输出。
    pub extras: Mutex<Vec<Extra>>,
    /// 最近一次匹配报告。
    pub match_rows: Mutex<Vec<MatchRow>>,
    /// 每个源的采集状态。键是源 ID。
    pub source_status: Mutex<HashMap<String, SourceStatus>>,
    /// 已生成节目单的解析结果，按文件 mtime 与长度失效。
    pub generated: Mutex<Option<Generated>>,
}

/// 单个 URL 的采集状态。
#[derive(Debug, Clone, Default, Serialize)]
pub struct UrlStatus {
    pub url: String,
    pub priority: i64,
    pub last_fetch_started_at_ms: Option<i64>,
    pub last_fetch_finished_at_ms: Option<i64>,
    pub last_fetch_ok: bool,
    pub last_error: Option<String>,
    pub channels: usize,
    pub programmes: usize,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// 一个源的采集状态。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SourceStatus {
    pub last_fetch_started_at_ms: Option<i64>,
    pub last_fetch_finished_at_ms: Option<i64>,
    pub last_fetch_ok: bool,
    pub last_error: Option<String>,
    pub channels: usize,
    pub programmes: usize,
    pub coverage_start_ms: Option<i64>,
    pub coverage_end_ms: Option<i64>,
    pub urls: Vec<UrlStatus>,
    /// 最近一次成功采集的 ETag / Last-Modified 快照，供条件请求复用。
    #[serde(skip)]
    pub cache_headers: HashMap<String, (Option<String>, Option<String>)>,
}

/// 缓存的解析结果。
pub struct Generated {
    modified: Option<SystemTime>,
    len: u64,
    data: Arc<DocumentData>,
}

/// 读已生成的节目单，带进程内缓存。
///
/// 每请求重新解析几 MB XML 太贵。文件没变就复用上次的解析结果。
/// 缓存键是 `(mtime, 长度)`：EPG 刷新是小时级，同秒同大小重写的概率可以忽略。
pub fn load_generated_cached(state: &AppState) -> Option<Arc<DocumentData>> {
    let config = state.snapshot();
    let path = output_file(&config);
    let meta = fs::metadata(&path).ok()?;
    let modified = meta.modified().ok();
    let len = meta.len();

    if let Some(cached) = state
        .epg
        .generated
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        if cached.modified == modified && cached.len == len {
            return Some(cached.data.clone());
        }
    }

    let body = fs::read_to_string(&path).ok()?;
    let data = Arc::new(parse(&body).ok()?);
    *state
        .epg
        .generated
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(Generated {
        modified,
        len,
        data: data.clone(),
    });
    Some(data)
}

/// 额外频道（`POST /panel/api/epg/extra`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Extra {
    pub source_id: String,
    pub source_channel_id: String,
    #[serde(default)]
    pub out_id: String,
    #[serde(default)]
    pub out_name: String,
}

impl Extra {
    /// 输出标识；空时回退源频道 ID。
    pub fn output_id(&self) -> String {
        if self.out_id.trim().is_empty() {
            self.source_channel_id.to_ascii_lowercase()
        } else {
            self.out_id.trim().to_ascii_lowercase()
        }
    }
}

/// 手工 EPG 映射。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mapping {
    pub source_id: String,
    pub source_channel_id: String,
    pub source_name: String,
    pub source_display_name: String,
}

/// 一条匹配报告。
#[derive(Debug, Clone, Default)]
pub struct MatchRow {
    pub gid: String,
    pub cid: String,
    pub name: String,
    pub group_name: String,
    pub out_id: String,
    pub via: String,
    pub source_id: String,
    pub raw_channel_id: String,
    pub upstream_display_name: String,
    pub programmes: usize,
}

/// 公开频道表的一行。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViewerChannel {
    pub icon: Option<String>,
    pub id: String,
    pub kind: String,
    pub last_stop: i64,
    pub name: String,
    pub programme_count: usize,
    pub source_id: String,
    pub source_name: String,
}

/// 公开频道表快照。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViewerData {
    pub generated_at: u64,
    pub channels: Vec<ViewerChannel>,
}

/// 生成过程中的频道来源信息。
#[derive(Debug, Clone, Default)]
struct ViewerMeta {
    name: String,
    source_id: String,
    source_name: String,
}

/// 公开频道表快照文件。
pub fn viewer_file(cache_dir: &Path) -> PathBuf {
    output_dir(cache_dir).join("viewer.json")
}

/// 读取公开频道表；缺失时从生成的 XML 反推最小形状。
pub fn load_viewer(config: &Config) -> Option<ViewerData> {
    let path = viewer_file(Path::new(&config.cache.disk_path));
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(viewer) = serde_json::from_slice::<ViewerData>(&bytes) {
            return Some(viewer);
        }
    }
    let document = load_generated(config).ok()?;
    let generated_at = fs::metadata(output_file(config))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Some(viewer_from_document(&document, generated_at))
}

/// 从生成的 XMLTV 反推公开频道表。
fn viewer_from_document(document: &DocumentData, generated_at: u64) -> ViewerData {
    let mut channels = Vec::new();
    for channel in &document.channels {
        let mut count = 0usize;
        let mut last_stop = 0i64;
        for programme in document
            .programmes
            .iter()
            .filter(|programme| programme.channel == channel.id)
        {
            count += 1;
            last_stop = last_stop.max(parse_time(&programme.stop).unwrap_or(0));
        }
        channels.push(ViewerChannel {
            icon: channel_icon(channel),
            id: channel.id.to_ascii_lowercase(),
            kind: "local".to_string(),
            last_stop,
            name: channel
                .names
                .first()
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| channel.id.clone()),
            programme_count: count,
            source_id: String::new(),
            source_name: String::new(),
        });
    }
    ViewerData {
        generated_at,
        channels,
    }
}

/// 频道图标地址。
pub fn channel_icon(channel: &Channel) -> Option<String> {
    for raw in &channel.raw_children {
        let wrapper = format!("<x>{raw}</x>");
        let Ok(document) = Document::parse(&wrapper) else {
            continue;
        };
        let Some(node) = document
            .root_element()
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "icon")
        else {
            continue;
        };
        if let Some(src) = node
            .attribute("src")
            .map(str::trim)
            .filter(|src| !src.is_empty())
        {
            return Some(src.to_string());
        }
    }
    None
}

/// 构建公开频道表快照。
fn viewer_snapshot(
    channels: &[Channel],
    programmes: &[Programme],
    meta: &HashMap<String, ViewerMeta>,
    generated_at: u64,
) -> ViewerData {
    let mut out = Vec::new();
    for channel in channels {
        let Some(source) = meta.get(&channel.id) else {
            continue;
        };
        let mut count = 0usize;
        let mut last_stop = 0i64;
        for programme in programmes
            .iter()
            .filter(|programme| programme.channel == channel.id)
        {
            count += 1;
            last_stop = last_stop.max(parse_time(&programme.stop).unwrap_or(0));
        }
        out.push(ViewerChannel {
            icon: channel_icon(channel),
            id: channel.id.to_ascii_lowercase(),
            kind: "local".to_string(),
            last_stop,
            name: source.name.clone(),
            programme_count: count,
            source_id: source.source_id.clone(),
            source_name: source.source_name.clone(),
        });
    }
    ViewerData {
        generated_at,
        channels: out,
    }
}

impl EpgState {
    /// 载入缓存的额外频道。
    pub fn load(cache_dir: &Path) -> Self {
        let state = Self::default();
        if let Ok(text) = fs::read_to_string(extra_file(cache_dir)) {
            if let Ok(extras) = serde_json::from_str::<Vec<Extra>>(&text) {
                *state.extras.lock().unwrap_or_else(|e| e.into_inner()) = extras;
            }
        }
        state
    }
}

fn extra_file(cache_dir: &Path) -> PathBuf {
    output_dir(cache_dir).join("extra.json")
}

/// 额外频道快照。
pub fn extras(state: &AppState) -> Vec<Extra> {
    state
        .epg
        .extras
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 新增或覆盖一个额外频道。
pub fn add_extra(
    state: &AppState,
    source_id: &str,
    source_channel_id: &str,
    out_id: &str,
    out_name: &str,
) -> Result<Extra> {
    let source_id = source_id.trim();
    let source_channel_id = source_channel_id.trim();
    if source_id.is_empty() || source_channel_id.is_empty() {
        bail!("缺少 source_id/source_channel_id");
    }
    if !state
        .epg_store
        .sources()
        .iter()
        .any(|source| source.id == source_id)
    {
        bail!("EPG 源不存在");
    }
    let extra = Extra {
        source_id: source_id.to_string(),
        source_channel_id: source_channel_id.to_string(),
        out_id: out_id.trim().to_string(),
        out_name: out_name.trim().to_string(),
    };
    let mut list = state.epg.extras.lock().unwrap_or_else(|e| e.into_inner());
    match list.iter_mut().find(|item| {
        item.source_id == extra.source_id && item.source_channel_id == extra.source_channel_id
    }) {
        Some(existing) => *existing = extra.clone(),
        None => list.push(extra.clone()),
    }
    save_extras(state, &list)?;
    Ok(extra)
}

/// 删除额外频道；`items` 为空时按单个键删除。
pub fn remove_extras(state: &AppState, keys: &[(String, String)]) -> Result<usize> {
    let mut list = state.epg.extras.lock().unwrap_or_else(|e| e.into_inner());
    let before = list.len();
    list.retain(|item| {
        !keys.iter().any(|(source_id, source_channel_id)| {
            item.source_id == *source_id && item.source_channel_id == *source_channel_id
        })
    });
    let removed = before - list.len();
    if removed > 0 {
        save_extras(state, &list)?;
    }
    Ok(removed)
}

fn save_extras(state: &AppState, extras: &[Extra]) -> Result<()> {
    let path = extra_file(Path::new(&state.cache_dir));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(extras)?;
    fs::write(path, text)?;
    Ok(())
}

/// 读取手工映射快照。自动绑定不参与匹配，只用于面板展示。
pub fn mappings(state: &AppState) -> HashMap<String, Mapping> {
    let names = state.epg_store.source_names();
    state
        .epg_store
        .bindings()
        .into_iter()
        .filter(|binding| binding.manual())
        .map(|binding| {
            (
                binding.key(),
                Mapping {
                    source_id: binding.source_id.clone(),
                    source_channel_id: binding.raw_channel_id.clone(),
                    source_name: names.get(&binding.source_id).cloned().unwrap_or_default(),
                    source_display_name: binding.upstream_display_name.clone(),
                },
            )
        })
        .collect()
}

/// 设置或清除手工映射；`source_id`/`source_channel_id` 为空表示清除。
pub fn set_mapping(
    state: &AppState,
    gid: &str,
    cid: &str,
    source_id: &str,
    source_channel_id: &str,
) -> Result<Option<Mapping>> {
    let config = state.snapshot();
    let Some(group) = config.group(gid) else {
        bail!("分组不存在");
    };
    if !group.channels.iter().any(|channel| channel.id == cid) {
        bail!("频道不存在");
    }
    let source_id = source_id.trim();
    let source_channel_id = source_channel_id.trim();
    if source_id.is_empty() || source_channel_id.is_empty() {
        state.epg_store.remove_binding(gid, cid)?;
        return Ok(None);
    }
    let Some(source) = state.epg_store.source(source_id) else {
        bail!("EPG 源不存在");
    };
    let binding = state.epg_store.set_binding(crate::epg_store::Binding {
        gid: gid.to_string(),
        cid: cid.to_string(),
        source_id: source.id.clone(),
        raw_channel_id: source_channel_id.to_string(),
        origin: "manual".into(),
        upstream_display_name: String::new(),
        updated_at_ms: 0,
    })?;
    Ok(Some(Mapping {
        source_id: binding.source_id,
        source_channel_id: binding.raw_channel_id,
        source_name: source.name,
        source_display_name: binding.upstream_display_name,
    }))
}

/// 一次采集的摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub channels: usize,
    pub programmes: usize,
    pub kept: usize,
    /// 写入的翻译条目数。
    pub translated: usize,
}

/// 采集中标记：离开作用域自动清除。
struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 节目单输出目录。
pub fn output_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("epg")
}

/// 读取已生成的节目单文件并解析。EPG 关闭或文件缺失时返回错误。
pub fn load_generated(config: &Config) -> Result<DocumentData> {
    let path = output_file(config);
    let body = fs::read_to_string(&path).with_context(|| format!("读 {}", path.display()))?;
    parse(&body)
}

/// 生成文件路径。
pub fn output_file(config: &Config) -> PathBuf {
    output_dir(Path::new(&config.cache.disk_path)).join("epg.xml")
}

/// 删除已生成的节目单与快照。没有可用源时调用。
///
/// 只删文件，不动 raw 缓存。重新添加源后可以再生成。
pub fn clear_generated(state: &AppState) -> Result<()> {
    let config = state.snapshot();
    let directory = output_dir(Path::new(&config.cache.disk_path));
    for name in ["epg.xml", "epg.xml.gz", "viewer.json"] {
        match fs::remove_file(directory.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("删 {name} 失败")),
        }
    }
    *state
        .epg
        .generated
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    state.epg.channels.store(0, Ordering::SeqCst);
    state.epg.programmes.store(0, Ordering::SeqCst);
    state.epg.kept.store(0, Ordering::SeqCst);
    *state
        .epg
        .match_rows
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Vec::new();
    *state
        .epg
        .last_error
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some("没有可用的 EPG 源".into());
    Ok(())
}

/// 采集源正文的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    /// 重新下载或执行脚本，并把正文写入 raw 缓存。
    Fetch,
    /// 只读 raw 缓存，不碰网络与脚本。
    CacheOnly,
}

fn raw_dir(cache_dir: &Path) -> PathBuf {
    output_dir(cache_dir).join("raw")
}

fn raw_file(cache_dir: &Path, source_id: &str) -> Option<PathBuf> {
    let name = crate::logo::sanitize(source_id)?;
    Some(raw_dir(cache_dir).join(format!("{name}.xml")))
}

/// 同一源内某条 URL 的 raw 缓存。文件名带 URL 短哈希，改顺序不影响缓存。
fn raw_url_file(cache_dir: &Path, source_id: &str, url: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let name = crate::logo::sanitize(source_id)?;
    let digest = Sha256::digest(url.as_bytes());
    let tag = hex::encode(&digest[..4]);
    Some(raw_dir(cache_dir).join(format!("{name}-{tag}.xml")))
}

/// 触发一次采集。未启用或已在采集时返回错误。
pub async fn refresh(state: &Arc<AppState>) -> Result<Summary> {
    run(state, SourceMode::Fetch).await
}

/// 只按 raw 缓存重新匹配并生成，不重新下载。
pub async fn regenerate(state: &Arc<AppState>) -> Result<Summary> {
    run(state, SourceMode::CacheOnly).await
}

/// 按全局 cron 采集。只抓使用全局节奏的源，其余源读 raw 缓存。
pub async fn refresh_managed(state: &Arc<AppState>) -> Result<Summary> {
    let ids: HashSet<String> = state
        .epg_store
        .sources()
        .into_iter()
        .filter(uses_global_schedule)
        .map(|source| source.id)
        .collect();
    if ids.is_empty() {
        bail!("没有使用全局节奏的 EPG 源");
    }
    run_with_ids(state, SourceMode::Fetch, Some(&ids)).await
}

async fn run(state: &Arc<AppState>, mode: SourceMode) -> Result<Summary> {
    run_with_ids(state, mode, None).await
}

async fn run_with_ids(
    state: &Arc<AppState>,
    mode: SourceMode,
    fetch_ids: Option<&HashSet<String>>,
) -> Result<Summary> {
    let config = state.snapshot();
    if !config.epg.enabled {
        bail!("EPG 未启用");
    }
    if state.epg.running.swap(true, Ordering::SeqCst) {
        bail!("EPG 采集进行中");
    }
    let _guard = RunningGuard(&state.epg.running);

    let result = collect(state, &config, mode, fetch_ids).await;
    record_result(state, &result);
    result
}

/// 只刷新一个源，再用全部 raw 缓存重新生成。
pub async fn refresh_source(state: &Arc<AppState>, source_id: &str) -> Result<Summary> {
    let config = state.snapshot();
    if !config.epg.enabled {
        bail!("EPG 未启用");
    }
    let Some(source) = state
        .epg_store
        .source(source_id)
        .filter(|source| source.enabled)
    else {
        bail!("EPG 源不存在或未启用");
    };
    if state.epg.running.swap(true, Ordering::SeqCst) {
        bail!("EPG 采集进行中");
    }
    let _guard = RunningGuard(&state.epg.running);

    let result = match source_document(state, &config, &source, SourceMode::Fetch).await {
        Ok(_) => collect(state, &config, SourceMode::CacheOnly, None).await,
        Err(error) => Err(error),
    };
    record_result(state, &result);
    result
}

fn record_result(state: &Arc<AppState>, result: &Result<Summary>) {
    match result {
        Ok(summary) => {
            state.epg.channels.store(summary.channels, Ordering::SeqCst);
            state
                .epg
                .programmes
                .store(summary.programmes, Ordering::SeqCst);
            state.epg.kept.store(summary.kept, Ordering::SeqCst);
            *state
                .epg
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            *state
                .epg
                .last_success
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(now_secs());
        }
        Err(error) => {
            *state
                .epg
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(format!("{error:#}"));
        }
    }
}

async fn collect(
    state: &Arc<AppState>,
    config: &Config,
    mode: SourceMode,
    fetch_ids: Option<&HashSet<String>>,
) -> Result<Summary> {
    let targets = channel_targets(config);
    let mappings = mappings(state);
    // 手工绑定优先。自动匹配不得覆盖手工绑定的源。
    let manual_source: HashMap<usize, String> = mappings
        .iter()
        .filter_map(|(key, mapping)| {
            let (gid, cid) = key.split_once('/')?;
            let index = targets
                .iter()
                .position(|target| target.gid == gid && target.cid == cid)?;
            Some((index, mapping.source_id.clone()))
        })
        .collect();
    let mut rows: Vec<MatchRow> = targets
        .iter()
        .map(|target| MatchRow {
            gid: target.gid.clone(),
            cid: target.cid.clone(),
            name: target.name.clone(),
            group_name: target.group_name.clone(),
            out_id: target.out_id.clone(),
            via: if duplicate_out_ids(&targets, &target.out_id) {
                "duplicate".into()
            } else {
                "none".into()
            },
            ..MatchRow::default()
        })
        .collect();
    let now = now_secs() as i64;
    let past = past_limit(config, now);
    let future = now + config.epg.future_days.max(0) * 86_400;

    let mut channels: Vec<Channel> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut programmes: Vec<Programme> = Vec::new();
    let mut sources = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut documents: HashMap<String, DocumentData> = HashMap::new();
    let mut viewer_meta: HashMap<String, ViewerMeta> = HashMap::new();

    for source in enabled_sources(state) {
        let source_mode = if mode == SourceMode::Fetch
            && fetch_ids.is_some_and(|ids| !ids.contains(&source.id))
        {
            SourceMode::CacheOnly
        } else {
            mode
        };
        let document = match source_document(state, config, &source, source_mode).await {
            Ok(document) => document,
            Err(error) => {
                errors.push(format!("{}：{error:#}", source.name));
                continue;
            }
        };
        sources += 1;
        documents.insert(source.id.clone(), document.clone());
        let (candidates, owners, source_programmes) = match_source(
            &targets, &document, &mappings, &source.id, config, past, future,
        );
        for candidate in &candidates {
            let target = &targets[candidate.target];
            let claimed = seen.contains(&target.out_id);
            if !candidate.manual {
                // 手工绑定到别的源时，自动匹配不能抢先占用输出频道。
                if let Some(manual_source_id) = manual_source.get(&candidate.target) {
                    if manual_source_id != &source.id {
                        continue;
                    }
                }
                // 输出频道已有更高优先级的源。后面的源不再追加节目。
                if claimed {
                    continue;
                }
            } else if claimed {
                // 相同输出频道的多个本地频道只保留频道顺序中的第一个。
                continue;
            }
            let owns_channel = owners.get(&candidate.channel_id) == Some(&candidate.target);
            let display_name = document
                .channels
                .iter()
                .find(|channel| channel.id == candidate.channel_id)
                .and_then(|channel| channel.names.first())
                .map(|(_, name)| name.clone())
                .unwrap_or_default();
            let row = &mut rows[candidate.target];
            row.via = candidate.via.clone();
            row.source_id = source.id.clone();
            row.raw_channel_id = candidate.channel_id.clone();
            row.upstream_display_name = display_name;
            row.programmes = if owns_channel {
                source_programmes
                    .get(&candidate.channel_id)
                    .map(Vec::len)
                    .unwrap_or(0)
            } else {
                0
            };
            if claimed {
                continue;
            }
            seen.insert(target.out_id.clone());
            viewer_meta.insert(
                target.out_id.clone(),
                ViewerMeta {
                    name: target.name.clone(),
                    source_id: source.id.clone(),
                    source_name: source.name.clone(),
                },
            );
            channels.push(Channel {
                id: target.out_id.clone(),
                names: vec![(None, target.name.clone())],
                raw_children: document
                    .channels
                    .iter()
                    .find(|channel| channel.id == candidate.channel_id)
                    .map(|channel| channel.raw_children.clone())
                    .unwrap_or_default(),
            });
            if owns_channel {
                for programme in source_programmes
                    .get(&candidate.channel_id)
                    .into_iter()
                    .flatten()
                {
                    let mut programme = programme.clone();
                    programme.channel = target.out_id.clone();
                    programmes.push(programme);
                }
            }
        }
    }

    // 额外频道：只按配置写入输出，不参与本地频道匹配。
    for extra in extras(state) {
        let Some(document) = documents.get(&extra.source_id) else {
            continue;
        };
        let Some(channel) = document
            .channels
            .iter()
            .find(|channel| channel.id == extra.source_channel_id)
        else {
            continue;
        };
        let out_id = extra.output_id();
        if out_id.is_empty() || !seen.insert(out_id.clone()) {
            continue;
        }
        let name = if extra.out_name.trim().is_empty() {
            channel
                .names
                .first()
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| out_id.clone())
        } else {
            extra.out_name.trim().to_string()
        };
        channels.push(Channel {
            id: out_id.clone(),
            names: vec![(None, name)],
            raw_children: channel.raw_children.clone(),
        });
        for programme in document
            .programmes
            .iter()
            .filter(|programme| programme.channel == extra.source_channel_id)
            .filter(|programme| in_window(programme, past, future))
        {
            let mut programme = programme.clone();
            programme.channel = out_id.clone();
            programmes.push(programme);
        }
    }

    // 源可能在采集途中被删除或停用。此时清掉旧输出，避免继续下发过期节目单。
    if !state
        .epg_store
        .sources()
        .iter()
        .any(|source| source.enabled)
    {
        clear_generated(state)?;
        bail!("没有可用的 EPG 源");
    }
    if sources == 0 {
        if errors.is_empty() {
            bail!("没有可用的 EPG 源");
        }
        bail!("{}", errors.join("；"));
    }
    if !errors.is_empty() && channels.is_empty() {
        bail!("{}", errors.join("；"));
    }

    let translate = translate_config(config);
    let mut translated = 0usize;
    if translate.enabled && !translate.url.is_empty() {
        match translate_programmes(state, config, &mut programmes, &translate).await {
            Ok(count) => translated = count,
            Err(error) => {
                tracing::warn!(%error, "EPG 翻译失败");
                errors.push(format!("翻译失败 {error:#}"));
            }
        }
    }
    for line in &errors {
        tracing::warn!(%line, "EPG 源告警");
    }

    let directory = output_dir(Path::new(&config.cache.disk_path));
    fs::create_dir_all(&directory).with_context(|| format!("建目录 {}", directory.display()))?;
    let xml = render(&channels, &programmes);
    let file = directory.join("epg.xml");
    fs::write(&file, xml.as_bytes()).with_context(|| format!("写 {}", file.display()))?;
    let gz = directory.join("epg.xml.gz");
    write_gzip(&gz, xml.as_bytes()).with_context(|| format!("写 {}", gz.display()))?;
    let viewer = viewer_snapshot(&channels, &programmes, &viewer_meta, now_secs());
    let viewer_file = viewer_file(Path::new(&config.cache.disk_path));
    if let Err(error) = fs::write(
        &viewer_file,
        serde_json::to_vec(&viewer).context("序列化公开频道表")?,
    ) {
        tracing::warn!(path = %viewer_file.display(), %error, "写入公开频道表失败");
    }
    // 采集结果写回自动绑定。手工绑定保持不变。
    let manual_keys = state.epg_store.manual_keys();
    let auto: Vec<crate::epg_store::Binding> = rows
        .iter()
        .filter(|row| {
            !row.source_id.is_empty()
                && row.via != "none"
                && row.via != "duplicate"
                && !manual_keys.contains(&format!("{}/{}", row.gid, row.cid))
        })
        .map(|row| crate::epg_store::Binding {
            gid: row.gid.clone(),
            cid: row.cid.clone(),
            source_id: row.source_id.clone(),
            raw_channel_id: row.raw_channel_id.clone(),
            origin: "auto".into(),
            upstream_display_name: row.upstream_display_name.clone(),
            updated_at_ms: 0,
        })
        .collect();
    if let Err(error) = state.epg_store.replace_auto_bindings(auto) {
        tracing::warn!(%error, "写入自动 EPG 绑定失败");
    }
    let matched_names: HashMap<String, String> = rows
        .iter()
        .filter(|row| !row.upstream_display_name.is_empty())
        .map(|row| {
            (
                format!("{}/{}", row.gid, row.cid),
                row.upstream_display_name.clone(),
            )
        })
        .collect();
    for mut binding in state
        .epg_store
        .bindings()
        .into_iter()
        .filter(|binding| binding.manual())
    {
        let Some(name) = matched_names.get(&binding.key()) else {
            continue;
        };
        if binding.upstream_display_name == *name {
            continue;
        }
        binding.upstream_display_name = name.clone();
        if let Err(error) = state.epg_store.set_binding(binding) {
            tracing::warn!(%error, "更新手工绑定显示名失败");
        }
    }
    *state
        .epg
        .match_rows
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = rows;

    Ok(Summary {
        channels: channels.len(),
        programmes: programmes.len(),
        kept: programmes.len(),
        translated,
    })
}

/// 本地频道及其 EPG 输出标识。
struct Target {
    gid: String,
    cid: String,
    name: String,
    group_name: String,
    out_id: String,
    tvg_id: Option<String>,
    normalized: String,
}

struct Candidate {
    target: usize,
    channel_id: String,
    via: String,
    manual: bool,
}

fn channel_targets(config: &Config) -> Vec<Target> {
    let mut targets = Vec::new();
    for group in &config.groups {
        for channel in &group.channels {
            let tvg_id = tvg_id_of(channel);
            targets.push(Target {
                gid: group.id.clone(),
                cid: channel.id.clone(),
                name: channel.name.clone(),
                group_name: group.name.clone(),
                out_id: channel_out_id(channel),
                tvg_id: tvg_id.map(|value| value.to_lowercase()),
                normalized: normalize_name(&channel.name),
            });
        }
    }
    targets
}

/// 频道在节目单里的输出 id。
///
/// 有 `tvg_id`（或 `tvg-id` / `epg_id`）时用它，否则用频道 id。两者都转小写。
/// 面板「我的订阅」与 `/sub/:token/epg.xml` 都用这个规则对齐频道。
pub fn channel_out_id(channel: &crate::config::model::Channel) -> String {
    tvg_id_of(channel)
        .unwrap_or_else(|| channel.id.clone())
        .to_ascii_lowercase()
}

/// 频道上配置的 `tvg_id`，去空白；空串按未配置。
fn tvg_id_of(channel: &crate::config::model::Channel) -> Option<String> {
    ["tvg_id", "tvg-id", "epg_id"]
        .iter()
        .find_map(|key| {
            channel
                .extra
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .map(str::to_string)
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

fn duplicate_out_ids(targets: &[Target], out_id: &str) -> bool {
    targets
        .iter()
        .filter(|target| target.out_id == out_id)
        .count()
        > 1
}

type MatchResult = (
    Vec<Candidate>,
    HashMap<String, usize>,
    HashMap<String, Vec<Programme>>,
);

fn match_source(
    targets: &[Target],
    document: &DocumentData,
    mappings: &HashMap<String, Mapping>,
    source_id: &str,
    config: &Config,
    past: i64,
    future: i64,
) -> MatchResult {
    let by_id: HashMap<String, &Channel> = document
        .channels
        .iter()
        .map(|channel| (channel.id.to_lowercase(), channel))
        .collect();
    let mut by_name: HashMap<String, &Channel> = HashMap::new();
    for channel in &document.channels {
        for (_, name) in &channel.names {
            by_name.entry(normalize_name(name)).or_insert(channel);
        }
    }
    let mut candidates = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let key = format!("{}/{}", target.gid, target.cid);
        let manual = mappings.get(&key).filter(|mapping| {
            mapping.source_id == source_id && !mapping.source_channel_id.is_empty()
        });
        let (channel, via, is_manual) = if let Some(mapping) = manual {
            let channel = by_id
                .get(&mapping.source_channel_id.to_lowercase())
                .copied();
            match channel {
                Some(channel) => (Some(channel), "manual", true),
                None => (None, "none", false),
            }
        } else {
            let by_tvg = target
                .tvg_id
                .as_ref()
                .and_then(|tvg_id| by_id.get(tvg_id).copied());
            let by_name_id = by_id.get(&target.name.trim().to_lowercase()).copied();
            if let Some(channel) = by_tvg {
                (Some(channel), "tvg-id", false)
            } else if let Some(channel) = by_name_id {
                // 源用频道 id 等于推流频道名时也能精确匹配。
                (Some(channel), "id", false)
            } else if config.epg.match_by_name {
                match by_name.get(&target.normalized) {
                    Some(channel) => (Some(*channel), "name", false),
                    None => (None, "none", false),
                }
            } else {
                (None, "none", false)
            }
        };
        if let Some(channel) = channel {
            candidates.push(Candidate {
                target: index,
                channel_id: channel.id.clone(),
                via: via.to_string(),
                manual: is_manual,
            });
        }
    }
    let mut owners: HashMap<String, usize> = HashMap::new();
    for candidate in &candidates {
        // 多个本地频道绑定同一个源频道时，节目只归频道顺序中的第一个。
        // 这样导出的节目表只有一份，播放器的 tvg-id 相同也不会重复。
        owners
            .entry(candidate.channel_id.clone())
            .or_insert(candidate.target);
    }
    let mut source_programmes: HashMap<String, Vec<Programme>> = HashMap::new();
    for programme in &document.programmes {
        if in_window(programme, past, future) {
            source_programmes
                .entry(programme.channel.clone())
                .or_default()
                .push(programme.clone());
        }
    }
    (candidates, owners, source_programmes)
}

/// 源是否使用全局 cron 采集。
fn uses_global_schedule(source: &crate::epg_store::Source) -> bool {
    source.enabled && source.refresh_secs == 0 && source.cron.trim().is_empty()
}

/// 启用中的源，按优先级升序。同优先级保持表内顺序。
fn enabled_sources(state: &AppState) -> Vec<crate::epg_store::Source> {
    let mut list: Vec<_> = state
        .epg_store
        .sources()
        .into_iter()
        .filter(|source| source.enabled)
        .collect();
    list.sort_by_key(|source| source.priority);
    list
}

/// 取一个源的合并文档。
///
/// `Fetch` 会抓取或执行脚本，并写 raw 缓存；`CacheOnly` 只读 raw 缓存。
async fn source_document(
    state: &Arc<AppState>,
    config: &Config,
    source: &crate::epg_store::Source,
    mode: SourceMode,
) -> Result<DocumentData> {
    if mode == SourceMode::CacheOnly {
        let mut document = load_raw_document(config, source).context("缺少缓存，请先采集")?;
        shift_document(&mut document, source.tz_shift_mins);
        return Ok(document);
    }
    if source.kind == "script" {
        if !config.epg.allow_scripts {
            bail!("已禁止脚本源");
        }
        let status_started = crate::users::model::now_ms();
        let body = match run_script(config, source).await {
            Ok(body) => body,
            Err(error) => {
                let status = SourceStatus {
                    last_fetch_started_at_ms: Some(status_started),
                    last_fetch_finished_at_ms: Some(crate::users::model::now_ms()),
                    last_fetch_ok: false,
                    last_error: Some(format!("{error:#}")),
                    ..SourceStatus::default()
                };
                store_source_status(state, &source.id, status);
                return Err(error);
            }
        };
        let mut document = match parse(&body) {
            Ok(document) => document,
            Err(error) => {
                let status = SourceStatus {
                    last_fetch_started_at_ms: Some(status_started),
                    last_fetch_finished_at_ms: Some(crate::users::model::now_ms()),
                    last_fetch_ok: false,
                    last_error: Some(format!("解析失败 {error:#}")),
                    ..SourceStatus::default()
                };
                store_source_status(state, &source.id, status);
                return Err(error.context("解析失败"));
            }
        };
        // 先平移时区，再算覆盖区间。面板显示的时间与节目时间一致。
        shift_document(&mut document, source.tz_shift_mins);
        let status = build_source_status(&document, status_started, true, None, Vec::new());
        store_source_status(state, &source.id, status);
        if let Some(path) = raw_file(Path::new(&config.cache.disk_path), &source.id) {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, body.as_bytes());
        }
        return Ok(document);
    }
    if source.kind != "xml" {
        bail!("未知源类型 {}", source.kind);
    }
    if source.urls.is_empty() {
        bail!("缺少 URL");
    }
    let directory = Path::new(&config.cache.disk_path);
    let previous = state
        .epg
        .source_status
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&source.id)
        .cloned()
        .unwrap_or_default();
    let mut urls = source.urls.clone();
    urls.sort_by_key(|url| url.priority);
    let started = crate::users::model::now_ms();
    let mut merged = DocumentData::default();
    let mut statuses: Vec<UrlStatus> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut cache_headers = HashMap::new();
    for url in &urls {
        if url.url.trim().is_empty() {
            continue;
        }
        let file = raw_url_file(directory, &source.id, &url.url);
        let previous_url = previous
            .urls
            .iter()
            .find(|item| item.url == url.url.trim())
            .cloned()
            .unwrap_or_default();
        let mut status = UrlStatus {
            url: url.url.trim().to_string(),
            priority: url.priority,
            last_fetch_started_at_ms: Some(crate::users::model::now_ms()),
            ..UrlStatus::default()
        };
        let fetched = fetch_url(state, url, &previous_url).await;
        match fetched {
            Ok(Fetched::Body {
                body,
                etag,
                last_modified,
            }) => match parse(&body) {
                Ok(document) => {
                    if let Some(path) = &file {
                        if let Some(parent) = path.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        let _ = fs::write(path, body.as_bytes());
                    }
                    status.last_fetch_ok = true;
                    status.channels = document.channels.len();
                    status.programmes = document.programmes.len();
                    status.etag = etag.clone();
                    status.last_modified = last_modified.clone();
                    cache_headers.insert(url.url.trim().to_string(), (etag, last_modified));
                    merge_document(&mut merged, document);
                }
                Err(error) => {
                    status.last_error = Some(format!("解析失败 {error:#}"));
                    errors.push(format!("{}：{}", url.url.trim(), error));
                }
            },
            Ok(Fetched::NotModified) => {
                if let Some(document) = file.as_deref().and_then(read_raw_document) {
                    status.last_fetch_ok = true;
                    status.channels = document.channels.len();
                    status.programmes = document.programmes.len();
                    status.etag = previous_url.etag.clone();
                    status.last_modified = previous_url.last_modified.clone();
                    cache_headers.insert(
                        url.url.trim().to_string(),
                        (
                            previous_url.etag.clone(),
                            previous_url.last_modified.clone(),
                        ),
                    );
                    merge_document(&mut merged, document);
                } else {
                    status.last_error = Some("上游 304，但没有缓存".into());
                    errors.push(format!("{}：上游 304，但没有缓存", url.url.trim()));
                }
            }
            Err(error) => {
                status.last_error = Some(format!("{error:#}"));
                errors.push(format!("{}：{error:#}", url.url.trim()));
            }
        }
        status.last_fetch_finished_at_ms = Some(crate::users::model::now_ms());
        statuses.push(status);
    }
    let finished = crate::users::model::now_ms();
    if merged.channels.is_empty() {
        let status = SourceStatus {
            last_fetch_started_at_ms: Some(started),
            last_fetch_finished_at_ms: Some(finished),
            last_fetch_ok: false,
            last_error: Some(errors.join("；")),
            urls: statuses,
            cache_headers,
            ..SourceStatus::default()
        };
        store_source_status(state, &source.id, status);
        bail!("{}", errors.join("；"));
    }
    // 先平移时区，再算覆盖区间。面板显示的时间与节目时间一致。
    shift_document(&mut merged, source.tz_shift_mins);
    let mut status = build_source_status(&merged, started, true, None, statuses);
    status.last_fetch_finished_at_ms = Some(finished);
    status.last_error = (!errors.is_empty()).then(|| errors.join("；"));
    status.cache_headers = cache_headers;
    store_source_status(state, &source.id, status);
    Ok(merged)
}

enum Fetched {
    Body {
        body: String,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    NotModified,
}

async fn fetch_url(
    state: &AppState,
    url: &crate::epg_store::SourceUrl,
    previous: &UrlStatus,
) -> Result<Fetched> {
    let mut request = state.http.get(url.url.trim());
    if !url.user_agent.trim().is_empty() {
        request = request.header(reqwest::header::USER_AGENT, url.user_agent.trim());
    }
    for header in &url.headers {
        let name = reqwest::header::HeaderName::from_bytes(header.name.as_bytes());
        let value = reqwest::header::HeaderValue::from_str(&header.value);
        if let (Ok(name), Ok(value)) = (name, value) {
            request = request.header(name, value);
        }
    }
    if let Some(etag) = previous.etag.as_deref() {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(etag) {
            request = request.header(reqwest::header::IF_NONE_MATCH, value);
        }
    }
    if let Some(modified) = previous.last_modified.as_deref() {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(modified) {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, value);
        }
    }
    let response = request.send().await.context("抓取失败")?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    let response = response.error_for_status().context("上游状态异常")?;
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let last_modified = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.bytes().await.context("读取正文失败")?;
    let body = decode_body(&bytes);
    Ok(Fetched::Body {
        body,
        etag,
        last_modified,
    })
}

/// 解压 gzip 正文；不是 gzip 时按 UTF-8 读取。
fn decode_body(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::read::GzDecoder::new(bytes);
        let mut text = String::new();
        if std::io::Read::read_to_string(&mut decoder, &mut text).is_ok() {
            return text;
        }
    }
    String::from_utf8_lossy(bytes).to_string()
}

fn build_source_status(
    document: &DocumentData,
    started: i64,
    ok: bool,
    error: Option<String>,
    urls: Vec<UrlStatus>,
) -> SourceStatus {
    let mut coverage_start: Option<i64> = None;
    let mut coverage_end: Option<i64> = None;
    for programme in &document.programmes {
        if let Some(start) = parse_time(&programme.start) {
            let start = start.saturating_mul(1_000);
            coverage_start = Some(coverage_start.map_or(start, |value| value.min(start)));
        }
        if let Some(stop) = parse_time(&programme.stop) {
            let stop = stop.saturating_mul(1_000);
            coverage_end = Some(coverage_end.map_or(stop, |value| value.max(stop)));
        }
    }
    SourceStatus {
        last_fetch_started_at_ms: Some(started),
        last_fetch_finished_at_ms: Some(crate::users::model::now_ms()),
        last_fetch_ok: ok,
        last_error: error,
        channels: document.channels.len(),
        programmes: document.programmes.len(),
        coverage_start_ms: coverage_start,
        coverage_end_ms: coverage_end,
        urls,
        cache_headers: HashMap::new(),
    }
}

fn store_source_status(state: &AppState, source_id: &str, status: SourceStatus) {
    state
        .epg
        .source_status
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(source_id.to_string(), status);
}

/// 合并同一个源内多个 URL 的文档。
///
/// 同一 `channel id` 只保留先到者。先到者由 URL 优先级决定。
fn merge_document(merged: &mut DocumentData, next: DocumentData) {
    let mut seen: HashSet<String> = merged
        .channels
        .iter()
        .map(|channel| channel.id.to_lowercase())
        .collect();
    let mut accepted: HashSet<String> = HashSet::new();
    for channel in next.channels {
        if seen.insert(channel.id.to_lowercase()) {
            accepted.insert(channel.id.clone());
            merged.channels.push(channel);
        }
    }
    merged.programmes.extend(
        next.programmes
            .into_iter()
            .filter(|programme| accepted.contains(&programme.channel)),
    );
}

/// 读 raw 缓存并合并。缺文件时返回 `None`。
fn load_raw_document(config: &Config, source: &crate::epg_store::Source) -> Option<DocumentData> {
    let directory = Path::new(&config.cache.disk_path);
    if source.kind == "xml" && !source.urls.is_empty() {
        let mut urls = source.urls.clone();
        urls.sort_by_key(|url| url.priority);
        let mut merged = DocumentData::default();
        let mut found = false;
        for url in &urls {
            let Some(document) = raw_url_file(directory, &source.id, &url.url)
                .as_deref()
                .and_then(read_raw_document)
            else {
                continue;
            };
            found = true;
            merge_document(&mut merged, document);
        }
        if found {
            return Some(merged);
        }
    }
    raw_file(directory, &source.id)
        .as_deref()
        .and_then(read_raw_document)
}

fn read_raw_document(path: &Path) -> Option<DocumentData> {
    let bytes = fs::read(path).ok()?;
    let body = decode_body(&bytes);
    parse(&body).ok()
}

async fn run_script(config: &Config, source: &crate::epg_store::Source) -> Result<String> {
    let name = crate::logo::sanitize(&source.script).context("脚本名不合法")?;
    let dir = Path::new(&config.cache.disk_path)
        .join("epg")
        .join("scripts");
    let script = dir.join(&name);
    if !script.is_file() {
        bail!("脚本不存在: {}", script.display());
    }
    let timeout_secs = if source.timeout_secs > 0 {
        source.timeout_secs
    } else {
        config.epg.script_timeout_secs
    };
    let timeout = std::time::Duration::from_secs(timeout_secs.clamp(5, 3600));
    let python = config
        .epg
        .python_bin
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("python3");
    let mut command = tokio::process::Command::new(python);
    command
        .arg(&script)
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .with_context(|| format!("脚本超时（{}s）", timeout.as_secs()))?
        .context("执行 python3 失败")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "脚本退出码 {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim().chars().take(200).collect::<String>()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !stdout.trim().is_empty() {
        return Ok(stdout);
    }
    for file in &source.output_files {
        let Some(name) = crate::logo::sanitize(file) else {
            continue;
        };
        let path = dir.join(name);
        if path.is_file() {
            return fs::read_to_string(&path).context("读取脚本产物失败");
        }
    }
    bail!("脚本无输出")
}

fn write_gzip(path: &Path, data: &[u8]) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    encoder.write_all(data)?;
    encoder.finish()?;
    Ok(())
}

/// 翻译配置（读 `[epg.translate]`）。
#[derive(Debug, Clone)]
pub struct TranslateCfg {
    pub enabled: bool,
    pub url: String,
    pub token: String,
    pub source: String,
    pub targets: Vec<String>,
    pub translation_first: bool,
    pub batch: usize,
}

/// 解析 `[epg.translate]`；`targets` 为空时回退到 `target`。
pub fn translate_config(config: &Config) -> TranslateCfg {
    let table = config.epg.extra.get("translate");
    let get = |key: &str| table.and_then(|table| table.get(key));
    let enabled = get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let engine = get("engine")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let explicit_url = get("url")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    let url = if !explicit_url.is_empty() {
        explicit_url.trim_end_matches('/').to_string()
    } else if engine.is_empty() || engine == "mtran" {
        get("mtran_url")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .trim_end_matches('/')
            .to_string()
    } else {
        String::new()
    };
    let token = get("token")
        .or_else(|| get("mtran_token"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let source = get("source")
        .or_else(|| get("mtran_source"))
        .and_then(|value| value.as_str())
        .unwrap_or("auto")
        .to_string();
    let translation_first = get("translation_first")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let mut targets: Vec<String> = get("targets")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| text.trim().to_string()))
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if targets.is_empty() {
        if let Some(target) = get("target").and_then(|value| value.as_str()) {
            let target = target.trim();
            if !target.is_empty() {
                targets.push(target.to_string());
            }
        }
    }
    let batch = get("mtran_batch")
        .and_then(|value| value.as_i64())
        .filter(|value| *value > 0)
        .unwrap_or(40)
        .min(500) as usize;
    TranslateCfg {
        enabled,
        url,
        token,
        source,
        targets,
        translation_first,
        batch,
    }
}

/// 翻译服务端点；未配置时返回空串。
pub fn translate_endpoint(config: &Config) -> String {
    let cfg = translate_config(config);
    crate::translate::endpoint(&cfg.url)
}

/// 翻译缓存条目数（状态页用）。
pub fn translate_cache_len(config: &Config) -> usize {
    load_translate_cache(config).len()
}

fn translate_cache_path(config: &Config) -> PathBuf {
    output_dir(Path::new(&config.cache.disk_path))
        .join(".translate")
        .join("cache.json")
}

fn load_translate_cache(config: &Config) -> HashMap<String, String> {
    fs::read_to_string(translate_cache_path(config))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_translate_cache(config: &Config, cache: &HashMap<String, String>) -> Result<()> {
    let path = translate_cache_path(config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string(cache)?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, text)?;
    fs::rename(&temp, &path)?;
    Ok(())
}

fn translate_key(target: &str, text: &str) -> String {
    let digest = Sha256::digest(format!("{target}\u{1f}{text}").as_bytes());
    hex::encode(digest)
}

/// 逐目标翻译节目文本，并把译文追加到 `programme.texts`。
async fn translate_programmes(
    state: &Arc<AppState>,
    config: &Config,
    programmes: &mut [Programme],
    cfg: &TranslateCfg,
) -> Result<usize> {
    if cfg.targets.is_empty() {
        bail!("未配置翻译目标语言");
    }
    let mut cache = load_translate_cache(config);
    let mut translated = 0usize;
    for target in &cfg.targets {
        let mut pending: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for programme in programmes.iter() {
            for (_, lang, text) in &programme.texts {
                if lang.as_deref() == Some(target.as_str()) {
                    continue;
                }
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                let key = translate_key(target, text);
                if cache.contains_key(&key) {
                    continue;
                }
                if seen.insert(key) {
                    pending.push(text.to_string());
                }
            }
        }
        for chunk in pending.chunks(cfg.batch) {
            let values = crate::translate::batch_with_token(
                &state.http,
                &cfg.url,
                &cfg.source,
                target,
                chunk,
                Some(cfg.token.as_str()),
            )
            .await?;
            for (text, value) in chunk.iter().zip(values) {
                cache.insert(translate_key(target, text), value);
            }
        }
        for programme in programmes.iter_mut() {
            let mut additions = Vec::new();
            for (name, lang, text) in &programme.texts {
                if lang.as_deref() == Some(target.as_str()) {
                    continue;
                }
                if let Some(value) = cache.get(&translate_key(target, text.trim())) {
                    additions.push((name.clone(), Some(target.clone()), value.clone()));
                }
            }
            translated += additions.len();
            if cfg.translation_first {
                let mut merged = additions;
                merged.extend(std::mem::take(&mut programme.texts));
                programme.texts = merged;
            } else {
                programme.texts.extend(additions);
            }
        }
    }
    save_translate_cache(config, &cache)?;
    Ok(translated)
}

/// 启动后台采集：启用时立即跑一次，之后按 `cron`（只识别 `0 */N * * *`）或 4 小时。
pub fn spawn(state: Arc<AppState>) {
    if !state.snapshot().epg.enabled {
        return;
    }
    tokio::spawn(async move {
        match refresh(&state).await {
            Ok(summary) => tracing::info!(
                channels = summary.channels,
                programmes = summary.kept,
                "EPG 采集完成"
            ),
            Err(error) => tracing::warn!(%error, "EPG 采集失败"),
        }
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let config = state.snapshot();
            if !config.epg.enabled {
                continue;
            }
            let now = now_secs() as i64;
            let offset = config.server.timezone_offset_hours as i64;
            let last_success = *state
                .epg
                .last_success
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let fallback_due = last_success
                .map(|at| at as i64 + 4 * 3_600)
                .unwrap_or(now + 4 * 3_600);
            let global_due = cron_preview(&config.epg.cron, now, offset, 1)
                .ok()
                .and_then(|next| next.first().copied())
                .unwrap_or(fallback_due);
            if global_due <= now {
                let has_global = state
                    .epg_store
                    .sources()
                    .into_iter()
                    .any(|source| uses_global_schedule(&source));
                if has_global {
                    match refresh_managed(&state).await {
                        Ok(summary) => tracing::info!(
                            channels = summary.channels,
                            programmes = summary.kept,
                            "EPG 采集完成"
                        ),
                        Err(error) => tracing::warn!(%error, "EPG 采集失败"),
                    }
                    continue;
                }
            }
            let sources = state.epg_store.sources();
            for source in sources.iter().filter(|source| source.enabled) {
                let last = state
                    .epg
                    .source_status
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&source.id)
                    .and_then(|status| {
                        status
                            .last_fetch_started_at_ms
                            .or(status.last_fetch_finished_at_ms)
                    });
                let due = if source.refresh_secs > 0 {
                    last.is_none_or(|at| {
                        at + source.refresh_secs.saturating_mul(1_000) as i64 <= now * 1_000
                    })
                } else if !source.cron.trim().is_empty() {
                    cron_preview(&source.cron, now, offset, 1)
                        .ok()
                        .and_then(|next| next.first().copied())
                        .is_some_and(|due| due <= now)
                } else {
                    false
                };
                if !due {
                    continue;
                }
                match refresh_source(&state, &source.id).await {
                    Ok(summary) => tracing::info!(
                        source = %source.name,
                        channels = summary.channels,
                        programmes = summary.kept,
                        "EPG 单源采集完成"
                    ),
                    Err(error) => {
                        tracing::warn!(source = %source.name, %error, "EPG 单源采集失败")
                    }
                }
            }
        }
    });
}

/// EPG 脚本条目（面板脚本列表）。
#[derive(Debug, Clone, Serialize)]
pub struct ScriptInfo {
    pub name: String,
    pub size: u64,
}

/// `<cache>/epg/scripts`。
pub fn scripts_dir(config: &Config) -> PathBuf {
    output_dir(Path::new(&config.cache.disk_path)).join("scripts")
}

/// 列出脚本目录中的 `.py` 文件。
pub fn list_scripts(config: &Config) -> Vec<ScriptInfo> {
    let mut scripts = Vec::new();
    if let Ok(read) = fs::read_dir(scripts_dir(config)) {
        for item in read.flatten() {
            let Ok(meta) = item.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = item.file_name().to_string_lossy().to_string();
            if name.ends_with(".py") {
                scripts.push(ScriptInfo {
                    name,
                    size: meta.len(),
                });
            }
        }
    }
    scripts.sort_by(|left, right| left.name.cmp(&right.name));
    scripts
}

/// 保存脚本；只接受 `.py` 文件名。
pub fn save_script(config: &Config, name: &str, content: &str) -> Result<ScriptInfo> {
    let name = crate::logo::sanitize(name).context("脚本名不合法")?;
    if !name.ends_with(".py") {
        bail!("只接受 .py 文件");
    }
    let dir = scripts_dir(config);
    fs::create_dir_all(&dir).with_context(|| format!("建目录 {}", dir.display()))?;
    let path = dir.join(&name);
    fs::write(&path, content.as_bytes()).with_context(|| format!("写 {}", path.display()))?;
    Ok(ScriptInfo {
        name,
        size: content.len() as u64,
    })
}

/// 删除脚本；文件不存在时返回 `false`。
pub fn delete_script(config: &Config, name: &str) -> Result<bool> {
    let name = crate::logo::sanitize(name).context("脚本名不合法")?;
    let path = scripts_dir(config).join(&name);
    if path.is_file() {
        fs::remove_file(&path).with_context(|| format!("删 {}", path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// 探测 `python3` 版本；不可用时返回 `None`。
pub async fn python_version() -> Option<String> {
    let output = tokio::process::Command::new("python3")
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return Some(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        None
    } else {
        Some(stderr)
    }
}

/// 公历天数 → `(年, 月, 日)`。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (year + if month <= 2 { 1 } else { 0 }, month, day)
}

/// Unix 秒 → `[server].timezone_offset_hours` 下的 `YYYY-MM-DDTHH:MM:SS`。
fn format_local(epoch: i64, offset_hours: i64) -> String {
    let local = epoch + offset_hours * 3_600;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        secs / 3_600,
        (secs % 3_600) / 60,
        secs % 60
    )
}

/// XMLTV 时间 → `YYYYMMDDHHMMSS +0000`。
fn format_time(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}{:02}{:02}{:02} +0000",
        secs / 3_600,
        (secs % 3_600) / 60,
        secs % 60
    )
}

/// 按源配置平移节目时间。
fn shift_document(document: &mut DocumentData, minutes: i64) {
    if minutes == 0 {
        return;
    }
    let delta = minutes * 60;
    for programme in &mut document.programmes {
        if let Some(start) = parse_time(&programme.start) {
            programme.start = format_time(start + delta);
        }
        if let Some(stop) = parse_time(&programme.stop) {
            programme.stop = format_time(stop + delta);
        }
    }
}

fn raw_document(state: &AppState, source: &crate::epg_store::Source) -> Option<DocumentData> {
    let config = state.snapshot();
    load_raw_document(&config, source)
}

fn title_text(programme: &Programme) -> String {
    programme
        .texts
        .iter()
        .find(|(name, _, _)| name == "title")
        .map(|(_, _, text)| text.clone())
        .unwrap_or_default()
}

fn desc_text(programme: &Programme) -> String {
    programme
        .texts
        .iter()
        .find(|(name, _, _)| name == "desc")
        .map(|(_, _, text)| text.clone())
        .unwrap_or_default()
}

/// 面板选节目用的频道节目表。
pub fn programmes_for(config: &Config, channel: &str) -> Result<Value> {
    let data = load_generated(config)?;
    let offset = config.server.timezone_offset_hours as i64;
    let now = now_secs() as i64;
    let pad_before = config.epg.record_pad_before_secs as i64;
    let pad_after = config.epg.record_pad_after_secs as i64;
    let mut rows: Vec<&Programme> = data
        .programmes
        .iter()
        .filter(|programme| programme.channel == channel)
        .collect();
    rows.sort_by_key(|programme| parse_time(&programme.start).unwrap_or(0));
    let programmes: Vec<Value> = rows
        .iter()
        .map(|programme| {
            let start = parse_time(&programme.start);
            let stop = parse_time(&programme.stop);
            json!({
                "channel": programme.channel,
                "start": programme.start,
                "stop": programme.stop,
                "start_epoch": start,
                "stop_epoch": stop,
                "disp_start": start.map(|value| format_local(value, offset)),
                "disp_end": stop.map(|value| format_local(value, offset)),
                "rec_start": start.map(|value| format_local(value - pad_before, offset)),
                "rec_end": stop.map(|value| format_local(value + pad_after, offset)),
                "live": matches!((start, stop), (Some(s), Some(e)) if s <= now && now < e),
                "title": title_text(programme),
                "desc": desc_text(programme),
            })
        })
        .collect();
    Ok(json!({
        "now": now,
        "channel": channel,
        "pad_before": pad_before,
        "pad_after": pad_after,
        "programmes": programmes,
    }))
}

/// 源频道搜索；`source_id = "__all__"` 时跨全部启用源。
pub fn source_channels(
    state: &AppState,
    source_id: &str,
    query: &str,
    offset: usize,
    limit: usize,
) -> Value {
    let query = query.trim().to_lowercase();
    let sources: Vec<_> = state
        .epg_store
        .sources()
        .into_iter()
        .filter(|source| source.enabled)
        .filter(|source| source_id == "__all__" || source.id == source_id)
        .collect();
    let mut items = Vec::new();
    for source in sources {
        let Some(document) = raw_document(state, &source) else {
            continue;
        };
        for channel in &document.channels {
            let names: Vec<String> = channel.names.iter().map(|(_, name)| name.clone()).collect();
            if !query.is_empty()
                && !channel.id.to_lowercase().contains(&query)
                && !names
                    .iter()
                    .any(|name| name.to_lowercase().contains(&query))
            {
                continue;
            }
            items.push(json!({
                "id": channel.id,
                "display_names": names,
                "source_id": source.id,
                "source_name": source.name,
            }));
        }
    }
    let total = items.len();
    let limit = limit.clamp(1, 200);
    let items: Vec<Value> = items
        .into_iter()
        .skip(offset.min(total))
        .take(limit)
        .collect();
    json!({ "items": items, "total": total, "offset": offset, "limit": limit })
}

/// 按名称相似度推荐源频道。
pub fn suggest(state: &AppState, gid: &str, cid: &str, limit: usize) -> Result<Value> {
    let config = state.snapshot();
    let Some(group) = config.group(gid) else {
        bail!("分组不存在");
    };
    let Some(channel) = group.channels.iter().find(|channel| channel.id == cid) else {
        bail!("频道不存在");
    };
    let mut suggestions = Vec::new();
    for source in state
        .epg_store
        .sources()
        .into_iter()
        .filter(|source| source.enabled)
    {
        let Some(document) = raw_document(state, &source) else {
            continue;
        };
        for candidate in &document.channels {
            for (_, display_name) in &candidate.names {
                let score = crate::logo::similarity(&channel.name, display_name);
                if score > 0.0 {
                    suggestions.push(json!({
                        "source_id": source.id,
                        "source_channel_id": candidate.id,
                        "source_name": source.name,
                        "display_name": display_name,
                        "score": (score * 100.0).round() as i64,
                    }));
                }
            }
        }
    }
    suggestions.sort_by(|left, right| {
        right["score"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&left["score"].as_i64().unwrap_or(0))
    });
    suggestions.truncate(limit.clamp(1, 50));
    Ok(json!({ "suggestions": suggestions }))
}

/// 时间偏移预览：取源第一条节目，返回原文与本机时间、偏移后时间。
pub fn tz_preview(state: &AppState, source_id: &str, shift_secs: i64) -> Value {
    let config = state.snapshot();
    let offset = config.server.timezone_offset_hours as i64;
    let Some(source) = state.epg_store.source(source_id) else {
        return json!({ "ok": false, "error": "EPG 源不存在" });
    };
    let Some(document) = raw_document(state, &source) else {
        return json!({ "ok": false, "error": "该源暂无采集数据" });
    };
    let item = document.programmes.iter().find_map(|programme| {
        let start = parse_time(&programme.start)?;
        Some(json!({
            "raw": programme.start,
            "current": format_local(start, offset),
            "shifted": format_local(start + shift_secs, offset),
        }))
    });
    match item {
        Some(item) => json!({ "ok": true, "items": [item] }),
        None => json!({ "ok": true, "items": [] }),
    }
}

#[derive(Debug, Clone)]
struct CronField {
    values: HashSet<u32>,
    wildcard: bool,
}

impl CronField {
    fn matches(&self, value: u32) -> bool {
        self.values.contains(&value)
    }
}

fn parse_cron_field(text: &str, min: u32, max: u32, sunday_seven: bool) -> Result<CronField> {
    let mut values = HashSet::new();
    let mut wildcard = false;
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            bail!("Cron 字段为空");
        }
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step.parse().context("Cron 步长不合法")?;
                if step == 0 {
                    bail!("Cron 步长不能为 0");
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            wildcard = true;
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (
                start.parse().context("Cron 起点不合法")?,
                end.parse().context("Cron 终点不合法")?,
            )
        } else {
            let value: u32 = range.parse().context("Cron 值不合法")?;
            (value, value)
        };
        if start > end || start < min || end > max {
            bail!("Cron 字段超出范围");
        }
        let mut value = start;
        while value <= end {
            let normalized = if sunday_seven && value == 7 { 0 } else { value };
            values.insert(normalized);
            value += step;
        }
    }
    if values.is_empty() {
        bail!("Cron 字段为空");
    }
    Ok(CronField { values, wildcard })
}

struct CronSpec {
    minute: CronField,
    hour: CronField,
    day: CronField,
    month: CronField,
    weekday: CronField,
}

fn parse_cron(expr: &str) -> Result<CronSpec> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        bail!("需要标准 5 字段表达式");
    }
    Ok(CronSpec {
        minute: parse_cron_field(fields[0], 0, 59, false)?,
        hour: parse_cron_field(fields[1], 0, 23, false)?,
        day: parse_cron_field(fields[2], 1, 31, false)?,
        month: parse_cron_field(fields[3], 1, 12, false)?,
        weekday: parse_cron_field(fields[4], 0, 7, true)?,
    })
}

fn cron_matches(spec: &CronSpec, epoch: i64, offset_hours: i64) -> bool {
    let local = epoch + offset_hours * 3_600;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (_, month, day) = civil_from_days(days);
    let weekday = (days + 4).rem_euclid(7) as u32;
    if !spec.month.matches(month as u32)
        || !spec.hour.matches((secs / 3_600) as u32)
        || !spec.minute.matches(((secs % 3_600) / 60) as u32)
    {
        return false;
    }
    let day_ok = spec.day.matches(day as u32);
    let weekday_ok = spec.weekday.matches(weekday);
    match (spec.day.wildcard, spec.weekday.wildcard) {
        (true, true) => true,
        (false, true) => day_ok,
        (true, false) => weekday_ok,
        (false, false) => day_ok || weekday_ok,
    }
}

/// 计算接下来的 `count` 个触发时间（Unix 秒）。
pub fn cron_preview(expr: &str, now: i64, offset_hours: i64, count: usize) -> Result<Vec<i64>> {
    let spec = parse_cron(expr)?;
    let mut next = Vec::new();
    let mut candidate = now.div_euclid(60) * 60 + 60;
    let limit = now + 366 * 86_400;
    while candidate <= limit && next.len() < count.clamp(1, 10) {
        if cron_matches(&spec, candidate, offset_hours) {
            next.push(candidate);
        }
        candidate += 60;
    }
    if next.is_empty() {
        bail!("一年内没有触发时间");
    }
    Ok(next)
}

/// 当前 Unix 秒。
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn past_limit(config: &Config, now: i64) -> i64 {
    let by_days = now - config.epg.keep_days.max(0) * 86_400;
    if config.epg.keep_yesterday {
        // 保留昨天 0 点以后的内容。
        by_days.min(now - (now % 86_400) - 86_400)
    } else {
        by_days
    }
}

/// 解析结果。
#[derive(Debug, Default, Clone)]
pub struct DocumentData {
    pub channels: Vec<Channel>,
    pub programmes: Vec<Programme>,
}

impl DocumentData {
    /// 只保留给定频道 id 的频道与节目。
    ///
    /// 判定是「id 命中」或「任一显示名命中」。`epg.xml` 的频道 id 就是
    /// [`channel_out_id`] 的结果，所以按 id 过滤即可。
    pub fn retain_channels(self, ids: &HashSet<String>) -> Self {
        let channels: Vec<Channel> = self
            .channels
            .into_iter()
            .filter(|channel| {
                ids.contains(channel.id.trim())
                    || channel
                        .names
                        .iter()
                        .any(|(_, name)| ids.contains(name.trim()))
            })
            .collect();
        let kept: HashSet<String> = channels.iter().map(|c| c.id.clone()).collect();
        let programmes = self
            .programmes
            .into_iter()
            .filter(|programme| kept.contains(programme.channel.trim()))
            .collect();
        Self {
            channels,
            programmes,
        }
    }
}

/// 解析频道的重命名与显示名。
#[derive(Debug, Clone, Default)]
pub struct Channel {
    pub id: String,
    pub names: Vec<(Option<String>, String)>,
    /// 原样保留的子节点（如 `icon`）。
    pub raw_children: Vec<String>,
}

/// 解析一条节目。
#[derive(Debug, Clone, Default)]
pub struct Programme {
    pub start: String,
    pub stop: String,
    pub channel: String,
    /// 文本子节点：`(元素名, lang, 文本)`。
    pub texts: Vec<(String, Option<String>, String)>,
    /// 原样保留的子节点（`icon`、`credits` 等）。
    pub raw_children: Vec<String>,
}

/// 解析 XMLTV 正文。
pub fn parse(body: &str) -> Result<DocumentData> {
    let document = Document::parse(body).context("XML 不合法")?;
    let root = document.root_element();
    if root.tag_name().name() != "tv" {
        bail!("根节点不是 tv");
    }
    let mut data = DocumentData::default();
    for node in root.children().filter(|node| node.is_element()) {
        match node.tag_name().name() {
            "channel" => {
                let id = node.attribute("id").unwrap_or("").trim().to_string();
                if id.is_empty() {
                    continue;
                }
                let mut channel = Channel {
                    id,
                    ..Channel::default()
                };
                for child in node.children().filter(|child| child.is_element()) {
                    if child.tag_name().name() == "display-name" {
                        channel.names.push((lang_of(child), text_of(child)));
                    } else if child.tag_name().name() == "icon" {
                        channel.raw_children.push(raw_of(body, child));
                    }
                }
                data.channels.push(channel);
            }
            "programme" => {
                let start = node.attribute("start").unwrap_or("").trim().to_string();
                let stop = node.attribute("stop").unwrap_or("").trim().to_string();
                let channel = node.attribute("channel").unwrap_or("").trim().to_string();
                if channel.is_empty() {
                    continue;
                }
                let mut programme = Programme {
                    start,
                    stop,
                    channel,
                    ..Programme::default()
                };
                for child in node.children().filter(|child| child.is_element()) {
                    let name = child.tag_name().name();
                    if is_text_element(name) {
                        programme
                            .texts
                            .push((name.to_string(), lang_of(child), text_of(child)));
                    } else {
                        programme.raw_children.push(raw_of(body, child));
                    }
                }
                data.programmes.push(programme);
            }
            _ => {}
        }
    }
    Ok(data)
}

fn is_text_element(name: &str) -> bool {
    matches!(
        name,
        "title"
            | "sub-title"
            | "desc"
            | "category"
            | "keyword"
            | "language"
            | "orig-language"
            | "date"
            | "episode-num"
            | "country"
    )
}

fn lang_of(node: roxmltree::Node) -> Option<String> {
    node.attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
        .or_else(|| node.attribute("lang"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn text_of(node: roxmltree::Node) -> String {
    node.text().unwrap_or("").trim().to_string()
}

fn raw_of(source: &str, node: roxmltree::Node) -> String {
    let range = node.range();
    source[range.start..range.end].to_string()
}

fn in_window(programme: &Programme, past: i64, future: i64) -> bool {
    let start = parse_time(&programme.start);
    let stop = parse_time(&programme.stop).or(start);
    match (start, stop) {
        (Some(start), Some(stop)) => stop >= past && start <= future,
        _ => true,
    }
}

/// 生成 XMLTV 正文。
pub fn render(channels: &[Channel], programmes: &[Programme]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<tv generator-info-name=\"tv-buddy\">\n");
    for channel in channels {
        out.push_str(&format!("  <channel id=\"{}\">\n", escape(&channel.id)));
        if channel.names.is_empty() {
            out.push_str(&format!(
                "    <display-name lang=\"zh\">{}</display-name>\n",
                escape(&channel.id)
            ));
        }
        for (lang, name) in &channel.names {
            out.push_str(&format!(
                "    <display-name{}>{}</display-name>\n",
                lang_attr(lang),
                escape(name)
            ));
        }
        for raw in &channel.raw_children {
            out.push_str(&format!("    {raw}\n"));
        }
        out.push_str("  </channel>\n");
    }
    for programme in programmes {
        out.push_str(&format!(
            "  <programme start=\"{}\" stop=\"{}\" channel=\"{}\">\n",
            escape(&programme.start),
            escape(&programme.stop),
            escape(&programme.channel)
        ));
        for (name, lang, text) in &programme.texts {
            out.push_str(&format!(
                "    <{name}{}>{}</{name}>\n",
                lang_attr(lang),
                escape(text)
            ));
        }
        for raw in &programme.raw_children {
            out.push_str(&format!("    {raw}\n"));
        }
        out.push_str("  </programme>\n");
    }
    out.push_str("</tv>\n");
    out
}

fn lang_attr(lang: &Option<String>) -> String {
    match lang {
        Some(value) => format!(" lang=\"{}\"", escape(value)),
        None => " lang=\"zh\"".to_string(),
    }
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// 解析 XMLTV 时间：`YYYYMMDDHHMMSS[ +HHMM]` → Unix 秒。
pub fn parse_time(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.len() < 14 {
        return None;
    }
    let digits = value.get(..14)?;
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let year: i64 = digits[0..4].parse().ok()?;
    let month: i64 = digits[4..6].parse().ok()?;
    let day: i64 = digits[6..8].parse().ok()?;
    let hour: i64 = digits[8..10].parse().ok()?;
    let minute: i64 = digits[10..12].parse().ok()?;
    let second: i64 = digits[12..14].parse().ok()?;

    let rest = value[14..].trim();
    let offset = if rest.is_empty() {
        0
    } else {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let body: String = rest
            .trim_start_matches(['+', '-'])
            .chars()
            .filter(|c| *c != ':')
            .collect();
        if body.len() < 4 {
            return None;
        }
        let hours: i64 = body[0..2].parse().ok()?;
        let minutes: i64 = body[2..4].parse().ok()?;
        sign * (hours * 3600 + minutes * 60)
    };

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// 公历日期 → 1970-01-01 起的天数。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="src">
  <channel id="cctv1"><display-name lang="zh">CCTV-1</display-name><icon src="http://logo/1.png"/></channel>
  <channel id="other"><display-name>Other</display-name></channel>
  <programme start="20261020080000 +0800" stop="20261020090000 +0800" channel="cctv1">
    <title lang="zh">新闻</title>
    <desc>早间新闻</desc>
    <category lang="en">News</category>
    <icon src="http://img/1.jpg"/>
  </programme>
  <programme start="20261020090000 +0800" stop="20261020100000 +0800" channel="other">
    <title>别的台</title>
  </programme>
</tv>"#;

    #[test]
    fn parse_keeps_structure() {
        let data = parse(SAMPLE).expect("应解析成功");
        assert_eq!(data.channels.len(), 2);
        assert_eq!(data.programmes.len(), 2);
        assert_eq!(data.channels[0].names[0].1, "CCTV-1");
        assert!(data.channels[0].raw_children[0].contains("icon"));
        assert_eq!(data.programmes[0].texts[0].0, "title");
        assert!(data.programmes[0].texts[1].1.is_none());
        assert!(data.programmes[0].raw_children[0].contains("icon"));
    }

    #[test]
    fn match_filters_unknown_channels() {
        let data = parse(SAMPLE).expect("应解析成功");
        let mut keys = HashSet::new();
        keys.insert("CCTV-1".to_string());
        let matched = data.retain_channels(&keys);
        assert_eq!(matched.channels.len(), 1);
        assert_eq!(matched.channels[0].id, "cctv1");
        assert_eq!(matched.programmes.len(), 1);
    }

    #[test]
    fn retain_channels_filters_by_id() {
        let data = parse(SAMPLE).expect("应解析成功");
        let keys: HashSet<String> = ["cctv1".to_string()].into_iter().collect();
        let matched = data.retain_channels(&keys);
        assert_eq!(matched.channels.len(), 1);
        assert!(matched.programmes.iter().all(|p| p.channel == "cctv1"));
    }

    #[test]
    fn retain_channels_keeps_nothing_when_empty() {
        let data = parse(SAMPLE).expect("应解析成功");
        let matched = data.retain_channels(&HashSet::new());
        assert!(matched.channels.is_empty());
        assert!(matched.programmes.is_empty());
    }

    #[test]
    fn channel_out_id_prefers_tvg_id_and_lowercases() {
        let mut channel = crate::config::model::Channel {
            id: "C1".into(),
            ..crate::config::model::Channel::default()
        };
        assert_eq!(channel_out_id(&channel), "c1", "无 tvg_id 时用小写频道 id");
        channel
            .extra
            .insert("tvg_id".into(), serde_json::json!("CCTV-1"));
        assert_eq!(channel_out_id(&channel), "cctv-1");
        channel
            .extra
            .insert("tvg_id".into(), serde_json::json!("   "));
        assert_eq!(channel_out_id(&channel), "c1", "空 tvg_id 按未配置");
    }

    #[test]
    fn translate_config_reads_targets_and_endpoint() {
        let mut config = Config::default();
        let mut translate = serde_json::Map::new();
        translate.insert("enabled".into(), serde_json::Value::Bool(true));
        translate.insert(
            "url".into(),
            serde_json::Value::String("http://mtran/".into()),
        );
        translate.insert("target".into(), serde_json::Value::String(" ja ".into()));
        let mut epg = serde_json::Map::new();
        epg.insert("translate".into(), serde_json::Value::Object(translate));
        config.epg.extra = epg;

        let cfg = translate_config(&config);
        assert!(cfg.enabled);
        assert_eq!(cfg.url, "http://mtran");
        assert_eq!(cfg.targets, vec!["ja"]);
        assert_eq!(cfg.source, "auto");
        assert!(!cfg.translation_first);
        assert_eq!(translate_endpoint(&config), "http://mtran/translate/batch");
    }

    #[test]
    fn translate_config_reads_mtran_aliases() {
        let mut config = Config::default();
        let mut translate = serde_json::Map::new();
        translate.insert("enabled".into(), serde_json::Value::Bool(true));
        translate.insert("engine".into(), serde_json::Value::String("mtran".into()));
        translate.insert(
            "mtran_url".into(),
            serde_json::Value::String("https://translate.example/translate".into()),
        );
        translate.insert(
            "mtran_token".into(),
            serde_json::Value::String("secret".into()),
        );
        translate.insert(
            "mtran_source".into(),
            serde_json::Value::String("en".into()),
        );
        translate.insert("mtran_batch".into(), serde_json::Value::Number((50).into()));
        translate.insert("target".into(), serde_json::Value::String("zh-cn".into()));
        let mut epg = serde_json::Map::new();
        epg.insert("translate".into(), serde_json::Value::Object(translate));
        config.epg.extra = epg;

        let cfg = translate_config(&config);
        assert_eq!(cfg.url, "https://translate.example/translate");
        assert_eq!(cfg.token, "secret");
        assert_eq!(cfg.source, "en");
        assert_eq!(cfg.batch, 50);
        assert_eq!(
            translate_endpoint(&config),
            "https://translate.example/translate/batch"
        );
    }

    #[test]
    fn translate_config_prefers_explicit_targets() {
        let mut config = Config::default();
        let mut translate = serde_json::Map::new();
        translate.insert(
            "targets".into(),
            serde_json::Value::Array(vec![
                serde_json::Value::String("zh-cn".into()),
                serde_json::Value::String("".into()),
                serde_json::Value::String("en".into()),
            ]),
        );
        translate.insert("target".into(), serde_json::Value::String("ja".into()));
        let mut epg = serde_json::Map::new();
        epg.insert("translate".into(), serde_json::Value::Object(translate));
        config.epg.extra = epg;

        assert_eq!(translate_config(&config).targets, vec!["zh-cn", "en"]);
    }

    #[test]
    fn render_adds_lang_and_escapes() {
        let channels = vec![Channel {
            id: "cctv1".into(),
            names: vec![(None, "CCTV & 1".into())],
            raw_children: vec![],
        }];
        let programmes = vec![Programme {
            start: "20261020080000 +0800".into(),
            stop: "20261020090000 +0800".into(),
            channel: "cctv1".into(),
            texts: vec![("title".into(), None, "新闻".into())],
            raw_children: vec![],
        }];
        let xml = render(&channels, &programmes);
        assert!(xml.contains("generator-info-name=\"tv-buddy\""));
        assert!(xml.contains("<display-name lang=\"zh\">CCTV &amp; 1</display-name>"));
        assert!(xml.contains("<title lang=\"zh\">新闻</title>"));
        // 结构可再解析。
        parse(&xml).expect("输出应可再解析");
    }

    #[test]
    fn parse_time_handles_offsets() {
        // 2026-10-20T08:00:00+08:00 == 2026-10-20T00:00:00Z
        assert_eq!(parse_time("20261020080000 +0800"), Some(1_792_454_400));
        assert_eq!(parse_time("20261020000000 +0000"), Some(1_792_454_400));
        assert_eq!(parse_time("短"), None);
    }

    #[test]
    fn window_filters_old_and_far_future() {
        let now = 1_792_454_400;
        let old = Programme {
            start: "20260101000000 +0000".into(),
            stop: "20260101010000 +0000".into(),
            ..Programme::default()
        };
        let fresh = Programme {
            start: "20261020080000 +0800".into(),
            stop: "20261020090000 +0800".into(),
            ..Programme::default()
        };
        let far = Programme {
            start: "20300101000000 +0000".into(),
            stop: "20300101010000 +0000".into(),
            ..Programme::default()
        };
        assert!(!in_window(&old, now - 3 * 86_400, now + 86_400));
        assert!(in_window(&fresh, now - 3 * 86_400, now + 86_400));
        assert!(!in_window(&far, now - 3 * 86_400, now + 86_400));
    }

    #[test]
    fn channel_targets_use_tvg_id_and_normalized_name() {
        let mut config = Config::default();
        let mut group = crate::config::model::Group::default();
        group.id = "g".into();
        let mut first = crate::config::model::Channel::default();
        first.id = "e1".into();
        first.name = "CCTV-1 综合".into();
        first
            .extra
            .insert("tvg_id".into(), serde_json::Value::String("CCTV1".into()));
        let mut second = crate::config::model::Channel::default();
        second.id = "e2".into();
        second.name = " cctv-3　综艺 ".into();
        let mut third = crate::config::model::Channel::default();
        third.id = "E3".into();
        third.name = "E3".into();
        group.channels = vec![first, second, third];
        config.groups.push(group);

        let targets = channel_targets(&config);
        assert_eq!(targets[0].out_id, "cctv1");
        assert_eq!(targets[1].out_id, "e2");
        assert_eq!(targets[2].out_id, "e3");
        assert_eq!(targets[1].normalized, "cctv-3综艺");
        assert_eq!(normalize_name("  A　B  "), "ab");
    }

    #[test]
    fn match_source_prefers_tvg_then_name_and_first_owner_wins() {
        let mut config = Config::default();
        let mut group = crate::config::model::Group::default();
        group.id = "g".into();
        let mut e1 = crate::config::model::Channel::default();
        e1.id = "e1".into();
        e1.name = "CCTV-1 综合".into();
        e1.extra
            .insert("tvg_id".into(), serde_json::Value::String("cctv1".into()));
        let mut e2 = crate::config::model::Channel::default();
        e2.id = "e2".into();
        e2.name = "cctv-1 综合".into();
        let mut e3 = crate::config::model::Channel::default();
        e3.id = "e3".into();
        e3.name = "CCTV-3 综艺".into();
        let mut e4 = crate::config::model::Channel::default();
        e4.id = "e4".into();
        e4.name = "CCTV-1 综合".into();
        e4.extra
            .insert("tvg_id".into(), serde_json::Value::String("missing".into()));
        group.channels = vec![e1, e2, e3, e4];
        config.groups.push(group);
        let targets = channel_targets(&config);
        let document = parse(
            r#"<tv><channel id="cctv1"><display-name>CCTV-1 综合</display-name></channel>
<channel id="cctv3"><display-name>CCTV-3 综艺</display-name></channel>
<programme start="20261020080000 +0000" stop="20261020090000 +0000" channel="cctv1"><title>A</title></programme>
<programme start="20261020090000 +0000" stop="20261020100000 +0000" channel="cctv1"><title>B</title></programme>
<programme start="20261020080000 +0000" stop="20261020090000 +0000" channel="cctv3"><title>C</title></programme></tv>"#,
        )
        .expect("解析");
        let (candidates, owners, programmes) = match_source(
            &targets,
            &document,
            &HashMap::new(),
            "s1",
            &config,
            0,
            4_000_000_000,
        );
        assert_eq!(candidates.len(), 4);
        assert_eq!(candidates[0].via, "tvg-id");
        assert_eq!(candidates[1].via, "name");
        assert_eq!(candidates[3].via, "name", "tvg_id 未命中时回退名称");
        assert_eq!(owners["cctv1"], 0, "配置顺序最靠前的同名频道获得节目");
        assert_eq!(programmes["cctv1"].len(), 2);
        assert_eq!(programmes["cctv3"].len(), 1);
    }

    #[test]
    fn manual_mapping_channel_is_used() {
        let mut config = Config::default();
        let mut group = crate::config::model::Group::default();
        group.id = "g".into();
        let mut channel = crate::config::model::Channel::default();
        channel.id = "e6".into();
        channel.name = "本地频道".into();
        group.channels = vec![channel];
        config.groups.push(group);
        let targets = channel_targets(&config);
        let document = parse(
            r#"<tv><channel id="only"><display-name>ONLY</display-name></channel>
<programme start="20261020080000 +0000" stop="20261020090000 +0000" channel="only"><title>A</title></programme></tv>"#,
        )
        .expect("解析");
        let mut mappings = HashMap::new();
        mappings.insert(
            "g/e6".into(),
            Mapping {
                source_id: "s1".into(),
                source_channel_id: "only".into(),
                ..Mapping::default()
            },
        );
        let (candidates, _, _) = match_source(
            &targets,
            &document,
            &mappings,
            "s1",
            &config,
            0,
            4_000_000_000,
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].via, "manual");
        assert_eq!(candidates[0].channel_id, "only");
    }

    #[test]
    fn merge_document_keeps_first_url_channel() {
        let mut merged = parse(
            r#"<tv><channel id="1"><display-name>CCTV1</display-name></channel>
<programme start="20261020080000 +0000" stop="20261020090000 +0000" channel="1"><title>A</title></programme></tv>"#,
        )
        .expect("解析");
        let next = parse(
            r#"<tv><channel id="1"><display-name>CCTV1 备用</display-name></channel>
<channel id="2"><display-name>CCTV2</display-name></channel>
<programme start="20261020080000 +0000" stop="20261020090000 +0000" channel="1"><title>B</title></programme>
<programme start="20261020080000 +0000" stop="20261020090000 +0000" channel="2"><title>C</title></programme></tv>"#,
        )
        .expect("解析");
        merge_document(&mut merged, next);
        assert_eq!(merged.channels.len(), 2);
        let titles: Vec<&str> = merged
            .programmes
            .iter()
            .map(|programme| {
                programme
                    .texts
                    .iter()
                    .find(|(name, _, _)| name == "title")
                    .map(|(_, _, text)| text.as_str())
                    .unwrap_or("")
            })
            .collect();
        assert!(titles.contains(&"A"));
        assert!(titles.contains(&"C"));
        assert!(!titles.contains(&"B"), "低优先级 URL 的同频道节目应丢弃");
    }

    #[test]
    fn matches_channel_id_equal_to_channel_name() {
        let mut config = Config::default();
        let mut group = crate::config::model::Group::default();
        group.id = "g".into();
        let mut channel = crate::config::model::Channel::default();
        channel.id = "c1".into();
        channel.name = "CCTV1".into();
        group.channels = vec![channel];
        config.groups.push(group);
        let targets = channel_targets(&config);
        let document = parse(
            r#"<tv><channel id="CCTV1"></channel>
<programme start="20261020080000 +0000" stop="20261020090000 +0000" channel="CCTV1"><title>A</title></programme></tv>"#,
        )
        .expect("解析");
        let (candidates, _, _) = match_source(
            &targets,
            &document,
            &HashMap::new(),
            "s1",
            &config,
            0,
            4_000_000_000,
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].via, "id");
        assert_eq!(candidates[0].channel_id, "CCTV1");
    }

    #[test]
    fn global_schedule_only_uses_default_sources() {
        let mut source = crate::epg_store::Source {
            enabled: true,
            refresh_secs: 0,
            cron: String::new(),
            ..crate::epg_store::Source::default()
        };
        assert!(uses_global_schedule(&source));
        source.refresh_secs = 3_600;
        assert!(!uses_global_schedule(&source), "显式间隔不跟全局 cron");
        source.refresh_secs = 0;
        source.cron = "0 3 * * *".into();
        assert!(!uses_global_schedule(&source), "源 cron 不跟全局 cron");
        source.cron.clear();
        source.enabled = false;
        assert!(!uses_global_schedule(&source), "停用源不采集");
    }

    #[test]
    fn cron_preview_parses_full_expression() {
        // 2023-11-14T22:13:20Z
        let now = 1_700_000_000;
        let next = cron_preview("0 3,5,12 * * *", now, 0, 3).expect("表达式应有效");
        let hours: Vec<i64> = next
            .iter()
            .map(|epoch| epoch.rem_euclid(86_400) / 3_600)
            .collect();
        assert_eq!(hours, vec![3, 5, 12]);
        assert!(cron_preview("bad", now, 0, 1).is_err());
    }
}
