//! 共享状态。
//!
//! 播放路径**只读**配置快照。热重载用 `ArcSwap` 原子替换（ADR-02）。
//! 配置改动走 [`AppState::mutate`]：以数据库为准重读 → 改 → 校验落库 → 替换快照。

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;

use crate::{
    auth::SessionStore,
    config::Config,
    logging::RingBuffer,
    proxy::{segments, AddressCache, SegmentIndex, TtlCache},
    stats::Stats,
    watchdog::Watchdog,
};

/// 上游请求总超时。
///
/// v0.8.2 实测：慢速正文在 **25.0 秒** 中断；中断后仍以 200 返回已收到的部分字节。
const UPSTREAM_TIMEOUT_SECS: u64 = 25;

/// 目标默认上游 User-Agent（F-010 §3.2）。
pub(crate) const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub struct AppState {
    /// 数据库文件路径（备份恢复时另存旁路文件用）。
    pub db_path: PathBuf,
    /// 配置仓库（SQLite 单行文档）。
    pub config_store: crate::config::ConfigStore,
    /// 当前配置快照。
    pub config: ArcSwap<Config>,
    /// 面板会话（内存，重启失效）。
    pub sessions: SessionStore,
    /// 用户、角色与访问令牌（SQLite，跨重启保留）。
    pub users: crate::users::UserStore,
    /// 上游请求客户端。
    pub http: reqwest::Client,
    /// 切片索引。
    pub segments: SegmentIndex,
    /// `[cache].backend = "memory"` 时的切片正文字典。
    pub memory: Arc<crate::cache_memory::MemoryStore>,
    /// 在线清单缓存（TTL 5 秒）。
    pub playlists: TtlCache,
    /// 订阅聚合源缓存（正文 + 状态，跨重启保留）。每个源按有效 `ttl_secs` 判定。
    pub sources: crate::source_cache::SourceCache,
    /// 上游解析脚本结果缓存（SWR）。
    pub resolver: crate::resolver::Cache,
    /// Remux 会话。
    pub remux: Arc<crate::remux::Manager>,
    /// DASH→HLS 会话与切片索引。
    pub dash_hls: Arc<crate::dash_hls::Manager>,
    /// DASH 时移录制会话。
    pub dash_timeshift: Arc<crate::dash_timeshift::Manager>,
    /// 定时录制任务。
    pub schedule: Arc<crate::schedule::Store>,
    /// 直播回看会话。
    pub timeshift: Arc<crate::timeshift::Manager>,
    /// 直连地址与模式分类缓存（每项自带 TTL）。
    pub modes: AddressCache,
    /// 缓存根目录（`[cache].disk_path`）。
    pub cache_dir: PathBuf,
    /// 运行指标。
    pub stats: Stats,
    /// 出网字节、在线观众与资源占用。
    pub metrics: Arc<crate::metrics::Metrics>,
    /// 看门狗状态（冷却）。
    pub watchdog: Watchdog,
    /// 逐频道上游健康探测结果。
    pub health: crate::health::Health,
    /// 节目单状态。
    pub epg: crate::epg::EpgState,
    /// IP 统计与封禁。
    pub ip_guard: crate::auth::IpGuard,
    /// 每 IP 播放会话上限与空闲超时。
    pub play_sessions: crate::auth::SessionGuard,
    /// 播放入口的两级限流与违规升级。
    pub rate_limiter: crate::auth::RateLimiter,
    /// 面板登录失败锁。
    pub login_guard: crate::auth::LoginGuard,
    /// 自助解封配额。
    pub unban_quota: crate::auth::UnbanQuota,
    /// 被动实测统计（推荐 TTL 与磁盘上限）。
    pub estimate: crate::cache_estimate::Estimator,
    /// 按代理配置缓存的 HTTP 客户端。
    proxy_clients: std::sync::Mutex<std::collections::HashMap<String, reqwest::Client>>,
    /// 最近日志行。
    pub logs: Arc<RingBuffer>,
    /// 进程启动时刻。
    pub started: Instant,
    /// 串行化配置改动。
    write_lock: Mutex<()>,
}

impl AppState {
    pub fn new(db_path: PathBuf, config: Config, logs: Arc<RingBuffer>) -> anyhow::Result<Self> {
        let ttl = Duration::from_secs(config.panel.session_ttl.max(60));
        let cache_dir = PathBuf::from(&config.cache.disk_path);
        let conn = crate::db::connect(&db_path)?;
        let config_store = crate::config::ConfigStore::new(conn.clone());
        // 库为空：把调用方给的配置写进去，作为初始状态。
        if config_store.load()?.is_none() {
            config_store.save(&config)?;
        }
        let users = crate::users::UserStore::attach(conn)?;
        let offset_hours = config.server.timezone_offset_hours as i64;
        ensure_cache_dirs(&cache_dir);
        let segments = SegmentIndex::new();
        segments.set_limits(
            segments::global_limit(config.cache.segments.global_max, config.cache.memory_max_mb),
            config.cache.segments.per_channel_max,
        );
        let sources_ttl = Duration::from_secs(config.cache.ttl.sub_url_secs.max(1));
        let estimate = crate::cache_estimate::Estimator::load_or_default(&cache_dir);
        estimate.set_auto_tune(config.cdn.auto_tune);
        Ok(Self {
            db_path,
            config_store,
            config: ArcSwap::from_pointee(config),
            sessions: SessionStore::new(ttl),
            users,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(UPSTREAM_TIMEOUT_SECS))
                .user_agent(DEFAULT_USER_AGENT)
                .build()
                .unwrap_or_default(),
            segments,
            memory: Arc::new(crate::cache_memory::MemoryStore::new()),
            playlists: TtlCache::new(Duration::from_secs(5)),
            sources: crate::source_cache::SourceCache::load(&cache_dir, sources_ttl),
            resolver: crate::resolver::Cache::default(),
            remux: Arc::new(crate::remux::Manager::default()),
            dash_hls: Arc::new(crate::dash_hls::Manager::new()),
            dash_timeshift: Arc::new(crate::dash_timeshift::Manager::new()),
            schedule: Arc::new(crate::schedule::Store::load(&cache_dir, offset_hours)),
            timeshift: Arc::new(crate::timeshift::Manager::default()),
            modes: AddressCache::new(),
            epg: crate::epg::EpgState::load(&cache_dir),
            cache_dir,
            stats: Stats::default(),
            metrics: Arc::new(crate::metrics::Metrics::new()),
            watchdog: Watchdog::new(),
            health: crate::health::Health::new(),
            ip_guard: crate::auth::IpGuard::new(),
            play_sessions: crate::auth::SessionGuard::new(),
            rate_limiter: crate::auth::RateLimiter::new(),
            login_guard: crate::auth::LoginGuard::new(),
            unban_quota: crate::auth::UnbanQuota::new(),
            estimate,
            proxy_clients: std::sync::Mutex::new(std::collections::HashMap::new()),
            logs,
            started: Instant::now(),
            write_lock: Mutex::new(()),
        })
    }

    /// 取当前配置快照。
    pub fn snapshot(&self) -> Arc<Config> {
        self.config.load_full()
    }

    /// 选择频道上游客户端。
    ///
    /// 频道级代理优先于分组级。目标在代理不可用时**不回退**到分组代理或直连；
    /// 这里只对“未配置代理”或代理配置无效（无法构造客户端）回退直连。
    pub fn http_for(&self, gid: &str, cid: &str) -> reqwest::Client {
        let config = self.snapshot();
        let Some(proxy) = config.proxy_for(gid, cid) else {
            return self.http.clone();
        };
        let Some(addr) = proxy.url() else {
            tracing::warn!(kind = %proxy.kind, "代理地址为空；改用直连");
            return self.http.clone();
        };
        let user = proxy.username().unwrap_or("");
        let pass = proxy.password().unwrap_or("");
        let key = format!("{addr}|{user}|{pass}");
        {
            let cache = self
                .proxy_clients
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(client) = cache.get(&key) {
                return client.clone();
            }
        }
        let built = reqwest::Proxy::all(&addr).and_then(|mut candidate| {
            if !user.is_empty() {
                candidate = candidate.basic_auth(user, pass);
            }
            reqwest::Client::builder()
                .timeout(Duration::from_secs(UPSTREAM_TIMEOUT_SECS))
                .user_agent(DEFAULT_USER_AGENT)
                .proxy(candidate)
                .build()
        });
        match built {
            Ok(client) => {
                let mut cache = self
                    .proxy_clients
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                cache.insert(key, client.clone());
                client
            }
            Err(error) => {
                tracing::warn!(%error, "代理客户端创建失败；改用直连");
                self.http.clone()
            }
        }
    }

    /// 原子替换配置。同时解除全部冷却（S-MODE-17）。
    pub fn replace(&self, config: Config) {
        self.watchdog.clear_all();
        self.segments.set_limits(
            segments::global_limit(config.cache.segments.global_max, config.cache.memory_max_mb),
            config.cache.segments.per_channel_max,
        );
        self.sources
            .set_ttl(Duration::from_secs(config.cache.ttl.sub_url_secs.max(1)));
        self.estimate.set_auto_tune(config.cdn.auto_tune);
        self.config.store(Arc::new(config));
    }

    /// 保存整份配置。与 [`AppState::mutate`] 共用写锁，避免并发丢更新。
    pub fn save_config(&self, config: Config) -> anyhow::Result<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.config_store.save(&config)?;
        self.replace(config);
        Ok(())
    }

    /// 改配置：读库 → 改 → 落库 → 替换快照。
    ///
    /// 以数据库为唯一事实源，避免内存态覆盖外部直改
    /// （管理 CLI 也写同一个库）。
    pub fn mutate<F>(&self, change: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut Config) -> anyhow::Result<()>,
    {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        // 记录损坏时直接报错，不能用内存快照覆盖掉它。
        let mut config = match self.config_store.load()? {
            Some(from_db) => from_db,
            None => (*self.snapshot()).clone(),
        };
        change(&mut config)?;
        self.config_store.save(&config)?;
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
