//! 直连模式辅助：地址缓存与画质选择（M3）。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §4、§4.2。
//! 证据：S-MODE-08/09/11/13/14/15/16/18。

use std::{
    collections::HashMap,
    sync::Mutex,
    time::Instant,
};

/// 带每项 TTL 的地址缓存。
#[derive(Default)]
pub struct AddressCache {
    inner: Mutex<HashMap<String, (Instant, String)>>,
}

impl AddressCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取缓存值；超过 `ttl` 秒即视为过期。
    pub fn get(&self, key: &str, ttl_secs: u64) -> Option<String> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (at, value) = guard.get(key)?;
        if at.elapsed().as_secs() < ttl_secs.max(1) {
            Some(value.clone())
        } else {
            None
        }
    }

    pub fn put(&self, key: &str, value: String) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(key.to_string(), (Instant::now(), value));
    }
}

/// 是否为「解析最佳画质」跳转方式。
pub fn wants_best_quality(mode: &str) -> bool {
    matches!(mode, "best" | "auto" | "resolve" | "best_quality" | "解析最佳画质")
}

/// 生效的上游地址缓存时长。0 表示用默认 300 秒（S-MODE-08）。
pub fn effective_redirect_ttl(configured: u64) -> u64 {
    if configured == 0 {
        300
    } else {
        configured
    }
}

/// 取主清单里码率最高的变体地址。
pub fn best_variant(playlist: &str, base: &str) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    let mut pending_bandwidth: Option<u64> = None;
    for line in playlist.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_bandwidth = Some(attribute(rest, "BANDWIDTH").unwrap_or(0));
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        // 只有紧跟 `#EXT-X-STREAM-INF` 的地址才算变体（主清单）。
        let Some(bandwidth) = pending_bandwidth.take() else {
            continue;
        };
        let absolute = absolute(base, trimmed);
        if best.as_ref().map(|(level, _)| bandwidth > *level).unwrap_or(true) {
            best = Some((bandwidth, absolute));
        }
    }
    best.map(|(_, url)| url)
}

/// 正文是否为 HLS 清单（S-MODE-18：Remux 只接受连续流）。
pub fn looks_like_hls(body: &str) -> bool {
    body.trim_start().starts_with("#EXTM3U")
}

/// 正文是否为 DASH 清单。
pub fn looks_like_mpd(body: &str) -> bool {
    let head = body.trim_start();
    head.starts_with("<?xml") || head.contains("<MPD")
}

/// 读 `KEY=VALUE` 形式的属性。
fn attribute(line: &str, key: &str) -> Option<u64> {
    let marker = format!("{key}=");
    let at = line.find(&marker)? + marker.len();
    let rest = &line[at..];
    let end = rest.find(',').unwrap_or(rest.len());
    rest[..end].trim_matches('"').parse().ok()
}

/// 绝对化地址。
fn absolute(base: &str, reference: &str) -> String {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return reference.to_string();
    }
    url::Url::parse(base)
        .and_then(|base| base.join(reference))
        .map(|url| url.to_string())
        .unwrap_or_else(|_| reference.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_best_bandwidth_variant() {
        let playlist = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=640x360\n360/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2000000\n1080/index.m3u8\n";
        let picked = best_variant(playlist, "http://up.example/live/master.m3u8").unwrap();
        assert_eq!(picked, "http://up.example/live/1080/index.m3u8");
    }

    #[test]
    fn media_playlist_has_no_variant() {
        assert!(best_variant("#EXTM3U\n#EXTINF:4,\nseg.ts\n", "http://up/x").is_none());
    }

    #[test]
    fn address_cache_honours_ttl() {
        let cache = AddressCache::new();
        cache.put("k", "http://up".into());
        assert_eq!(cache.get("k", 300).as_deref(), Some("http://up"));
        assert!(cache.get("missing", 300).is_none());
    }

    #[test]
    fn redirect_ttl_defaults_to_300() {
        assert_eq!(effective_redirect_ttl(0), 300);
        assert_eq!(effective_redirect_ttl(60), 60);
    }

    #[test]
    fn classification_helpers() {
        assert!(looks_like_hls("#EXTM3U\n#EXTINF:4,\nseg.ts\n"));
        assert!(!looks_like_hls("<MPD></MPD>"));
        assert!(looks_like_mpd("<MPD></MPD>"));
        assert!(looks_like_mpd("<?xml version=\"1.0\"?><MPD/>"));
    }
}
