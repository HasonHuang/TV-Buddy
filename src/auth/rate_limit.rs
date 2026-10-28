//! 两级播放入口限流、违规计数与升级封禁。
//!
//! 语义是**自定**的。目标 v0.8.2 的 `[auth]` 限流相关键没有观测到生效行为
//! （F-010），所以规则按 IPTV 场景自定，不能当作目标契约。
//!
//! 规则：
//!
//! - **只计播放入口请求**（`/{gid}/{cid}.m3u8`、`.mpd`、`.txt`），不计切片。
//!   切片的频率由码率决定，限切片等于限画质；限入口等于限换台频率。
//! - **L2 是突发上限**（默认 8 次 / 30 秒），**L1 是持续上限**（默认 30 次 / 30 秒）。
//!   两级独立判定，任一超限即触发。
//! - 触发后冷却 `cooldown_secs` 秒，期间返回 `Throttle`，并记一次违规。
//! - 违规在 `violation_window_secs` 内有效，并按 `violation_decay_secs` 衰减：
//!   每过满一个衰减周期，最旧的一条违规失效。
//! - 有效违规数达到 `ban_threshold` 时返回 `Ban`，由调用方封禁
//!   `ban_duration_secs` 秒。
//!
//! 生效条件是 `[auth].ip_guard_enabled = true`。默认配置下本模块不参与。

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// 地址表容量。到达上限后不再登记新地址（失败放开，不阻断播放）。
const CAPACITY: usize = 10_000;

/// 一次入口请求的判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 放行。
    Allow,
    /// 冷却中。值为建议的 `Retry-After` 秒数。
    Throttle(u64),
    /// 违规累计到阈值，应封禁。
    Ban,
}

/// 限流参数。全部来自 `[auth]`，由调用方换算成 `Duration`。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub l1_window: Duration,
    pub l1_limit: u64,
    pub l2_window: Duration,
    pub l2_limit: u64,
    pub cooldown: Duration,
    pub violation_window: Duration,
    pub violation_decay: Duration,
    pub ban_threshold: u64,
}

#[derive(Debug, Default)]
struct Entry {
    l1: Vec<Instant>,
    l2: Vec<Instant>,
    violations: Vec<Instant>,
    last_decay: Option<Instant>,
    cooldown_until: Option<Instant>,
}

/// 按 IP 的两级限流器。
#[derive(Debug, Default)]
pub struct RateLimiter {
    inner: Mutex<HashMap<String, Entry>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 判定一次播放入口请求并计数。
    pub fn check(&self, ip: &str, limits: &Limits) -> Decision {
        if ip.is_empty() || ip == "unknown" {
            return Decision::Allow;
        }
        let now = Instant::now();
        let mut inner = self.lock();
        if !inner.contains_key(ip) && inner.len() >= CAPACITY {
            return Decision::Allow;
        }
        let entry = inner.entry(ip.to_string()).or_default();

        if let Some(until) = entry.cooldown_until {
            if until > now {
                return Decision::Throttle(until.duration_since(now).as_secs().max(1));
            }
            entry.cooldown_until = None;
        }

        entry
            .l1
            .retain(|at| now.saturating_duration_since(*at) < limits.l1_window);
        entry
            .l2
            .retain(|at| now.saturating_duration_since(*at) < limits.l2_window);
        entry.l1.push(now);
        entry.l2.push(now);

        // 某一级的 limit 为 0 表示该级关闭。
        let over = (limits.l1_limit > 0 && entry.l1.len() as u64 > limits.l1_limit)
            || (limits.l2_limit > 0 && entry.l2.len() as u64 > limits.l2_limit);
        if !over {
            return Decision::Allow;
        }

        entry.cooldown_until = Some(now + limits.cooldown);
        entry.violations.push(now);
        self.decay(entry, now, limits);

        if entry.violations.len() as u64 >= limits.ban_threshold {
            entry.violations.clear();
            return Decision::Ban;
        }
        Decision::Throttle(limits.cooldown.as_secs().max(1))
    }

    /// 违规是否已到封禁阈值。只在调用方已经决定封禁后清空。
    pub fn reset_violations(&self, ip: &str) {
        let mut inner = self.lock();
        if let Some(entry) = inner.get_mut(ip) {
            entry.violations.clear();
            entry.l1.clear();
            entry.l2.clear();
            entry.cooldown_until = None;
        }
    }

    /// 当前某个地址的有效违规数。
    pub fn violations(&self, ip: &str) -> usize {
        let inner = self.lock();
        inner
            .get(ip)
            .map(|entry| entry.violations.len())
            .unwrap_or(0)
    }

    /// 清理过期记录；返回清理掉的地址数。
    pub fn purge(&self, limits: &Limits) -> usize {
        let now = Instant::now();
        let mut inner = self.lock();
        let before = inner.len();
        inner.retain(|_, entry| {
            entry
                .l1
                .retain(|at| now.saturating_duration_since(*at) < limits.l1_window);
            entry
                .l2
                .retain(|at| now.saturating_duration_since(*at) < limits.l2_window);
            entry
                .violations
                .retain(|at| now.saturating_duration_since(*at) < limits.violation_window);
            let cooling = entry
                .cooldown_until
                .map(|until| until > now)
                .unwrap_or(false);
            cooling || !entry.l1.is_empty() || !entry.l2.is_empty() || !entry.violations.is_empty()
        });
        before - inner.len()
    }

    /// 违规衰减：每满一个 `violation_decay` 周期，最旧的一条失效。
    fn decay(&self, entry: &mut Entry, now: Instant, limits: &Limits) {
        entry
            .violations
            .retain(|at| now.saturating_duration_since(*at) < limits.violation_window);
        let decay = limits.violation_decay;
        if decay.is_zero() {
            return;
        }
        let last = entry.last_decay.unwrap_or(now);
        let steps =
            (now.saturating_duration_since(last).as_secs() / decay.as_secs().max(1)) as usize;
        if steps > 0 {
            entry.last_decay = Some(now);
            for _ in 0..steps {
                if entry.violations.is_empty() {
                    break;
                }
                entry.violations.remove(0);
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            l1_window: Duration::from_secs(30),
            l1_limit: 30,
            l2_window: Duration::from_secs(30),
            l2_limit: 3,
            cooldown: Duration::from_millis(1),
            violation_window: Duration::from_secs(300),
            violation_decay: Duration::from_secs(1800),
            ban_threshold: 3,
        }
    }

    #[test]
    fn burst_limit_trips_and_cools_down() {
        let limiter = RateLimiter::new();
        let limits = limits();
        for _ in 0..3 {
            assert_eq!(limiter.check("1.1.1.1", &limits), Decision::Allow);
        }
        assert_eq!(limiter.check("1.1.1.1", &limits), Decision::Throttle(1));
        // 冷却中继续拦。
        assert_eq!(limiter.check("1.1.1.1", &limits), Decision::Throttle(1));
    }

    #[test]
    fn sustained_limit_trips_even_without_burst() {
        let mut limits = limits();
        limits.l2_limit = 100;
        limits.l1_limit = 2;
        let limiter = RateLimiter::new();
        assert_eq!(limiter.check("1.1.1.1", &limits), Decision::Allow);
        assert_eq!(limiter.check("1.1.1.1", &limits), Decision::Allow);
        assert!(matches!(
            limiter.check("1.1.1.1", &limits),
            Decision::Throttle(_)
        ));
    }

    #[test]
    fn violations_escalate_to_ban() {
        let mut limits = limits();
        limits.cooldown = Duration::ZERO;
        limits.l2_limit = 1;
        let limiter = RateLimiter::new();
        // 第 1 次放行，之后每次超限 → 违规 +1。第三次违规到阈值。
        assert_eq!(limiter.check("1.1.1.1", &limits), Decision::Allow);
        assert!(matches!(
            limiter.check("1.1.1.1", &limits),
            Decision::Throttle(_)
        ));
        assert!(matches!(
            limiter.check("1.1.1.1", &limits),
            Decision::Throttle(_)
        ));
        assert_eq!(limiter.check("1.1.1.1", &limits), Decision::Ban);
        // 封禁由调用方执行，之后计数清零。
        limiter.reset_violations("1.1.1.1");
        assert_eq!(limiter.violations("1.1.1.1"), 0);
    }

    #[test]
    fn addresses_are_independent() {
        let limiter = RateLimiter::new();
        let limits = limits();
        for _ in 0..4 {
            let _ = limiter.check("1.1.1.1", &limits);
        }
        assert_eq!(limiter.check("2.2.2.2", &limits), Decision::Allow);
    }

    #[test]
    fn unknown_ip_is_not_tracked() {
        let limiter = RateLimiter::new();
        let limits = limits();
        for _ in 0..10 {
            assert_eq!(limiter.check("unknown", &limits), Decision::Allow);
        }
        assert_eq!(limiter.check("", &limits), Decision::Allow);
    }

    #[test]
    fn purge_drops_idle_entries() {
        let mut limits = limits();
        limits.cooldown = Duration::ZERO;
        limits.l2_limit = 1;
        limits.violation_window = Duration::from_millis(1);
        limits.l1_window = Duration::from_millis(1);
        limits.l2_window = Duration::from_millis(1);
        let limiter = RateLimiter::new();
        let _ = limiter.check("1.1.1.1", &limits);
        let _ = limiter.check("1.1.1.1", &limits);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(limiter.purge(&limits), 1);
    }
}
