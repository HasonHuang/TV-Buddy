//! 被动实测：从真实播放流量推算推荐 TTL 与磁盘上限。
//!
//! 依据 `docs/design/06-low-spec-optimization.md` §3：
//!
//! - 清单解析得到 `#EXT-X-TARGETDURATION`（T）、`#EXTINF` 条数（N）与窗口时长（W）；
//! - 切片体积按分组做 EMA（α=0.3），**不为估算多打一次上游**；
//! - 推荐 TTL = `clamp(ceil60(W + 2T), 300, 900)`，取各分组最大值；
//! - 工作集 = `round10(3 × N × seg_mb × 1.5)`；
//! - `[cdn].auto_tune = false` 时不采集，只沿用已落盘的旧值。
//!
//! 统计落盘到 `<cache>/cache_estimate.json`（带 schema 版本），重启后延续。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, RwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// MB 定义（十进制）。
///
/// 设计稿 §3.3 的算例按十进制 MB（4,463,308 B ≈ 4.46 MB），保持一致便于对拍。
pub const MB: u64 = 1_000_000;

/// 无实测数据时的回退 TTL（秒）。
pub const FALLBACK_TTL_SECS: u64 = 600;
/// 无实测数据时的回退磁盘上限（MB）。
pub const FALLBACK_DISK_MB: u64 = 150;

/// 推荐 TTL 下限（秒）：覆盖重连与回退。
const TTL_MIN_SECS: u64 = 300;
/// 推荐 TTL 上限（秒）：再长只增加边缘存储，不增加命中。
const TTL_MAX_SECS: u64 = 900;
/// 切片体积滑动平均权重。
const EMA_ALPHA: f64 = 0.3;
/// 落盘节流（秒）。
const PERSIST_MIN_SECS: u64 = 300;
/// 估算文件的结构版本；不匹配时丢弃旧文件。
const SCHEMA_VERSION: u32 = 1;
/// 磁盘工作集按 3 个并发频道估。
const CONCURRENT_CHANNELS: f64 = 3.0;
/// 工作集余量：另有 2 个频道可能随时开播。
const WORKING_HEADROOM: f64 = 1.5;

/// 一次媒体清单的窗口参数。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowSample {
    /// `#EXT-X-TARGETDURATION`。
    pub target_duration: u64,
    /// `#EXTINF` 条数。
    pub count: u64,
    /// `#EXTINF` 时长之和（秒）。
    pub window_secs: f64,
}

impl WindowSample {
    /// 推荐 TTL（秒）：`clamp(ceil60(W + 2T), 300, 900)`。
    pub fn ttl_secs(&self) -> u64 {
        let raw = self.window_secs + 2.0 * self.target_duration as f64;
        let ceil60 = (raw.max(0.0) / 60.0).ceil() * 60.0;
        (ceil60 as u64).clamp(TTL_MIN_SECS, TTL_MAX_SECS)
    }
}

/// 单个分组的观测累计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct GroupStat {
    /// 最近一次清单的窗口参数。
    window: Option<WindowSample>,
    /// 切片体积 EMA（字节）。
    seg_bytes: Option<f64>,
    /// 累计采样次数。
    samples: u32,
    /// 最近更新时间（Unix 秒）。
    updated_epoch: u64,
}

/// `cache_estimate.json` 的内容。
#[derive(Debug, Default, Serialize, Deserialize)]
struct Persisted {
    #[serde(default)]
    v: u32,
    #[serde(default)]
    groups: HashMap<String, GroupStat>,
}

/// 被动实测统计。
pub struct Estimator {
    groups: RwLock<HashMap<String, GroupStat>>,
    /// 上次落盘时刻；`None` 表示尚未落盘（不节流）。
    last_persist: Mutex<Option<Instant>>,
    /// 是否采集（与 `[cdn].auto_tune` 同步）。
    auto_tune: AtomicBool,
    /// 落盘路径。
    path: PathBuf,
}

impl Estimator {
    /// 读 `<cache>/cache_estimate.json`；缺失、损坏或版本不符时返回空统计。
    pub fn load_or_default(cache_dir: &Path) -> Self {
        let path = cache_dir.join("cache_estimate.json");
        let groups = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Persisted>(&text).ok())
            .filter(|file| file.v == SCHEMA_VERSION)
            .map(|file| file.groups)
            .unwrap_or_default();
        Self {
            groups: RwLock::new(groups),
            last_persist: Mutex::new(None),
            auto_tune: AtomicBool::new(true),
            path,
        }
    }

    /// 同步 `[cdn].auto_tune`。
    pub fn set_auto_tune(&self, on: bool) {
        self.auto_tune.store(on, Ordering::Relaxed);
    }

    /// 当前是否在采集。
    pub fn auto_tune(&self) -> bool {
        self.auto_tune.load(Ordering::Relaxed)
    }

    /// 采集媒体清单的窗口参数；`auto_tune=false` 时直接返回。
    pub fn observe_playlist(&self, gid: &str, body: &str) {
        if !self.auto_tune() {
            return;
        }
        self.record_playlist(gid, body);
    }

    /// 采集切片体积（EMA）；`auto_tune=false` 或 `bytes=0` 时直接返回。
    pub fn observe_segment(&self, gid: &str, bytes: u64) {
        if !self.auto_tune() {
            return;
        }
        self.record_segment(gid, bytes);
    }

    /// 忽略 `auto_tune` 的清单采集；供「立即估算」一次性探测使用。
    pub fn probe_playlist(&self, gid: &str, body: &str) {
        self.record_playlist(gid, body);
    }

    /// 忽略 `auto_tune` 的切片采集；供「立即估算」一次性探测使用。
    pub fn probe_segment(&self, gid: &str, bytes: u64) {
        self.record_segment(gid, bytes);
    }

    fn record_playlist(&self, gid: &str, body: &str) {
        let Some((count, window_secs)) = crate::proxy::media::window_stats(body) else {
            return;
        };
        let target_duration = crate::proxy::media::target_duration(body).unwrap_or_else(|| {
            // 上游没写 TARGETDURATION 时按均值兜底，至少 1 秒。
            (window_secs / count as f64).round().max(1.0) as u64
        });
        let sample = WindowSample {
            target_duration,
            count,
            window_secs,
        };
        let mut groups = self.write();
        let stat = groups.entry(gid.to_string()).or_default();
        stat.window = Some(sample);
        stat.samples = stat.samples.saturating_add(1);
        stat.updated_epoch = now_epoch();
    }

    fn record_segment(&self, gid: &str, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let mut groups = self.write();
        let stat = groups.entry(gid.to_string()).or_default();
        let next = bytes as f64;
        stat.seg_bytes = Some(match stat.seg_bytes {
            Some(prev) => prev * (1.0 - EMA_ALPHA) + next * EMA_ALPHA,
            None => next,
        });
        stat.samples = stat.samples.saturating_add(1);
        stat.updated_epoch = now_epoch();
    }

    /// 推荐 TTL（秒）：各分组推荐值的最大者；无数据返回 `None`。
    pub fn ttl_secs(&self) -> Option<u64> {
        self.read()
            .values()
            .filter_map(|stat| stat.window.as_ref())
            .map(WindowSample::ttl_secs)
            .max()
    }

    /// 工作集（MB）：`round10(3 × N × seg_mb × 1.5)`；缺窗口或切片体积时返回 `None`。
    pub fn working_mb(&self) -> Option<u64> {
        let mut best: Option<f64> = None;
        for stat in self.read().values() {
            let (Some(window), Some(seg_bytes)) = (stat.window.as_ref(), stat.seg_bytes) else {
                continue;
            };
            let seg_mb = seg_bytes / MB as f64;
            let value = CONCURRENT_CHANNELS * window.count as f64 * seg_mb * WORKING_HEADROOM;
            best = Some(best.map_or(value, |prev| prev.max(value)));
        }
        best.map(round10)
    }

    /// 有窗口样本的分组数。
    pub fn group_count(&self) -> usize {
        self.read()
            .values()
            .filter(|stat| stat.window.is_some())
            .count()
    }

    /// 最近一次采样的年龄（秒）；无数据返回 `None`。
    pub fn updated_secs_ago(&self) -> Option<u64> {
        let newest = self
            .read()
            .values()
            .map(|stat| stat.updated_epoch)
            .max()
            .filter(|epoch| *epoch > 0)?;
        Some(now_epoch().saturating_sub(newest))
    }

    /// 落盘；`force=false` 时按 [`PERSIST_MIN_SECS`] 节流，`auto_tune=false` 时跳过。
    pub fn persist(&self, force: bool) {
        if !self.auto_tune() && !force {
            return;
        }
        {
            let mut last = self.lock_persist();
            if !force {
                if let Some(previous) = *last {
                    if previous.elapsed() < Duration::from_secs(PERSIST_MIN_SECS) {
                        return;
                    }
                }
            }
            *last = Some(Instant::now());
        }
        let file = Persisted {
            v: SCHEMA_VERSION,
            groups: self.read().clone(),
        };
        let Ok(text) = serde_json::to_string(&file) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&self.path, text) {
            tracing::warn!(path = %self.path.display(), error = %err, "写估算文件失败");
        }
    }

    /// 清空统计（缓存清空时调用）。
    pub fn clear(&self) {
        self.write().clear();
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, GroupStat>> {
        self.groups
            .read()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, GroupStat>> {
        self.groups
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn lock_persist(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.last_persist
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// 取整到最近的 10（容量估算只求量级）。
fn round10(value: f64) -> u64 {
    ((value / 10.0).round() * 10.0).max(0.0) as u64
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iptv-est-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        dir
    }

    /// 与线上样本同形的媒体清单：T=8、7 片 × 8s。
    fn playlist() -> String {
        let mut body = String::from("#EXTM3U\n#EXT-X-TARGETDURATION:8\n");
        for index in 0..7 {
            body.push_str(&format!("#EXTINF:8.00000000,\n{index}.jpg\n"));
        }
        body
    }

    #[test]
    fn ttl_formula_clamps() {
        let sample = |target, count, window| WindowSample {
            target_duration: target,
            count,
            window_secs: window,
        };
        // 实测样本：W+2T = 72 → ceil60 = 120 → 取 300 下限。
        assert_eq!(sample(8, 7, 56.0).ttl_secs(), 300);
        // 8s × 20 片 = 160 → 160+16 = 176 → ceil60 = 180 → 仍在 300 以内。
        assert_eq!(sample(8, 20, 160.0).ttl_secs(), 300);
        // 8s × 120 片 = 960 → 960+16 = 976 → ceil60 = 1020 → 取 900 上限。
        assert_eq!(sample(8, 120, 960.0).ttl_secs(), 900);
        // 中间段：10s × 10 片 = 100 → 100+20 = 120 → ceil60 = 120 → 300。
        assert_eq!(sample(10, 10, 100.0).ttl_secs(), 300);
        // 40s × 12 片 = 480 → 480+80 = 560 → ceil60 = 600 → 600。
        assert_eq!(sample(40, 12, 480.0).ttl_secs(), 600);
    }

    #[test]
    fn segment_ema_converges() {
        let dir = temp_dir("ema");
        let estimator = Estimator::load_or_default(&dir);
        for _ in 0..40 {
            estimator.observe_segment("g", 4_463_308);
        }
        assert!(estimator.working_mb().is_none(), "缺窗口参数时无工作集");
        estimator.observe_playlist("g", &playlist());
        // 3 × 7 × 4.463308 × 1.5 = 140.6 → round10 = 140（设计稿 §3.3 第 1 行）。
        assert_eq!(estimator.working_mb(), Some(140), "EMA 收敛后与设计稿一致");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_samples_is_none() {
        let dir = temp_dir("empty");
        let estimator = Estimator::load_or_default(&dir);
        assert!(estimator.ttl_secs().is_none());
        assert!(estimator.working_mb().is_none());
        assert_eq!(estimator.group_count(), 0);
        assert!(estimator.updated_secs_ago().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ttl_takes_max_across_groups() {
        let dir = temp_dir("max");
        let estimator = Estimator::load_or_default(&dir);
        estimator.observe_playlist("a", &playlist());
        let mut big = String::from("#EXTM3U\n#EXT-X-TARGETDURATION:10\n");
        for index in 0..40 {
            big.push_str(&format!("#EXTINF:10.0,\n{index}.ts\n"));
        }
        estimator.observe_playlist("b", &big);
        // b: W=400, T=10 → 420 → ceil60 = 420 → 300 与 420 比较取 420。
        assert_eq!(estimator.ttl_secs(), Some(420));
        assert_eq!(estimator.group_count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_round_trip() {
        let dir = temp_dir("round");
        let estimator = Estimator::load_or_default(&dir);
        estimator.observe_playlist("g", &playlist());
        estimator.observe_segment("g", 1_000_000);
        estimator.persist(true);

        let reloaded = Estimator::load_or_default(&dir);
        assert_eq!(reloaded.ttl_secs(), estimator.ttl_secs());
        assert_eq!(reloaded.working_mb(), estimator.working_mb());
        assert_eq!(reloaded.group_count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_schema_is_discarded() {
        let dir = temp_dir("schema");
        std::fs::write(
            dir.join("cache_estimate.json"),
            r#"{"v":999,"groups":{"g":{"window":{"target_duration":8,"count":7,"window_secs":56.0},"seg_bytes":4463308.0,"samples":1,"updated_epoch":1}}}"#,
        )
        .expect("写旧文件");
        let estimator = Estimator::load_or_default(&dir);
        assert_eq!(estimator.group_count(), 0, "版本不符时丢弃");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_ignored() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("cache_estimate.json"), "{ not json").expect("写坏文件");
        let estimator = Estimator::load_or_default(&dir);
        assert_eq!(estimator.group_count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_tune_off_stops_collecting() {
        let dir = temp_dir("off");
        let estimator = Estimator::load_or_default(&dir);
        estimator.set_auto_tune(false);
        estimator.observe_playlist("g", &playlist());
        estimator.observe_segment("g", 4_463_308);
        assert!(estimator.ttl_secs().is_none());
        assert!(estimator.working_mb().is_none());
        assert!(!dir.join("cache_estimate.json").exists(), "关闭后不落盘");

        // 「立即估算」不受 auto_tune 门控。
        estimator.probe_playlist("g", &playlist());
        estimator.probe_segment("g", 4_463_308);
        assert_eq!(estimator.ttl_secs(), Some(300));
        assert!(estimator.working_mb().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_target_duration_falls_back_to_mean() {
        let dir = temp_dir("notd");
        let estimator = Estimator::load_or_default(&dir);
        estimator.observe_playlist("g", "#EXTM3U\n#EXTINF:8.0,\na.ts\n#EXTINF:8.0,\nb.ts\n");
        // T 缺失 → 均值 8 → W=16 → 16+16 = 32 → ceil60 = 60 → 取 300 下限。
        assert_eq!(estimator.ttl_secs(), Some(300));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_drops_samples() {
        let dir = temp_dir("clear");
        let estimator = Estimator::load_or_default(&dir);
        estimator.observe_playlist("g", &playlist());
        estimator.clear();
        assert!(estimator.ttl_secs().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
