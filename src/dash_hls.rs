//! DASH → HLS：把 MPD 转成 HLS 主清单、媒体清单，并按需代理初始化段与切片。
//!
//! 证据：F-016。目标行为：
//! - `/{gid}/{cid}.m3u8` 返回 HLS master；媒体清单为 `/{gid}/{cid}/{track}/index.txt`；
//! - 轨道名 `video_{高}p` / `audio-{n}`；init 与切片伪装成图片扩展名；
//! - 支持 MPD 级 `<BaseURL>` 与 `SegmentTimeline`；
//! - 出口不含 `#EXT-X-KEY`（KID 只识别，不解密）。
//!
//! 本模块不调用 ffmpeg：MPD 解析后按 HLS 清单重写，切片字节原样代理。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
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
use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{
    dash::{self, Presentation, Stream},
    state::AppState,
};

/// 默认伪装扩展名。
const EXTS: &[&str] = &["jpg", "png", "gif", "bmp", "webp"];

/// 一个可代理的 DASH 文件。
#[derive(Debug, Clone)]
pub struct Entry {
    /// 上游候选地址；第一个为首选。
    pub urls: Vec<String>,
    /// 伪装扩展名。
    pub ext: String,
}

impl Entry {
    fn content_type(&self) -> &'static str {
        match self.ext.as_str() {
            "png" => "image/png",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            "webp" => "image/webp",
            "vtt" => "text/vtt; charset=utf-8",
            _ => "image/jpeg",
        }
    }
}

/// `/panel/api/dash/status` 的轨道摘要。目标字段见 F-016 §1。
#[derive(Debug, Clone, Serialize)]
pub struct TrackStatus {
    pub name: String,
    pub kind: String,
    pub lang: Option<String>,
    pub label: String,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub bandwidth: Option<u64>,
    pub codecs: Option<String>,
    pub frame_rate: Option<String>,
    pub audio_channels: Option<String>,
}

fn track_status(stream: &Stream) -> TrackStatus {
    let label = match stream.kind.as_str() {
        "video" => stream
            .height
            .map(|height| format!("{height}p"))
            .unwrap_or_else(|| stream.name.clone()),
        "audio" => audio_label(stream),
        "text" => stream.lang.clone().unwrap_or_else(|| stream.name.clone()),
        _ => stream.name.clone(),
    };
    TrackStatus {
        name: stream.name.clone(),
        kind: stream.kind.clone(),
        lang: stream.lang.clone(),
        label,
        width: stream.width,
        height: stream.height,
        bandwidth: stream.bandwidth,
        codecs: stream.codecs.clone(),
        frame_rate: stream.frame_rate.clone(),
        audio_channels: stream.audio_channels.clone(),
    }
}

fn track_statuses(presentation: &Presentation) -> Vec<TrackStatus> {
    presentation.streams.iter().map(track_status).collect()
}

fn latest_segment(presentation: &Presentation) -> u64 {
    presentation
        .streams
        .iter()
        .filter_map(|stream| stream.segments.last().map(|segment| segment.number))
        .max()
        .unwrap_or(0)
}

fn audio_label(stream: &Stream) -> String {
    match stream.audio_channels.as_deref() {
        Some("1") => format!("{} (Mono)", stream.name),
        Some("2") => format!("{} (Stereo)", stream.name),
        Some(channels) => format!("{} ({}ch)", stream.name, channels),
        None => stream.name.clone(),
    }
}

#[derive(Debug)]
struct Session {
    upstream: String,
    keep_seconds: u64,
    pool_size: u32,
    entries: HashMap<String, Entry>,
    tracks: Vec<TrackStatus>,
    ready: bool,
    segment_count: usize,
    started: Instant,
    /// 最近一次客户端请求时间。
    last: Instant,
    /// 最近一次发现新切片的时间；用于看门狗判定停更。
    progress: Instant,
    /// 已见最大切片编号。
    latest: u64,
    /// 当前连续重建次数。
    rebuilds: u32,
    /// 冷却截止时间。
    cooling_until: Option<Instant>,
    /// 各视频轨最近成功的上游目录前缀与时间；只在池启用时写入。
    video_ok: HashMap<String, (String, Instant)>,
}

/// DASH 会话管理器。
#[derive(Default)]
pub struct Manager {
    sessions: Mutex<HashMap<String, Session>>,
    janitor: AtomicBool,
}

impl Manager {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(gid: &str, cid: &str) -> String {
        format!("{gid}/{cid}")
    }

    fn settings(state: &AppState, gid: &str, cid: &str) -> Option<(String, u64, u32)> {
        let config = state.snapshot();
        let group = config.group(gid)?;
        let channel = group.channels.iter().find(|channel| channel.id == cid)?;
        Some((
            channel.url.clone(),
            group.mpd_keep_seconds,
            channel.url_pool_size.max(1),
        ))
    }

    fn upsert(&self, gid: &str, cid: &str, upstream: String, keep: u64, pool: u32) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .entry(Self::key(gid, cid))
            .and_modify(|session| {
                session.upstream = upstream.clone();
                session.keep_seconds = keep;
                session.pool_size = pool;
                session.last = Instant::now();
            })
            .or_insert_with(|| Session {
                upstream,
                keep_seconds: keep,
                pool_size: pool,
                entries: HashMap::new(),
                tracks: Vec::new(),
                ready: false,
                segment_count: 0,
                started: Instant::now(),
                last: Instant::now(),
                progress: Instant::now(),
                latest: 0,
                rebuilds: 0,
                cooling_until: None,
                video_ok: HashMap::new(),
            });
    }

    fn touch(&self, gid: &str, cid: &str) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(session) = sessions.get_mut(&Self::key(gid, cid)) {
            session.last = Instant::now();
        }
    }

    fn set_entries(&self, gid: &str, cid: &str, entries: Vec<(String, Entry)>) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let session = sessions
            .entry(Self::key(gid, cid))
            .or_insert_with(|| Session {
                upstream: String::new(),
                keep_seconds: 0,
                pool_size: 1,
                entries: HashMap::new(),
                tracks: Vec::new(),
                ready: false,
                segment_count: 0,
                started: Instant::now(),
                last: Instant::now(),
                progress: Instant::now(),
                latest: 0,
                rebuilds: 0,
                cooling_until: None,
                video_ok: HashMap::new(),
            });
        session.entries.extend(entries);
        session.last = Instant::now();
    }

    fn set_ready(&self, gid: &str, cid: &str, segment_count: usize, latest: u64) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(session) = sessions.get_mut(&Self::key(gid, cid)) {
            session.ready = true;
            session.segment_count = segment_count;
            session.last = Instant::now();
            if latest > session.latest {
                session.latest = latest;
                session.progress = Instant::now();
                session.rebuilds = 0;
                session.cooling_until = None;
            }
        }
    }

    fn set_tracks(&self, gid: &str, cid: &str, tracks: Vec<TrackStatus>) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(session) = sessions.get_mut(&Self::key(gid, cid)) {
            session.tracks = tracks;
            session.last = Instant::now();
        }
    }

    /// `/panel/api/dash/status` 摘要。目标形状见 F-016 §1。
    pub fn status(&self) -> Vec<serde_json::Value> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .iter()
            .map(|(key, session)| {
                let mut parts = key.splitn(2, '/');
                let gid = parts.next().unwrap_or("");
                let cid = parts.next().unwrap_or("");
                serde_json::json!({
                    "stream_key": format!("{gid}_{cid}"),
                    "source_url": session.upstream,
                    "phase": if session.ready { "ready" } else { "starting" },
                    "segment_count": session.segment_count,
                    "last_error": serde_json::Value::Null,
                    "checked_ms": 0,
                    "uptime_secs": session.started.elapsed().as_secs(),
                    "active_tracks": session.tracks,
                    "group_id": gid,
                    "channel_id": cid,
                })
            })
            .collect()
    }

    fn upstream(&self, gid: &str, cid: &str) -> Option<String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .get(&Self::key(gid, cid))
            .map(|session| session.upstream.clone())
            .filter(|upstream| !upstream.is_empty())
    }

    fn entry(&self, gid: &str, cid: &str, path: &str) -> Option<Entry> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .get(&Self::key(gid, cid))
            .and_then(|session| session.entries.get(path).cloned())
    }

    /// 读取一条视频轨的粘性前缀。过期或不存在时返回 `None`。
    fn sticky_prefix(&self, gid: &str, cid: &str, track: &str) -> Option<String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let (prefix, at) = sessions.get(&Self::key(gid, cid))?.video_ok.get(track)?;
        sticky_fresh(*at, Instant::now()).then(|| prefix.clone())
    }

    /// 记录一条视频轨最近成功的上游前缀。
    fn remember_prefix(&self, gid: &str, cid: &str, track: &str, prefix: &str) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let Some(session) = sessions.get_mut(&Self::key(gid, cid)) else {
            return;
        };
        if !session.video_ok.contains_key(track) && session.video_ok.len() >= VIDEO_STICKY_MAX {
            session.video_ok.clear();
        }
        session
            .video_ok
            .insert(track.to_string(), (prefix.to_string(), Instant::now()));
    }

    fn ensure_maintenance(self: &Arc<Self>, state: &Arc<AppState>) {
        if self.janitor.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = self.clone();
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                let dash = state.snapshot().cache.dash.clone();
                let tick = dash.watchdog_tick_secs.clamp(1, 3_600);
                tokio::time::sleep(Duration::from_secs(tick)).await;

                let idle = Duration::from_secs(dash.ondemand_idle_secs.clamp(5, 86_400));
                let stall = Duration::from_secs(dash.watchdog_stall_secs.clamp(1, 86_400));
                let active = Duration::from_secs(tick.saturating_mul(3).max(3));
                let mut due = Vec::new();
                {
                    let now = Instant::now();
                    let mut sessions = manager.sessions.lock().unwrap_or_else(|e| e.into_inner());
                    sessions.retain(|_, session| session.last.elapsed() <= idle);
                    for (key, session) in sessions.iter_mut() {
                        if let Some(until) = session.cooling_until {
                            if until > now {
                                continue;
                            }
                            session.cooling_until = None;
                            session.rebuilds = 0;
                            session.progress = now;
                        }
                        // 只检查仍有客户端请求的会话，避免空闲会话被反复重建。
                        if dash.watchdog_stall_secs == 0
                            || session.last.elapsed() > active
                            || session.progress.elapsed() < stall
                        {
                            continue;
                        }
                        due.push(key.clone());
                    }
                }

                let stagger = Duration::from_millis(dash.watchdog_rebuild_stagger_ms.min(60_000));
                let max = dash.watchdog_rebuild_max;
                let cooldown = Duration::from_secs(dash.watchdog_cooldown_secs);
                for (index, key) in due.iter().enumerate() {
                    if index > 0 && !stagger.is_zero() {
                        tokio::time::sleep(stagger).await;
                    }
                    let (gid, cid) = key.split_once('/').unwrap_or((key.as_str(), ""));
                    let stalled = {
                        let now = Instant::now();
                        let mut sessions =
                            manager.sessions.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(session) = sessions.get_mut(key) else {
                            continue;
                        };
                        if session.cooling_until.is_some()
                            || session.progress.elapsed() < stall
                            || session.last.elapsed() > active
                        {
                            continue;
                        }
                        let stalled_secs = session.progress.elapsed().as_secs();
                        session.rebuilds = session.rebuilds.saturating_add(1);
                        session.entries.clear();
                        // 重建后从 pool[0] 重新探测，不复用旧的成功地址。
                        session.video_ok.clear();
                        session.ready = false;
                        session.segment_count = 0;
                        session.progress = now;
                        session.last = now;
                        if max > 0 && session.rebuilds >= max && !cooldown.is_zero() {
                            session.cooling_until = Some(now + cooldown);
                        }
                        (session.rebuilds, stalled_secs)
                    };
                    if stalled.0 >= max && max > 0 && !cooldown.is_zero() {
                        warn!(
                            gid,
                            cid,
                            attempt = stalled.0,
                            stalled_secs = stalled.1,
                            cooldown_secs = cooldown.as_secs(),
                            "DASH 看门狗连续重建仍未恢复，进入冷却"
                        );
                    } else {
                        warn!(
                            gid,
                            cid,
                            attempt = stalled.0,
                            stalled_secs = stalled.1,
                            "DASH 看门狗销毁重建"
                        );
                    }
                }
            }
        });
    }

    /// 生成 HLS 主清单，并登记会话。
    pub async fn master(
        self: &Arc<Self>,
        state: &Arc<AppState>,
        gid: &str,
        cid: &str,
        upstream: &str,
        self_base: &str,
    ) -> Result<String, String> {
        let (_, keep, pool) =
            Self::settings(state, gid, cid).unwrap_or_else(|| (upstream.to_string(), 0, 1));
        let dash = state.snapshot().cache.dash.clone();
        let keep = effective_keep(keep, dash.playlist_min_secs);
        let xml = fetch_mpd(state, gid, cid, upstream, &dash).await?;
        let presentation = dash::parse_presentation(&xml, upstream)
            .map_err(|error| format!("MPD 解析失败: {error:#}"))?;
        let body = render_master(&presentation, gid, cid, self_base);
        self.upsert(gid, cid, upstream.to_string(), keep, pool);
        self.set_tracks(gid, cid, track_statuses(&presentation));
        self.set_ready(
            gid,
            cid,
            presentation
                .streams
                .iter()
                .map(|stream| stream.segments.len())
                .max()
                .unwrap_or(0),
            latest_segment(&presentation),
        );
        self.ensure_maintenance(state);
        info!(
            gid,
            cid,
            streams = presentation.streams.len(),
            "DASH 主清单就绪"
        );
        Ok(body)
    }

    /// 处理 DASH 子路径：媒体清单、init、切片。
    ///
    /// `rest` 形如 `video_720p/index.txt`、`video_720p/init.webp`。
    pub async fn serve(
        self: &Arc<Self>,
        state: &Arc<AppState>,
        gid: &str,
        cid: &str,
        rest: &str,
        self_base: &str,
    ) -> Option<Response> {
        let (configured, keep, pool) = Self::settings(state, gid, cid)?;
        let segment_cache = format!(
            "public, max-age={}",
            state.snapshot().cache.ttl.segment_secs.max(1)
        );
        let upstream = self.upstream(gid, cid).unwrap_or(configured);
        let (track, file) = rest.split_once('/')?;
        if track.is_empty() || file.is_empty() {
            return None;
        }
        if file == "index.txt" || file == "index.m3u8" {
            let dash = state.snapshot().cache.dash.clone();
            let xml = match fetch_mpd(state, gid, cid, &upstream, &dash).await {
                Ok(xml) => xml,
                Err(error) => {
                    warn!(gid, cid, error = %error, "DASH 媒体清单抓取失败");
                    return Some((StatusCode::BAD_GATEWAY, "Upstream Error").into_response());
                }
            };
            let presentation = match dash::parse_presentation(&xml, &upstream) {
                Ok(value) => value,
                Err(error) => {
                    warn!(gid, cid, error = %error, "DASH MPD 解析失败");
                    return Some((StatusCode::BAD_GATEWAY, "Upstream Error").into_response());
                }
            };
            let Some(stream) = presentation
                .streams
                .iter()
                .find(|stream| stream.name == track)
            else {
                return Some((StatusCode::NOT_FOUND, "Segment not found").into_response());
            };
            let formats = disguise_formats(state);
            let window = effective_keep(keep, dash.playlist_min_secs);
            let start_offset = if presentation.is_live {
                dash.start_time_offset_secs.max(0.0)
            } else {
                0.0
            };
            let rendered = render_media(
                &presentation,
                stream,
                MediaOptions {
                    gid,
                    cid,
                    self_base,
                    keep_seconds: window,
                    pool_size: pool,
                    formats: &formats,
                    track_index: presentation
                        .streams
                        .iter()
                        .position(|candidate| candidate.name == track)
                        .unwrap_or(0),
                    start_offset,
                    publish_delay_secs: dash.publish_delay_secs,
                    fetch_extra_segments: dash.fetch_extra_segments,
                },
            );
            self.upsert(gid, cid, upstream, keep, pool);
            self.set_tracks(gid, cid, track_statuses(&presentation));
            self.set_ready(gid, cid, rendered.segment_count, rendered.latest_number);
            self.set_entries(gid, cid, rendered.entries);
            self.touch(gid, cid);
            let mut response = Response::new(Body::from(rendered.body));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
            let cache = if presentation.is_live {
                "public, max-age=2, s-maxage=2"
            } else {
                "public, max-age=5, s-maxage=5"
            };
            insert_header(&mut response, header::CACHE_CONTROL, cache);
            return Some(response);
        }

        // init / 媒体切片：先查登记表；缺失时重取 MPD 再登记一次。
        let mut entry = self.entry(gid, cid, rest);
        if entry.is_none() {
            if let Ok(xml) = crate::http::routes::stream::fetch(state, &upstream, gid, cid).await {
                if let Ok(presentation) = dash::parse_presentation(&xml, &upstream) {
                    let dash = state.snapshot().cache.dash.clone();
                    let formats = disguise_formats(state);
                    let window = effective_keep(keep, dash.playlist_min_secs);
                    let start_offset = if presentation.is_live {
                        dash.start_time_offset_secs.max(0.0)
                    } else {
                        0.0
                    };
                    let mut segment_count = 0;
                    for (index, stream) in presentation.streams.iter().enumerate() {
                        let rendered = render_media(
                            &presentation,
                            stream,
                            MediaOptions {
                                gid,
                                cid,
                                self_base,
                                keep_seconds: window,
                                pool_size: pool,
                                formats: &formats,
                                track_index: index,
                                start_offset,
                                publish_delay_secs: dash.publish_delay_secs,
                                fetch_extra_segments: dash.fetch_extra_segments,
                            },
                        );
                        segment_count = segment_count.max(rendered.segment_count);
                        self.set_entries(gid, cid, rendered.entries);
                    }
                    self.set_ready(gid, cid, segment_count, latest_segment(&presentation));
                }
            }
            entry = self.entry(gid, cid, rest);
        }
        let Some(entry) = entry else {
            return Some((StatusCode::NOT_FOUND, "Segment not found").into_response());
        };
        let path = cache_path(&state.cache_dir, gid, cid, rest);
        if let Ok(bytes) = std::fs::read(&path) {
            self.touch(gid, cid);
            return Some(bytes_response(bytes, &entry, &segment_cache));
        }
        let mut last_error = None;
        let dash = state.snapshot().cache.dash.clone();
        let timeout = Duration::from_secs(dash.segment_timeout_secs.clamp(1, 120));
        // 池启用时首轮用更短的超时。黑洞型上游不报错，只会挂满 8 秒。
        let probe_timeout = Duration::from_secs(dash.segment_timeout_secs.clamp(1, 3));
        let pool_on = pool > 1 && entry.urls.len() > 1;
        // 配置热重载后，已登记的 entry 可能带着旧池大小的地址表。按当前池再截一次。
        let cap = if pool_on {
            entry.urls.len().min(pool as usize).min(POOL_MAX)
        } else {
            entry.urls.len()
        };
        let mut order: Vec<String> = entry.urls[..cap].to_vec();
        let video = track.starts_with("video");
        if pool_on && video {
            if let Some(prefix) = self.sticky_prefix(gid, cid, track) {
                if let Some(index) = order.iter().position(|url| base_prefix(url) == prefix) {
                    order[..=index].rotate_right(1);
                }
            }
        }
        for plan in attempt_plan(order.len(), pool_on, dash.retry_max) {
            let url = &order[plan.url];
            let limit = if pool_on && plan.round == 0 {
                probe_timeout
            } else {
                timeout
            };
            match tokio::time::timeout(
                limit,
                crate::http::routes::stream::fetch_bytes(state, url, gid, cid),
            )
            .await
            {
                Ok(Ok(bytes)) => {
                    if pool_on && video {
                        self.remember_prefix(gid, cid, track, base_prefix(url));
                    }
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&path, &bytes);
                    self.touch(gid, cid);
                    return Some(bytes_response(bytes, &entry, &segment_cache));
                }
                Ok(Err(error)) => {
                    last_error = Some(error.to_string());
                }
                Err(_) => {
                    last_error = Some("segment timeout".to_string());
                }
            }
            if let Some(round) = plan.sleep {
                tokio::time::sleep(retry_delay(&dash, round as u32)).await;
            }
        }
        warn!(
            gid,
            cid,
            path = rest,
            error = last_error.as_deref().unwrap_or("no url"),
            "DASH 切片抓取失败"
        );
        state.stats.record_upstream_failure();
        Some((StatusCode::BAD_GATEWAY, "Upstream Error").into_response())
    }
}

fn disguise_formats(state: &AppState) -> Vec<String> {
    let config = state.snapshot();
    if config.cache.disguise.enabled {
        let formats = &config.cache.disguise.formats;
        if !formats.is_empty() {
            return formats.clone();
        }
    }
    EXTS.iter().map(|ext| ext.to_string()).collect()
}

struct RenderedMedia {
    body: String,
    entries: Vec<(String, Entry)>,
    segment_count: usize,
    latest_number: u64,
}

/// 生成 HLS master。
pub fn render_master(presentation: &Presentation, gid: &str, cid: &str, self_base: &str) -> String {
    let audio: Vec<&Stream> = presentation
        .streams
        .iter()
        .filter(|stream| stream.kind == "audio")
        .collect();
    let text: Vec<&Stream> = presentation
        .streams
        .iter()
        .filter(|stream| stream.kind == "text")
        .collect();
    let mut body = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n");
    for (index, stream) in audio.iter().enumerate() {
        body.push_str(&format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",LANGUAGE=\"{}\",NAME=\"{}\",DEFAULT={},AUTOSELECT=YES",
            escape(stream.lang.as_deref().unwrap_or("und")),
            escape(&audio_label(stream)),
            if index == 0 { "YES" } else { "NO" },
        ));
        if let Some(channels) = channels(stream) {
            body.push_str(&format!(",CHANNELS=\"{channels}\""));
        }
        body.push_str(&format!(
            ",URI=\"{}\"\n",
            media_url(self_base, gid, cid, &stream.name)
        ));
    }
    for (index, stream) in text.iter().enumerate() {
        body.push_str(&format!(
            "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",LANGUAGE=\"{}\",NAME=\"{}\",DEFAULT={},AUTOSELECT=YES,URI=\"{}\"\n",
            escape(stream.lang.as_deref().unwrap_or("und")),
            escape(&audio_label(stream)),
            if index == 0 { "YES" } else { "NO" },
            media_url(self_base, gid, cid, &stream.name)
        ));
    }

    let variants: Vec<&Stream> = if presentation
        .streams
        .iter()
        .any(|stream| stream.kind == "video")
    {
        presentation
            .streams
            .iter()
            .filter(|stream| stream.kind == "video")
            .collect()
    } else {
        presentation
            .streams
            .iter()
            .filter(|stream| stream.kind != "text")
            .take(1)
            .collect()
    };
    for stream in variants {
        let mut attrs = vec![format!(
            "BANDWIDTH={}",
            stream.bandwidth.unwrap_or(1).max(1)
        )];
        if let Some(codecs) = codecs(stream, audio.first().copied()) {
            attrs.push(format!("CODECS=\"{}\"", escape(&codecs)));
        }
        if let (Some(width), Some(height)) = (stream.width, stream.height) {
            attrs.push(format!("RESOLUTION={width}x{height}"));
        }
        if let Some(frame_rate) = &stream.frame_rate {
            if let Some(value) = frame_rate_number(frame_rate) {
                attrs.push(format!("FRAME-RATE={value}"));
            }
        }
        if !audio.is_empty() {
            attrs.push("AUDIO=\"audio\"".to_string());
        }
        if !text.is_empty() {
            attrs.push("SUBTITLES=\"subs\"".to_string());
        }
        body.push_str(&format!(
            "#EXT-X-STREAM-INF:{}\n{}\n",
            attrs.join(","),
            media_url(self_base, gid, cid, &stream.name)
        ));
    }
    body
}

/// 媒体清单渲染参数。
struct MediaOptions<'a> {
    gid: &'a str,
    cid: &'a str,
    self_base: &'a str,
    keep_seconds: u64,
    pool_size: u32,
    formats: &'a [String],
    track_index: usize,
    start_offset: f64,
    publish_delay_secs: f64,
    fetch_extra_segments: u64,
}

/// 生成媒体清单并登记 init/切片。
fn render_media(
    presentation: &Presentation,
    stream: &Stream,
    options: MediaOptions<'_>,
) -> RenderedMedia {
    let MediaOptions {
        gid,
        cid,
        self_base,
        keep_seconds,
        pool_size,
        formats,
        track_index,
        start_offset,
        publish_delay_secs,
        fetch_extra_segments,
    } = options;
    let mut picked: Vec<&dash::Segment> = stream.segments.iter().collect();
    if presentation.is_live {
        // 目标行为：publish_delay_secs > 0 时按该延迟压后清单末端；
        // 为 0 时使用 fetch_extra_segments 段作为备用缓冲。
        let withhold = publish_withhold(&picked, publish_delay_secs, fetch_extra_segments);
        picked.truncate(picked.len().saturating_sub(withhold));
    }
    if keep_seconds > 0 {
        let mut total = 0.0;
        let mut start = picked.len();
        for (index, segment) in picked.iter().enumerate().rev() {
            total += segment.duration.max(0.001);
            start = index;
            if total >= keep_seconds as f64 {
                break;
            }
        }
        picked = picked.split_off(start);
    }
    if picked.is_empty() {
        picked = stream.segments.iter().rev().take(1).collect();
        picked.reverse();
    }
    let target = picked
        .iter()
        .map(|segment| segment.duration.ceil() as u64)
        .max()
        .unwrap_or(1)
        .max(1);
    let first = picked.first().map(|segment| segment.number).unwrap_or(0);
    let is_live = presentation.is_live;
    let mut body = format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:{first}\n#EXT-X-DISCONTINUITY-SEQUENCE:0\n"
    );
    if is_live && start_offset > 0.0 {
        body.push_str(&format!("#EXT-X-START:TIME-OFFSET=-{start_offset:.3}\n"));
    }
    body.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
    let mut entries = Vec::new();
    let init_ext = pick_ext(formats, track_index);
    if !stream.init.is_empty() {
        let init_name = format!("init.{init_ext}");
        let path = format!("{}/{init_name}", stream.name);
        entries.push((
            path.clone(),
            Entry {
                urls: choose_urls(&stream.init, &stream.kind, 0, pool_size),
                ext: init_ext.clone(),
            },
        ));
        body.push_str(&format!(
            "#EXT-X-MAP:URI=\"{}\"\n",
            media_file_url(self_base, gid, cid, &path)
        ));
    }
    for (index, segment) in picked.iter().enumerate() {
        if segment.urls.is_empty() {
            continue;
        }
        let ext = pick_ext(formats, track_index + index + 1);
        let name = format!(
            "{}_{}.{}",
            segment.number,
            short_hash(&segment.urls[0]),
            ext
        );
        let path = format!("{}/{name}", stream.name);
        entries.push((
            path.clone(),
            Entry {
                urls: choose_urls(&segment.urls, &stream.kind, index, pool_size),
                ext,
            },
        ));
        body.push_str(&format!(
            "#EXTINF:{:.6},\n{}\n",
            segment.duration.max(0.001),
            media_file_url(self_base, gid, cid, &path)
        ));
    }
    if !is_live {
        body.push_str("#EXT-X-ENDLIST\n");
    }
    RenderedMedia {
        body,
        entries,
        segment_count: picked.len(),
        latest_number: picked.last().map(|segment| segment.number).unwrap_or(0),
    }
}

/// 池成员上限。
///
/// 目标面板的池大小输入范围是 2–6（F-015）。配置值不校验，这里再钳一次，
/// 防止异常 MPD 的 `<BaseURL>` 数量把单次请求的尝试次数拉高。
const POOL_MAX: usize = 6;

/// 生成一个切片的候选地址顺序。
///
/// - `urls.len() <= 1` 或 `pool_size <= 1`：池关闭，原样返回。这是兼容路径。
/// - 否则取前 `len = min(地址数, pool_size, POOL_MAX)` 条作为池。
///   视频固定从 `pool[0]` 起步；音频/字幕从 `pool[1..len]` 轮询，`pool[0]` 最后兜底。
fn choose_urls(urls: &[String], kind: &str, index: usize, pool_size: u32) -> Vec<String> {
    if urls.len() <= 1 || pool_size <= 1 {
        return urls.to_vec();
    }
    // 池大小是成员上限，不是"额外轮询数"。第 len 条及之后的地址不参与回退。
    // 进入本分支后 urls.len() >= 2 且 pool_size >= 2，所以 len >= 2。
    let len = urls.len().min(pool_size as usize).min(POOL_MAX);
    // 目标面板自述：视频固定 pool[0]，音频/字幕在 pool[1..] 轮询。
    let start = if kind == "video" {
        0
    } else {
        1 + index % (len - 1)
    };
    let mut out = Vec::with_capacity(len);
    for offset in 0..len {
        out.push(urls[(start + offset) % len].clone());
    }
    out
}

/// 一次抓取尝试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Attempt {
    /// 用第几个候选地址。
    url: usize,
    /// 第几轮。
    round: usize,
    /// 本次之后是否退避，值为退避序号。
    sleep: Option<usize>,
}

/// 生成一次抓取的尝试计划。
///
/// - 池关闭：与旧循环逐项等价。每个地址连续用满 `retry_max+1` 次，
///   最后一次不退避，地址之间不退避。
/// - 池启用：按成员轮询。首轮每个成员各试一次，失败立即换址，轮末才退避。
///   总尝试数 = `max(retry_max+1, 成员数)`，不随成员数相乘膨胀。
fn attempt_plan(url_count: usize, pooled: bool, retry_max: u32) -> Vec<Attempt> {
    if url_count == 0 {
        return Vec::new();
    }
    let tries = retry_max.min(32) as usize + 1;
    if !pooled {
        let mut plan = Vec::with_capacity(url_count * tries);
        for url in 0..url_count {
            for attempt in 0..tries {
                plan.push(Attempt {
                    url,
                    round: attempt,
                    sleep: (attempt + 1 < tries).then_some(attempt),
                });
            }
        }
        return plan;
    }
    let budget = tries.max(url_count);
    (0..budget)
        .map(|step| Attempt {
            url: step % url_count,
            round: step / url_count,
            sleep: (step + 1 < budget && (step + 1) % url_count == 0).then_some(step / url_count),
        })
        .collect()
}

/// 取 URL 的目录前缀：去掉最后一段。
///
/// 用目录前缀判断"同一个上游"。切片名每个都不同，直接比整条 URL 永远不相等。
fn base_prefix(url: &str) -> &str {
    match url.rfind('/') {
        Some(index) => &url[..=index],
        None => url,
    }
}

/// 视频粘性有效期。超时后回到常规轮询顺序。
const VIDEO_STICKY_TTL: Duration = Duration::from_secs(300);

/// 粘性记录上限，防止异常 MPD 让会话状态无界增长。
const VIDEO_STICKY_MAX: usize = 16;

fn sticky_fresh(at: Instant, now: Instant) -> bool {
    match now.checked_duration_since(at) {
        Some(age) => age <= VIDEO_STICKY_TTL,
        None => true,
    }
}

fn pick_ext(formats: &[String], index: usize) -> String {
    if formats.is_empty() {
        return "jpg".to_string();
    }
    formats[index % formats.len()]
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

fn short_hash(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    hex::encode(&digest[..8])
}

fn media_url(self_base: &str, gid: &str, cid: &str, track: &str) -> String {
    media_file_url(self_base, gid, cid, &format!("{track}/index.txt"))
}

fn media_file_url(self_base: &str, gid: &str, cid: &str, rest: &str) -> String {
    let base = self_base.trim_end_matches('/');
    if base.is_empty() {
        format!("/{gid}/{cid}/{rest}")
    } else {
        format!("{base}/{gid}/{cid}/{rest}")
    }
}

fn codecs(stream: &Stream, audio: Option<&Stream>) -> Option<String> {
    let mut values = Vec::new();
    if let Some(codecs) = &stream.codecs {
        values.push(codecs.clone());
    }
    if let Some(codecs) = audio.and_then(|stream| stream.codecs.as_ref()) {
        if !values.contains(codecs) {
            values.push(codecs.clone());
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(values.join(","))
    }
}

fn channels(stream: &Stream) -> Option<&str> {
    let value = stream.audio_channels.as_deref()?;
    // `audioSamplingRate` 不是声道数；只在明显是 1/2/6/8 时输出。
    match value {
        "1" | "2" | "6" | "8" => Some(value),
        _ => None,
    }
}

fn frame_rate_number(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some((numerator, denominator)) = value.split_once('/') {
        let numerator: f64 = numerator.parse().ok()?;
        let denominator: f64 = denominator.parse().ok()?;
        if denominator == 0.0 {
            return None;
        }
        Some(format!("{:.3}", numerator / denominator))
    } else {
        let value: f64 = value.parse().ok()?;
        Some(format!("{value:.3}"))
    }
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn cache_path(root: &Path, gid: &str, cid: &str, rest: &str) -> PathBuf {
    root.join("dash")
        .join(safe(gid))
        .join(safe(cid))
        .join(safe_path(rest))
}

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
    if mapped.is_empty() {
        "empty".to_string()
    } else {
        mapped
    }
}

fn safe_path(value: &str) -> String {
    value.split('/').map(safe).collect::<Vec<_>>().join("/")
}

fn bytes_response(bytes: Vec<u8>, entry: &Entry, cache: &str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    if let Ok(value) = HeaderValue::from_str(entry.content_type()) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    insert_header(&mut response, header::CACHE_CONTROL, cache);
    response
}

fn insert_header(response: &mut Response, name: header::HeaderName, value: &str) {
    if let Ok(parsed) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, parsed);
    }
}

/// 计算 DASH 重试等待时间。
fn retry_delay(config: &crate::config::model::CacheDashCfg, attempt: u32) -> Duration {
    let initial = config.retry_initial_ms as f64;
    let factor = config.retry_backoff.max(1.0);
    let max = config.retry_max_ms.min(60_000) as f64;
    let delay = (initial * factor.powi(attempt as i32)).min(max);
    Duration::from_millis(delay.max(config.min_request_interval_ms as f64) as u64)
}

/// 计算清单末端需压后的切片数。
///
/// 目标实测（4 秒切片）：
/// - `publish_delay_secs=0` → 压后 `fetch_extra_segments` 段；
/// - `publish_delay_secs=4/8/20/24/30` → 分别压后 1/2/5/6/8 段。
fn publish_withhold(
    segments: &[&dash::Segment],
    publish_delay_secs: f64,
    fetch_extra_segments: u64,
) -> usize {
    if segments.is_empty() {
        return 0;
    }
    if publish_delay_secs > 0.0 {
        let mut total = 0.0;
        let mut count = 0;
        for segment in segments.iter().rev() {
            total += segment.duration.max(0.001);
            count += 1;
            if total >= publish_delay_secs {
                break;
            }
        }
        count.min(segments.len())
    } else {
        (fetch_extra_segments as usize).min(segments.len())
    }
}

/// 计算 DASH 媒体清单窗口：分组 `mpd_keep_seconds` 优先，否则用 `playlist_min_secs`。
///
/// 目标实测：`playlist_min_secs=30`、4 秒切片 → 8 条；`=12` → 3 条。
fn effective_keep(group_keep: u64, playlist_min_secs: f64) -> u64 {
    if group_keep > 0 {
        group_keep
    } else if playlist_min_secs > 0.0 {
        playlist_min_secs.ceil() as u64
    } else {
        0
    }
}

/// 拉取 MPD，按 `[cache.dash]` 的超时与重试参数执行。
async fn fetch_mpd(
    state: &Arc<AppState>,
    gid: &str,
    cid: &str,
    url: &str,
    config: &crate::config::model::CacheDashCfg,
) -> Result<String, String> {
    let timeout = Duration::from_secs(config.mpd_timeout_secs.clamp(1, 120));
    let attempts = config.retry_max.min(32);
    let mut last_error = String::new();
    for attempt in 0..=attempts {
        match tokio::time::timeout(
            timeout,
            crate::http::routes::stream::fetch(state, url, gid, cid),
        )
        .await
        {
            Ok(Ok(body)) => return Ok(body),
            Ok(Err(error)) => last_error = error.to_string(),
            Err(_) => last_error = "MPD timeout".to_string(),
        }
        if attempt < attempts {
            tokio::time::sleep(retry_delay(config, attempt)).await;
        }
    }
    Err(format!("Upstream Error: {last_error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::{parse_presentation, Segment};

    fn presentation() -> Presentation {
        parse_presentation(
            r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT8S">
  <BaseURL>http://origin.example/dash/</BaseURL>
  <Period>
    <AdaptationSet contentType="video" maxWidth="1280" maxHeight="720">
      <SegmentTemplate timescale="1000" initialization="init-$RepresentationID$.mp4"
        media="chunk-$RepresentationID$-$Number%03d$.m4s" startNumber="1">
        <SegmentTimeline><S d="2000" r="3"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" codecs="avc1.4d401f" bandwidth="2500000" width="1280" height="720"/>
    </AdaptationSet>
    <AdaptationSet contentType="audio" lang="zh">
      <SegmentTemplate timescale="1000" initialization="a-init.mp4"
        media="a-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="2000" r="3"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="a0" codecs="mp4a.40.2" bandwidth="128000"/>
    </AdaptationSet>
  </Period>
</MPD>"#,
            "http://origin.example/master.mpd",
        )
        .expect("MPD")
    }

    #[test]
    fn master_has_audio_and_video() {
        let body = render_master(&presentation(), "g", "c", "http://self");
        assert!(body.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
        assert!(body.contains("video_720p/index.txt"));
        assert!(body.contains("audio-0/index.txt"));
        assert!(body.contains("RESOLUTION=1280x720"));
    }

    #[test]
    fn media_rewrites_segments_and_honors_keep_window() {
        let p = presentation();
        let stream = p.streams.iter().find(|s| s.kind == "video").unwrap();
        let rendered = render_media(
            &p,
            stream,
            MediaOptions {
                gid: "g",
                cid: "c",
                self_base: "http://self",
                keep_seconds: 4,
                pool_size: 1,
                formats: &["webp".to_string()],
                track_index: 0,
                start_offset: 0.0,
                publish_delay_secs: 0.0,
                fetch_extra_segments: 0,
            },
        );
        assert!(rendered
            .body
            .contains("#EXT-X-MAP:URI=\"http://self/g/c/video_720p/init.webp\""));
        assert!(rendered.body.contains("#EXT-X-ENDLIST"));
        // 4 秒窗口只保留最后两段。
        assert_eq!(rendered.entries.len(), 1 + 2);
    }

    #[test]
    fn render_media_caps_entry_urls() {
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S">
  <BaseURL>http://cdn-a.example/dash/</BaseURL>
  <BaseURL>http://cdn-b.example/dash/</BaseURL>
  <BaseURL>http://cdn-c.example/dash/</BaseURL>
  <Period>
    <AdaptationSet contentType="video">
      <SegmentTemplate timescale="1000" initialization="init.mp4" media="v-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="2000" r="1"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let presentation = parse_presentation(xml, "http://origin.example/live.mpd").expect("MPD");
        let stream = &presentation.streams[0];
        let render = |pool: u32| {
            render_media(
                &presentation,
                stream,
                MediaOptions {
                    gid: "g",
                    cid: "c",
                    self_base: "http://self",
                    keep_seconds: 0,
                    pool_size: pool,
                    formats: &["jpg".to_string()],
                    track_index: 0,
                    start_offset: 0.0,
                    publish_delay_secs: 0.0,
                    fetch_extra_segments: 0,
                },
            )
        };
        // 池上限 2：init 与切片都只留两条候选，视频从 cdn-a 起步。
        let capped = render(2);
        assert!(capped
            .entries
            .iter()
            .all(|(_, entry)| entry.urls.len() == 2));
        assert!(capped.entries[0].1.urls[0].contains("cdn-a"));
        // 池关闭：三条候选全部保留，与改动前一致。
        let full = render(1);
        assert!(full.entries.iter().all(|(_, entry)| entry.urls.len() == 3));
        // 条目数量与清单正文不受池大小影响。
        assert_eq!(capped.entries.len(), full.entries.len());
        assert_eq!(capped.body.lines().count(), full.body.lines().count());
    }

    #[test]
    fn playlist_min_keeps_target_window() {
        let p = parse_presentation(
            r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic">
  <Period>
    <AdaptationSet contentType="video">
      <SegmentTemplate timescale="1000" initialization="init.mp4" media="s-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="4000" r="9"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>"#,
            "http://origin.example/live.mpd",
        )
        .expect("MPD");
        let stream = &p.streams[0];
        let rendered = render_media(
            &p,
            stream,
            MediaOptions {
                gid: "g",
                cid: "c",
                self_base: "",
                keep_seconds: 30,
                pool_size: 1,
                formats: &["jpg".to_string()],
                track_index: 0,
                start_offset: 0.0,
                publish_delay_secs: 0.0,
                fetch_extra_segments: 0,
            },
        );
        assert_eq!(rendered.segment_count, 8);
        assert_eq!(rendered.body.matches("#EXTINF:").count(), 8);
        assert!(rendered.body.contains("#EXT-X-MEDIA-SEQUENCE:3"));
    }

    #[test]
    fn publish_delay_withholds_live_edge() {
        let p = parse_presentation(
            r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic">
  <Period>
    <AdaptationSet contentType="video">
      <SegmentTemplate timescale="1000" initialization="init.mp4" media="s-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="4000" r="9"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>"#,
            "http://origin.example/live.mpd",
        )
        .expect("MPD");
        let stream = &p.streams[0];
        let make = |publish_delay_secs: f64, fetch_extra_segments: u64| {
            render_media(
                &p,
                stream,
                MediaOptions {
                    gid: "g",
                    cid: "c",
                    self_base: "",
                    keep_seconds: 12,
                    pool_size: 1,
                    formats: &["jpg".to_string()],
                    track_index: 0,
                    start_offset: 0.0,
                    publish_delay_secs,
                    fetch_extra_segments,
                },
            )
        };
        // 延迟 20s / 4s = 5 段；余下最后 12s 窗口 3 段。
        let delayed = make(20.0, 2);
        assert_eq!(delayed.segment_count, 3);
        assert_eq!(delayed.latest_number, 5);
        assert!(delayed.body.contains("#EXT-X-MEDIA-SEQUENCE:3"));
        // 延迟为 0 时压后 fetch_extra_segments 段。
        let buffered = make(0.0, 2);
        assert_eq!(buffered.segment_count, 3);
        assert_eq!(buffered.latest_number, 8);
        // 30s 延迟向上取整为 8 段。
        let ceil = make(30.0, 0);
        assert_eq!(ceil.latest_number, 2);
    }

    #[test]
    fn pool_rotates_audio_urls() {
        let urls = vec![
            "http://a/1".to_string(),
            "http://b/1".to_string(),
            "http://c/1".to_string(),
        ];
        // 未截断时与旧实现逐项相同：视频 [a,b,c]，音频从 pool[1] 起环展开。
        assert_eq!(
            choose_urls(&urls, "video", 1, 3),
            vec!["http://a/1", "http://b/1", "http://c/1"]
        );
        assert_eq!(
            choose_urls(&urls, "audio", 0, 3),
            vec!["http://b/1", "http://c/1", "http://a/1"]
        );
        assert_eq!(
            choose_urls(&urls, "audio", 1, 3),
            vec!["http://c/1", "http://a/1", "http://b/1"]
        );
    }

    #[test]
    fn pool_caps_to_size() {
        let urls = vec![
            "http://a/1".to_string(),
            "http://b/1".to_string(),
            "http://c/1".to_string(),
            "http://d/1".to_string(),
        ];
        // 池上限 2：只用前两条。
        assert_eq!(
            choose_urls(&urls, "video", 0, 2),
            vec!["http://a/1", "http://b/1"]
        );
        assert_eq!(
            choose_urls(&urls, "video", 7, 2),
            vec!["http://a/1", "http://b/1"]
        );
        assert_eq!(
            choose_urls(&urls, "audio", 0, 2),
            vec!["http://b/1", "http://a/1"]
        );
        assert_eq!(
            choose_urls(&urls, "audio", 3, 2),
            vec!["http://b/1", "http://a/1"]
        );
    }

    #[test]
    fn pool_caps_audio_ring() {
        let urls = vec![
            "http://a/1".to_string(),
            "http://b/1".to_string(),
            "http://c/1".to_string(),
            "http://d/1".to_string(),
            "http://e/1".to_string(),
        ];
        assert_eq!(
            choose_urls(&urls, "video", 0, 3),
            vec!["http://a/1", "http://b/1", "http://c/1"]
        );
        assert_eq!(
            choose_urls(&urls, "audio", 0, 3),
            vec!["http://b/1", "http://c/1", "http://a/1"]
        );
        assert_eq!(
            choose_urls(&urls, "audio", 1, 3),
            vec!["http://c/1", "http://a/1", "http://b/1"]
        );
        // 第 4、5 条永不出现。
        for index in 0..6 {
            let picked = choose_urls(&urls, "audio", index, 3);
            assert!(!picked
                .iter()
                .any(|url| url.starts_with("http://d") || url.starts_with("http://e")));
        }
    }

    #[test]
    fn pool_is_clamped_to_panel_max() {
        let urls: Vec<String> = (0..10)
            .map(|index| format!("http://cdn{index}/1"))
            .collect();
        // 配置写 99，有效池仍是 6。
        let picked = choose_urls(&urls, "video", 0, 99);
        assert_eq!(picked.len(), POOL_MAX);
        assert_eq!(picked[0], "http://cdn0/1");
    }

    #[test]
    fn pool_larger_than_urls_keeps_all() {
        let urls = vec!["http://a/1".to_string(), "http://b/1".to_string()];
        assert_eq!(
            choose_urls(&urls, "video", 0, 6),
            vec!["http://a/1", "http://b/1"]
        );
        assert_eq!(
            choose_urls(&urls, "audio", 0, 6),
            vec!["http://b/1", "http://a/1"]
        );
    }

    #[test]
    fn pool_disabled_returns_all() {
        let urls = vec![
            "http://a/1".to_string(),
            "http://b/1".to_string(),
            "http://c/1".to_string(),
            "http://d/1".to_string(),
        ];
        for pool in [0, 1] {
            assert_eq!(choose_urls(&urls, "video", 2, pool), urls);
            assert_eq!(choose_urls(&urls, "audio", 2, pool), urls);
        }
    }

    #[test]
    fn single_url_ignores_pool() {
        let urls = vec!["http://a/1".to_string()];
        assert_eq!(choose_urls(&urls, "video", 0, 6), urls);
        assert_eq!(choose_urls(&urls, "audio", 5, 6), urls);
    }

    #[test]
    fn attempt_plan_legacy_matches_old_loop() {
        // 旧循环：每个地址连续 3 次，最后一次不退避，地址之间不退避。
        let plan = attempt_plan(2, false, 2);
        assert_eq!(
            plan,
            vec![
                Attempt {
                    url: 0,
                    round: 0,
                    sleep: Some(0)
                },
                Attempt {
                    url: 0,
                    round: 1,
                    sleep: Some(1)
                },
                Attempt {
                    url: 0,
                    round: 2,
                    sleep: None
                },
                Attempt {
                    url: 1,
                    round: 0,
                    sleep: Some(0)
                },
                Attempt {
                    url: 1,
                    round: 1,
                    sleep: Some(1)
                },
                Attempt {
                    url: 1,
                    round: 2,
                    sleep: None
                },
            ]
        );
    }

    #[test]
    fn attempt_plan_pool_round_robin() {
        // 成员多于尝试数：每个成员至少试一次，全部不退避。
        let plan = attempt_plan(3, true, 1);
        assert_eq!(plan.len(), 3);
        assert_eq!(
            plan.iter().map(|item| item.url).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(plan.iter().all(|item| item.sleep.is_none()));

        // 默认 retry_max=8：总尝试数 9，只在轮末退避。
        let plan = attempt_plan(3, true, 8);
        assert_eq!(plan.len(), 9);
        assert_eq!(
            plan.iter().map(|item| item.url).collect::<Vec<_>>(),
            vec![0, 1, 2, 0, 1, 2, 0, 1, 2]
        );
        assert_eq!(
            plan.iter().map(|item| item.round).collect::<Vec<_>>(),
            vec![0, 0, 0, 1, 1, 1, 2, 2, 2]
        );
        assert_eq!(plan[2].sleep, Some(0));
        assert_eq!(plan[5].sleep, Some(1));
        assert_eq!(plan[8].sleep, None);

        // 成员多于预算时不会膨胀。
        let plan = attempt_plan(6, true, 0);
        assert_eq!(plan.len(), 6);
        assert_eq!(
            plan.iter().map(|item| item.url).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5]
        );
        assert!(plan.iter().all(|item| item.sleep.is_none()));
    }

    #[test]
    fn attempt_plan_single_member_equals_legacy() {
        assert_eq!(attempt_plan(1, true, 2), attempt_plan(1, false, 2));
        assert!(attempt_plan(0, true, 8).is_empty());
    }

    #[test]
    fn base_prefix_strips_file_name() {
        assert_eq!(base_prefix("http://a/dash/s-1.m4s"), "http://a/dash/");
        assert_eq!(
            base_prefix("http://a/dash/s-1.m4s?token=x"),
            "http://a/dash/"
        );
        assert_eq!(base_prefix("http://a/"), "http://a/");
        assert_eq!(base_prefix("plain"), "plain");
    }

    #[test]
    fn sticky_fresh_expires() {
        let now = Instant::now();
        assert!(sticky_fresh(now, now));
        assert!(sticky_fresh(now - VIDEO_STICKY_TTL, now));
        assert!(!sticky_fresh(
            now - VIDEO_STICKY_TTL - Duration::from_secs(1),
            now
        ));
    }

    #[test]
    fn live_media_has_no_endlist() {
        let p = parse_presentation(
            r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic" timeShiftBufferDepth="PT4S">
  <Period>
    <AdaptationSet contentType="video">
      <SegmentTemplate timescale="1000" media="s-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="2000" r="-1"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>"#,
            "http://origin.example/live.mpd",
        )
        .expect("MPD");
        let stream = &p.streams[0];
        let rendered = render_media(
            &p,
            stream,
            MediaOptions {
                gid: "g",
                cid: "c",
                self_base: "",
                keep_seconds: 0,
                pool_size: 1,
                formats: &["jpg".to_string()],
                track_index: 0,
                start_offset: 0.0,
                publish_delay_secs: 0.0,
                fetch_extra_segments: 0,
            },
        );
        assert!(!rendered.body.contains("#EXT-X-ENDLIST"));
        assert_eq!(rendered.body.matches("#EXTINF:").count(), 2);
    }

    #[test]
    fn media_sequence_uses_mpd_number() {
        let p = presentation();
        let stream = p.streams.iter().find(|s| s.kind == "video").unwrap();
        assert_eq!(stream.segments[0].number, 1);
        let rendered = render_media(
            &p,
            stream,
            MediaOptions {
                gid: "g",
                cid: "c",
                self_base: "",
                keep_seconds: 0,
                pool_size: 1,
                formats: &["jpg".to_string()],
                track_index: 0,
                start_offset: 0.0,
                publish_delay_secs: 0.0,
                fetch_extra_segments: 0,
            },
        );
        assert!(rendered.body.contains("#EXT-X-MEDIA-SEQUENCE:1"));
        assert!(rendered.body.contains("/g/c/video_720p/1_"));
    }

    #[test]
    fn group_keep_overrides_playlist_min() {
        assert_eq!(effective_keep(4, 30.0), 4);
        assert_eq!(effective_keep(0, 30.0), 30);
        assert_eq!(effective_keep(0, 0.0), 0);
    }

    #[test]
    fn live_media_uses_playlist_min_and_start_offset() {
        let p = presentation();
        let stream = p.streams.iter().find(|s| s.kind == "video").unwrap();
        let mut live = p.clone();
        live.is_live = true;
        let rendered = render_media(
            &live,
            stream,
            MediaOptions {
                gid: "g",
                cid: "c",
                self_base: "",
                keep_seconds: 0,
                pool_size: 1,
                formats: &["jpg".to_string()],
                track_index: 0,
                start_offset: 15.0,
                publish_delay_secs: 0.0,
                fetch_extra_segments: 0,
            },
        );
        assert!(rendered.body.contains("#EXT-X-START:TIME-OFFSET=-15.000"));
        assert!(!rendered.body.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn retry_delay_backs_off_and_caps() {
        let config = crate::config::model::CacheDashCfg::default();
        assert_eq!(retry_delay(&config, 0).as_millis(), 200);
        assert_eq!(retry_delay(&config, 1).as_millis(), 400);
        assert_eq!(retry_delay(&config, 8).as_millis(), 2_000);
        let mut config = config;
        config.min_request_interval_ms = 500;
        assert_eq!(retry_delay(&config, 0).as_millis(), 500);
    }

    #[test]
    fn segment_struct_is_public() {
        let segment = Segment {
            number: 1,
            time: 0,
            duration: 2.0,
            urls: vec!["http://a".into()],
        };
        assert_eq!(segment.duration, 2.0);
    }
}
