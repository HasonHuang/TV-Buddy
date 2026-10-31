//! 配置模型。
//!
//! 键名与默认值对齐目标 v0.8.2（`docs/spec/08-target-spec.md` §11）。
//! 存储是 SQLite 里的 JSON 文档。未建模的段与键落在 `extra`，保存时原样写回，
//! 这样不丢第三方配置。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 未建模的段与键。
///
/// 与存储格式一致，直接用 JSON 值承载。保存时原样写回，不丢第三方配置。
pub type Extra = Map<String, Value>;

/// 顶层配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerCfg,
    pub panel: PanelCfg,
    pub cache: CacheCfg,
    /// `[cdn]`——外部 CDN 兜底与被动实测开关（见 `docs/design/06-low-spec-optimization.md` §6）。
    pub cdn: CdnCfg,
    pub auth: AuthCfg,
    pub epg: EpgCfg,
    pub playlist: PlaylistCfg,
    pub share: ShareCfg,
    /// `[subtitle]`——字幕样式与机翻（面板「字幕设置」卡片）。
    pub subtitle: SubtitleCfg,
    pub health: HealthCfg,
    pub stats: StatsCfg,
    /// `[features]`——面板功能入口开关。只影响侧栏入口，不拦截接口。
    pub features: FeaturesCfg,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// 未建模的段与键。
    #[serde(flatten)]
    pub extra: Extra,
}

impl Config {
    /// 按 id 找分组。
    pub fn group(&self, gid: &str) -> Option<&Group> {
        self.groups.iter().find(|group| group.id == gid)
    }

    /// 按 id 找可变分组。
    pub fn group_mut(&mut self, gid: &str) -> Option<&mut Group> {
        self.groups.iter_mut().find(|group| group.id == gid)
    }

    /// `[assets].map`：原始 URL → 本地 HLS 入口。
    pub fn asset_map(&self) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        let Some(table) = self
            .extra
            .get("assets")
            .and_then(Value::as_object)
            .and_then(|assets| assets.get("map"))
            .and_then(Value::as_object)
        else {
            return out;
        };
        for (url, local) in table {
            if let Some(local) = local.as_str() {
                out.insert(url.clone(), local.to_string());
            }
        }
        out
    }

    /// 查本地 HLS 入口。
    pub fn asset_local(&self, url: &str) -> Option<&str> {
        self.extra
            .get("assets")
            .and_then(Value::as_object)
            .and_then(|assets| assets.get("map"))
            .and_then(Value::as_object)
            .and_then(|map| map.get(url))
            .and_then(Value::as_str)
    }

    /// 频道的有效上游代理：频道级优先，其次分组级。
    pub fn proxy_for(&self, gid: &str, cid: &str) -> Option<&ProxyCfg> {
        let group = self.group(gid)?;
        group
            .channels
            .iter()
            .find(|channel| channel.id == cid)
            .and_then(|channel| channel.proxy.as_ref())
            .or(group.proxy.as_ref())
    }

    /// 频道的有效预取配置：频道级 > 分组级 > 全局。
    ///
    /// 与 [`Config::proxy_for`] 同序，但**返回合并后的值**——全局是基线，
    /// 分组/频道只覆盖显式设置过的字段（`docs/design/06-low-spec-optimization.md` §7）。
    pub fn prefetch_for(&self, gid: &str, cid: &str) -> CachePrefetchCfg {
        let mut cfg = self.cache.prefetch.clone();
        let Some(group) = self.group(gid) else {
            return cfg;
        };
        if let Some(over) = group.prefetch.as_ref() {
            cfg.apply(over);
        }
        if let Some(channel) = group.channels.iter().find(|channel| channel.id == cid) {
            if let Some(over) = channel.prefetch.as_ref() {
                cfg.apply(over);
            }
        }
        cfg
    }
}

/// `[server]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerCfg {
    pub bind: String,
    pub not_found_url: String,
    pub error_url: String,
    pub home_url: String,
    pub m3u_header_lines: Vec<String>,
    pub timezone_offset_hours: i32,
    pub catchup_tpl: String,
    pub log_level: String,
    pub trust_proxy: Vec<String>,
    /// 低配模式开关。目标只在 `/panel/api/status` 回显，本实现同此。
    pub low_resource_mode: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self {
            bind: "[::]:19899".into(),
            not_found_url: "https://material.yang-1989.eu.org/v/404.mp4".into(),
            error_url: "https://material.yang-1989.eu.org/v/mistake.mp4".into(),
            home_url: "https://yang-1989.eu.org/".into(),
            m3u_header_lines: Vec::new(),
            timezone_offset_hours: 8,
            catchup_tpl: "guonei".into(),
            log_level: "info".into(),
            trust_proxy: Vec::new(),
            low_resource_mode: false,
            extra: Extra::new(),
        }
    }
}

/// `[features]`——面板功能入口开关。
///
/// 关掉某项后，侧栏不再显示该分区，相关接口返回 `403`。
/// 语义与部署规格无关：高配与低配行为一致。
/// 节目指南与频道热度不在这里，分别由 `[epg].enabled` 与 `[stats].enabled` 控制。
/// 核心分区（概览、缓存、分组、设置、安全、日志）没有开关，始终显示。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeaturesCfg {
    /// 定时录制。
    pub recording: bool,
    /// 回看管理。
    pub livestream: bool,
    /// 台标管理。
    pub logo: bool,
    /// 订阅聚合。
    pub playlist: bool,
    /// 视频素材。
    pub assets: bool,
    /// 用户与设备。
    pub users: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for FeaturesCfg {
    fn default() -> Self {
        Self {
            recording: true,
            livestream: true,
            logo: true,
            playlist: true,
            assets: true,
            users: true,
            extra: Extra::new(),
        }
    }
}

/// `[panel]`。
///
/// 账号不再存在这里。用户、角色与令牌在 `<cache>/users.db`，
/// 见 `docs/design/09-users-and-devices.md`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelCfg {
    pub enabled: bool,
    pub session_ttl: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for PanelCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            session_ttl: 86400,
            extra: Extra::new(),
        }
    }
}

/// `[cdn]`——外部 CDN 侧兜底。
///
/// 默认全关：`enabled=false` 时所有响应头与状态码逐字节等于旧行为，
/// 便于先配 CDN 面板再逐项打开（`docs/design/06-low-spec-optimization.md` §6）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CdnCfg {
    /// 总开关。
    pub enabled: bool,
    /// 被动实测：累计分组窗口统计与切片体积，供推荐 TTL / 磁盘上限使用。
    pub auto_tune: bool,
    /// 切片 `Cache-Control` 的 `max-age`（秒）；`0` = 用实测推荐值。
    pub segment_max_age_secs: u64,
    /// 命中路径（二次请求）也标 `public`，供只能「遵循源站头」的 CDN 共享缓存。
    pub shared_cache_hits: bool,
    /// 空体加 `no-store`；强制 TTL 时会被 CDN 忽略，此时用 [`CdnCfg::empty_status`]。
    pub dont_cache_empty: bool,
    /// 上游失败时改用的状态码（默认 `502`），避免「200 + 空体」被缓存一个周期。
    pub empty_status: u16,
    /// 固定对外基址；`None` 时仍按 `X-Forwarded-Proto` + `Host` 推导。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_base: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CdnCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_tune: true,
            segment_max_age_secs: 0,
            shared_cache_hits: false,
            dont_cache_empty: false,
            empty_status: 502,
            public_base: None,
            extra: Extra::new(),
        }
    }
}

/// `[cache]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheCfg {
    /// `disk`（默认）或 `memory`。
    ///
    /// `memory` 时切片正文留在进程内，不写 `seg_bytes`；进程重启即清空。
    /// 总量受 [`CacheCfg::memory_max_mb`] 限制。索引与清单仍按原路径处理。
    pub backend: String,
    pub disk_path: String,
    /// 内存清单条数上限。目标同键**未观测到生效**（S-CACHE-03）；本实现保留键面，
    /// 不消费（U-10）。
    pub m3u8_max_entries: u64,
    /// 内存切片索引条数上限。同样未观测到目标生效，本实现不消费；实际限制走
    /// `[cache.segments]` 与 `[cache].memory_max_mb`（U-10）。
    pub segment_max_entries: u64,
    /// 切片缓存总量上限（MB）；`0` = 不限。与 `[cache.gc].max_disk_mb` 同义，
    /// 同时非 0 时取更小者（U-10）。
    pub disk_max_mb: u64,
    /// 内存切片索引的字节预算（MB）；`0` = 不限。按每条约 256 B 估算成条目数，
    /// 再与 `[cache.segments].global_max` 取更小者（U-10）。
    pub memory_max_mb: u64,
    /// 段缓存上限（MB）。目标同键**未观测到生效**（S-CACHE-03）；其默认值 50 与用户
    /// 显式填写的 50 无法区分，一旦生效会把缓存静默压到 50 MB，因此本实现保留键面
    /// 但**不消费**，限容走 `[cache.gc]`（U-10）。
    pub segment_cache_max_mb: u64,
    pub prefetch: CachePrefetchCfg,
    pub segments: CacheSegmentsCfg,
    /// `[cache.gc]`——磁盘回收；补「切片只写不删」的洞。
    pub gc: CacheGcCfg,
    pub ttl: CacheTtlCfg,
    pub dash: CacheDashCfg,
    pub remux: CacheRemuxCfg,
    pub disguise: DisguiseCfg,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CacheCfg {
    fn default() -> Self {
        Self {
            backend: "disk".into(),
            disk_path: "./cache".into(),
            m3u8_max_entries: 500,
            segment_max_entries: 2000,
            disk_max_mb: 0,
            memory_max_mb: 0,
            segment_cache_max_mb: 50,
            prefetch: CachePrefetchCfg::default(),
            segments: CacheSegmentsCfg::default(),
            gc: CacheGcCfg::default(),
            ttl: CacheTtlCfg::default(),
            dash: CacheDashCfg::default(),
            remux: CacheRemuxCfg::default(),
            disguise: DisguiseCfg::default(),
            extra: Extra::new(),
        }
    }
}

/// `[cache.prefetch]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CachePrefetchCfg {
    pub enabled: bool,
    pub concurrency: u32,
    pub timeout: u64,
    pub prefetch_limit: u64,
    pub skip_tail: u64,
    pub mpd_readahead: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CachePrefetchCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            concurrency: 4,
            timeout: 10,
            prefetch_limit: 0,
            skip_tail: 1,
            mpd_readahead: 1,
            extra: Extra::new(),
        }
    }
}

impl CachePrefetchCfg {
    /// 叠加分组/频道级覆盖；`None` 字段保持基线值。
    pub fn apply(&mut self, over: &PrefetchOverrideCfg) {
        if let Some(enabled) = over.enabled {
            self.enabled = enabled;
        }
        if let Some(limit) = over.prefetch_limit {
            self.prefetch_limit = limit;
        }
        if let Some(concurrency) = over.concurrency {
            self.concurrency = concurrency;
        }
    }
}

/// `[[groups.prefetch]]` / `[[groups.channels.prefetch]]`——预取的三态覆盖。
///
/// 三态：未设置（`None`，继承上层）/ 显式开 / 显式关。全字段 `Option`，
/// 未设置时不序列化，因此「跟随全局」的分组写回配置不产生噪音。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrefetchOverrideCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 目标只保留最近的 N 片；`0` 表示不限。
    #[serde(alias = "limit", skip_serializing_if = "Option::is_none")]
    pub prefetch_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `[cache.segments]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheSegmentsCfg {
    pub global_max: u64,
    pub per_channel_max: u64,
    pub retain_count: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CacheSegmentsCfg {
    fn default() -> Self {
        Self {
            global_max: 0,
            per_channel_max: 0,
            retain_count: 20,
            extra: Extra::new(),
        }
    }
}

/// `[cache.gc]`——磁盘回收。
///
/// 切片正文只写不删；1 GB 盘必须靠这里回收
/// （`docs/design/06-low-spec-optimization.md` §6）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheGcCfg {
    pub enabled: bool,
    /// 回收上限（MB）；`0` = 按可用空间与工作集自动推算。
    pub max_disk_mb: u64,
    /// 超过该时长的文件直接删（秒）。
    pub max_age_secs: u64,
    /// 扫描周期（秒）。
    pub interval_secs: u64,
    /// 单次扫描最多处理的文件数，避免 1 vCPU 抖动。
    pub scan_limit: u64,
    /// 自动上限最多占可用空间的比例。
    pub disk_budget_ratio: f64,
    /// 给系统、日志与其它缓存目录预留的空间（MB）。
    pub reserve_mb: u64,
    /// 自动上限的下限（MB）。
    pub min_mb: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CacheGcCfg {
    fn default() -> Self {
        Self {
            // 默认开：不开则 1 GB 磁盘会被切片吃满。
            enabled: true,
            max_disk_mb: 0,
            max_age_secs: 1_800,
            interval_secs: 300,
            scan_limit: 500,
            disk_budget_ratio: 0.8,
            reserve_mb: 100,
            min_mb: 50,
            extra: Extra::new(),
        }
    }
}

/// `[cache.ttl]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheTtlCfg {
    pub sub_url_secs: u64,
    pub segment_secs: u64,
    pub refresh_ratio: f64,
    pub fallback_mpd_secs: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CacheTtlCfg {
    fn default() -> Self {
        Self {
            sub_url_secs: 1_800,
            segment_secs: 300,
            refresh_ratio: 0.4,
            fallback_mpd_secs: 8,
            extra: Extra::new(),
        }
    }
}

/// `[cache.dash]`——目标 DASH 会话调参。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheDashCfg {
    pub mpd_refresh_min_secs: f64,
    pub mpd_refresh_max_secs: f64,
    pub mpd_timeout_secs: u64,
    pub segment_timeout_secs: u64,
    pub fetch_concurrency: u32,
    pub ondemand_idle_secs: u64,
    pub mpd_body_dedup: bool,
    pub whole_file_serve: String,
    pub whole_file_budget_mb: u64,
    pub obfuscate_filenames: bool,
    pub playlist_min_secs: f64,
    pub retain_min_secs: f64,
    pub fetch_extra_segments: u64,
    /// 快速起播窗口。目标 v0.8.2 无可观测语义；保留字段以兼容配置回写。
    pub fast_start_secs: f64,
    pub start_time_offset_secs: f64,
    /// 直播清单发布延迟；>0 时按切片时长向上取整压后末端。
    pub publish_delay_secs: f64,
    pub continuous_tail_min_ratio: f64,
    pub retry_max: u32,
    pub retry_initial_ms: u64,
    pub retry_backoff: f64,
    pub retry_max_ms: u64,
    pub min_request_interval_ms: u64,
    pub playlist_stall_secs: u64,
    pub watchdog_tick_secs: u64,
    pub watchdog_stall_secs: u64,
    pub watchdog_rebuild_max: u32,
    pub watchdog_cooldown_secs: u64,
    pub watchdog_rebuild_stagger_ms: u64,
    pub session_max_lifetime_secs: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CacheDashCfg {
    fn default() -> Self {
        Self {
            mpd_refresh_min_secs: 4.0,
            mpd_refresh_max_secs: 12.0,
            mpd_timeout_secs: 10,
            segment_timeout_secs: 8,
            fetch_concurrency: 20,
            ondemand_idle_secs: 60,
            mpd_body_dedup: true,
            whole_file_serve: "auto".into(),
            whole_file_budget_mb: 0,
            obfuscate_filenames: true,
            playlist_min_secs: 30.0,
            retain_min_secs: 90.0,
            fetch_extra_segments: 2,
            fast_start_secs: 12.0,
            start_time_offset_secs: 15.0,
            publish_delay_secs: 0.0,
            continuous_tail_min_ratio: 0.6,
            retry_max: 8,
            retry_initial_ms: 200,
            retry_backoff: 2.0,
            retry_max_ms: 2_000,
            min_request_interval_ms: 80,
            playlist_stall_secs: 0,
            watchdog_tick_secs: 15,
            watchdog_stall_secs: 45,
            watchdog_rebuild_max: 5,
            watchdog_cooldown_secs: 3_600,
            watchdog_rebuild_stagger_ms: 300,
            session_max_lifetime_secs: 0,
            extra: Extra::new(),
        }
    }
}

/// `[cache.remux]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheRemuxCfg {
    pub target_duration_secs: f64,
    pub window_segments: u64,
    pub publish_delay_segments: u64,
    pub reconnect_backoff_ms: Vec<u64>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CacheRemuxCfg {
    fn default() -> Self {
        Self {
            target_duration_secs: 5.0,
            window_segments: 6,
            publish_delay_segments: 2,
            reconnect_backoff_ms: vec![200, 1_000, 2_000],
            extra: Extra::new(),
        }
    }
}

/// `[cache.disguise]`——切片伪装（S-DIS-*）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisguiseCfg {
    pub enabled: bool,
    pub formats: Vec<String>,
    pub subtitle: bool,
    pub playlist: bool,
    pub master: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for DisguiseCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            formats: ["jpg", "png", "gif", "bmp", "webp"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            subtitle: false,
            playlist: true,
            master: false,
            extra: Extra::new(),
        }
    }
}

/// `[subtitle]`——字幕样式与机翻。
///
/// 形状对齐目标 v0.8.2 的 `GET /panel/api/config`（默认值取自该抓包）。面板「系统设置 →
/// 字幕设置」按这段做字段级表单，所以必须建模：不建模时它落在 `extra` 里，新配置读不到默认值。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleCfg {
    /// 播放侧兼容模式。目标有该键，行为未测出（D19）。
    pub compat_mode: String,
    /// 播放侧字幕 UA 关键字。同上，未测出行为。
    pub srt_ua_keywords: Vec<String>,
    pub style: SubtitleStyleCfg,
    pub translate: SubtitleTranslateCfg,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for SubtitleCfg {
    fn default() -> Self {
        Self {
            compat_mode: "auto".into(),
            srt_ua_keywords: vec![
                "ijkplayer".into(),
                "ijkmediaplayer".into(),
                "tv.danmaku.ijk".into(),
            ],
            style: SubtitleStyleCfg::default(),
            translate: SubtitleTranslateCfg::default(),
            extra: Extra::new(),
        }
    }
}

/// `[subtitle.style]`。字段与目标一致；**样式注入尚未实现**（见 `PROGRESS.md` §2.3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleStyleCfg {
    pub enabled: bool,
    /// 空串 = 跟随播放器默认。
    pub color: String,
    /// 百分比字符串，例如 `"80%"`。
    pub font_size: String,
    pub bold: bool,
    /// 0–100，越大越靠下。
    pub position: u32,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for SubtitleStyleCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            color: String::new(),
            font_size: "80%".into(),
            bold: false,
            position: 90,
            extra: Extra::new(),
        }
    }
}

/// `[subtitle.translate]`。机翻由 `src/http/routes/media.rs` 消费。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleTranslateCfg {
    pub enabled: bool,
    /// MTranServer 基址；留空则用内置免费机翻。
    pub url: String,
    /// 目标抓包里出现过 `mtran_token` 别名。
    #[serde(alias = "mtran_token")]
    pub token: String,
    pub source: String,
    /// 主目标语言。
    pub target: String,
    /// 多目标语言；空时回落到 `target`。
    pub targets: Vec<String>,
    /// `true` 时译文在上。
    pub translation_first: bool,
    pub bilingual: bool,
    pub set_default: bool,
    pub timeout_ms: u64,
    #[serde(alias = "mtran_batch")]
    pub batch: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for SubtitleTranslateCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            token: String::new(),
            source: "auto".into(),
            target: "zh-cn".into(),
            targets: Vec::new(),
            translation_first: true,
            bilingual: true,
            set_default: true,
            timeout_ms: 1500,
            batch: 40,
            extra: Extra::new(),
        }
    }
}

/// `[auth]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthCfg {
    pub token_enabled: bool,
    pub token_ttl_hours: u64,
    pub token_failure_url: String,
    /// 切片路径签名开关。只在 [`AuthCfg::token_enabled`] 为真时生效。
    ///
    /// 开启后清单里的切片地址带 `l/<exp>/<sig>` 前缀。同一时间窗内
    /// 所有用户拿到相同地址，CDN 可以共享缓存。
    pub slice_sign_enabled: bool,
    /// 签名时间窗（秒）。窗口越短，撤销越快，缓存复用越低。
    pub slice_sign_window_secs: u64,
    /// 签名密钥（十六进制）。留空时保存配置会自动生成 32 字节。
    pub slice_sign_secret: String,
    /// 导入 M3U 时忽略 `tvg-id`，每个新频道取随机 id（`r-<8hex>`）。
    ///
    /// 目标开启后已有频道与切片前缀不变（F-010 §4），所以它只影响新导入的频道。
    pub random_id_enabled: bool,
    pub ip_guard_enabled: bool,
    /// 每 IP 并发播放会话上限；`0` = 不限。
    ///
    /// 语义自定（F-010 未观测到目标行为）：会话 = `(IP, 频道)`，超出时踢掉最久未用的那路。
    /// 只在 [`AuthCfg::ip_guard_enabled`] 为真时生效。
    pub max_sessions_per_ip: u32,
    /// 播放会话空闲超时（秒）。超过该时长没有播放请求即释放会话。
    ///
    /// 只在 [`AuthCfg::ip_guard_enabled`] 为真时生效。
    pub session_ttl_secs: u64,
    pub limit_url: String,
    pub ban_url: String,
    /// 白名单（精确地址 / IPv4 CIDR / `*`）。命中后豁免封禁与并发会话上限。
    ///
    /// 只在 [`AuthCfg::ip_guard_enabled`] 为真时生效。语义自定（F-010）：
    /// 目标键面里有它，但豁免规则在目标上未观测到行为。
    pub whitelist: Vec<String>,
    /// 登录失败统计窗口（秒）。
    pub auth_fail_window_secs: u64,
    /// 窗口内免费失败次数。
    pub auth_fail_free_per_window: u64,
    /// 超出免费次数后，再累计多少次触发锁定。
    pub auth_fail_ban_threshold: u64,
    /// 首次锁定秒数。目标首次锁实测固定 ~300s（`auth_fail_ban_duration_secs` 改小也不变），
    /// 所以本实现的锁定就用这个键的默认值 `300`；目标文件里没有这个键名。
    pub auth_fail_lock_secs: u64,
    // --- 限流、违规与升级封禁：语义自定（F-010 未观测到目标行为） ---
    // 只计播放入口请求，不计切片。全部只在 `ip_guard_enabled` 为真时生效。
    /// 一级（持续）限流窗口（秒）。
    pub l1_window_secs: u64,
    /// 一级窗口次数上限；`0` = 关闭该级。
    pub l1_limit: u64,
    /// 二级（突发）限流窗口（秒）。
    pub l2_window_secs: u64,
    /// 二级窗口次数上限；`0` = 关闭该级。
    pub l2_limit: u64,
    /// 超限后的冷却秒数。冷却期内返回 `429` + `Retry-After`。
    pub cooldown_secs: u64,
    /// 违规统计窗口（秒）。
    pub violation_window_secs: u64,
    /// 违规衰减（秒）：每满一个周期，最旧的一条违规失效。
    pub violation_decay_secs: u64,
    /// 有效违规多少次升级为封禁。
    pub ban_threshold: u64,
    /// 违规升级为封禁时的时长（秒）。面板手动封禁仍用接口入参。
    pub ban_duration_secs: u64,
    /// 第 2 次登录锁的时长（秒）。
    pub auth_fail_ban_duration_secs: u64,
    /// 第 3 次及以后登录锁的时长（秒）。
    pub auth_fail_ban_repeat_duration_secs: u64,
    /// 登录锁升级统计窗口（秒）。超出窗口后重新按第 1 次计。
    pub auth_fail_ban_escalate_window_secs: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for AuthCfg {
    fn default() -> Self {
        Self {
            token_enabled: false,
            token_ttl_hours: 24,
            token_failure_url: "https://material.yang-1989.eu.org/v/Failure.mp4".into(),
            slice_sign_enabled: true,
            slice_sign_window_secs: 300,
            slice_sign_secret: String::new(),
            random_id_enabled: false,
            ip_guard_enabled: false,
            max_sessions_per_ip: 1,
            session_ttl_secs: 30,
            limit_url: "https://material.yang-1989.eu.org/v/limit.mp4".into(),
            ban_url: "https://material.yang-1989.eu.org/v/Ban.mp4".into(),
            whitelist: Vec::new(),
            auth_fail_window_secs: 60,
            auth_fail_free_per_window: 15,
            auth_fail_ban_threshold: 6,
            auth_fail_lock_secs: 300,
            l1_window_secs: 30,
            l1_limit: 30,
            l2_window_secs: 30,
            l2_limit: 8,
            cooldown_secs: 3,
            violation_window_secs: 300,
            violation_decay_secs: 1800,
            ban_threshold: 3,
            ban_duration_secs: 2_592_000,
            auth_fail_ban_duration_secs: 3_600,
            auth_fail_ban_repeat_duration_secs: 2_592_000,
            auth_fail_ban_escalate_window_secs: 86_400,
            extra: Extra::new(),
        }
    }
}

/// `[health]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthCfg {
    pub enabled: bool,
    pub interval_secs: u64,
    pub timeout_secs: u64,
    pub concurrency: u32,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for HealthCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 300,
            timeout_secs: 10,
            concurrency: 3,
            extra: Extra::new(),
        }
    }
}

/// `[stats]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsCfg {
    pub enabled: bool,
    pub online_window_secs: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for StatsCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            online_window_secs: 90,
            extra: Extra::new(),
        }
    }
}

/// 上游代理覆盖（分组或频道级）。
///
/// 证据：S-GAP-05、F-010 §3.2。目标字段为
/// `proxy{kind,addr,username,password}`；`addr` 通常不带协议前缀。
/// `kind` 见 `http`/`https`/`socks5`/`socks5h`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProxyCfg {
    pub kind: String,
    pub addr: String,
    #[serde(rename = "username", alias = "user")]
    pub user: Option<String>,
    #[serde(rename = "password", alias = "pass")]
    pub pass: Option<String>,
}

impl Default for ProxyCfg {
    fn default() -> Self {
        Self {
            kind: "http".into(),
            addr: String::new(),
            user: None,
            pass: None,
        }
    }
}

impl ProxyCfg {
    /// 构造 reqwest 可接受的代理 URL。
    ///
    /// 目标允许 `addr` 省略协议：`kind` 决定协议，`addr` 只写 `host:port`。
    pub fn url(&self) -> Option<String> {
        let addr = self.addr.trim();
        if addr.is_empty() {
            return None;
        }
        if addr.contains("://") {
            return Some(addr.to_string());
        }
        let kind = self.kind.trim().to_ascii_lowercase();
        let scheme = match kind.as_str() {
            "" | "http" => "http",
            "https" => "https",
            "socks4" => "socks4",
            "socks4a" => "socks4a",
            "socks5" => "socks5",
            "socks5h" => "socks5h",
            other => {
                tracing::warn!(kind = other, "未知代理类型；按 http 处理");
                "http"
            }
        };
        Some(format!("{scheme}://{addr}"))
    }

    /// 代理认证用户名；空字符串按未配置处理。
    pub fn username(&self) -> Option<&str> {
        self.user
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
    }

    /// 代理认证密码。
    pub fn password(&self) -> Option<&str> {
        self.pass.as_deref()
    }
}

/// `[[groups]]`——工作模式属于分组（S-MODE-02）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// `proxy` / `remux` / `dash` / `redirect` / `rewrite` / `passthrough`。
    pub mode: String,
    /// `public` / `hidden` / `archived` / `open`。
    pub access: String,
    /// `hls` / `mpd`。
    pub format: String,
    pub remux_container: String,
    pub mpd_keep_seconds: u64,
    pub redirect_ttl: u64,
    pub redirect_mode: String,
    pub headers: Vec<Value>,
    /// 分组级上游代理；频道级优先。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyCfg>,
    /// 分组级预取覆盖；缺省继承全局。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefetch: Option<PrefetchOverrideCfg>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<Channel>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Group {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            mode: "proxy".into(),
            access: "public".into(),
            format: "hls".into(),
            remux_container: "auto".into(),
            mpd_keep_seconds: 0,
            redirect_ttl: 0,
            redirect_mode: "direct".into(),
            headers: Vec::new(),
            proxy: None,
            prefetch: None,
            channels: Vec::new(),
            extra: Extra::new(),
        }
    }
}

/// `[[groups.channels]]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub url: String,
    pub headers: Vec<Value>,
    /// 频道级上游代理；覆盖分组级。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyCfg>,
    /// 频道级预取覆盖；覆盖分组级与全局。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefetch: Option<PrefetchOverrideCfg>,
    pub selected_tracks: Vec<String>,
    pub dash_mode: String,
    pub livestream: bool,
    pub timeshift_hours: u32,
    pub timeshift_max_gb: u64,
    pub timeshift_offset_secs: u64,
    pub dash_timeshift: bool,
    pub dash_timeshift_hours: u32,
    pub dash_timeshift_max_gb: u64,
    pub dash_timeshift_offset_secs: u64,
    pub resolver_catchup: bool,
    pub catchup_days: u32,
    pub url_pool_size: u32,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            headers: Vec::new(),
            proxy: None,
            prefetch: None,
            selected_tracks: Vec::new(),
            dash_mode: "continuous".into(),
            livestream: false,
            timeshift_hours: 4,
            timeshift_max_gb: 0,
            timeshift_offset_secs: 60,
            dash_timeshift: false,
            dash_timeshift_hours: 4,
            dash_timeshift_max_gb: 0,
            dash_timeshift_offset_secs: 60,
            resolver_catchup: false,
            catchup_days: 7,
            url_pool_size: 1,
            extra: Extra::new(),
        }
    }
}

/// `[epg]`——节目单（S-EP-05、F-008）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EpgCfg {
    pub enabled: bool,
    pub cron: String,
    pub public_viewer_enabled: bool,
    pub keep_days: i64,
    pub future_days: i64,
    pub keep_yesterday: bool,
    pub match_by_name: bool,
    pub allow_scripts: bool,
    pub python_bin: Option<String>,
    pub script_timeout_secs: u64,
    pub record_pad_before_secs: u64,
    pub record_pad_after_secs: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<EpgSource>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for EpgCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            cron: "0 */4 * * *".into(),
            public_viewer_enabled: true,
            keep_days: 3,
            future_days: 1,
            keep_yesterday: true,
            match_by_name: true,
            allow_scripts: true,
            python_bin: None,
            script_timeout_secs: 300,
            record_pad_before_secs: 60,
            record_pad_after_secs: 180,
            sources: Vec::new(),
            extra: Extra::new(),
        }
    }
}

/// `[[epg.sources]]`——采集源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EpgSource {
    pub id: String,
    pub name: String,
    /// `xml`（按 `url` 拉 XMLTV）或 `script`（跑 Python，需外部依赖）。
    pub kind: String,
    pub enabled: bool,
    pub url: String,
    pub script: String,
    pub output_files: Vec<String>,
    pub cron: String,
    pub timeout_secs: u64,
    pub tz_shift_mins: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for EpgSource {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "新源".into(),
            kind: "xml".into(),
            enabled: true,
            url: String::new(),
            script: String::new(),
            output_files: Vec::new(),
            cron: String::new(),
            timeout_secs: 0,
            tz_shift_mins: 0,
            extra: Extra::new(),
        }
    }
}

/// `[playlist]`——订阅聚合（F-013 §5）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistCfg {
    /// 聚合源缓存时长（秒）。
    pub merge_ttl_secs: u64,
    /// `top` 或 `bottom`。
    pub merge_position: String,
    pub global_exclude: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<PlaylistSource>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for PlaylistCfg {
    fn default() -> Self {
        Self {
            merge_ttl_secs: 300,
            merge_position: "bottom".into(),
            global_exclude: Vec::new(),
            sources: Vec::new(),
            extra: Extra::new(),
        }
    }
}

/// `[[playlist.sources]]`——聚合源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistSource {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    /// 源正文缓存时长（秒）；`0` 表示不按时间失效。
    pub ttl_secs: u64,
    /// 源分组名 → 目标分组名；空值表示保留原名。
    pub group_map: BTreeMap<String, String>,
    /// `keep`（默认）、`map` 或 `merge`；未知值按 `keep`。
    pub group_mode: String,
    /// `merge` 模式的目标分组名；空值表示保留原名。
    pub merge_group: String,
    /// 未出现在 `group_map` 的分组：`drop`（默认）或 `keep`。
    pub unlisted_groups: String,
    /// 频道名白名单；支持 `group:` 与 `id:` 前缀。
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for PlaylistSource {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            enabled: true,
            ttl_secs: 0,
            group_map: BTreeMap::new(),
            group_mode: "keep".into(),
            merge_group: String::new(),
            unlisted_groups: "drop".into(),
            include: Vec::new(),
            exclude: Vec::new(),
            extra: Extra::new(),
        }
    }
}

/// `[share]`——多 VPS 分享（F-013 §10）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareCfg {
    pub enabled: bool,
    /// `master` 或 `follower`。
    pub role: String,
    /// 分享密钥；对端用 `?key=` 传入。
    pub key: String,
    /// 允许拉取的源地址（精确或 `a.b.c.0/24`）。
    pub trust_ips: Vec<String>,
    /// follower 拉取 master 的基地址。
    pub master_url: String,
    /// master 汇总的 follower 基地址。
    pub follower_urls: Vec<String>,
    /// 同步周期（秒）。
    pub poll_secs: u64,
    /// 目标键：面板文案是「同时分享 token」。**本实现未消费**。
    ///
    /// 实测：`share_token = false` 时 `/api/share/pull` 载荷里仍有 Token（F-013），
    /// 所以该键不控制载荷内容。语义未知，见 `PROGRESS.md` §3.3。
    pub share_token: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for ShareCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            role: "master".into(),
            key: String::new(),
            trust_ips: Vec::new(),
            master_url: String::new(),
            follower_urls: Vec::new(),
            poll_secs: 300,
            share_token: false,
            extra: Extra::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_proxy_overrides_group_proxy() {
        let mut config = Config::default();
        let mut group = Group {
            id: "g".into(),
            ..Group::default()
        };
        group.proxy = Some(ProxyCfg {
            addr: "http://group:8080".into(),
            ..ProxyCfg::default()
        });
        group.channels.push(Channel {
            id: "c".into(),
            proxy: Some(ProxyCfg {
                addr: "http://channel:8080".into(),
                ..ProxyCfg::default()
            }),
            ..Channel::default()
        });
        config.groups.push(group);

        assert_eq!(
            config.proxy_for("g", "c").expect("频道代理").addr,
            "http://channel:8080"
        );
        assert_eq!(
            config.proxy_for("g", "other").expect("分组代理").addr,
            "http://group:8080"
        );
        assert!(config.proxy_for("missing", "c").is_none());
    }

    #[test]
    fn cache_subsections_round_trip() {
        let body = serde_json::json!({
            "cache": {
                "disk_path": "cache",
                "prefetch": { "enabled": false, "concurrency": 2 },
                "segments": { "global_max": 10, "per_channel_max": 3 },
                "ttl": { "segment_secs": 42 },
                "dash": { "mpd_timeout_secs": 7 },
                "remux": {
                    "window_segments": 8,
                    "publish_delay_segments": 2,
                    "reconnect_backoff_ms": [100, 500]
                }
            }
        });
        let config: Config = serde_json::from_value(body).expect("解析配置");
        assert!(!config.cache.prefetch.enabled);
        assert_eq!(config.cache.prefetch.concurrency, 2);
        assert_eq!(config.cache.segments.global_max, 10);
        assert_eq!(config.cache.segments.per_channel_max, 3);
        assert_eq!(config.cache.ttl.segment_secs, 42);
        assert_eq!(config.cache.dash.mpd_timeout_secs, 7);
        assert_eq!(config.cache.remux.window_segments, 8);
        let encoded = serde_json::to_value(&config).expect("序列化");
        let decoded: Config = serde_json::from_value(encoded).expect("再解析");
        assert_eq!(decoded.cache.remux.reconnect_backoff_ms, vec![100, 500]);
    }

    #[test]
    fn proxy_is_parsed_from_json() {
        let body = serde_json::json!({
            "groups": [{
                "id": "g",
                "name": "组",
                "proxy": {
                    "kind": "http",
                    "addr": "http://127.0.0.1:8080",
                    "username": "u",
                    "password": "p"
                },
                "channels": [{
                    "id": "c",
                    "name": "频道",
                    "url": "http://up/live.m3u8"
                }]
            }]
        });
        let config: Config = serde_json::from_value(body).expect("解析配置");
        let proxy = config.proxy_for("g", "c").expect("代理");
        assert_eq!(proxy.kind, "http");
        assert_eq!(proxy.user.as_deref(), Some("u"));
        assert_eq!(proxy.pass.as_deref(), Some("p"));
    }

    #[test]
    fn panel_json_accepts_target_proxy_and_header_objects() {
        let body = serde_json::json!({
            "groups": [{
                "id": "g",
                "name": "组",
                "headers": [{"name": "X-Group", "value": "g"}],
                "proxy": {
                    "kind": "socks5",
                    "addr": "127.0.0.1:1080",
                    "username": "u",
                    "password": "p"
                },
                "channels": [{
                    "id": "c",
                    "name": "频道",
                    "url": "http://up/live.m3u8",
                    "headers": [{"name": "X-Channel", "value": "c"}]
                }]
            }]
        });
        let config: Config = serde_json::from_value(body).expect("解析面板 JSON");
        let proxy = config.proxy_for("g", "c").expect("代理");
        assert_eq!(proxy.url().as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(proxy.username(), Some("u"));
        assert_eq!(proxy.password(), Some("p"));
        let group = config.group("g").expect("分组");
        assert_eq!(
            group.headers[0]
                .as_object()
                .and_then(|table| table.get("name"))
                .and_then(Value::as_str),
            Some("X-Group")
        );

        let encoded = serde_json::to_value(&config).expect("序列化面板 JSON");
        assert_eq!(encoded["groups"][0]["proxy"]["username"], "u");
        assert!(encoded["groups"][0]["proxy"].get("user").is_none());
    }

    #[test]
    fn proxy_url_uses_kind_as_scheme() {
        let proxy = ProxyCfg {
            kind: "socks5".into(),
            addr: "127.0.0.1:1080".into(),
            ..ProxyCfg::default()
        };
        assert_eq!(proxy.url().as_deref(), Some("socks5://127.0.0.1:1080"));

        let proxy = ProxyCfg {
            kind: "socks5".into(),
            addr: "socks5h://proxy:1080".into(),
            ..ProxyCfg::default()
        };
        assert_eq!(proxy.url().as_deref(), Some("socks5h://proxy:1080"));

        let proxy = ProxyCfg {
            kind: "http".into(),
            addr: "".into(),
            ..ProxyCfg::default()
        };
        assert!(proxy.url().is_none());
    }

    /// 造一个带分组级/频道级预取覆盖的配置。
    fn config_with_prefetch(
        group_over: Option<PrefetchOverrideCfg>,
        channel_over: Option<PrefetchOverrideCfg>,
    ) -> Config {
        let mut config = Config::default();
        let mut group = Group {
            id: "g".into(),
            ..Group::default()
        };
        group.prefetch = group_over;
        group.channels.push(Channel {
            id: "c".into(),
            prefetch: channel_over,
            ..Channel::default()
        });
        config.groups.push(group);
        config
    }

    #[test]
    fn prefetch_channel_overrides_group_and_global() {
        let mut config = config_with_prefetch(
            Some(PrefetchOverrideCfg {
                enabled: Some(false),
                prefetch_limit: Some(6),
                ..PrefetchOverrideCfg::default()
            }),
            Some(PrefetchOverrideCfg {
                enabled: Some(true),
                concurrency: Some(2),
                ..PrefetchOverrideCfg::default()
            }),
        );
        config.cache.prefetch.concurrency = 8;

        let merged = config.prefetch_for("g", "c");
        assert!(merged.enabled, "频道覆盖分组的 false");
        assert_eq!(merged.prefetch_limit, 6, "频道未设，取分组值");
        assert_eq!(merged.concurrency, 2, "频道覆盖分组与全局");

        let group_only = config.prefetch_for("g", "other");
        assert!(!group_only.enabled);
        assert_eq!(group_only.prefetch_limit, 6);
        assert_eq!(group_only.concurrency, 8, "频道未命中时回落全局");
    }

    #[test]
    fn prefetch_partial_override_keeps_other_fields() {
        let config = config_with_prefetch(
            Some(PrefetchOverrideCfg {
                prefetch_limit: Some(3),
                ..PrefetchOverrideCfg::default()
            }),
            None,
        );
        let merged = config.prefetch_for("g", "c");
        assert_eq!(merged.prefetch_limit, 3);
        assert_eq!(merged.concurrency, 4, "全局默认");
        assert!(merged.enabled, "全局默认");
        assert_eq!(merged.skip_tail, 1, "全局默认");
    }

    #[test]
    fn prefetch_unset_equals_global_defaults() {
        let config = Config::default();
        let merged = config.prefetch_for("missing", "c");
        let base = CachePrefetchCfg::default();
        assert_eq!(merged.enabled, base.enabled);
        assert_eq!(merged.concurrency, base.concurrency);
        assert_eq!(merged.prefetch_limit, base.prefetch_limit);
        assert_eq!(merged.skip_tail, base.skip_tail);
        assert_eq!(merged.timeout, base.timeout);
    }

    #[test]
    fn prefetch_unknown_group_is_global() {
        let mut config = Config::default();
        config.cache.prefetch.enabled = false;
        config.groups.push(Group {
            id: "g".into(),
            ..Group::default()
        });
        assert!(!config.prefetch_for("nope", "c").enabled);
    }

    #[test]
    fn prefetch_override_reads_json_and_limit_alias() {
        let body = serde_json::json!({
            "groups": [{
                "id": "hot",
                "name": "热",
                "prefetch": { "enabled": true, "limit": 6, "concurrency": 2 },
                "channels": [{
                    "id": "c",
                    "name": "频道",
                    "url": "http://up/live.m3u8",
                    "prefetch": { "enabled": false }
                }]
            }]
        });
        let config: Config = serde_json::from_value(body).expect("解析配置");
        let group = config.group("hot").expect("分组");
        assert_eq!(
            group.prefetch.as_ref().and_then(|over| over.prefetch_limit),
            Some(6),
            "`limit` 是 `prefetch_limit` 的别名"
        );
        let merged = config.prefetch_for("hot", "c");
        assert!(!merged.enabled);
        assert_eq!(merged.prefetch_limit, 6);
        assert_eq!(merged.concurrency, 2);

        let channel = &group.channels[0];
        let over = channel.prefetch.as_ref().expect("频道覆盖");
        assert_eq!(over.enabled, Some(false));
        assert_eq!(over.prefetch_limit, None, "未设置的字段保持 None");
    }

    #[test]
    fn cdn_and_gc_defaults_are_inert() {
        let config = Config::default();
        assert!(!config.cdn.enabled);
        assert!(config.cdn.auto_tune);
        assert_eq!(config.cdn.segment_max_age_secs, 0);
        assert_eq!(config.cdn.empty_status, 502);
        assert!(config.cdn.public_base.is_none());
        assert!(config.cache.gc.enabled);
        assert_eq!(config.cache.gc.max_disk_mb, 0);
        assert_eq!(config.cache.gc.max_age_secs, 1_800);
    }

    #[test]
    fn cdn_and_gc_round_trip_through_json() {
        let mut config = Config::default();
        config.cdn.enabled = true;
        config.cdn.public_base = Some("https://cdn.example".into());
        config.cache.gc.max_disk_mb = 200;
        let encoded = serde_json::to_value(&config).expect("序列化");
        assert_eq!(encoded["cdn"]["enabled"], true);
        assert_eq!(encoded["cache"]["gc"]["max_disk_mb"], 200);
        assert_eq!(encoded["cdn"]["public_base"], "https://cdn.example");
        let decoded: Config = serde_json::from_value(encoded).expect("再解析");
        assert!(decoded.cdn.enabled);
        assert_eq!(
            decoded.cdn.public_base.as_deref(),
            Some("https://cdn.example")
        );
        assert_eq!(decoded.cache.gc.max_disk_mb, 200);
    }

    #[test]
    fn group_without_prefetch_serializes_unchanged() {
        let mut config = Config::default();
        config.groups.push(Group {
            id: "g".into(),
            name: "组".into(),
            ..Group::default()
        });
        let encoded = serde_json::to_value(&config).expect("序列化");
        assert!(encoded["groups"][0].get("prefetch").is_none(), "{encoded}");
        let decoded: Config = serde_json::from_value(encoded).expect("再解析");
        assert!(decoded.group("g").expect("分组").prefetch.is_none());
    }
}
