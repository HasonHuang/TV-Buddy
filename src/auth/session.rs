//! 面板会话存储。
//!
//! 会话存在内存。进程重启即失效——与目标一致（F-004）。
//! 登录成功签发 UUID v4（S-AUTH-01）。

use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};

use uuid::Uuid;

pub struct SessionStore {
    ttl: Duration,
    sessions: RwLock<HashMap<String, Instant>>,
}

impl SessionStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// 签发新会话。同时清掉过期会话。
    pub fn issue(&self) -> String {
        let token = Uuid::new_v4().to_string();
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, seen| seen.elapsed() < self.ttl);
        map.insert(token.clone(), Instant::now());
        token
    }

    /// 校验会话。命中则续期。
    pub fn verify(&self, token: &str) -> bool {
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        match map.get(token).map(|seen| seen.elapsed()) {
            Some(elapsed) if elapsed < self.ttl => {
                map.insert(token.to_string(), Instant::now());
                true
            }
            Some(_) => {
                map.remove(token);
                false
            }
            None => false,
        }
    }

    /// 注销会话。
    pub fn revoke(&self, token: &str) {
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        map.remove(token);
    }

    /// 当前会话数。
    pub fn len(&self) -> usize {
        self.sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}
