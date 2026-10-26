//! DASH 时移：按轨道录制 DASH 切片，并提供回看清单与切片。
//!
//! 证据：F-016 §5。目标（v0.8.2）实测契约（2026-10-26 复验）：
//!
//! | 情形 | 目标 |
//! | --- | --- |
//! | 清单路径 `/{gid}/{cid}/dash-timeshift/{track}` 或 `…/{playlist,playlist.txt,playlist.m3u8}` | 200 |
//! | 缺 `playseek`，或格式不对 | `400 missing playseek` |
//! | 区间与已录制区间不重叠、反向或等长 | `404 no segments in range` |
//! | 未知轨道 | `404 no segments in range` |
//! | 切片 `/{track}/{序号}.{伪装扩展名}` 未命中 | `404 segment not found` |
//! | 非 `dash` 组、未知 `gid`/`cid`、未开 `dash_timeshift` | 同上两种 404，**不跳 302** |
//!
//! 其他实测事实：
//! - 切片名主干是**上游 MPD 段号**，不是本机计数器；
//! - `#EXT-X-MEDIA-SEQUENCE` = 首条切片的段号；
//! - `#EXT-X-PROGRAM-DATE-TIME` = 切片**到达本机**的时间（UTC）；
//! - 切片接受 `<段号>.<伪装扩展名>` 与 `<段号>.m4s`（磁盘原名）；
//! - init 接受 `init.<伪装扩展名>` 与 `init.mp4`（磁盘原名）；
//! - 清单 `Cache-Control: public, max-age=3600, s-maxage=3600`，切片 `private, max-age=3600`；
//! - 落盘 `/cache/dash_timeshift/{gid}_{cid}/{index.json,init/,segments/}`。
//!
//! 本模块不做转封装：按 MPD 逐轨道抓原字节，落盘后原样回放。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use tracing::{info, warn};

use crate::{dash, state::AppState};

/// 空闲多久后回收会话。
const IDLE: Duration = Duration::from_secs(900);
/// 清单文件名别名。空名（只有轨道）也算清单。
const PLAYLIST_NAMES: &[&str] = &["playlist", "playlist.txt", "playlist.m3u8"];
/// 单轮最多补录的新切片数。
const MAX_NEW_PER_POLL: usize = 64;
/// MPD 没有给出时长时的轮询间隔。
const FALLBACK_INTERVAL: f64 = 2.0;
/// 默认伪装扩展名。
const DEFAULT_EXTS: &[&str] = &["jpg", "png", "gif", "bmp", "webp"];
/// 图片类扩展名。切片与 init 都接受。
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp"];
/// 媒体切片在磁盘上的原名扩展名。
const MEDIA_EXT: &str = "m4s";
/// init 在磁盘上的原名扩展名。
const INIT_EXT: &str = "mp4";

/// 一条已录制切片。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Recorded {
    /// 上游 MPD 段号。同时是切片文件名的主干。
    seq: u64,
    /// 时长（秒）。
    dur: f64,
    /// 到达本机的时间（Unix 秒）。
    ts: i64,
}

/// 单条轨道的录制索引。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Track {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    bandwidth: Option<u64>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    codecs: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    has_init: bool,
    #[serde(default)]
    segments: VecDeque<Recorded>,
}

/// 落盘索引。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Index {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    tracks: HashMap<String, Track>,
}

/// 一个频道时移会话。
pub struct Session {
    gid: String,
    cid: String,
    dir: PathBuf,
    index: Mutex<Index>,
    /// 已录制的上游段号。启动时由索引重建。
    seen: Mutex<HashMap<String, HashSet<u64>>>,
    bytes: Mutex<u64>,
    hours: Mutex<u32>,
    max_gb: Mutex<u64>,
    last_error: Mutex<Option<String>>,
    last: Mutex<Instant>,
    stop: AtomicBool,
}

/// 面板可读状态。
pub struct SessionInfo {
    pub tracks: Vec<String>,
    pub audio_count: usize,
    pub segments: usize,
    pub bytes: u64,
    pub oldest_age_secs: Option<i64>,
    pub running: bool,
    pub last_error: Option<String>,
    pub retain_hours: u32,
    pub max_gb: u64,
    pub target_duration: f64,
}

/// 清单查询结果。
enum Playlist {
    Body(String),
    Empty,
    UnknownTrack,
}

impl Session {
    fn touch(&self) {
        *self.last.lock().unwrap_or_else(|error| error.into_inner()) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .elapsed()
    }

    fn set_error(&self, value: Option<String>) {
        if let Some(text) = &value {
            warn!(gid = %self.gid, cid = %self.cid, error = %text, "DASH 时移录制失败");
        }
        *self
            .last_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = value;
    }

    fn set_limits(&self, hours: u32, max_gb: u64) {
        *self.hours.lock().unwrap_or_else(|error| error.into_inner()) = hours;
        *self
            .max_gb
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = max_gb;
    }

    fn track_dir(&self, track: &str) -> PathBuf {
        self.dir.join("segments").join(safe(track))
    }

    fn init_path(&self, track: &str) -> PathBuf {
        self.dir
            .join("init")
            .join(format!("{}.{INIT_EXT}", safe(track)))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    fn bytes_now(&self) -> u64 {
        *self.bytes.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn seen_for(&self, track: &str) -> HashSet<u64> {
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(track)
            .cloned()
            .unwrap_or_default()
    }

    /// 从磁盘载入索引，重建去重集合与字节计数。
    fn load(&self) {
        let Ok(text) = fs::read_to_string(self.index_path()) else {
            return;
        };
        let Ok(index) = serde_json::from_str::<Index>(&text) else {
            warn!(dir = %self.dir.display(), "DASH 时移索引损坏，重新开始");
            return;
        };
        let mut seen: HashMap<String, HashSet<u64>> = HashMap::new();
        let mut total = 0u64;
        for (name, track) in &index.tracks {
            seen.insert(
                name.clone(),
                track.segments.iter().map(|item| item.seq).collect(),
            );
            for item in &track.segments {
                if let Ok(meta) = fs::metadata(
                    self.track_dir(name)
                        .join(format!("{}.{MEDIA_EXT}", item.seq)),
                ) {
                    total += meta.len();
                }
            }
            if track.has_init {
                if let Ok(meta) = fs::metadata(self.init_path(name)) {
                    total += meta.len();
                }
            }
        }
        *self.index.lock().unwrap_or_else(|error| error.into_inner()) = index;
        *self.seen.lock().unwrap_or_else(|error| error.into_inner()) = seen;
        *self.bytes.lock().unwrap_or_else(|error| error.into_inner()) = total;
    }

    /// 原子写索引。
    fn persist(&self) {
        let text = {
            let index = self.index.lock().unwrap_or_else(|error| error.into_inner());
            match serde_json::to_vec(&*index) {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!(%error, "DASH 时移索引序列化失败");
                    return;
                }
            }
        };
        if fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let path = self.index_path();
        let temporary = path.with_extension("json.tmp");
        if fs::write(&temporary, &text).is_ok() {
            let _ = fs::rename(&temporary, &path);
        }
    }

    fn init_missing(&self, track: &str) -> bool {
        let index = self.index.lock().unwrap_or_else(|error| error.into_inner());
        index
            .tracks
            .get(track)
            .map(|track| !track.has_init)
            .unwrap_or(true)
    }

    /// 保存初始化段。
    fn save_init(&self, stream: &dash::Stream, bytes: &[u8]) {
        let dir = self.dir.join("init");
        if fs::create_dir_all(&dir).is_err()
            || fs::write(self.init_path(&stream.name), bytes).is_err()
        {
            return;
        }
        let mut index = self.index.lock().unwrap_or_else(|error| error.into_inner());
        let entry = index.tracks.entry(stream.name.clone()).or_default();
        fill(entry, stream);
        entry.has_init = true;
    }

    /// 保存一条切片。已录过的段号直接跳过，返回 `false`。
    fn record(&self, stream: &dash::Stream, segment: &dash::Segment, bytes: &[u8]) -> bool {
        // 先占位再写盘：并发调用（请求线程与录制循环）不会重复登记。
        {
            let mut index = self.index.lock().unwrap_or_else(|error| error.into_inner());
            let entry = index.tracks.entry(stream.name.clone()).or_default();
            if entry.segments.iter().any(|item| item.seq == segment.number) {
                return false;
            }
            fill(entry, stream);
            // 清单按段号升序输出。正常追加不需要排序；上游乱序时才排一次。
            let out_of_order = entry
                .segments
                .back()
                .map(|last| last.seq > segment.number)
                .unwrap_or(false);
            entry.segments.push_back(Recorded {
                seq: segment.number,
                dur: segment.duration.max(0.001),
                ts: crate::schedule::now_epoch(),
            });
            if out_of_order {
                entry
                    .segments
                    .make_contiguous()
                    .sort_by_key(|item| item.seq);
            }
        }
        let dir = self.track_dir(&stream.name);
        if fs::create_dir_all(&dir).is_err()
            || fs::write(dir.join(format!("{}.{MEDIA_EXT}", segment.number)), bytes).is_err()
        {
            // 写盘失败要撤回登记，避免索引指向不存在的文件。
            let mut index = self.index.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = index.tracks.get_mut(&stream.name) {
                entry.segments.retain(|item| item.seq != segment.number);
            }
            return false;
        }
        *self.bytes.lock().unwrap_or_else(|error| error.into_inner()) += bytes.len() as u64;
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(stream.name.clone())
            .or_default()
            .insert(segment.number);
        true
    }

    /// 删除过期与超额切片。
    fn prune(&self, retain_hours: u32, max_gb: u64) {
        let now = crate::schedule::now_epoch();
        let window = retain_hours.max(1) as i64 * 3600;
        let limit = max_gb.saturating_mul(1 << 30);
        let mut freed = 0u64;
        {
            let mut index = self.index.lock().unwrap_or_else(|error| error.into_inner());
            for (name, track) in index.tracks.iter_mut() {
                while let Some(front) = track.segments.front() {
                    if now - front.ts <= window {
                        break;
                    }
                    let Some(old) = track.segments.pop_front() else {
                        break;
                    };
                    let path = self
                        .track_dir(name)
                        .join(format!("{}.{MEDIA_EXT}", old.seq));
                    if let Ok(meta) = fs::metadata(&path) {
                        freed += meta.len();
                    }
                    let _ = fs::remove_file(&path);
                }
            }
            // 超额时按到达时间从旧到新删除，跨轨道统一取最旧。
            while max_gb > 0 && self.bytes_now().saturating_sub(freed) > limit {
                let Some((track, seq)) = oldest(&index) else {
                    break;
                };
                let Some(entry) = index
                    .tracks
                    .get_mut(&track)
                    .and_then(|track| track.segments.pop_front())
                else {
                    break;
                };
                debug_assert_eq!(entry.seq, seq);
                let path = self
                    .track_dir(&track)
                    .join(format!("{}.{MEDIA_EXT}", entry.seq));
                if let Ok(meta) = fs::metadata(&path) {
                    freed += meta.len();
                }
                let _ = fs::remove_file(&path);
            }
        }
        if freed > 0 {
            let mut bytes = self.bytes.lock().unwrap_or_else(|error| error.into_inner());
            *bytes = bytes.saturating_sub(freed);
        }
    }

    /// 生成回看清单。
    fn playlist(&self, track: &str, start: i64, end: i64) -> Playlist {
        self.touch();
        let index = self.index.lock().unwrap_or_else(|error| error.into_inner());
        let Some(entry) = index.tracks.get(track) else {
            return Playlist::UnknownTrack;
        };
        let picked: Vec<&Recorded> = entry
            .segments
            .iter()
            .filter(|item| {
                let tail = item.ts + item.dur.ceil() as i64;
                tail > start && item.ts < end
            })
            .collect();
        if picked.is_empty() {
            return Playlist::Empty;
        }
        let target = picked
            .iter()
            .map(|item| item.dur.ceil() as u64)
            .max()
            .unwrap_or(1)
            .max(1);
        let first = picked[0];
        let mut body = format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:{}\n#EXT-X-PROGRAM-DATE-TIME:{}\n",
            first.seq,
            crate::schedule::format_utc_iso(first.ts)
        );
        if entry.has_init {
            let init_ext = pick_ext(track, first.seq);
            body.push_str(&format!(
                "#EXT-X-MAP:URI=\"/{}/{}/dash-timeshift/{}/init.{init_ext}\"\n",
                self.gid, self.cid, track
            ));
        }
        for (index, item) in picked.iter().enumerate() {
            let ext = pick_ext(track, first.seq + index as u64 + 1);
            body.push_str(&format!(
                "#EXTINF:{:.3},\n/{}/{}/dash-timeshift/{}/{}.{ext}\n",
                item.dur, self.gid, self.cid, track, item.seq
            ));
        }
        body.push_str("#EXT-X-ENDLIST\n");
        Playlist::Body(body)
    }

    /// 读取切片或 init。
    ///
    /// `stem` 是 `init` 或段号；`ext` 必须是伪装扩展名或磁盘原名。
    fn read(&self, track: &str, stem: &str, ext: &str) -> Option<(Vec<u8>, &'static str)> {
        let index = self.index.lock().unwrap_or_else(|error| error.into_inner());
        let entry = index.tracks.get(track)?;
        self.touch();
        if stem == "init" {
            if !entry.has_init || !(IMAGE_EXTS.contains(&ext) || ext == INIT_EXT) {
                return None;
            }
            let bytes = fs::read(self.init_path(track)).ok()?;
            return Some((bytes, media_type(ext, INIT_EXT)));
        }
        let seq: u64 = stem.parse().ok()?;
        if stem.len() > 1 && stem.starts_with('0') {
            return None;
        }
        if !entry.segments.iter().any(|item| item.seq == seq) {
            return None;
        }
        if !(IMAGE_EXTS.contains(&ext) || ext == MEDIA_EXT) {
            return None;
        }
        let bytes = fs::read(self.track_dir(track).join(format!("{seq}.{MEDIA_EXT}"))).ok()?;
        Some((bytes, media_type(ext, MEDIA_EXT)))
    }

    fn info(&self) -> SessionInfo {
        let index = self.index.lock().unwrap_or_else(|error| error.into_inner());
        let segments = index
            .tracks
            .values()
            .map(|track| track.segments.len())
            .max()
            .unwrap_or(0);
        let oldest = index
            .tracks
            .values()
            .filter_map(|track| track.segments.front().map(|item| item.ts))
            .min();
        SessionInfo {
            tracks: index.tracks.keys().cloned().collect(),
            audio_count: index
                .tracks
                .values()
                .filter(|track| track.kind == "audio")
                .count(),
            segments,
            bytes: self.bytes_now(),
            oldest_age_secs: oldest.map(|ts| (crate::schedule::now_epoch() - ts).max(0)),
            running: !self.stop.load(Ordering::SeqCst),
            last_error: self
                .last_error
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
            retain_hours: *self.hours.lock().unwrap_or_else(|error| error.into_inner()),
            max_gb: *self
                .max_gb
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            target_duration: index
                .tracks
                .values()
                .filter_map(|track| track.segments.front().map(|item| item.dur))
                .fold(FALLBACK_INTERVAL, f64::max),
        }
    }

    fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 补上轨道元数据。
fn fill(entry: &mut Track, stream: &dash::Stream) {
    entry.kind.clone_from(&stream.kind);
    entry.bandwidth = stream.bandwidth;
    entry.resolution = match (stream.width, stream.height) {
        (Some(width), Some(height)) => Some(format!("{width}x{height}")),
        _ => None,
    };
    entry.codecs.clone_from(&stream.codecs);
    entry.language.clone_from(&stream.lang);
}

/// 解析 `playseek=<本地 yyyymmddHHMMSS>-<本地 yyyymmddHHMMSS>`。
///
/// 只校验格式；区间是否为空由调用方判断（目标对两种情况给不同状态码）。
fn parse_seek(text: &str, offset_hours: i64) -> Option<(i64, i64)> {
    let (start, end) = text.split_once('-')?;
    let start = parse_compact(start, offset_hours)?;
    let end = parse_compact(end, offset_hours)?;
    Some((start, end))
}

fn parse_compact(text: &str, offset_hours: i64) -> Option<i64> {
    if text.len() != 14 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let expanded = format!(
        "{}-{}-{}T{}:{}:{}",
        &text[0..4],
        &text[4..6],
        &text[6..8],
        &text[8..10],
        &text[10..12],
        &text[12..14]
    );
    crate::schedule::parse_local(&expanded, offset_hours)
}

/// 索引里最旧条目所属的轨道与段号。
fn oldest(index: &Index) -> Option<(String, u64)> {
    index
        .tracks
        .iter()
        .filter_map(|(name, track)| track.segments.front().map(|item| (name, item.ts, item.seq)))
        .min_by_key(|(_, ts, _)| *ts)
        .map(|(name, _, seq)| (name.clone(), seq))
}

/// 轨道名与段号到伪装扩展名。同一段号恒定。
fn pick_ext(track: &str, seq: u64) -> String {
    let digest = Sha256Digest::digest(format!("{track}/{seq}").as_bytes());
    DEFAULT_EXTS[(digest[0] as usize) % DEFAULT_EXTS.len()].to_string()
}

/// `sha2` 的轻量封装，避免为一行引入别名。
struct Sha256Digest;

impl Sha256Digest {
    fn digest(input: &[u8]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(input);
        let out = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&out);
        bytes
    }
}

/// 按请求扩展名给出 `Content-Type`。
fn media_type(ext: &str, original: &str) -> &'static str {
    if ext == original {
        return "video/mp4";
    }
    match ext {
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

/// 轨道名到落盘目录名。只保留安全字符，避免路径穿越。
fn safe(value: &str) -> String {
    let mapped: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if mapped.is_empty() || mapped.chars().all(|ch| ch == '_') {
        "empty".to_string()
    } else {
        mapped
    }
}

/// 管理器。
#[derive(Default)]
pub struct Manager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    janitor: AtomicBool,
}

struct Settings {
    url: String,
    hours: u32,
    max_gb: u64,
}

fn key(gid: &str, cid: &str) -> String {
    format!("{gid}/{cid}")
}

/// 只在 `dash` 模式且点亮 `dash_timeshift` 时返回参数。
fn settings(state: &AppState, gid: &str, cid: &str) -> Option<Settings> {
    let config = state.snapshot();
    let group = config.group(gid)?;
    if group.mode != "dash" {
        return None;
    }
    let channel = group.channels.iter().find(|channel| channel.id == cid)?;
    if !channel.dash_timeshift {
        return None;
    }
    Some(Settings {
        url: channel.url.clone(),
        hours: channel.dash_timeshift_hours.max(1),
        max_gb: channel.dash_timeshift_max_gb,
    })
}

impl Manager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取会话；不满足录制条件时返回 `None`。
    pub fn ensure(
        self: &Arc<Self>,
        state: &Arc<AppState>,
        gid: &str,
        cid: &str,
    ) -> Option<Arc<Session>> {
        let settings = settings(state, gid, cid)?;
        let key = key(gid, cid);
        if let Some(session) = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&key)
            .cloned()
        {
            session.touch();
            session.set_limits(settings.hours, settings.max_gb);
            return Some(session);
        }
        let dir =
            state
                .cache_dir
                .join("dash_timeshift")
                .join(format!("{}_{}", safe(gid), safe(cid)));
        let _ = fs::create_dir_all(&dir);
        let session = Arc::new(Session {
            gid: gid.to_string(),
            cid: cid.to_string(),
            dir,
            index: Mutex::new(Index::default()),
            seen: Mutex::new(HashMap::new()),
            bytes: Mutex::new(0),
            hours: Mutex::new(settings.hours),
            max_gb: Mutex::new(settings.max_gb),
            last_error: Mutex::new(None),
            last: Mutex::new(Instant::now()),
            stop: AtomicBool::new(false),
        });
        session.load();
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(key, session.clone());
        info!(gid, cid, "DASH 时移录制开始");
        spawn_poller(session.clone(), state.clone());
        self.ensure_janitor();
        Some(session)
    }

    /// 已录制片数；会话不存在时为 0。
    pub fn count(&self, gid: &str, cid: &str) -> usize {
        self.info(gid, cid).map(|info| info.segments).unwrap_or(0)
    }

    /// 面板状态。
    pub fn info(&self, gid: &str, cid: &str) -> Option<SessionInfo> {
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&key(gid, cid))
            .cloned()?;
        Some(session.info())
    }

    /// 处理 `/{gid}/{cid}/dash-timeshift/*`。
    ///
    /// 目标是「只要前缀命中就由本处理器答复」：未知分组、非 `dash` 组、
    /// 未开时移的频道都返回 `404`，不落回 302 分流。
    pub async fn handle(
        self: &Arc<Self>,
        state: &Arc<AppState>,
        gid: &str,
        cid: &str,
        rest: &str,
        query: &HashMap<String, String>,
    ) -> Response {
        let (track, file) = match rest.split_once('/') {
            Some((track, file)) => (track, file),
            None => (rest, ""),
        };
        if track.is_empty() || rest.ends_with('/') {
            return empty_not_found();
        }
        if file.is_empty() || PLAYLIST_NAMES.contains(&file) {
            let Some(seek) = query
                .get("playseek")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            else {
                return text_response(StatusCode::BAD_REQUEST, "missing playseek");
            };
            let offset = state.snapshot().server.timezone_offset_hours as i64;
            // 目标实测：格式不对 → `400 missing playseek`；格式对但区间为空或反向
            // → `404 no segments in range`。两者不能合并。
            let Some((start, end)) = parse_seek(seek, offset) else {
                return text_response(StatusCode::BAD_REQUEST, "missing playseek");
            };
            if end <= start {
                return text_response(StatusCode::NOT_FOUND, "no segments in range");
            }
            let Some(session) = self.ensure(state, gid, cid) else {
                return text_response(StatusCode::NOT_FOUND, "no segments in range");
            };
            // 首次请求时录制器可能还没跑完一轮；同步补一轮，避免误报。
            if session.info().segments == 0 {
                poll_once(&session, state).await;
            }
            return match session.playlist(track, start, end) {
                Playlist::Body(body) => playlist_response(body),
                Playlist::Empty => text_response(StatusCode::NOT_FOUND, "no segments in range"),
                Playlist::UnknownTrack => {
                    text_response(StatusCode::NOT_FOUND, "no segments in range")
                }
            };
        }
        let Some((stem, ext)) = file.rsplit_once('.') else {
            return text_response(StatusCode::NOT_FOUND, "segment not found");
        };
        let Some(session) = self.ensure(state, gid, cid) else {
            return text_response(StatusCode::NOT_FOUND, "segment not found");
        };
        match session.read(track, stem, &ext.to_ascii_lowercase()) {
            Some((bytes, content_type)) => segment_response(bytes, content_type),
            None => text_response(StatusCode::NOT_FOUND, "segment not found"),
        }
    }

    /// 启动入口：点亮后立即开始录制。
    pub fn warm(self: &Arc<Self>, state: &Arc<AppState>) {
        let config = state.snapshot();
        let entries: Vec<(String, String)> = config
            .groups
            .iter()
            .filter(|group| group.mode == "dash")
            .flat_map(|group| {
                group
                    .channels
                    .iter()
                    .filter(|channel| channel.dash_timeshift)
                    .map(|channel| (group.id.clone(), channel.id.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (gid, cid) in entries {
            self.ensure(state, &gid, &cid);
        }
    }

    fn ensure_janitor(self: &Arc<Self>) {
        if self.janitor.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let reaped: Vec<Arc<Session>> = {
                    let mut sessions = manager
                        .sessions
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let keys: Vec<String> = sessions
                        .iter()
                        .filter(|(_, session)| {
                            session.stop.load(Ordering::SeqCst) || session.idle_for() > IDLE
                        })
                        .map(|(key, _)| key.clone())
                        .collect();
                    keys.into_iter()
                        .filter_map(|key| sessions.remove(&key))
                        .collect()
                };
                for session in reaped {
                    session.shutdown();
                    session.persist();
                    info!(gid = %session.gid, cid = %session.cid, "DASH 时移会话回收");
                }
            }
        });
    }
}

/// 后台录制循环。
fn spawn_poller(session: Arc<Session>, state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            if session.stop.load(Ordering::SeqCst) || session.idle_for() > IDLE {
                break;
            }
            poll_once(&session, &state).await;
            let interval = settings(&state, &session.gid, &session.cid)
                .map(|_| 2.0)
                .unwrap_or(FALLBACK_INTERVAL);
            tokio::time::sleep(Duration::from_secs_f64(interval)).await;
        }
        session.shutdown();
        session.persist();
    });
}

/// 抓一次 MPD 并补录新切片。
async fn poll_once(session: &Arc<Session>, state: &Arc<AppState>) {
    let Some(current) = settings(state, &session.gid, &session.cid) else {
        return;
    };
    session.set_limits(current.hours, current.max_gb);
    session.touch();
    match crate::http::routes::stream::fetch(state, &current.url, &session.gid, &session.cid).await
    {
        Ok(xml) => match dash::parse_presentation(&xml, &current.url) {
            Ok(presentation) => {
                session.set_error(None);
                record_presentation(session, state, &presentation).await;
                session.prune(current.hours, current.max_gb);
                session.persist();
            }
            Err(error) => session.set_error(Some(format!("MPD 解析失败: {error:#}"))),
        },
        Err(error) => session.set_error(Some(format!("上游连接失败: {error}"))),
    }
}

/// 把一轮 MPD 里的新切片写盘。
async fn record_presentation(
    session: &Arc<Session>,
    state: &AppState,
    presentation: &dash::Presentation,
) {
    for stream in &presentation.streams {
        if !stream.init.is_empty() && session.init_missing(&stream.name) {
            if let Ok(bytes) = crate::http::routes::stream::fetch_bytes(
                state,
                &stream.init[0],
                &session.gid,
                &session.cid,
            )
            .await
            {
                session.save_init(stream, &bytes);
            }
        }
        let seen = session.seen_for(&stream.name);
        let pending: Vec<&dash::Segment> = stream
            .segments
            .iter()
            .filter(|segment| !seen.contains(&segment.number) && !segment.urls.is_empty())
            .collect();
        let skip = pending.len().saturating_sub(MAX_NEW_PER_POLL);
        for segment in pending.into_iter().skip(skip) {
            match crate::http::routes::stream::fetch_bytes(
                state,
                &segment.urls[0],
                &session.gid,
                &session.cid,
            )
            .await
            {
                Ok(bytes) => {
                    session.record(stream, segment, &bytes);
                }
                Err(error) => session.set_error(Some(format!("切片抓取失败: {error}"))),
            }
        }
    }
}

fn playlist_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600, s-maxage=3600"),
    );
    response
}

fn segment_response(bytes: Vec<u8>, content_type: &'static str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    response
}

fn text_response(status: StatusCode, body: &'static str) -> Response {
    (
        status,
        [("content-type", "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}

fn empty_not_found() -> Response {
    (StatusCode::NOT_FOUND, "").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(dir: PathBuf) -> Arc<Session> {
        Arc::new(Session {
            gid: "g".into(),
            cid: "c".into(),
            dir,
            index: Mutex::new(Index::default()),
            seen: Mutex::new(HashMap::new()),
            bytes: Mutex::new(0),
            hours: Mutex::new(4),
            max_gb: Mutex::new(0),
            last_error: Mutex::new(None),
            last: Mutex::new(Instant::now()),
            stop: AtomicBool::new(false),
        })
    }

    fn stream() -> dash::Stream {
        dash::Stream {
            id: "v0".into(),
            name: "video_720p".into(),
            kind: "video".into(),
            codecs: Some("avc1.4d401f".into()),
            bandwidth: Some(800_000),
            lang: None,
            width: Some(1280),
            height: Some(720),
            frame_rate: None,
            audio_channels: None,
            init: vec!["http://up/init.mp4".into()],
            segments: Vec::new(),
        }
    }

    fn segment(number: u64) -> dash::Segment {
        dash::Segment {
            number,
            time: number * 2000,
            duration: 2.0,
            urls: vec![format!("http://up/{number}.m4s")],
        }
    }

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("dash-ts-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn playlist_uses_upstream_number_and_arrival_time() {
        let dir = temp_dir();
        let session = session(dir.clone());
        let stream = stream();
        session.save_init(&stream, b"init");
        assert!(session.record(&stream, &segment(7), b"aa"));
        assert!(session.record(&stream, &segment(8), b"bb"));
        let now = crate::schedule::now_epoch();
        let body = match session.playlist("video_720p", now - 60, now + 60) {
            Playlist::Body(body) => body,
            _ => panic!("应生成清单"),
        };
        assert!(body.starts_with("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(body.contains("#EXT-X-MEDIA-SEQUENCE:7"), "{body}");
        assert!(body.contains("#EXT-X-PROGRAM-DATE-TIME:"), "{body}");
        assert!(body.contains("#EXT-X-MAP:URI=\"/g/c/dash-timeshift/video_720p/init."));
        assert!(body.contains("/g/c/dash-timeshift/video_720p/7."), "{body}");
        assert!(body.contains("/g/c/dash-timeshift/video_720p/8."), "{body}");
        assert!(body.ends_with("#EXT-X-ENDLIST\n"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn playlist_reports_empty_and_unknown() {
        let dir = temp_dir();
        let session = session(dir.clone());
        session.record(&stream(), &segment(1), b"aa");
        let now = crate::schedule::now_epoch();
        assert!(matches!(
            session.playlist("video_720p", now + 3600, now + 7200),
            Playlist::Empty
        ));
        assert!(matches!(
            session.playlist("missing", now - 10, now + 10),
            Playlist::UnknownTrack
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn segment_name_is_upstream_number_and_ext_whitelisted() {
        let dir = temp_dir();
        let session = session(dir.clone());
        let stream = stream();
        session.save_init(&stream, b"init");
        session.record(&stream, &segment(9), b"nine");
        assert!(session.read("video_720p", "9", "png").is_some());
        assert!(session.read("video_720p", "9", "m4s").is_some());
        assert!(session.read("video_720p", "9", "mp4").is_none());
        assert!(session.read("video_720p", "09", "png").is_none());
        assert!(session.read("video_720p", "10", "png").is_none());
        assert!(session.read("video_720p", "init", "webp").is_some());
        assert!(session.read("video_720p", "init", "mp4").is_some());
        assert!(session.read("video_720p", "init", "m4s").is_none());
        assert!(session.read("video_720p", "../index", "json").is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_drops_old_entries() {
        let dir = temp_dir();
        let session = session(dir.clone());
        let stream = stream();
        session.record(&stream, &segment(1), b"aa");
        {
            let mut index = session.index.lock().unwrap();
            if let Some(track) = index.tracks.get_mut("video_720p") {
                if let Some(front) = track.segments.front_mut() {
                    front.ts = crate::schedule::now_epoch() - 7200;
                }
            }
        }
        session.prune(1, 0);
        assert_eq!(session.info().segments, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn index_round_trips() {
        let dir = temp_dir();
        let first = session(dir.clone());
        let stream = stream();
        first.save_init(&stream, b"init");
        first.record(&stream, &segment(42), b"aa");
        first.persist();
        let restored = session(dir.clone());
        restored.load();
        assert_eq!(restored.info().segments, 1);
        assert!(restored.seen_for("video_720p").contains(&42));
        assert!(restored.read("video_720p", "42", "jpg").is_some());
        assert!(restored.read("video_720p", "init", "mp4").is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn track_names_cannot_escape_the_directory() {
        assert_eq!(safe("../../etc"), "______etc");
        assert_eq!(safe("video_180p"), "video_180p");
    }

    #[test]
    fn extension_is_stable_per_segment() {
        assert_eq!(pick_ext("video_720p", 9), pick_ext("video_720p", 9));
        assert!(DEFAULT_EXTS.contains(&pick_ext("video_720p", 9).as_str()));
    }
}
