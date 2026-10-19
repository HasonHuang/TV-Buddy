//! 共享状态。
//!
//! 播放路径**只读**配置快照。热重载用 `ArcSwap` 原子替换（ADR-02）。

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;

use crate::{auth::SessionStore, config::Config};

pub struct AppState {
    /// 配置文件路径（保存与重载用）。
    pub config_path: PathBuf,
    /// 当前配置快照。
    pub config: ArcSwap<Config>,
    /// 面板会话（内存，重启失效）。
    pub sessions: SessionStore,
    /// 进程启动时刻。
    pub started: Instant,
}

impl AppState {
    pub fn new(config_path: PathBuf, config: Config) -> Self {
        let ttl = Duration::from_secs(config.panel.session_ttl.max(60));
        Self {
            config_path,
            config: ArcSwap::from_pointee(config),
            sessions: SessionStore::new(ttl),
            started: Instant::now(),
        }
    }

    /// 取当前配置快照。
    pub fn snapshot(&self) -> Arc<Config> {
        self.config.load_full()
    }

    /// 原子替换配置。
    pub fn replace(&self, config: Config) {
        self.config.store(Arc::new(config));
    }

    pub fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}
