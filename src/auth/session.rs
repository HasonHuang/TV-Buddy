//! 面板会话存储。
//!
//! 会话存在内存。进程重启即失效——与目标一致（F-004）。
//! 登录成功签发 UUID v4（S-AUTH-01）。会话绑定用户，供权限判定使用。

use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};

use uuid::Uuid;

/// 会话里保存的身份信息。
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub user_id: String,
    pub username: String,
    pub is_admin: bool,
}

struct Entry {
    seen: Instant,
    info: SessionInfo,
}

pub struct SessionStore {
    ttl: Duration,
    sessions: RwLock<HashMap<String, Entry>>,
}

impl SessionStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// 签发新会话。同时清掉过期会话。
    pub fn issue(&self, user_id: &str, username: &str, is_admin: bool) -> String {
        let token = Uuid::new_v4().to_string();
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, entry| entry.seen.elapsed() < self.ttl);
        map.insert(
            token.clone(),
            Entry {
                seen: Instant::now(),
                info: SessionInfo {
                    user_id: user_id.to_string(),
                    username: username.to_string(),
                    is_admin,
                },
            },
        );
        token
    }

    /// 校验会话。命中则续期。
    pub fn verify(&self, token: &str) -> bool {
        self.lookup(token).is_some()
    }

    /// 校验会话并返回身份。命中则续期。
    pub fn lookup(&self, token: &str) -> Option<SessionInfo> {
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        match map.get(token).map(|entry| entry.seen.elapsed()) {
            Some(elapsed) if elapsed < self.ttl => {
                if let Some(entry) = map.get_mut(token) {
                    entry.seen = Instant::now();
                    return Some(entry.info.clone());
                }
                None
            }
            Some(_) => {
                map.remove(token);
                None
            }
            None => None,
        }
    }

    /// 注销会话。
    pub fn revoke(&self, token: &str) {
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        map.remove(token);
    }

    /// 踢掉某个用户的全部会话。删除或改密后调用。
    pub fn revoke_user(&self, user_id: &str) {
        let mut map = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, entry| entry.info.user_id != user_id);
    }

    /// 当前会话数。
    pub fn len(&self) -> usize {
        self.sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_session_carries_identity() {
        let store = SessionStore::new(Duration::from_secs(60));
        let token = store.issue("u1", "alice", false);
        let info = store.lookup(&token).expect("会话");
        assert_eq!(info.user_id, "u1");
        assert_eq!(info.username, "alice");
        assert!(!info.is_admin);
    }

    #[test]
    fn revoking_a_user_drops_all_their_sessions() {
        let store = SessionStore::new(Duration::from_secs(60));
        let first = store.issue("u1", "alice", false);
        let second = store.issue("u1", "alice", false);
        let other = store.issue("u2", "bob", true);
        store.revoke_user("u1");
        assert!(store.lookup(&first).is_none());
        assert!(store.lookup(&second).is_none());
        assert!(store.lookup(&other).is_some());
    }

    #[test]
    fn expired_sessions_are_dropped() {
        let store = SessionStore::new(Duration::from_millis(1));
        let token = store.issue("u1", "alice", false);
        std::thread::sleep(Duration::from_millis(10));
        assert!(store.lookup(&token).is_none());
    }
}
