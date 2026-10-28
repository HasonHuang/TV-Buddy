//! 播放会话限制：每 IP 并发上限与空闲超时。
//!
//! 语义是**自定**的。目标 v0.8.2 的 `[auth].max_sessions_per_ip` 与
//! `[auth].session_ttl_secs` 没有观测到任何生效行为（F-010），所以这两条
//! 规则按 IPTV 场景自定，不能当作目标契约。
//!
//! 规则：
//!
//! - 一个会话 = `(客户端 IP, 频道)`。
//! - 每次播放请求续期。超过 `session_ttl_secs` 没有请求即过期。
//! - 同一 IP 的会话数超过 `max_sessions_per_ip` 时，踢掉最久未用的那个，
//!   并把被踢的 `(IP, 频道)` 记进限流表。
//! - 被踢方在 TTL 内再请求，返回 `Limit`。调用方据此跳 `limit_url`。
//!
//! 生效条件是 `[auth].ip_guard_enabled = true`。默认配置下本模块不参与，
//! 用户可见行为与未实现时完全一致。

use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::Mutex,
    time::{Duration, Instant},
};

/// 地址表容量。到达上限后不再登记新地址（失败放开，不阻断播放）。
const CAPACITY: usize = 10_000;

/// 一次播放请求的判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 放行。
    Allow,
    /// 超限被踢。调用方跳 `limit_url`。
    Limit,
}

#[derive(Debug)]
struct Session {
    channel: String,
    seen: Instant,
}

#[derive(Debug, Default)]
struct Inner {
    sessions: HashMap<String, Vec<Session>>,
    /// 被踢出且尚未到期的 `(IP, 频道)`。
    kicked: HashMap<(String, String), Instant>,
}

#[derive(Debug, Default)]
pub struct SessionGuard {
    inner: Mutex<Inner>,
}

impl SessionGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 判定并登记一次播放请求。
    ///
    /// `ttl` 是空闲超时，`max` 是每 IP 并发上限。两者都由调用方从配置取，
    /// 便于单测直接给值。
    pub fn check(&self, ip: &str, channel: &str, ttl: Duration, max: usize) -> Verdict {
        if ip.is_empty() || ip == "unknown" || max == 0 {
            return Verdict::Allow;
        }
        let now = Instant::now();
        let mut inner = self.lock();

        // 限流表命中：被踢的频道在 TTL 内不许回。
        if let Some(until) = inner.kicked.get(&(ip.to_string(), channel.to_string())) {
            if *until > now {
                return Verdict::Limit;
            }
        }
        inner.kicked.retain(|_, until| *until > now);

        if inner.sessions.len() >= CAPACITY && !inner.sessions.contains_key(ip) {
            return Verdict::Allow;
        }

        let mut list = inner.sessions.remove(ip).unwrap_or_default();
        list.retain(|session| now.saturating_duration_since(session.seen) <= ttl);

        if let Some(session) = list.iter_mut().find(|session| session.channel == channel) {
            session.seen = now;
            inner.sessions.insert(ip.to_string(), list);
            return Verdict::Allow;
        }

        if list.len() >= max {
            // 踢掉最久未用的会话，并把它记进限流表。
            if let Some(index) = (0..list.len()).min_by_key(|index| list[*index].seen) {
                let evicted = list.remove(index);
                inner
                    .kicked
                    .insert((ip.to_string(), evicted.channel), now + ttl);
            }
        }

        list.push(Session {
            channel: channel.to_string(),
            seen: now,
        });
        inner.sessions.insert(ip.to_string(), list);
        Verdict::Allow
    }

    /// 清理过期会话与过期限流记录；返回清理数量。
    pub fn purge(&self, ttl: Duration) -> usize {
        let now = Instant::now();
        let mut inner = self.lock();
        let before = inner.sessions.values().map(Vec::len).sum::<usize>() + inner.kicked.len();
        inner.sessions.retain(|_, list| {
            list.retain(|session| now.saturating_duration_since(session.seen) <= ttl);
            !list.is_empty()
        });
        inner.kicked.retain(|_, until| *until > now);
        before - (inner.sessions.values().map(Vec::len).sum::<usize>() + inner.kicked.len())
    }

    /// 当前会话总数。
    pub fn len(&self) -> usize {
        let inner = self.lock();
        inner.sessions.values().map(Vec::len).sum()
    }

    /// 当前是否没有会话。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 某个地址的会话数与频道。
    pub fn sessions_of(&self, ip: &str) -> Vec<String> {
        let inner = self.lock();
        let mut channels: Vec<String> = inner
            .sessions
            .get(ip)
            .map(|list| list.iter().map(|s| s.channel.clone()).collect())
            .unwrap_or_default();
        channels.sort();
        channels
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// 地址是否命中白名单。
///
/// 支持的写法：`*`（全部）、精确地址、IPv4 CIDR（`10.0.0.0/8`）。
/// IPv6 只支持精确匹配，不做前缀；写错规则按「不命中」处理，不报错。
pub fn whitelisted(rules: &[String], ip: &str) -> bool {
    if ip.is_empty() || ip == "unknown" {
        return false;
    }
    rules.iter().any(|rule| {
        let rule = rule.trim();
        rule == "*" || rule == ip || cidr_match(rule, ip)
    })
}

fn cidr_match(rule: &str, ip: &str) -> bool {
    let Some((network, bits)) = rule.split_once('/') else {
        return false;
    };
    let (Ok(network), Ok(bits), Ok(address)) = (
        network.trim().parse::<Ipv4Addr>(),
        bits.trim().parse::<u8>(),
        ip.parse::<Ipv4Addr>(),
    ) else {
        return false;
    };
    if bits > 32 {
        return false;
    }
    let mask = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(bits))
    };
    u32::from(network) & mask == u32::from(address) & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ttl() -> Duration {
        Duration::from_secs(30)
    }

    #[test]
    fn same_channel_refreshes_without_evicting() {
        let guard = SessionGuard::new();
        assert_eq!(guard.check("1.1.1.1", "c1", ttl(), 1), Verdict::Allow);
        assert_eq!(guard.check("1.1.1.1", "c1", ttl(), 1), Verdict::Allow);
        assert_eq!(guard.sessions_of("1.1.1.1"), vec!["c1"]);
    }

    #[test]
    fn second_channel_kicks_the_first() {
        let guard = SessionGuard::new();
        assert_eq!(guard.check("1.1.1.1", "c1", ttl(), 1), Verdict::Allow);
        // 换频道：新频道入，旧频道被踢。
        assert_eq!(guard.check("1.1.1.1", "c2", ttl(), 1), Verdict::Allow);
        assert_eq!(guard.sessions_of("1.1.1.1"), vec!["c2"]);
        // 被踢的频道在 TTL 内回来 → 限流。
        assert_eq!(guard.check("1.1.1.1", "c1", ttl(), 1), Verdict::Limit);
        // 当前频道继续放行。
        assert_eq!(guard.check("1.1.1.1", "c2", ttl(), 1), Verdict::Allow);
    }

    #[test]
    fn cap_two_keeps_both_channels() {
        let guard = SessionGuard::new();
        assert_eq!(guard.check("1.1.1.1", "c1", ttl(), 2), Verdict::Allow);
        assert_eq!(guard.check("1.1.1.1", "c2", ttl(), 2), Verdict::Allow);
        assert_eq!(guard.sessions_of("1.1.1.1"), vec!["c1", "c2"]);
        // 第三个才会踢人。
        assert_eq!(guard.check("1.1.1.1", "c3", ttl(), 2), Verdict::Allow);
        assert_eq!(guard.sessions_of("1.1.1.1"), vec!["c2", "c3"]);
    }

    #[test]
    fn different_ips_are_independent() {
        let guard = SessionGuard::new();
        assert_eq!(guard.check("1.1.1.1", "c1", ttl(), 1), Verdict::Allow);
        assert_eq!(guard.check("2.2.2.2", "c2", ttl(), 1), Verdict::Allow);
        assert_eq!(guard.len(), 2);
    }

    #[test]
    fn zero_cap_or_unknown_ip_allows() {
        let guard = SessionGuard::new();
        assert_eq!(guard.check("1.1.1.1", "c1", ttl(), 0), Verdict::Allow);
        assert_eq!(guard.check("1.1.1.1", "c2", ttl(), 0), Verdict::Allow);
        assert_eq!(guard.check("unknown", "c1", ttl(), 1), Verdict::Allow);
        assert_eq!(guard.check("", "c1", ttl(), 1), Verdict::Allow);
        assert_eq!(guard.len(), 0);
    }

    #[test]
    fn purge_drops_expired() {
        let guard = SessionGuard::new();
        assert_eq!(
            guard.check("1.1.1.1", "c1", Duration::from_millis(1), 1),
            Verdict::Allow
        );
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(guard.purge(Duration::from_millis(1)), 1);
        assert_eq!(guard.len(), 0);
        // 过期的踢人记录也一并清掉。
        assert_eq!(guard.check("1.1.1.1", "c1", ttl(), 1), Verdict::Allow);
    }

    #[test]
    fn whitelist_matches_exact_wildcard_and_cidr() {
        let rules = vec![
            "*".to_string(),
            "10.0.0.0/8".to_string(),
            "192.168.1.5".to_string(),
        ];
        assert!(whitelisted(&rules, "8.8.8.8"));
        assert!(whitelisted(&rules[1..], "10.1.2.3"));
        assert!(whitelisted(&rules[1..], "192.168.1.5"));
        assert!(!whitelisted(&rules[1..], "192.168.1.6"));
        assert!(!whitelisted(&rules[1..], "11.0.0.1"));
        assert!(!whitelisted(&rules, ""));
        assert!(!whitelisted(&rules, "unknown"));
    }

    #[test]
    fn whitelist_cidr_edges() {
        assert!(cidr_match("0.0.0.0/0", "1.2.3.4"));
        assert!(cidr_match("1.2.3.4/32", "1.2.3.4"));
        assert!(!cidr_match("1.2.3.4/32", "1.2.3.5"));
        // 写错的规则按不命中处理。
        assert!(!cidr_match("1.2.3.4/33", "1.2.3.4"));
        assert!(!cidr_match("not-an-ip/8", "1.2.3.4"));
        assert!(!cidr_match("1.2.3.4", "1.2.3.4"));
    }
}
