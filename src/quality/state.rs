//! 质量结果的内存快照与运行状态。

use std::{
    collections::HashMap,
    sync::{Mutex, RwLock},
};

use sha2::{Digest, Sha256};

use crate::quality::rules::Reason;
use crate::quality::store::QualityResult;

/// 规则判定通过。
pub const FLAG_OK: u16 = 1;
/// 本次可达。
pub const FLAG_REACHABLE: u16 = 2;
/// 使用了 Host 测量缓存。
pub const FLAG_CACHED: u16 = 4;
/// 结果超过两个周期未更新。
pub const FLAG_STALE: u16 = 8;

/// 单频道紧凑结果。完整字段留在 SQLite（P2）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub url_hash: u64,
    pub checked_at: i64,
    pub first_fail_at: Option<i64>,
    pub last_ok_at: Option<i64>,
    pub next_check_at: i64,
    pub delay_ms: u32,
    pub speed_kib: u32,
    pub width: u16,
    pub height: u16,
    pub flags: u16,
    pub reason: Reason,
    pub consecutive_fails: u16,
}

impl Entry {
    pub fn ok(&self) -> bool {
        self.flags & FLAG_OK != 0
    }

    pub fn reachable(&self) -> bool {
        self.flags & FLAG_REACHABLE != 0
    }

    /// 从持久化结果恢复内存条目。`url_hash` 由当前频道地址计算。
    pub fn from_result(result: &QualityResult, url_hash: u64) -> Self {
        Self {
            url_hash,
            checked_at: result.checked_at,
            first_fail_at: result.first_fail_at,
            last_ok_at: result.ok.then_some(result.checked_at),
            next_check_at: result.next_check_at,
            delay_ms: result.delay_ms.unwrap_or(0),
            speed_kib: result.speed_kib.unwrap_or(0),
            width: result.width.unwrap_or(0),
            height: result.height.unwrap_or(0),
            flags: if result.ok {
                FLAG_OK | FLAG_REACHABLE
            } else {
                0
            },
            reason: Reason::parse(&result.reason),
            consecutive_fails: 0,
        }
    }
}

/// 一次校验任务。
#[derive(Debug, Clone)]
pub struct RunState {
    pub run_id: String,
    pub trigger: String,
    pub phase: String,
    pub started_at: i64,
    pub total: usize,
    pub tested: usize,
    pub valid: usize,
    pub admitted: usize,
    pub removed: usize,
    pub rejected: usize,
    pub skipped: usize,
    pub hidden: usize,
    pub bytes: u64,
    pub cancelled: bool,
    pub finished: bool,
    pub paused_by_guard: bool,
    pub measure_only: bool,
    pub error: Option<String>,
}

/// 校验范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunScope {
    All,
    Source(String),
    Group(String),
    Channel(String, String),
}

/// 运行模式。P1 一频道一地址，quick 与 full 行为相同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Quick,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    Busy,
    Disabled,
    InvalidScope,
}

/// 内存质量状态。
pub struct QualityState {
    entries: RwLock<HashMap<String, Entry>>,
    run: Mutex<Option<RunState>>,
}

impl Default for QualityState {
    fn default() -> Self {
        Self::new()
    }
}

impl QualityState {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            run: Mutex::new(None),
        }
    }

    fn key(gid: &str, cid: &str) -> String {
        format!("{gid}/{cid}")
    }

    pub fn get(&self, gid: &str, cid: &str) -> Option<Entry> {
        self.entries
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(&Self::key(gid, cid))
            .cloned()
    }

    pub fn put(&self, gid: &str, cid: &str, entry: Entry) {
        self.entries
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .insert(Self::key(gid, cid), entry);
    }

    pub fn forget(&self, gid: &str, cid: &str) {
        self.entries
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&Self::key(gid, cid));
    }

    pub fn forget_group(&self, gid: &str) {
        let prefix = format!("{gid}/");
        self.entries
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|key, _| !key.starts_with(&prefix));
    }

    pub fn snapshot(&self) -> HashMap<String, Entry> {
        self.entries
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn try_start_run(
        &self,
        run_id: &str,
        trigger: &str,
        total: usize,
        measure_only: bool,
    ) -> bool {
        let mut run = self.run.lock().unwrap_or_else(|error| error.into_inner());
        if run.as_ref().is_some_and(|state| !state.finished) {
            return false;
        }
        *run = Some(RunState {
            run_id: run_id.to_string(),
            trigger: trigger.to_string(),
            phase: "queued".into(),
            started_at: crate::schedule::now_epoch(),
            total,
            tested: 0,
            valid: 0,
            admitted: 0,
            removed: 0,
            rejected: 0,
            skipped: 0,
            hidden: 0,
            bytes: 0,
            cancelled: false,
            finished: false,
            paused_by_guard: false,
            measure_only,
            error: None,
        });
        true
    }

    pub fn finish_run(&self) {
        let mut run = self.run.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(state) = run.as_mut() {
            state.finished = true;
        }
    }

    /// 只结束指定任务。任务已被新任务替换时返回 `false`。
    pub fn finish_run_id(&self, run_id: &str) -> bool {
        let mut run = self.run.lock().unwrap_or_else(|error| error.into_inner());
        match run.as_mut() {
            Some(state) if state.run_id == run_id => {
                state.finished = true;
                true
            }
            _ => false,
        }
    }

    pub fn run_status(&self) -> Option<RunState> {
        self.run
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn update_run(&self, update: impl FnOnce(&mut RunState)) {
        let mut run = self.run.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(state) = run.as_mut() {
            update(state);
        }
    }

    pub fn cancel_run(&self) {
        self.update_run(|state| state.cancelled = true);
    }

    pub fn is_cancelled(&self) -> bool {
        self.run
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(|state| state.cancelled)
    }
}

/// 配置地址的稳定哈希。去掉 `$` 描述段。
pub fn url_hash(url: &str) -> u64 {
    let cleaned = url.split('$').next().unwrap_or(url).trim();
    let digest = Sha256::digest(cleaned.as_bytes());
    u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::rules::Reason;

    #[test]
    fn entry_is_updated_and_forgotten() {
        let state = QualityState::new();
        let entry = Entry {
            url_hash: 7,
            checked_at: 100,
            first_fail_at: None,
            last_ok_at: Some(100),
            next_check_at: 200,
            delay_ms: 120,
            speed_kib: 2048,
            width: 1920,
            height: 1080,
            flags: FLAG_OK | FLAG_REACHABLE,
            reason: Reason::Pass,
            consecutive_fails: 0,
        };
        state.put("g", "c", entry.clone());
        assert_eq!(state.get("g", "c").expect("entry").speed_kib, 2048);
        state.forget("g", "c");
        assert!(state.get("g", "c").is_none());
    }

    #[test]
    fn forget_group_keeps_other_groups() {
        let state = QualityState::new();
        state.put("g1", "c1", Entry::default());
        state.put("g2", "c1", Entry::default());
        state.forget_group("g1");
        assert!(state.get("g1", "c1").is_none());
        assert!(state.get("g2", "c1").is_some());
    }

    #[test]
    fn run_lock_allows_one_run_at_a_time() {
        let state = QualityState::new();
        assert!(state.try_start_run("r1", "manual", 10, false));
        assert!(!state.try_start_run("r2", "manual", 10, false));
        state.finish_run();
        assert!(state.try_start_run("r3", "manual", 10, false));
    }

    #[test]
    fn stale_run_cannot_finish_new_run() {
        let state = QualityState::new();
        assert!(state.try_start_run("r1", "manual", 1, false));
        assert!(state.finish_run_id("r1"));
        assert!(state.try_start_run("r2", "manual", 1, false));
        assert!(!state.finish_run_id("r1"), "旧任务不能结束新任务");
        let run = state.run_status().expect("run");
        assert_eq!(run.run_id, "r2");
        assert!(!run.finished);
    }

    #[test]
    fn entry_from_result_marks_hidden_failure() {
        let result = crate::quality::store::QualityResult {
            action: "hidden".into(),
            reason: "unreachable".into(),
            ok: false,
            delay_ms: Some(120),
            speed_kib: Some(2048),
            width: Some(1920),
            height: Some(1080),
            checked_at: 100,
            first_fail_at: Some(90),
            next_check_at: 400,
            ..Default::default()
        };
        let entry = Entry::from_result(&result, 7);
        assert!(!entry.ok());
        assert_eq!(entry.reason, Reason::Unreachable);
        assert_eq!(entry.url_hash, 7);
        assert_eq!(entry.checked_at, 100);
        assert_eq!(entry.first_fail_at, Some(90));
        assert_eq!(entry.next_check_at, 400);
        assert_eq!(entry.delay_ms, 120);
        assert_eq!(entry.speed_kib, 2048);
        assert_eq!(entry.width, 1920);
        assert_eq!(entry.height, 1080);
    }

    #[test]
    fn url_hash_ignores_dollar_suffix() {
        assert_eq!(
            url_hash("http://example.com/live.m3u8$1080p"),
            url_hash("http://example.com/live.m3u8")
        );
        assert_ne!(
            url_hash("http://example.com/a.m3u8"),
            url_hash("http://example.com/b.m3u8")
        );
    }
}
