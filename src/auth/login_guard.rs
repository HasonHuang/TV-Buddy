//! 面板登录失败锁。
//!
//! 规则见 F-010（`docs/findings/dynamic/11-security-and-metrics.md` §4）：
//! 窗口内失败数达到 `free + threshold` 时，后续 `/panel/api/login` 返回 `429`。
//! 锁不波及既有 Bearer 会话。首次锁定时长固定 300 秒。

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// 锁定期间 `/login` 的等待秒数。
pub const LOCK_SECS: u64 = 300;
/// 记录容量上限。
const CAPACITY: usize = 10_000;

#[derive(Debug, Default)]
struct Entry {
    failures: Vec<Instant>,
    locked_until: Option<Instant>,
    /// 历次锁定的时刻，用于升级阶梯。
    locks: Vec<Instant>,
}

/// 按 IP 记录登录失败。
#[derive(Debug, Default)]
pub struct LoginGuard {
    inner: Mutex<HashMap<String, Entry>>,
}

impl LoginGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登录前检查。达到阈值时立即锁定并返回剩余秒数。
    ///
    /// 返回值 `None` 表示可以继续校验口令。锁定时长固定为 `lock`。
    pub fn check(
        &self,
        ip: &str,
        window: Duration,
        free: u64,
        threshold: u64,
        lock: Duration,
    ) -> Option<u64> {
        self.check_escalating(
            ip,
            window,
            free,
            threshold,
            lock,
            lock,
            lock,
            Duration::ZERO,
        )
    }

    /// 登录前检查，锁定时长按阶梯升级。
    ///
    /// 同一地址在 `escalate_window` 内重复触发锁定时：
    /// 第 1 次用 `first`，第 2 次用 `second`，第 3 次及以后用 `repeat`。
    /// `escalate_window` 为零时每次都按第 1 次处理。
    #[allow(clippy::too_many_arguments)]
    pub fn check_escalating(
        &self,
        ip: &str,
        window: Duration,
        free: u64,
        threshold: u64,
        first: Duration,
        second: Duration,
        repeat: Duration,
        escalate_window: Duration,
    ) -> Option<u64> {
        let mut inner = self.lock();
        let entry = inner.entry(ip.to_string()).or_default();
        let now = Instant::now();
        if let Some(until) = entry.locked_until {
            if until > now {
                return Some(until.duration_since(now).as_secs().max(1));
            }
            entry.locked_until = None;
            entry.failures.clear();
        }
        entry.failures.retain(|at| now.duration_since(*at) < window);
        if entry.failures.len() as u64 >= free.saturating_add(threshold) {
            let duration = if escalate_window.is_zero() {
                first
            } else {
                entry
                    .locks
                    .retain(|at| now.duration_since(*at) < escalate_window);
                match entry.locks.len() {
                    0 => first,
                    1 => second,
                    _ => repeat,
                }
            };
            entry.locks.push(now);
            entry.locked_until = Some(now + duration);
            entry.failures.clear();
            return Some(duration.as_secs().max(1));
        }
        None
    }

    /// 记一次失败。
    pub fn failure(&self, ip: &str, window: Duration) {
        let mut inner = self.lock();
        if !inner.contains_key(ip) && inner.len() >= CAPACITY {
            return;
        }
        let entry = inner.entry(ip.to_string()).or_default();
        let now = Instant::now();
        entry.failures.retain(|at| now.duration_since(*at) < window);
        entry.failures.push(now);
    }

    /// 登录成功：清空失败计数。锁已生效时不清锁（与目标一致）。
    pub fn success(&self, ip: &str) {
        let mut inner = self.lock();
        if let Some(entry) = inner.get_mut(ip) {
            entry.failures.clear();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Duration {
        Duration::from_secs(60)
    }

    #[test]
    fn locks_after_free_plus_threshold_failures() {
        let guard = LoginGuard::new();
        for _ in 0..5 {
            assert_eq!(
                guard.check("1.2.3.4", window(), 2, 3, Duration::from_secs(300)),
                None
            );
            guard.failure("1.2.3.4", window());
        }
        let remaining = guard
            .check("1.2.3.4", window(), 2, 3, Duration::from_secs(300))
            .expect("应锁定");
        assert_eq!(remaining, 300);
    }

    #[test]
    fn lock_survives_correct_password() {
        let guard = LoginGuard::new();
        for _ in 0..5 {
            let _ = guard.check("1.2.3.4", window(), 2, 3, Duration::from_secs(300));
            guard.failure("1.2.3.4", window());
        }
        assert!(guard
            .check("1.2.3.4", window(), 2, 3, Duration::from_secs(300))
            .is_some());
        guard.success("1.2.3.4");
        assert!(guard
            .check("1.2.3.4", window(), 2, 3, Duration::from_secs(300))
            .is_some());
    }

    #[test]
    fn failures_expire_with_window() {
        let guard = LoginGuard::new();
        guard.failure("1.2.3.4", Duration::ZERO);
        assert_eq!(
            guard.check("1.2.3.4", Duration::ZERO, 0, 1, Duration::from_secs(300)),
            None
        );
    }
}
