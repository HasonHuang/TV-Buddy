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

/// 统计键上限。路径来自请求，必须有界，避免无效地址占满内存。
const MAX_GROUP_KEYS: usize = 1024;
const MAX_CHANNEL_KEYS: usize = 16384;

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
    ///
    /// 达到键上限后，新键不再登记。已有键继续累加。
    pub fn record_request(&self, gid: &str) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        if gid.is_empty() {
            return;
        }
        let mut groups = self.per_group.lock().unwrap_or_else(|e| e.into_inner());
        let at_capacity = groups.len() >= MAX_GROUP_KEYS;
        match groups.get_mut(gid) {
            Some(count) => *count += 1,
            None if !at_capacity => {
                groups.insert(gid.to_string(), 1);
            }
            None => {}
        }
    }

    /// 记一次频道请求。`key` 形如 `gid/cid`。
    ///
    /// 达到键上限后，新键不再登记。已有键继续累加。
    pub fn record_channel(&self, key: &str) {
        if key.is_empty() {
            return;
        }
        let mut channels = self.per_channel.lock().unwrap_or_else(|e| e.into_inner());
        let at_capacity = channels.len() >= MAX_CHANNEL_KEYS;
        match channels.get_mut(key) {
            Some(count) => *count += 1,
            None if !at_capacity => {
                channels.insert(key.to_string(), 1);
            }
            None => {}
        }
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

    /// 频道键是否已登记。用于跳过高频请求里的频道列表扫描。
    pub fn channel_tracked(&self, key: &str) -> bool {
        self.per_channel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(key)
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

    #[test]
    fn caps_new_stat_keys() {
        let stats = Stats::default();
        for index in 0..=MAX_GROUP_KEYS {
            stats.record_request(&format!("g{index}"));
        }
        {
            let groups = stats.per_group.lock().unwrap();
            assert_eq!(groups.len(), MAX_GROUP_KEYS);
        }

        for index in 0..=MAX_CHANNEL_KEYS {
            stats.record_channel(&format!("g/c{index}"));
        }
        {
            let channels = stats.per_channel.lock().unwrap();
            assert_eq!(channels.len(), MAX_CHANNEL_KEYS);
        }

        // 已达上限时，已有键仍继续累加。
        stats.record_channel("g/c0");
        let channels = stats.per_channel.lock().unwrap();
        assert_eq!(channels.get("g/c0"), Some(&2));
    }
}
