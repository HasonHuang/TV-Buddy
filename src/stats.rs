//! 运行指标（M4）。
//!
//! 形状为本实现自定（目标 `/panel/api/stats` 未逐字段采证），见
//! [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §6.4。

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

#[derive(Debug, Default)]
pub struct Stats {
    requests: AtomicU64,
    playlists: AtomicU64,
    segments: AtomicU64,
    segment_hits: AtomicU64,
    upstream_failures: AtomicU64,
    per_group: Mutex<HashMap<String, u64>>,
    /// 频道累计请求，键形如 `gid/cid`。
    per_channel: Mutex<HashMap<String, u64>>,
}

/// 指标快照。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub requests: u64,
    pub playlists: u64,
    pub segments: u64,
    pub segment_hits: u64,
    pub upstream_failures: u64,
    pub groups: Vec<(String, u64)>,
}

impl Stats {
    /// 记一次请求；`gid` 为空表示不属于任何分组。
    pub fn record_request(&self, gid: &str) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        if gid.is_empty() {
            return;
        }
        let mut groups = self.per_group.lock().unwrap_or_else(|e| e.into_inner());
        *groups.entry(gid.to_string()).or_insert(0) += 1;
    }

    /// 记一次频道请求。`key` 形如 `gid/cid`。
    pub fn record_channel(&self, key: &str) {
        if key.is_empty() {
            return;
        }
        let mut channels = self.per_channel.lock().unwrap_or_else(|e| e.into_inner());
        *channels.entry(key.to_string()).or_insert(0) += 1;
    }

    /// 某频道的累计请求数。未记录过的频道返回 0。
    pub fn channel_requests(&self, key: &str) -> u64 {
        self.per_channel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .copied()
            .unwrap_or(0)
    }

    pub fn record_playlist(&self) {
        self.playlists.fetch_add(1, Ordering::Relaxed);
    }

    /// 记一次切片请求；`hit` 表示命中本地缓存。
    pub fn record_segment(&self, hit: bool) {
        self.segments.fetch_add(1, Ordering::Relaxed);
        if hit {
            self.segment_hits.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_upstream_failure(&self) {
        self.upstream_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut groups: Vec<(String, u64)> = self
            .per_group
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(id, count)| (id.clone(), *count))
            .collect();
        groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Snapshot {
            requests: self.requests.load(Ordering::Relaxed),
            playlists: self.playlists.load(Ordering::Relaxed),
            segments: self.segments.load(Ordering::Relaxed),
            segment_hits: self.segment_hits.load(Ordering::Relaxed),
            upstream_failures: self.upstream_failures.load(Ordering::Relaxed),
            groups,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_requests_per_group() {
        let stats = Stats::default();
        stats.record_request("g1");
        stats.record_request("g1");
        stats.record_request("g2");
        stats.record_request("");
        stats.record_segment(true);
        stats.record_segment(false);
        stats.record_playlist();
        stats.record_upstream_failure();

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.requests, 4);
        assert_eq!(snapshot.segments, 2);
        assert_eq!(snapshot.segment_hits, 1);
        assert_eq!(snapshot.playlists, 1);
        assert_eq!(snapshot.upstream_failures, 1);
        assert_eq!(snapshot.groups, vec![("g1".into(), 2), ("g2".into(), 1)]);
    }
}
