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
    logging::RingBuffer,
    proxy::{AddressCache, SegmentIndex, TtlCache},
    stats::Stats,
    watchdog::Watchdog,
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
    /// 上游请求客户端。
    pub http: reqwest::Client,
    /// 切片索引。
    pub segments: SegmentIndex,
    /// 在线清单缓存（TTL 5 秒）。
    pub playlists: TtlCache,
    /// 直连地址与模式分类缓存（每项自带 TTL）。
    pub modes: AddressCache,
    /// 缓存根目录（`[cache].disk_path`）。
    pub cache_dir: PathBuf,
    /// 运行指标。
    pub stats: Stats,
    /// 看门狗状态（冷却）。
    pub watchdog: Watchdog,
    /// 最近日志行。
    pub logs: Arc<RingBuffer>,
    /// 进程启动时刻。
    pub started: Instant,
    /// 串行化配置改动。
    write_lock: Mutex<()>,
}

impl AppState {
    pub fn new(config_path: PathBuf, config: Config, logs: Arc<RingBuffer>) -> Self {
        let ttl = Duration::from_secs(config.panel.session_ttl.max(60));
        let cache_dir = PathBuf::from(&config.cache.disk_path);
        ensure_cache_dirs(&cache_dir);
        Self {
            config_path,
            config: ArcSwap::from_pointee(config),
            sessions: SessionStore::new(ttl),
            play_tokens: PlayTokenStore::new(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            segments: SegmentIndex::new(),
            playlists: TtlCache::new(Duration::from_secs(5)),
            modes: AddressCache::new(),
            cache_dir,
            stats: Stats::default(),
            watchdog: Watchdog::new(),
            logs,
            started: Instant::now(),
            write_lock: Mutex::new(()),
        }
    }

    /// 取当前配置快照。
    pub fn snapshot(&self) -> Arc<Config> {
        self.config.load_full()
    }

    /// 原子替换配置。同时解除全部冷却（S-MODE-17）。
    pub fn replace(&self, config: Config) {
        self.watchdog.clear_all();
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

    /// 在线清单缓存条目数。
    pub fn playlist_cache_entries(&self) -> usize {
        self.playlists.len()
    }
}

/// 建立缓存子目录。失败只记日志（后续写缓存会再报错）。
fn ensure_cache_dirs(root: &std::path::Path) {
    for name in ["m3u8", "seg_url", "seg_bytes", "sub_url", "dash"] {
        let dir = root.join(name);
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), error = %err, "创建缓存目录失败");
        }
    }
}
