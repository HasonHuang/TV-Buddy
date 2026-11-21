//! 共享状态。
//!
//! 播放路径**只读**配置快照。热重载用 `ArcSwap` 原子替换（ADR-02）。
//! 配置改动走 [`AppState::mutate`]：以数据库为准重读 → 改 → 校验落库 → 替换快照。

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;

use crate::{
    auth::SessionStore,
    config::Config,
    logging::RingBuffer,
    proxy::{AddressCache, SegmentIndex, TtlCache},
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
    /// 进程内共用的数据库连接。
    pub conn: crate::db::SharedConn,
    /// 配置仓库（SQLite 单行文档）。
    pub config_store: crate::config::ConfigStore,
    /// 质量结果仓库。
    pub quality_store: crate::quality::store::QualityStore,
    /// 当前配置快照。
    pub config: ArcSwap<Config>,
    /// 配置里的频道键 `gid/cid`。统计中间件用它做 O(1) 校验，
    /// 避免随机 cid 每次都扫描分组频道列表。
    channel_keys: ArcSwap<HashSet<String>>,
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
    /// 面板 ffmpeg 安装任务。
    pub ffmpeg_install: Arc<crate::tools::install::Installer>,
    /// ffprobe 可用性缓存。`None` 表示尚未检测。
    pub ffprobe_ready: tokio::sync::RwLock<Option<bool>>,
    /// 运行指标。
    pub stats: Stats,
    /// 出网字节、在线观众与资源占用。
    pub metrics: Arc<crate::metrics::Metrics>,
    /// 看门狗状态（冷却）。
    pub watchdog: Watchdog,
    /// 逐频道上游健康探测结果。
    pub health: crate::health::Health,
    /// 可用性校验结果与运行状态。
    pub quality: crate::quality::state::QualityState,
    /// 节目单状态。
    pub epg: crate::epg::EpgState,
    /// EPG 源、绑定与关键字规则（SQLite）。
    pub epg_store: crate::epg_store::Store,
    /// 指标历史（SQLite）。
    pub history: crate::metrics_store::Store,
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
    /// 质量探测客户端。重定向上限更小。
    probe_clients: std::sync::Mutex<std::collections::HashMap<String, reqwest::Client>>,
    /// 同一时间只跑一次订阅同步。
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
    /// 质量准入与同步门控串行化。
    pub admission_lock: Arc<tokio::sync::Mutex<()>>,
    /// 已排队或正在执行的手动同步源 id。同一源不重复排队。
    pub sync_pending: Mutex<std::collections::HashSet<String>>,
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
        // 启用了切片签名但没有密钥：生成一个并落库，避免每次重启都换密钥。
        if crate::sign::needs_secret(&config) {
            config_store.save(&config)?;
        }
        // `save` 可能补了密钥；以库里的版本进快照。
        let config = config_store.load()?.unwrap_or(config);
        if !config.auth.token_enabled {
            tracing::warn!("auth.token_enabled=false：播放地址不校验令牌。建议在面板安全页开启。");
        }
        let users = crate::users::UserStore::attach(conn.clone())?;
        let history = crate::metrics_store::Store::new(conn.clone())?;
        let quality_store = crate::quality::store::QualityStore::new(conn.clone());
        let epg_store = crate::epg_store::Store::attach(conn.clone())?;
        let epg = crate::epg::EpgState::load(&cache_dir);
        let offset_hours = config.server.timezone_offset_hours as i64;
        ensure_cache_dirs(&cache_dir);
        let segments = SegmentIndex::new();
        segments.set_limits(config.cache.segments.per_channel_max);
        let sources_ttl = Duration::from_secs(config.cache.ttl.sub_url_secs.max(1));
        let estimate = crate::cache_estimate::Estimator::load_or_default(&cache_dir);
        estimate.set_auto_tune(config.cdn.auto_tune);
        let channel_keys = channel_keys_of(&config);
        let state = Self {
            db_path,
            conn,
            config_store,
            quality_store,
            config: ArcSwap::from_pointee(config),
            channel_keys: ArcSwap::from_pointee(channel_keys),
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
            epg,
            epg_store,
            history,
            cache_dir,
            ffmpeg_install: Arc::new(crate::tools::install::Installer::new()),
            ffprobe_ready: tokio::sync::RwLock::new(None),
            stats: Stats::default(),
            metrics: Arc::new(crate::metrics::Metrics::new()),
            watchdog: Watchdog::new(),
            health: crate::health::Health::new(),
            quality: crate::quality::state::QualityState::new(),
            ip_guard: crate::auth::IpGuard::new(),
            play_sessions: crate::auth::SessionGuard::new(),
            rate_limiter: crate::auth::RateLimiter::new(),
            login_guard: crate::auth::LoginGuard::new(),
            unban_quota: crate::auth::UnbanQuota::new(),
            estimate,
            proxy_clients: std::sync::Mutex::new(std::collections::HashMap::new()),
            probe_clients: std::sync::Mutex::new(std::collections::HashMap::new()),
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            admission_lock: Arc::new(tokio::sync::Mutex::new(())),
            sync_pending: Mutex::new(std::collections::HashSet::new()),
            logs,
            started: Instant::now(),
            write_lock: Mutex::new(()),
        };
        state.hydrate_manual_quality()?;
        Ok(state)
    }

    /// 从结果表恢复手工频道的下线状态。重启后订阅渲染不丢失。
    ///
    /// 频道已不存在或地址哈希不匹配时跳过。
    pub fn hydrate_manual_quality(&self) -> anyhow::Result<()> {
        let config = self.snapshot();
        for result in self.quality_store.manual_entries()? {
            // 只有 hidden 需要恢复下线状态。kept 失败可能是宽限内保留，
            // 重启后恢复失败 Entry 会绕过宽限。
            if result.action != "hidden" {
                continue;
            }
            let Some(group) = config.group(&result.gid) else {
                continue;
            };
            let Some(channel) = group
                .channels
                .iter()
                .find(|channel| channel.id == result.cid)
            else {
                continue;
            };
            if result.url_hash != crate::quality::admission::url_hash_hex(&channel.url) {
                continue;
            }
            let entry = crate::quality::state::Entry::from_result(
                &result,
                crate::quality::state::url_hash(&channel.url),
            );
            self.quality.put(&result.gid, &result.cid, entry);
        }
        Ok(())
    }

    /// 数据目录：数据库文件所在目录。面板安装的外部工具放在这里。
    pub fn data_dir(&self) -> PathBuf {
        crate::tools::data_dir(&self.db_path)
    }

    pub fn tools_dir(&self) -> PathBuf {
        crate::tools::dir(&self.data_dir())
    }

    pub fn ffmpeg(&self) -> PathBuf {
        crate::tools::ffmpeg(&self.data_dir())
    }

    pub fn ffprobe(&self) -> PathBuf {
        crate::tools::ffprobe(&self.data_dir())
    }

    /// ffprobe 是否可用。首次检测后缓存结果。
    pub async fn ffprobe_ready(&self) -> bool {
        if let Some(value) = *self.ffprobe_ready.read().await {
            return value;
        }
        let mut guard = self.ffprobe_ready.write().await;
        if let Some(value) = *guard {
            return value;
        }
        let output = tokio::process::Command::new(self.ffprobe())
            .arg("-version")
            .output()
            .await;
        let value = output
            .map(|output| output.status.success() && ffprobe_version_ok(&output.stdout))
            .unwrap_or(false);
        *guard = Some(value);
        value
    }

    /// 清除 ffprobe 可用性缓存。安装或删除 ffprobe 后调用。
    pub async fn invalidate_ffprobe_ready(&self) {
        *self.ffprobe_ready.write().await = None;
    }

    /// 取当前配置快照。
    pub fn snapshot(&self) -> Arc<Config> {
        self.config.load_full()
    }

    /// 从数据库重读并替换快照。写锁内完成，避免与保存交错。
    ///
    /// 返回 `Ok(false)` 表示库里没有配置。此时保留当前快照。
    pub fn reload_from_db(&self) -> anyhow::Result<bool> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        match self.config_store.load()? {
            Some(mut config) => {
                if crate::sign::needs_secret(&config) {
                    crate::sign::ensure_secret(&mut config);
                    let revision = self.config_store.save(&config)?;
                    config.revision = revision;
                }
                self.replace(config);
                Ok(true)
            }
            None => Ok(false),
        }
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
        self.client_for_proxy(proxy)
    }

    /// 按代理配置取客户端。相同配置复用同一个客户端。
    ///
    /// 代理地址为空或客户端创建失败时回退直连。
    pub fn client_for_proxy(&self, proxy: &crate::config::model::ProxyCfg) -> reqwest::Client {
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

    /// 质量探测客户端。重定向上限 5，按代理配置缓存。
    pub fn probe_client_for(&self, gid: &str, cid: &str) -> reqwest::Client {
        let config = self.snapshot();
        let Some(proxy) = config.proxy_for(gid, cid) else {
            return self.probe_client(None);
        };
        self.probe_client(Some(proxy))
    }

    /// 质量候选的探测客户端。候选尚未进入配置，按给定代理构造。
    pub(crate) fn probe_client_for_candidate(
        &self,
        proxy: Option<&crate::config::model::ProxyCfg>,
    ) -> reqwest::Client {
        self.probe_client(proxy)
    }

    fn probe_client(&self, proxy: Option<&crate::config::model::ProxyCfg>) -> reqwest::Client {
        let key = proxy
            .map(|proxy| {
                format!(
                    "{}|{}|{}",
                    proxy.url().unwrap_or_default(),
                    proxy.username().unwrap_or(""),
                    proxy.password().unwrap_or("")
                )
            })
            .unwrap_or_else(|| "direct".to_string());
        {
            let cache = self
                .probe_clients
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(client) = cache.get(&key) {
                return client.clone();
            }
        }
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(UPSTREAM_TIMEOUT_SECS))
            .user_agent(DEFAULT_USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(5));
        if let Some(proxy) = proxy {
            if let Some(addr) = proxy.url() {
                let user = proxy.username().unwrap_or("");
                let pass = proxy.password().unwrap_or("");
                let built = reqwest::Proxy::all(&addr).and_then(|mut candidate| {
                    if !user.is_empty() {
                        candidate = candidate.basic_auth(user, pass);
                    }
                    Ok(candidate)
                });
                match built {
                    Ok(candidate) => builder = builder.proxy(candidate),
                    Err(error) => tracing::warn!(%error, "探测代理创建失败；改用直连"),
                }
            }
        }
        match builder.build() {
            Ok(client) => {
                let mut cache = self
                    .probe_clients
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                cache.insert(key, client.clone());
                client
            }
            Err(error) => {
                tracing::warn!(%error, "探测客户端创建失败；改用直连");
                self.http.clone()
            }
        }
    }

    /// 原子替换配置。同时解除全部冷却（S-MODE-17）。
    pub fn replace(&self, config: Config) {
        self.watchdog.clear_all();
        self.segments
            .set_limits(config.cache.segments.per_channel_max);
        self.sources
            .set_ttl(Duration::from_secs(config.cache.ttl.sub_url_secs.max(1)));
        self.estimate.set_auto_tune(config.cdn.auto_tune);
        // 配置删除分组/频道后，统计键和频道校验集一起收缩，避免旧键常驻。
        let channel_keys = channel_keys_of(&config);
        let group_ids: HashSet<&str> = config
            .groups
            .iter()
            .map(|group| group.id.as_str())
            .collect();
        self.stats.retain_groups(|id| group_ids.contains(id));
        self.stats.retain_channels(|key| channel_keys.contains(key));
        self.channel_keys.store(Arc::new(channel_keys));
        self.config.store(Arc::new(config));
    }

    /// 频道键是否在当前配置中。`key` 形如 `gid/cid`。
    pub fn channel_configured(&self, key: &str) -> bool {
        self.channel_keys.load().contains(key)
    }

    /// 保存整份配置。与 [`AppState::mutate`] 共用写锁，避免并发丢更新。
    ///
    /// `expected` 非零且与库中版本不同时返回 [`crate::config::StaleRevision`]。
    /// 每次成功写入都自增版本，并返回新值。
    pub fn save_config(&self, mut config: Config, expected: Option<u64>) -> anyhow::Result<u64> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        config.normalize();
        crate::sign::ensure_secret(&mut config);
        let revision = self.config_store.save_if_revision(&config, expected)?;
        config.revision = revision;
        self.replace(config);
        Ok(revision)
    }

    /// 整份替换配置，跳过 `revision` 校验。用于备份恢复等显式覆盖操作。
    ///
    /// 与 [`AppState::save_config`] 共用写锁。写入后返回新版本并替换快照。
    pub fn replace_config(&self, mut config: Config) -> anyhow::Result<u64> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        config.normalize();
        crate::sign::ensure_secret(&mut config);
        let revision = self.config_store.save(&config)?;
        config.revision = revision;
        self.replace(config);
        Ok(revision)
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
        config.normalize();
        crate::sign::ensure_secret(&mut config);
        let revision = self.config_store.save(&config)?;
        config.revision = revision;
        self.replace(config);
        Ok(())
    }

    /// 改配置并原子写入质量结果。
    ///
    /// 闭包在事务内执行。闭包返回的结果与实际成员变更一起提交。
    /// 调用方负责先拿 [`AppState::admission_lock`]。
    pub fn mutate_admission<F>(
        &self,
        meta: &[(String, String)],
        change: F,
    ) -> anyhow::Result<Vec<crate::quality::store::QualityResult>>
    where
        F: FnOnce(&mut Config) -> anyhow::Result<Vec<crate::quality::store::QualityResult>>,
    {
        self.mutate_admission_with_delete(meta, &[], change)
    }

    /// 同 [`Self::mutate_admission`]，并在同一事务里删除指定结果键。
    pub fn mutate_admission_with_delete<F>(
        &self,
        meta: &[(String, String)],
        delete_keys: &[String],
        change: F,
    ) -> anyhow::Result<Vec<crate::quality::store::QualityResult>>
    where
        F: FnOnce(&mut Config) -> anyhow::Result<Vec<crate::quality::store::QualityResult>>,
    {
        let delete_keys = delete_keys.to_vec();
        self.mutate_admission_full(meta, move |config| Ok((change(config)?, delete_keys)))
    }

    /// 同 [`Self::mutate_admission`]，闭包同时返回要删除的结果键。
    pub fn mutate_admission_full<F>(
        &self,
        meta: &[(String, String)],
        change: F,
    ) -> anyhow::Result<Vec<crate::quality::store::QualityResult>>
    where
        F: FnOnce(
            &mut Config,
        )
            -> anyhow::Result<(Vec<crate::quality::store::QualityResult>, Vec<String>)>,
    {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction()?;
        let mut config = match self.config_store.load_tx(&tx)? {
            Some(from_db) => from_db,
            None => (*self.snapshot()).clone(),
        };
        let (results, delete_keys) = change(&mut config)?;
        let revision = self.config_store.save_tx(&tx, &config)?;
        for key in &delete_keys {
            self.quality_store.delete_tx(&tx, key)?;
        }
        self.quality_store.upsert_many_tx(&tx, &results)?;
        for (key, value) in meta {
            self.quality_store.meta_set_tx(&tx, key, value)?;
        }
        tx.commit()?;
        config.revision = revision;
        self.replace(config);
        Ok(results)
    }

    /// 提交质量结果与 meta，不改配置，但与配置写入共用写锁。
    ///
    /// 闭包拿最新配置复核结果。源停用/删除与结果提交因此串行。
    pub fn commit_quality_results<F>(
        &self,
        meta: &[(String, String)],
        mut results: Vec<crate::quality::store::QualityResult>,
        change: F,
    ) -> anyhow::Result<Vec<(String, String, crate::quality::state::Entry)>>
    where
        F: FnOnce(
            &Config,
            &mut Vec<crate::quality::store::QualityResult>,
        ) -> Vec<(String, String, crate::quality::state::Entry)>,
    {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction()?;
        let config = match self.config_store.load_tx(&tx)? {
            Some(from_db) => from_db,
            None => (*self.snapshot()).clone(),
        };
        let manual = change(&config, &mut results);
        self.quality_store.upsert_many_tx(&tx, &results)?;
        for (key, value) in meta {
            self.quality_store.meta_set_tx(&tx, key, value)?;
        }
        tx.commit()?;
        Ok(manual)
    }

    pub fn version(&self) -> &'static str {
        crate::VERSION
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// 在线清单缓存条目数。
    pub fn playlist_cache_entries(&self) -> usize {
        self.playlists.len()
    }
}

/// 输出是否来自 ffprobe。`ffmpeg -version` 不含该标记。
fn ffprobe_version_ok(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout).contains("ffprobe version")
}

/// 收集配置里的 `gid/cid`。统计中间件用它判断请求是否指向真实频道。
fn channel_keys_of(config: &Config) -> HashSet<String> {
    let mut keys = HashSet::new();
    for group in &config.groups {
        for channel in &group.channels {
            keys.insert(format!("{}/{}", group.id, channel.id));
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{Channel, Group};
    use crate::quality::store::QualityResult;

    fn test_state() -> (std::sync::Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("tvb-state-{}", uuid::Uuid::new_v4()));
        let state = AppState::new(
            dir.join("tvb.db"),
            Config::default(),
            std::sync::Arc::new(crate::logging::RingBuffer::default()),
        )
        .expect("state");
        (std::sync::Arc::new(state), dir)
    }

    #[test]
    fn mutate_admission_commits_config_and_results() {
        let (state, dir) = test_state();
        let item = QualityResult {
            key: "sync:s:c".into(),
            action: "admitted".into(),
            ..QualityResult::default()
        };
        let written = state
            .mutate_admission(&[("last_run_at".into(), "1".into())], |config| {
                config.groups.push(Group {
                    id: "g".into(),
                    ..Group::default()
                });
                Ok(vec![item])
            })
            .unwrap();
        assert_eq!(written.len(), 1);
        assert!(state.snapshot().group("g").is_some());
        assert!(state.quality_store.get("sync:s:c").unwrap().is_some());
        assert_eq!(
            state
                .quality_store
                .meta_get("last_run_at")
                .unwrap()
                .as_deref(),
            Some("1")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mutate_admission_rolls_back_on_closure_error() {
        let (state, dir) = test_state();
        let error = state
            .mutate_admission(&[], |config| {
                config.groups.push(Group {
                    id: "g".into(),
                    ..Group::default()
                });
                anyhow::bail!("stop");
            })
            .unwrap_err();
        assert!(error.to_string().contains("stop"));
        assert!(state.snapshot().group("g").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mutate_admission_rolls_back_on_invalid_config() {
        let (state, dir) = test_state();
        let error = state
            .mutate_admission(&[], |config| {
                config.server.bind = String::new();
                Ok(vec![QualityResult {
                    key: "sync:s:c".into(),
                    ..QualityResult::default()
                }])
            })
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert!(state.quality_store.get("sync:s:c").unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_entries_hydrate_after_restart() {
        let dir = std::env::temp_dir().join(format!("tvb-hydrate-{}", uuid::Uuid::new_v4()));
        let mut config = Config::default();
        config.groups.push(Group {
            id: "g1".into(),
            channels: vec![Channel {
                id: "c1".into(),
                url: "http://up/live.m3u8".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        let state = std::sync::Arc::new(
            AppState::new(
                dir.join("tvb.db"),
                config,
                std::sync::Arc::new(crate::logging::RingBuffer::default()),
            )
            .expect("state"),
        );
        state
            .quality_store
            .upsert(&QualityResult {
                key: "channel:g1/c1".into(),
                scope: "manual".into(),
                action: "hidden".into(),
                reason: "unreachable".into(),
                gid: "g1".into(),
                cid: "c1".into(),
                url_hash: crate::quality::admission::url_hash_hex("http://up/live.m3u8"),
                checked_at: 1,
                ..QualityResult::default()
            })
            .unwrap();
        state.hydrate_manual_quality().unwrap();
        assert!(state.quality.get("g1", "c1").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn grace_kept_manual_result_is_not_hydrated() {
        let dir = std::env::temp_dir().join(format!("tvb-hydrate-{}", uuid::Uuid::new_v4()));
        let mut config = Config::default();
        config.groups.push(Group {
            id: "g1".into(),
            channels: vec![Channel {
                id: "c1".into(),
                url: "http://up/live.m3u8".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        let state = std::sync::Arc::new(
            AppState::new(
                dir.join("tvb.db"),
                config,
                std::sync::Arc::new(crate::logging::RingBuffer::default()),
            )
            .expect("state"),
        );
        state
            .quality_store
            .upsert(&QualityResult {
                key: "channel:g1/c1".into(),
                scope: "manual".into(),
                action: "kept".into(),
                reason: "unreachable".into(),
                gid: "g1".into(),
                cid: "c1".into(),
                url_hash: crate::quality::admission::url_hash_hex("http://up/live.m3u8"),
                checked_at: 1,
                ..QualityResult::default()
            })
            .unwrap();
        state.hydrate_manual_quality().unwrap();
        assert!(
            state.quality.get("g1", "c1").is_none(),
            "宽限内保留不得恢复为失败状态"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ffprobe_version_marker_is_required() {
        assert!(ffprobe_version_ok(b"ffprobe version 6.1"));
        assert!(!ffprobe_version_ok(b"ffmpeg version 6.1"));
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
