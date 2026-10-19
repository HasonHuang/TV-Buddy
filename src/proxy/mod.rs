//! 上游抓取与切片伪装（M2）。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §4。
//! 证据：F-004、F-006、F-017。

pub mod media;
pub mod segments;

pub use media::{parse_segment, prefix, prefix_matches, rewrite_media};
pub use segments::{save_index, segment_path, SegmentIndex};

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// 带 TTL 的小缓存。用于缓存在线清单。
pub struct TtlCache {
    ttl: Duration,
    inner: Mutex<HashMap<String, (Instant, String)>>,
}

impl TtlCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (at, value) = guard.get(key)?;
        if at.elapsed() < self.ttl {
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
