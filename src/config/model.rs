//! 配置模型。
//!
//! 字段与目标 v0.8.2 的 `config.toml` 对齐（`docs/spec/08-target-spec.md` §11）。
//! 未建模的段与键落在 `extra`，保存时原样写回。这样不丢第三方配置。

use serde::{Deserialize, Serialize};

/// 顶层配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerCfg,
    pub panel: PanelCfg,
    pub cache: CacheCfg,
    pub auth: AuthCfg,
    pub epg: EpgCfg,
    pub health: HealthCfg,
    pub stats: StatsCfg,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// 未建模的段与键。
    #[serde(flatten)]
    pub extra: toml::Table,
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
    #[serde(flatten)]
    pub extra: toml::Table,
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
            extra: toml::Table::new(),
        }
    }
}

/// `[panel]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelCfg {
    pub enabled: bool,
    pub username: String,
    /// 口令的 SHA-256 十六进制。默认口令 `admin`。
    pub password_sha256: String,
    pub session_ttl: u64,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for PanelCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            username: "admin".into(),
            // sha256("admin")
            password_sha256: "8c6976e5b5410415bde908bd4dee15dfb167a9c873fc4bb8a81f6f2ab448a918"
                .into(),
            session_ttl: 86400,
            extra: toml::Table::new(),
        }
    }
}

/// `[cache]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheCfg {
    pub backend: String,
    pub disk_path: String,
    pub m3u8_max_entries: u64,
    pub segment_max_entries: u64,
    pub disk_max_mb: u64,
    pub memory_max_mb: u64,
    pub segment_cache_max_mb: u64,
    pub disguise: DisguiseCfg,
    #[serde(flatten)]
    pub extra: toml::Table,
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
            disguise: DisguiseCfg::default(),
            extra: toml::Table::new(),
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
    pub extra: toml::Table,
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
            extra: toml::Table::new(),
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
    pub random_id_enabled: bool,
    pub ip_guard_enabled: bool,
    pub max_sessions_per_ip: u32,
    pub session_ttl_secs: u64,
    pub limit_url: String,
    pub ban_url: String,
    pub whitelist: Vec<String>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for AuthCfg {
    fn default() -> Self {
        Self {
            token_enabled: false,
            token_ttl_hours: 24,
            token_failure_url: "https://material.yang-1989.eu.org/v/Failure.mp4".into(),
            random_id_enabled: false,
            ip_guard_enabled: false,
            max_sessions_per_ip: 1,
            session_ttl_secs: 30,
            limit_url: "https://material.yang-1989.eu.org/v/limit.mp4".into(),
            ban_url: "https://material.yang-1989.eu.org/v/Ban.mp4".into(),
            whitelist: Vec::new(),
            extra: toml::Table::new(),
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
    pub extra: toml::Table,
}

impl Default for HealthCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 300,
            timeout_secs: 10,
            concurrency: 3,
            extra: toml::Table::new(),
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
    pub extra: toml::Table,
}

impl Default for StatsCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            online_window_secs: 90,
            extra: toml::Table::new(),
        }
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
    pub headers: Vec<toml::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<Channel>,
    #[serde(flatten)]
    pub extra: toml::Table,
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
            channels: Vec::new(),
            extra: toml::Table::new(),
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
    pub headers: Vec<toml::Value>,
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
    pub extra: toml::Table,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            headers: Vec::new(),
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
            extra: toml::Table::new(),
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
    pub extra: toml::Table,
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
            extra: toml::Table::new(),
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
    pub extra: toml::Table,
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
            extra: toml::Table::new(),
        }
    }
}
