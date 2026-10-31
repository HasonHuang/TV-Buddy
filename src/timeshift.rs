//! 直播回看：按 `livestream` 频道持续缓冲切片，产出 VOD 清单。
//!
//! 证据：F-009 §1.8。目标行为：
//! - 点亮 `livestream` 后按 `timeshift_hours` 缓冲；
//! - `?start=<unix>&end=<unix>` 或 `?playseek=<本地 yyyymmddHHMMSS-…>`；
//! - 回看清单带 `#EXT-X-PLAYLIST-TYPE:VOD` 与 `#EXT-X-ENDLIST`；
//! - 分片 `/{gid}/{cid}/timeshift/<16hex>.<ext>`，图片类型，`private, max-age=3600`；
//! - 越界或未命中 → 落回实时清单。

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
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{http::routes::stream::channel_headers, state::AppState};

/// 空闲多久后回收会话。
const IDLE: Duration = Duration::from_secs(900);
/// 伪装扩展名轮换。
const EXTS: &[&str] = &["png", "gif", "bmp", "jpg", "webp"];

#[derive(Debug, Clone)]
struct Entry {
    start: i64,
    duration: f64,
    name: String,
    size: u64,
}

/// 回看会话。
pub struct Session {
    gid: String,
    cid: String,
    dir: PathBuf,
    hours: u32,
    max_bytes: u64,
    entries: Mutex<VecDeque<Entry>>,
    last: Mutex<Instant>,
    stop: AtomicBool,
}

impl Session {
    fn touch(&self) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }

    /// 生成回看清单；无交集返回 `None`。
    pub fn playlist(&self, start: i64, end: i64, slice: &str) -> Option<String> {
        self.touch();
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let picked: Vec<&Entry> = entries
            .iter()
            .filter(|entry| {
                let tail = entry.start + entry.duration as i64;
                tail > start && entry.start < end
            })
            .collect();
        if picked.is_empty() {
            return None;
        }
        let target = picked
            .iter()
            .map(|entry| entry.duration.ceil() as u64)
            .max()
            .unwrap_or(6)
            .max(1);
        let mut body = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:0\n"
        );
        let dir = if slice.is_empty() {
            format!("/{}/{}/timeshift", self.gid, self.cid)
        } else {
            format!("/{}/{}/{slice}/timeshift", self.gid, self.cid)
        };
        for entry in picked {
            body.push_str(&format!(
                "#EXTINF:{:.3},\n{dir}/{}\n",
                entry.duration, entry.name
            ));
        }
        body.push_str("#EXT-X-ENDLIST\n");
        Some(body)
    }

    fn segment(&self, name: &str) -> Option<Vec<u8>> {
        if !valid_name(name) {
            return None;
        }
        self.touch();
        fs::read(self.dir.join(name)).ok()
    }

    /// 停止会话：删除目录。
    fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = fs::remove_dir_all(&self.dir);
    }

    fn push(&self, entry: Entry) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.push_back(entry);
        let now = crate::schedule::now_epoch();
        let window = self.hours.max(1) as i64 * 3600;
        while let Some(front) = entries.front() {
            let expired = now - front.start > window;
            let oversized = self.max_bytes > 0
                && entries.iter().map(|entry| entry.size).sum::<u64>() > self.max_bytes;
            if !expired && !oversized {
                break;
            }
            if let Some(old) = entries.pop_front() {
                let _ = fs::remove_file(self.dir.join(&old.name));
            }
            if !oversized {
                break;
            }
        }
    }
}

fn valid_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    EXTS.contains(&ext) && stem.len() == 16 && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn content_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

/// 管理器。
#[derive(Default)]
pub struct Manager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    janitor: AtomicBool,
}

impl Manager {
    /// 设备会话；频道未点亮 `livestream` 时返回 `None`。
    ///
    /// DASH 频道的回看不走本模块，见 [`crate::dash_timeshift`]。
    pub fn ensure(
        self: &Arc<Self>,
        state: &Arc<AppState>,
        gid: &str,
        cid: &str,
    ) -> Option<Arc<Session>> {
        let config = state.snapshot();
        let group = config.group(gid)?;
        let channel = group.channels.iter().find(|channel| channel.id == cid)?;
        if !channel.livestream {
            return None;
        }
        let key = format!("{gid}/{cid}");
        if let Some(session) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
        {
            session.touch();
            return Some(session);
        }
        let dir = state.cache_dir.join("timeshift").join(gid).join(cid);
        let _ = fs::create_dir_all(&dir);
        let session = Arc::new(Session {
            gid: gid.to_string(),
            cid: cid.to_string(),
            dir,
            hours: channel.timeshift_hours,
            max_bytes: channel.timeshift_max_gb.saturating_mul(1 << 30),
            entries: Mutex::new(VecDeque::new()),
            last: Mutex::new(Instant::now()),
            stop: AtomicBool::new(false),
        });
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, session.clone());
        info!(gid, cid, "回看会话开始");
        spawn_poller(session.clone(), state.clone(), channel.url.clone());
        self.ensure_janitor();
        Some(session)
    }

    /// 已缓冲切片数；会话不存在时为 0。
    pub fn count(&self, gid: &str, cid: &str) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&format!("{gid}/{cid}"))
            .map(|session| {
                session
                    .entries
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .len()
            })
            .unwrap_or(0)
    }

    /// 回看清单；未点亮、未命中或越界返回 `None`（调用方落回实时）。
    pub fn playlist(
        self: &Arc<Self>,
        state: &Arc<AppState>,
        gid: &str,
        cid: &str,
        start: i64,
        end: i64,
    ) -> Option<String> {
        let session = self.ensure(state, gid, cid)?;
        let slice = crate::sign::prefix(&state.snapshot(), gid, cid);
        session.playlist(start, end, &slice)
    }

    /// 切片响应；名字不属于回看命名时返回 `None`。
    pub fn serve(&self, gid: &str, cid: &str, name: &str) -> Option<Response> {
        if !valid_name(name) {
            return None;
        }
        let session = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&format!("{gid}/{cid}"))
            .cloned();
        let Some(session) = session else {
            return Some((StatusCode::NOT_FOUND, "Segment not found").into_response());
        };
        let Some(bytes) = session.segment(name) else {
            return Some((StatusCode::NOT_FOUND, "Segment not found").into_response());
        };
        let ext = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("png");
        let mut response = (StatusCode::OK, Body::from(bytes)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(content_type(ext)),
        );
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=3600"),
        );
        Some(response)
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
                    let mut sessions = manager.sessions.lock().unwrap_or_else(|e| e.into_inner());
                    let keys: Vec<String> = sessions
                        .iter()
                        .filter(|(_, session)| session.idle_for() > IDLE)
                        .map(|(key, _)| key.clone())
                        .collect();
                    keys.into_iter()
                        .filter_map(|key| sessions.remove(&key))
                        .collect()
                };
                for session in reaped {
                    session.shutdown();
                    info!(gid = %session.gid, cid = %session.cid, "回看会话回收");
                }
            }
        });
    }
}

/// 解析回看区间：优先 `playseek`，其次 `start`/`end`。
pub fn range(query: &HashMap<String, String>, offset_hours: i64) -> Option<(i64, i64)> {
    if let Some(seek) = query.get("playseek") {
        if let Some(pair) = parse_playseek(seek, offset_hours) {
            return Some(pair);
        }
    }
    let start = query.get("start")?.parse::<i64>().ok()?;
    let end = query.get("end")?.parse::<i64>().ok()?;
    (end > start).then_some((start, end))
}

/// `yyyyMMddHHMMSS-yyyyMMddHHMMSS`，按本地时区。
pub fn parse_playseek(text: &str, offset_hours: i64) -> Option<(i64, i64)> {
    let (start, end) = text.split_once('-')?;
    let start = parse_compact(start, offset_hours)?;
    let end = parse_compact(end, offset_hours)?;
    (end > start).then_some((start, end))
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

fn spawn_poller(session: Arc<Session>, state: Arc<AppState>, url: String) {
    tokio::spawn(async move {
        let mut seen_remote: HashSet<String> = HashSet::new();
        let offset = crate::proxy::media::ext_offset(&session.gid, &session.cid);
        let mut sequence = 0usize;
        loop {
            if session.stop.load(Ordering::SeqCst) || session.idle_for() > IDLE {
                break;
            }
            let config = state.snapshot();
            let Some(channel) = config.group(&session.gid).and_then(|group| {
                group
                    .channels
                    .iter()
                    .find(|channel| channel.id == session.cid)
            }) else {
                break;
            };
            if !channel.livestream {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            let headers = channel_headers(&state, &session.gid, &session.cid);
            let mut base = url.clone();
            let mut request = state.http_for(&session.gid, &session.cid).get(&base);
            for (name, value) in &headers {
                request = request.header(name, value);
            }
            let mut playlist = match request.send().await {
                Ok(response) if response.status().is_success() => {
                    response.text().await.unwrap_or_default()
                }
                Ok(response) => {
                    warn!(status = %response.status(), "回看上游状态异常");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                Err(error) => {
                    warn!(%error, "回看上游连接失败");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            if crate::proxy::media::is_master(&playlist) {
                if let Some((_, variant)) = crate::proxy::media::best_variant(&playlist, &base) {
                    base = variant;
                    let mut request = state.http_for(&session.gid, &session.cid).get(&base);
                    for (name, value) in &headers {
                        request = request.header(name, value);
                    }
                    playlist = match request.send().await {
                        Ok(response) => response.text().await.unwrap_or_default(),
                        Err(_) => String::new(),
                    };
                }
            }
            let mut duration = 0f64;
            for line in playlist.lines().map(str::trim) {
                if let Some(rest) = line.strip_prefix("#EXTINF:") {
                    duration = rest
                        .split(',')
                        .next()
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0.0);
                    continue;
                }
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Ok(segment) = url::Url::parse(&base).and_then(|base| base.join(line)) else {
                    continue;
                };
                let identifier = segment.to_string();
                if !seen_remote.insert(identifier.clone()) {
                    duration = 0.0;
                    continue;
                }
                let mut request = state.http_for(&session.gid, &session.cid).get(&identifier);
                for (name, value) in &headers {
                    request = request.header(name, value);
                }
                let bytes = match request.send().await {
                    Ok(response) if response.status().is_success() => {
                        response.bytes().await.ok().map(|bytes| bytes.to_vec())
                    }
                    _ => None,
                };
                let Some(bytes) = bytes else {
                    duration = 0.0;
                    continue;
                };
                let digest = Sha256::digest(identifier.as_bytes());
                let stem: String = hex::encode(digest)[..16].to_string();
                let ext = EXTS[(sequence + offset) % EXTS.len()];
                let name = format!("{stem}.{ext}");
                if fs::write(session.dir.join(&name), &bytes).is_ok() {
                    session.push(Entry {
                        start: crate::schedule::now_epoch(),
                        duration: if duration > 0.0 { duration } else { 2.0 },
                        name,
                        size: bytes.len() as u64,
                    });
                    sequence += 1;
                }
                duration = 0.0;
            }
            // 清单截断时清空去重集合，避免无限增长。
            if seen_remote.len() > 4096 {
                seen_remote.clear();
            }
            let interval = crate::proxy::media::target_duration(&playlist)
                .unwrap_or(5)
                .clamp(2, 10);
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
    });
}

/// 面板/启动入口：点亮频道后立即开始缓冲。
pub fn warm(state: &Arc<AppState>) {
    let config = state.snapshot();
    let entries: Vec<(String, String)> = config
        .groups
        .iter()
        .flat_map(|group| {
            group
                .channels
                .iter()
                .filter(|channel| channel.livestream)
                .map(|channel| (group.id.clone(), channel.id.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    for (gid, cid) in entries {
        state.timeshift.ensure(state, &gid, &cid);
    }
}

/// 响应头之外的 VOD 清单正文（由 stream 路由直接返回）。
pub fn vad_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, must-revalidate"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            gid: "g".into(),
            cid: "c".into(),
            dir: std::env::temp_dir().join(format!("timeshift-{}", uuid::Uuid::new_v4())),
            hours: 4,
            max_bytes: 0,
            entries: Mutex::new(VecDeque::new()),
            last: Mutex::new(Instant::now()),
            stop: AtomicBool::new(false),
        }
    }

    #[test]
    fn names_are_hex_and_image_only() {
        assert!(valid_name("0123456789abcdef.png"));
        assert!(!valid_name("0123456789abcde.png"));
        assert!(!valid_name("0123456789abcdef.exe"));
        assert_eq!(content_type("webp"), "image/webp");
    }

    #[test]
    fn playlist_selects_overlapping_entries() {
        let session = session();
        fs::create_dir_all(&session.dir).expect("建目录");
        let base = crate::schedule::now_epoch();
        session.push(Entry {
            start: base,
            duration: 5.0,
            name: "1111111111111111.png".into(),
            size: 1,
        });
        session.push(Entry {
            start: base + 5,
            duration: 5.0,
            name: "2222222222222222.gif".into(),
            size: 1,
        });
        let body = session.playlist(base + 2, base + 8, "").expect("清单");
        assert!(body.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(body.contains("#EXT-X-ENDLIST"));
        assert!(body.contains("/g/c/timeshift/1111111111111111.png"));
        assert!(body.contains("/g/c/timeshift/2222222222222222.gif"));
        assert!(session.playlist(0, 50, "").is_none());

        let signed = session
            .playlist(base + 2, base + 8, "l/123/abc")
            .expect("签名清单");
        assert!(signed.contains("/g/c/l/123/abc/timeshift/"), "{signed}");
    }

    #[test]
    fn playseek_parses_local_range() {
        let query = HashMap::from([(
            "playseek".to_string(),
            "20261020190000-20261020200000".to_string(),
        )]);
        let (start, end) = range(&query, 8).expect("区间");
        assert_eq!(end - start, 3600);
        assert_eq!(
            crate::schedule::format_local(start, 8),
            "2026-10-20T19:00:00"
        );
        let query = HashMap::from([
            ("start".to_string(), "100".to_string()),
            ("end".to_string(), "200".to_string()),
        ]);
        assert_eq!(range(&query, 8), Some((100, 200)));
    }
}
