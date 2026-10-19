//! 看门狗：连续失败计数与冷却（S-MODE-17）。
//!
//! 规则：同一频道连续失败 5 次 → 冷却 3600 s；配置重载**立即解除**冷却。
//! 冷却期内不再请求上游，直接返回相同的 503 文案。

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// 触发冷却的连续失败次数。
const MAX_FAILURES: u32 = 5;
/// 冷却时长。
const COOLDOWN: Duration = Duration::from_secs(3600);

#[derive(Debug, Default, Clone)]
struct Entry {
    failures: u32,
    cooling_until: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct Watchdog {
    inner: Mutex<HashMap<String, Entry>>,
}

impl Watchdog {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否处于冷却期。
    pub fn is_cooling(&self, key: &str) -> bool {
        let mut map = self.lock();
        match map.get_mut(key).and_then(|entry| entry.cooling_until) {
            Some(until) if until > Instant::now() => true,
            Some(_) => {
                // 冷却到期：复位。
                map.insert(key.to_string(), Entry::default());
                false
            }
            None => false,
        }
    }

    /// 记一次失败；返回当前是否已进入冷却。
    pub fn record_failure(&self, key: &str) -> bool {
        let mut map = self.lock();
        let entry = map.entry(key.to_string()).or_default();
        entry.failures += 1;
        if entry.failures >= MAX_FAILURES {
            entry.cooling_until = Some(Instant::now() + COOLDOWN);
            true
        } else {
            false
        }
    }

    /// 记一次成功：清零计数与冷却。
    pub fn record_success(&self, key: &str) {
        self.lock().remove(key);
    }

    /// 清空全部状态（配置重载时调用）。
    pub fn clear_all(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_after_five_failures() {
        let watchdog = Watchdog::new();
        for _ in 0..MAX_FAILURES - 1 {
            assert!(!watchdog.record_failure("g/c"));
        }
        assert!(watchdog.record_failure("g/c"), "第 5 次失败进入冷却");
        assert!(watchdog.is_cooling("g/c"));
        watchdog.clear_all();
        assert!(!watchdog.is_cooling("g/c"), "配置重载立即解除冷却");
    }

    #[test]
    fn success_resets_counter() {
        let watchdog = Watchdog::new();
        for _ in 0..MAX_FAILURES - 1 {
            watchdog.record_failure("g/c");
        }
        watchdog.record_success("g/c");
        assert!(!watchdog.record_failure("g/c"));
    }
}
