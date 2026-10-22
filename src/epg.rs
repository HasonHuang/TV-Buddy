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
    /// 手工映射：键 `gid/cid`。
    pub mappings: Mutex<HashMap<String, Mapping>>,
    /// 最近一次匹配报告。
    pub match_rows: Mutex<Vec<MatchRow>>,
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
    pub out_id: String,
    pub via: String,
    pub source_id: String,
    pub programmes: usize,
}

impl EpgState {
    /// 载入缓存的映射表。
    pub fn load(cache_dir: &Path) -> Self {
        let state = Self::default();
        if let Ok(text) = fs::read_to_string(mapping_file(cache_dir)) {
            if let Ok(mappings) = serde_json::from_str::<HashMap<String, Mapping>>(&text) {
                *state.mappings.lock().unwrap_or_else(|e| e.into_inner()) = mappings;
            }
        }
        state
    }
}

fn mapping_file(cache_dir: &Path) -> PathBuf {
    output_dir(cache_dir).join("mappings.json")
}

/// 读取映射快照。
pub fn mappings(state: &AppState) -> HashMap<String, Mapping> {
    state
        .epg
        .mappings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
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
    let mut map = state.epg.mappings.lock().unwrap_or_else(|e| e.into_inner());
    let key = format!("{gid}/{cid}");
    if source_id.is_empty() || source_channel_id.is_empty() {
        map.remove(&key);
        save_mappings(state, &map)?;
        return Ok(None);
    }
    let Some(source) = config
        .epg
        .sources
        .iter()
        .find(|source| source.id == source_id)
    else {
        bail!("EPG 源不存在");
    };
    let mapping = Mapping {
        source_id: source_id.to_string(),
        source_channel_id: source_channel_id.to_string(),
        source_name: source.name.clone(),
        source_display_name: String::new(),
    };
    map.insert(key, mapping.clone());
    save_mappings(state, &map)?;
    Ok(Some(mapping))
}

fn save_mappings(state: &AppState, mappings: &HashMap<String, Mapping>) -> Result<()> {
    let path = mapping_file(Path::new(&state.cache_dir));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(mappings)?;
    fs::write(path, text)?;
    Ok(())
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

/// 触发一次采集。未启用或已在采集时返回错误。
pub async fn refresh(state: &Arc<AppState>) -> Result<Summary> {
    let config = state.snapshot();
    if !config.epg.enabled {
        bail!("EPG 未启用");
    }
    if state.epg.running.swap(true, Ordering::SeqCst) {
        bail!("EPG 采集进行中");
    }
    let _guard = RunningGuard(&state.epg.running);

    let result = collect(state, &config).await;
    match &result {
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
    result
}

async fn collect(state: &Arc<AppState>, config: &Config) -> Result<Summary> {
    let targets = channel_targets(config);
    let mappings = mappings(state);
    let mut rows: Vec<MatchRow> = targets
        .iter()
        .map(|target| MatchRow {
            gid: target.gid.clone(),
            cid: target.cid.clone(),
            name: target.name.clone(),
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
    let mut mapping_displays: HashMap<(String, String), String> = HashMap::new();

    for source in config.epg.sources.iter().filter(|source| source.enabled) {
        let body = match source_body(state, config, source).await {
            Ok(body) => body,
            Err(error) => {
                errors.push(format!("{}：{error:#}", source.name));
                continue;
            }
        };
        sources += 1;
        let document = match parse(&body) {
            Ok(document) => document,
            Err(error) => {
                errors.push(format!("{}：解析失败 {error:#}", source.name));
                continue;
            }
        };
        for mapping in mappings.values() {
            if mapping.source_id != source.id {
                continue;
            }
            if let Some(channel) = document
                .channels
                .iter()
                .find(|channel| channel.id == mapping.source_channel_id)
            {
                if let Some((_, name)) = channel.names.first() {
                    mapping_displays.insert(
                        (mapping.source_id.clone(), mapping.source_channel_id.clone()),
                        name.clone(),
                    );
                }
            }
        }
        let (candidates, owners, source_programmes) = match_source(
            &targets, &document, &mappings, &source.id, config, past, future,
        );
        for candidate in &candidates {
            let row = &mut rows[candidate.target];
            if row.source_id.is_empty() || candidate.manual {
                row.via = candidate.via.clone();
                row.source_id = source.id.clone();
                row.programmes = if owners.get(&candidate.channel_id) == Some(&candidate.target) {
                    source_programmes
                        .get(&candidate.channel_id)
                        .map(Vec::len)
                        .unwrap_or(0)
                } else {
                    0
                };
            }
            let target = &targets[candidate.target];
            if !seen.contains(&target.out_id) {
                seen.insert(target.out_id.clone());
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
            }
            if owners.get(&candidate.channel_id) == Some(&candidate.target) {
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
    *state
        .epg
        .match_rows
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = rows;
    if !mapping_displays.is_empty() {
        let mut map = state
            .epg
            .mappings
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for mapping in map.values_mut() {
            if let Some(name) = mapping_displays
                .get(&(mapping.source_id.clone(), mapping.source_channel_id.clone()))
            {
                mapping.source_display_name = name.clone();
            }
        }
        let _ = save_mappings(state, &map);
    }

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
            let tvg_id = ["tvg_id", "tvg-id", "epg_id"]
                .iter()
                .find_map(|key| {
                    channel
                        .extra
                        .get(*key)
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
                .map(str::to_string);
            let out_id = tvg_id.clone().unwrap_or_else(|| channel.id.clone());
            targets.push(Target {
                gid: group.id.clone(),
                cid: channel.id.clone(),
                name: channel.name.clone(),
                out_id,
                tvg_id: tvg_id.map(|value| value.to_lowercase()),
                normalized: normalize_name(&channel.name),
            });
        }
    }
    targets
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
            if let Some(channel) = by_tvg {
                (Some(channel), "tvg-id", false)
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
        owners.insert(candidate.channel_id.clone(), candidate.target);
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

/// 取 EPG 源正文：`xml` 抓 URL；`script` 跑 `<cache>/epg/scripts/` 下的 Python。
async fn source_body(
    state: &Arc<AppState>,
    config: &Config,
    source: &crate::config::model::EpgSource,
) -> Result<String> {
    match source.kind.as_str() {
        "xml" => {
            if source.url.trim().is_empty() {
                bail!("缺少 url");
            }
            let response = state
                .http
                .get(source.url.trim())
                .send()
                .await
                .context("抓取失败")?
                .error_for_status()
                .context("上游状态异常")?;
            response.text().await.context("读取正文失败")
        }
        "script" => run_script(config, source).await,
        other => bail!("未知源类型 {other}"),
    }
}

async fn run_script(config: &Config, source: &crate::config::model::EpgSource) -> Result<String> {
    let name = crate::logo::sanitize(&source.script).context("脚本名不合法")?;
    let dir = Path::new(&config.cache.disk_path)
        .join("epg")
        .join("scripts");
    let script = dir.join(&name);
    if !script.is_file() {
        bail!("脚本不存在: {}", script.display());
    }
    let timeout = std::time::Duration::from_secs(source.timeout_secs.clamp(5, 3600));
    let mut command = tokio::process::Command::new("python3");
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
        .and_then(|value| value.as_integer())
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
    let enabled = state.snapshot().epg.enabled;
    if !enabled {
        return;
    }
    let hours = cron_hours(&state.snapshot().epg.cron).unwrap_or(4).max(1);
    tokio::spawn(async move {
        match refresh(&state).await {
            Ok(summary) => tracing::info!(
                channels = summary.channels,
                programmes = summary.kept,
                "EPG 采集完成"
            ),
            Err(error) => tracing::warn!(%error, "EPG 采集失败"),
        }
        let mut ticker = tokio::time::interval(Duration::from_secs(hours * 3600));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if !state.snapshot().epg.enabled {
                continue;
            }
            match refresh(&state).await {
                Ok(summary) => tracing::info!(
                    channels = summary.channels,
                    programmes = summary.kept,
                    "EPG 采集完成"
                ),
                Err(error) => tracing::warn!(%error, "EPG 采集失败"),
            }
        }
    });
}

/// `0 */N * * *` → N 小时。
fn cron_hours(cron: &str) -> Option<u64> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() < 5 || fields[0] != "0" || fields[2..] != ["*", "*", "*"] {
        return None;
    }
    let hours = fields[1].strip_prefix("*/")?;
    hours.parse().ok()
}

fn now_secs() -> u64 {
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
#[derive(Debug, Default)]
pub struct DocumentData {
    pub channels: Vec<Channel>,
    pub programmes: Vec<Programme>,
}

impl DocumentData {
    /// 只保留能匹配到本服务频道的内容。
    #[cfg(test)]
    fn matched(self, keys: &HashSet<String>) -> Self {
        let channels: Vec<Channel> = self
            .channels
            .into_iter()
            .filter(|channel| {
                keys.contains(channel.id.trim())
                    || channel
                        .names
                        .iter()
                        .any(|(_, name)| keys.contains(name.trim()))
            })
            .collect();
        let ids: HashSet<String> = channels.iter().map(|c| c.id.clone()).collect();
        let programmes = self
            .programmes
            .into_iter()
            .filter(|programme| ids.contains(programme.channel.trim()))
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
    out.push_str("<tv generator-info-name=\"iptv-proxy\">\n");
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
        let matched = data.matched(&keys);
        assert_eq!(matched.channels.len(), 1);
        assert_eq!(matched.channels[0].id, "cctv1");
        assert_eq!(matched.programmes.len(), 1);
    }

    #[test]
    fn translate_config_reads_targets_and_endpoint() {
        let mut config = Config::default();
        let mut translate = toml::Table::new();
        translate.insert("enabled".into(), toml::Value::Boolean(true));
        translate.insert("url".into(), toml::Value::String("http://mtran/".into()));
        translate.insert("target".into(), toml::Value::String(" ja ".into()));
        let mut epg = toml::Table::new();
        epg.insert("translate".into(), toml::Value::Table(translate));
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
        let mut translate = toml::Table::new();
        translate.insert("enabled".into(), toml::Value::Boolean(true));
        translate.insert("engine".into(), toml::Value::String("mtran".into()));
        translate.insert(
            "mtran_url".into(),
            toml::Value::String("https://translate.example/translate".into()),
        );
        translate.insert("mtran_token".into(), toml::Value::String("secret".into()));
        translate.insert("mtran_source".into(), toml::Value::String("en".into()));
        translate.insert("mtran_batch".into(), toml::Value::Integer(50));
        translate.insert("target".into(), toml::Value::String("zh-cn".into()));
        let mut epg = toml::Table::new();
        epg.insert("translate".into(), toml::Value::Table(translate));
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
        let mut translate = toml::Table::new();
        translate.insert(
            "targets".into(),
            toml::Value::Array(vec![
                toml::Value::String("zh-cn".into()),
                toml::Value::String("".into()),
                toml::Value::String("en".into()),
            ]),
        );
        translate.insert("target".into(), toml::Value::String("ja".into()));
        let mut epg = toml::Table::new();
        epg.insert("translate".into(), toml::Value::Table(translate));
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
        assert!(xml.contains("generator-info-name=\"iptv-proxy\""));
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
            .insert("tvg_id".into(), toml::Value::String("cctv1".into()));
        let mut second = crate::config::model::Channel::default();
        second.id = "e2".into();
        second.name = " cctv-3　综艺 ".into();
        group.channels = vec![first, second];
        config.groups.push(group);

        let targets = channel_targets(&config);
        assert_eq!(targets[0].out_id, "cctv1");
        assert_eq!(targets[1].out_id, "e2");
        assert_eq!(targets[1].normalized, "cctv-3综艺");
        assert_eq!(normalize_name("  A　B  "), "ab");
    }

    #[test]
    fn match_source_prefers_tvg_then_name_and_last_owner_wins() {
        let mut config = Config::default();
        let mut group = crate::config::model::Group::default();
        group.id = "g".into();
        let mut e1 = crate::config::model::Channel::default();
        e1.id = "e1".into();
        e1.name = "CCTV-1 综合".into();
        e1.extra
            .insert("tvg_id".into(), toml::Value::String("cctv1".into()));
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
            .insert("tvg_id".into(), toml::Value::String("missing".into()));
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
        assert_eq!(owners["cctv1"], 3, "配置顺序最后的同名频道获得节目");
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
    fn cron_hours_parses_simple_pattern() {
        assert_eq!(cron_hours("0 */4 * * *"), Some(4));
        assert_eq!(cron_hours("0 3 * * *"), None);
        assert_eq!(cron_hours(""), None);
    }
}
