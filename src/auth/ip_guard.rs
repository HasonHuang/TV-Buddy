//! IP 统计与封禁。
//!
//! 面板可显式封禁与解封；播放路径（非 `/panel`）在封禁期内返回 `403`。
//! 统计只保留最近 10 000 个地址，超出后忽略新地址。

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// 默认封禁时长（秒）。
pub const DEFAULT_BAN_SECS: u64 = 3600;
/// 最长封禁时长（秒）。
pub const MAX_BAN_SECS: u64 = 30 * 24 * 3600;
/// 统计容量上限。
const CAPACITY: usize = 10_000;

#[derive(Debug, Clone, Default)]
struct Entry {
    requests: u64,
    failures: u64,
    banned_until: Option<Instant>,
    unbanned_at: Option<Instant>,
}

impl Entry {
    fn banned(&self) -> bool {
        self.banned_until
            .map(|until| until > Instant::now())
            .unwrap_or(false)
    }
}

/// 单地址快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub ip: String,
    pub requests: u64,
    pub failures: u64,
    pub banned: bool,
    pub remaining_secs: u64,
}

#[derive(Debug, Default)]
pub struct IpGuard {
    inner: Mutex<HashMap<String, Entry>>,
}

impl IpGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次请求。
    pub fn record_request(&self, ip: &str) {
        if ip.is_empty() || ip == "unknown" {
            return;
        }
        let mut inner = self.lock();
        if !inner.contains_key(ip) && inner.len() >= CAPACITY {
            return;
        }
        inner.entry(ip.to_string()).or_default().requests += 1;
    }

    /// 记一次上游失败。
    pub fn record_failure(&self, ip: &str) {
        if ip.is_empty() || ip == "unknown" {
            return;
        }
        let mut inner = self.lock();
        if !inner.contains_key(ip) && inner.len() >= CAPACITY {
            return;
        }
        inner.entry(ip.to_string()).or_default().failures += 1;
    }

    /// 封禁一个地址；返回到期时刻的剩余秒数。
    pub fn ban(&self, ip: &str, seconds: u64) -> u64 {
        let seconds = if seconds == 0 {
            DEFAULT_BAN_SECS
        } else {
            seconds.min(MAX_BAN_SECS)
        };
        let mut inner = self.lock();
        let entry = inner.entry(ip.to_string()).or_default();
        entry.banned_until = Some(Instant::now() + Duration::from_secs(seconds));
        seconds
    }

    /// 解封；返回是否命中记录。
    pub fn unban(&self, ip: &str) -> bool {
        let mut inner = self.lock();
        match inner.get_mut(ip) {
            Some(entry) => {
                let was = entry.banned_until.take().is_some();
                if was {
                    entry.unbanned_at = Some(Instant::now());
                }
                was
            }
            None => false,
        }
    }

    /// 当前封禁中的地址。
    pub fn banned_ips(&self) -> Vec<String> {
        let mut ips: Vec<String> = self
            .snapshot()
            .into_iter()
            .filter(|entry| entry.banned)
            .map(|entry| entry.ip)
            .collect();
        ips.sort();
        ips
    }

    /// 最近 `window` 内解封的地址。
    pub fn recent_unbans(&self, window: Duration) -> Vec<String> {
        let inner = self.lock();
        let mut ips: Vec<String> = inner
            .iter()
            .filter_map(|(ip, entry)| {
                entry
                    .unbanned_at
                    .filter(|at| at.elapsed() < window)
                    .map(|_| ip.clone())
            })
            .collect();
        ips.sort();
        ips
    }

    /// 是否处于封禁期。
    pub fn is_banned(&self, ip: &str) -> bool {
        let inner = self.lock();
        match inner.get(ip) {
            Some(entry) => entry.banned(),
            None => false,
        }
    }

    /// 全部地址快照，按请求数降序。
    pub fn snapshot(&self) -> Vec<Snapshot> {
        let inner = self.lock();
        let mut entries: Vec<Snapshot> = inner
            .iter()
            .map(|(ip, entry)| {
                let remaining = entry
                    .banned_until
                    .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
                    .unwrap_or(0);
                Snapshot {
                    ip: ip.clone(),
                    requests: entry.requests,
                    failures: entry.failures,
                    banned: entry.banned(),
                    remaining_secs: remaining,
                }
            })
            .collect();
        entries.sort_by(|a, b| b.requests.cmp(&a.requests).then(a.ip.cmp(&b.ip)));
        entries
    }

    /// `(地址数, 封禁数)`。
    pub fn summary(&self) -> (usize, usize) {
        let inner = self.lock();
        let banned = inner.values().filter(|entry| entry.banned()).count();
        (inner.len(), banned)
    }

    /// 清理过期封禁与零计数条目；返回清理数量。
    pub fn prune(&self) -> usize {
        let mut inner = self.lock();
        let before = inner.len();
        inner.retain(|_, entry| {
            if entry.banned() {
                return true;
            }
            entry.banned_until.is_none() && (entry.requests > 0 || entry.failures > 0)
        });
        before - inner.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_bans() {
        let guard = IpGuard::new();
        guard.record_request("1.2.3.4");
        guard.record_request("1.2.3.4");
        guard.record_failure("1.2.3.4");
        assert!(!guard.is_banned("1.2.3.4"));

        assert_eq!(guard.ban("1.2.3.4", 0), DEFAULT_BAN_SECS);
        assert!(guard.is_banned("1.2.3.4"));
        let (total, banned) = guard.summary();
        assert_eq!((total, banned), (1, 1));

        let entries = guard.snapshot();
        assert_eq!(entries[0].requests, 2);
        assert_eq!(entries[0].failures, 1);
        assert!(entries[0].remaining_secs > 0);

        assert!(guard.unban("1.2.3.4"));
        assert!(!guard.is_banned("1.2.3.4"));
        assert!(!guard.unban("1.2.3.4"));
        assert!(!guard.is_banned("9.9.9.9"));
    }

    #[test]
    fn tracks_bans_and_unbans() {
        let guard = IpGuard::new();
        guard.ban("1.1.1.1", 60);
        guard.ban("2.2.2.2", 60);
        assert_eq!(guard.banned_ips(), vec!["1.1.1.1", "2.2.2.2"]);
        assert!(guard.unban("1.1.1.1"));
        assert_eq!(guard.banned_ips(), vec!["2.2.2.2"]);
        assert_eq!(
            guard.recent_unbans(Duration::from_secs(60)),
            vec!["1.1.1.1"]
        );
    }

    #[test]
    fn ban_is_capped() {
        let guard = IpGuard::new();
        assert_eq!(guard.ban("8.8.8.8", MAX_BAN_SECS * 2), MAX_BAN_SECS);
    }

    #[test]
    fn empty_ip_is_ignored() {
        let guard = IpGuard::new();
        guard.record_request("");
        guard.record_request("unknown");
        assert_eq!(guard.summary().0, 0);
    }

    #[test]
    fn prune_removes_idle_entries() {
        let guard = IpGuard::new();
        guard.record_request("1.1.1.1");
        guard.ban("2.2.2.2", 60);
        // 计数为 0 的条目会被清掉；封禁中的保留。
        assert_eq!(guard.prune(), 0);
    }
}
