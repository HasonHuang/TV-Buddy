//! 播放 Token。
//!
//! 目标用 48 位十六进制、全局稳定、TTL 24 小时、轮换后旧值宽限约 1 小时（S-AUTH-02/03/09）。
//! 本实现把 Token 放内存：进程重启即换新（与面板会话一致）。

use std::{
    sync::RwLock,
    time::{Duration, Instant},
};

use uuid::Uuid;

/// 旧 Token 宽限时长（S-AUTH-09）。
const GRACE: Duration = Duration::from_secs(3600);

#[derive(Debug)]
struct Inner {
    current: String,
    /// 当前 Token 的签发时刻。用于算 `remaining_secs`。
    created: Instant,
    old: Option<(String, Instant)>,
}

pub struct PlayTokenStore {
    inner: RwLock<Inner>,
}

impl Default for PlayTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayTokenStore {
    /// 启动时生成首个 Token。
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                current: generate(),
                created: Instant::now(),
                old: None,
            }),
        }
    }

    /// 当前 Token。
    pub fn current(&self) -> String {
        self.read().current.clone()
    }

    /// 校验 Token：当前值或宽限期内的旧值都算有效。M2 的播放链路使用。
    #[allow(dead_code)]
    pub fn verify(&self, token: &str) -> bool {
        let inner = self.read();
        if inner.current == token {
            return true;
        }
        match &inner.old {
            Some((old, at)) => old == token && at.elapsed() < GRACE,
            None => false,
        }
    }

    /// 轮换：旧值转入宽限，返回新值。供 P1 的 Token 管理接口使用。
    #[allow(dead_code)]
    pub fn rotate(&self) -> String {
        let mut inner = self.write();
        let fresh = generate();
        inner.old = Some((inner.current.clone(), Instant::now()));
        inner.current = fresh.clone();
        inner.created = Instant::now();
        fresh
    }

    /// 采用对端的 Token（多 VPS 同步）。旧值只在新值不同时保留宽限。
    pub fn adopt(&self, current: String, old: Option<String>) -> bool {
        let mut inner = self.write();
        if inner.current == current {
            return false;
        }
        inner.old = old
            .filter(|value| !value.is_empty() && *value != current)
            .map(|value| (value, Instant::now()))
            .or_else(|| Some((inner.current.clone(), Instant::now())));
        inner.current = current;
        inner.created = Instant::now();
        true
    }

    /// 只读快照：`(current, old)`。供 P1 的 Token 管理接口使用。
    #[allow(dead_code)]
    pub fn snapshot(&self) -> (String, Option<String>) {
        let inner = self.read();
        (
            inner.current.clone(),
            inner.old.as_ref().map(|(value, _)| value.clone()),
        )
    }

    /// 当前与旧值的剩余有效期（秒）。旧值按 1 小时宽限算。
    pub fn remaining(&self, ttl_hours: u64) -> (u64, Option<u64>) {
        let inner = self.read();
        let ttl = ttl_hours.max(1) * 3600;
        let current = ttl.saturating_sub(inner.created.elapsed().as_secs());
        let old = inner
            .old
            .as_ref()
            .map(|(_, at)| 3600u64.saturating_sub(at.elapsed().as_secs()));
        (current, old)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// 生成 48 位十六进制 Token。
fn generate() -> String {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(first.as_bytes());
    bytes.extend_from_slice(second.as_bytes());
    hex::encode(&bytes[..24])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopt_switches_token() {
        let store = PlayTokenStore::new();
        let before = store.current();
        assert!(store.adopt("a".repeat(48), None));
        assert!(!store.adopt("a".repeat(48), None));
        assert_eq!(store.current(), "a".repeat(48));
        let (current, old) = store.snapshot();
        assert_eq!(current, "a".repeat(48));
        assert_eq!(old.as_deref(), Some(before.as_str()));
        assert!(store.verify(&before), "旧值在宽限期内仍有效");
    }

    #[test]
    fn token_is_48_hex() {
        let store = PlayTokenStore::new();
        let token = store.current();
        assert_eq!(token.len(), 48);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(store.verify(&token));
    }

    #[test]
    fn rotate_keeps_old_token_in_grace() {
        let store = PlayTokenStore::new();
        let before = store.current();
        let after = store.rotate();
        assert_ne!(before, after);
        assert!(store.verify(&before), "旧 Token 在宽限期内仍有效");
        assert!(store.verify(&after));
        assert!(!store.verify("deadbeef"));
    }
}
