//! 共享状态。
//!
//! 播放路径**只读**配置快照。热重载用 `ArcSwap` 原子替换（ADR-02）。
//! 配置改动走 [`AppState::mutate`]：以磁盘为准重读 → 改 → 校验落盘 → 替换快照。

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;

use crate::{
    auth::{PlayTokenStore, SessionStore},
    config::Config,
};

pub struct AppState {
    /// 配置文件路径（保存与重载用）。
    pub config_path: PathBuf,
    /// 当前配置快照。
    pub config: ArcSwap<Config>,
    /// 面板会话（内存，重启失效）。
    pub sessions: SessionStore,
    /// 播放 Token（内存，重启失效）。
    pub play_tokens: PlayTokenStore,
    /// 进程启动时刻。
    pub started: Instant,
    /// 串行化配置改动。
    write_lock: Mutex<()>,
}

impl AppState {
    pub fn new(config_path: PathBuf, config: Config) -> Self {
        let ttl = Duration::from_secs(config.panel.session_ttl.max(60));
        Self {
            config_path,
            config: ArcSwap::from_pointee(config),
            sessions: SessionStore::new(ttl),
            play_tokens: PlayTokenStore::new(),
            started: Instant::now(),
            write_lock: Mutex::new(()),
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

    /// 改配置：读盘 → 改 → 落盘 → 替换快照。
    ///
    /// 以磁盘为唯一事实源，避免内存态覆盖外部直改（`04-config-and-storage.md` §2）。
    pub fn mutate<F>(&self, change: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut Config) -> anyhow::Result<()>,
    {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut config = match crate::config::load(&self.config_path) {
            Ok(from_disk) => from_disk,
            Err(err) => {
                tracing::warn!(error = %err, "重读配置失败，改用内存快照");
                (*self.snapshot()).clone()
            }
        };
        change(&mut config)?;
        crate::config::save(&self.config_path, &config)?;
        self.replace(config);
        Ok(())
    }

    pub fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}
