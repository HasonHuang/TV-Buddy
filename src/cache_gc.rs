//! 磁盘回收：切片正文只写不删，靠这里兜住 1 GB 盘的可用空间。
//!
//! 依据 `docs/design/06-low-spec-optimization.md` §6.2：
//!
//! - 上限 = `clamp(min(工作集, 可用空间 × ratio − reserve), min_mb, total × 0.7)`；
//! - 淘汰：超龄 → 索引子预算（上限的 10%）→ 超量 LRU（按 mtime）→ 磁盘见底进入紧急模式；
//! - 用 mtime LRU 而**不是**按频道整组淘汰：`segment_path` 的文件名经过 `slug()`，
//!   标识超 48 字符或含非字母数字时被替换成 SHA-256，无法反解回 `gid/cid`；
//!   而直播切片的 mtime 本身就是热度信号（正在看的频道刚被写过），LRU 等价且更简单。
//!
//! 删除文件后**不**重建内存索引：`segments` 里残留条目的后果是命中索引但读盘失败 →
//! 回源一次，无害；重建索引的成本高于收益。

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    cache_estimate::{FALLBACK_DISK_MB, FALLBACK_TTL_SECS, MB, MIN_CONCURRENT_CHANNELS},
    config::model::CacheCfg,
    proxy::segments,
    state::AppState,
};

/// 上限防呆系数：单个缓存最多占文件系统的 70%。
const TOTAL_RATIO: f64 = 0.7;
/// 自动上限最多占可用空间的比例。
const DISK_BUDGET_RATIO: f64 = 0.8;
/// 给系统、日志与其它数据预留的空间（MB）。
const RESERVE_MB: u64 = 100;
/// 自动上限的下限（MB）。
const MIN_CAP_MB: u64 = 50;
/// 回收任务周期（秒）；也是扫描间隔的下限。
const GC_INTERVAL_SECS: u64 = 60;
/// 启动后的首轮延迟，避开冷启动读盘高峰。
const STARTUP_DELAY_SECS: u64 = 30;
/// 单次扫描文件数的下限。
const GC_SCAN_FLOOR: u64 = 500;
/// 单次扫描文件数的上限。
const GC_SCAN_MAX: u64 = 5_000;
/// 单个切片的估算大小（MB），用于由上限推导扫描数。
const SEGMENT_EST_MB: u64 = 4;
/// 磁盘索引占缓存上限的比例。
const INDEX_RATIO: f64 = 0.10;

/// 上限的限制来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitedBy {
    /// `[cache].limit_mb` 非 0。
    Manual,
    /// 实测工作集最小。
    Working,
    /// 可用空间预算最小。
    Budget,
    /// 文件系统总量上限最小。
    Ceiling,
    /// 计算结果低于下限。
    Floor,
    /// 无实测工作集，回退值 150 MB 是限制因素。
    NoData,
    /// 读不到文件系统空间。
    SpaceUnknown,
}

impl LimitedBy {
    /// 返回接口用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Working => "working",
            Self::Budget => "budget",
            Self::Ceiling => "ceiling",
            Self::Floor => "floor",
            Self::NoData => "no_data",
            Self::SpaceUnknown => "space_unknown",
        }
    }
}

/// 一次上限计算的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapPlan {
    /// 目标占用上限（MB）。
    pub cap_mb: u64,
    /// 真正起作用的限制来源。
    pub limited_by: LimitedBy,
    /// 可用空间预算（MB）；读不到空间或手动上限时为 `None`。
    pub budget_mb: Option<u64>,
    /// 文件系统总量上限（MB）；读不到空间或手动上限时为 `None`。
    pub ceiling_mb: Option<u64>,
}

/// 一次扫描的回收计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// 目标占用上限（字节）。
    pub cap_bytes: u64,
    /// 可用空间低于预留：忽略超龄门槛并删到一半。
    pub emergency: bool,
}

/// 解析 `[cache]` 的显式总量上限（MB）；`0` 表示自动计算。
pub fn explicit_cap_mb(cache: &CacheCfg) -> u64 {
    cache.limit_mb
}

/// 计算上限与限制来源。
///
/// 手动上限优先。其余情况：
///
/// - 读不到空间：`max(MIN_CAP_MB, 工作集或回退值)`，来源为 `SpaceUnknown`；
/// - 有空间：`working = min(工作集或回退值, 可用空间 × 0.8 − 100)`，
///   再夹到 `[MIN_CAP_MB, max(总量 × 0.7, MIN_CAP_MB)]`。
///
/// `limited_by` 按「触顶 → 触底 → 预算 → 无数据 → 工作集」的顺序判定。
pub fn cap_plan(explicit_mb: u64, space: Option<(u64, u64)>, working_mb: Option<u64>) -> CapPlan {
    if explicit_mb != 0 {
        return CapPlan {
            cap_mb: explicit_mb,
            limited_by: LimitedBy::Manual,
            budget_mb: None,
            ceiling_mb: None,
        };
    }
    let Some((total_mb, free_mb)) = space else {
        return CapPlan {
            cap_mb: working_mb.unwrap_or(FALLBACK_DISK_MB).max(MIN_CAP_MB),
            limited_by: LimitedBy::SpaceUnknown,
            budget_mb: None,
            ceiling_mb: None,
        };
    };
    let working = working_mb.unwrap_or(FALLBACK_DISK_MB);
    let budget_mb = (free_mb as f64 * DISK_BUDGET_RATIO - RESERVE_MB as f64).max(0.0) as u64;
    let ceiling_mb = ((total_mb as f64 * TOTAL_RATIO) as u64).max(MIN_CAP_MB);
    let raw = working.min(budget_mb);
    let cap_mb = raw.clamp(MIN_CAP_MB, ceiling_mb);
    let limited_by = if raw > ceiling_mb {
        LimitedBy::Ceiling
    } else if raw < MIN_CAP_MB {
        LimitedBy::Floor
    } else if budget_mb < working {
        LimitedBy::Budget
    } else if working_mb.is_none() {
        LimitedBy::NoData
    } else {
        LimitedBy::Working
    };
    CapPlan {
        cap_mb,
        limited_by,
        budget_mb: Some(budget_mb),
        ceiling_mb: Some(ceiling_mb),
    }
}

/// 由上限推导单次扫描的文件数，限制在 `[GC_SCAN_FLOOR, GC_SCAN_MAX]`。
pub fn scan_limit(cap_mb: u64) -> usize {
    (cap_mb / SEGMENT_EST_MB).clamp(GC_SCAN_FLOOR, GC_SCAN_MAX) as usize
}

/// 组装回收计划。`space` 为空（`statvfs` 失败）时按工作集或回退值设限，不进入紧急模式。
pub fn plan(explicit_mb: u64, space: Option<(u64, u64)>, working_mb: Option<u64>) -> Plan {
    let cap = cap_plan(explicit_mb, space, working_mb);
    Plan {
        cap_bytes: cap.cap_mb.saturating_mul(MB),
        emergency: space.is_some_and(|(_, free)| free < RESERVE_MB),
    }
}

/// 取目录所在文件系统的 `(总容量 MB, 可用容量 MB)`；失败返回 `None`。
///
/// 用 `f_bavail` 而非 `f_bfree`：前者是扣除保留块后**非特权用户真正可写**的块数，
/// 才是回收该看的量。乘数用 `f_frsize`（POSIX 规定块计数以 fundamental
/// fragment size 计），为 0 时退回 `f_bsize`。
pub(crate) fn fs_space(dir: &Path) -> Option<(u64, u64)> {
    let vfs = rustix::fs::statvfs(dir).ok()?;
    let unit = if vfs.f_frsize != 0 {
        vfs.f_frsize
    } else {
        vfs.f_bsize
    };
    if unit == 0 {
        return None;
    }
    Some((
        vfs.f_blocks.saturating_mul(unit) / MB,
        vfs.f_bavail.saturating_mul(unit) / MB,
    ))
}

/// 统计目录内的文件数与总字节数（递归）。
pub fn dir_usage(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() {
            files += 1;
            bytes += meta.len();
        } else if meta.is_dir() {
            let (sub_files, sub_bytes) = dir_usage(&entry.path());
            files += sub_files;
            bytes += sub_bytes;
        }
    }
    (files, bytes)
}

/// 淘汰：超龄 → 索引子预算 → 超量 LRU（→ 紧急模式按一半回收）。
///
/// `body_dir` 与 `index_dir` 的占用合计与 `cap_bytes` 比较。
/// 返回 `(删除文件数, 删除字节数)`。
fn sweep(
    body_dir: &Path,
    index_dir: &Path,
    cap_bytes: u64,
    max_age: Duration,
    limit: usize,
    emergency: bool,
) -> (u64, u64) {
    let mut removed_files = 0u64;
    let mut removed_bytes = 0u64;

    // 1) 超龄：两个目录一起按 mtime 从旧到新删。紧急模式忽略门槛。
    if !emergency && !max_age.is_zero() {
        let now = SystemTime::now();
        for (path, len, mtime) in scan_files(&[body_dir, index_dir]) {
            if removed_files as usize >= limit {
                break;
            }
            let age = now.duration_since(mtime).unwrap_or_default();
            if age > max_age {
                drop_file(&path, len, &mut removed_files, &mut removed_bytes);
            }
        }
    }

    // 2) 索引子预算：先压索引，正文至少保留 90%。
    let index_budget = (cap_bytes as f64 * INDEX_RATIO) as u64;
    let (index_files, index_bytes) = segments::prune_index(
        index_dir,
        index_budget,
        limit.saturating_sub(removed_files as usize),
    );
    removed_files += index_files;
    removed_bytes += index_bytes;

    // 3) 重新扫描两个目录，合计当前占用。
    let files = scan_files(&[body_dir, index_dir]);
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();

    // 4) 超量：删到 ≤ cap；紧急模式删到 cap / 2。
    let target = if emergency { cap_bytes / 2 } else { cap_bytes };
    for (path, len, _) in &files {
        if total <= target || removed_files as usize >= limit {
            break;
        }
        if drop_file(path, *len, &mut removed_files, &mut removed_bytes) {
            total = total.saturating_sub(*len);
        }
    }
    if removed_files > 0 {
        tracing::info!(
            files = removed_files,
            mb = removed_bytes / MB,
            emergency,
            "磁盘回收完成"
        );
    }
    (removed_files, removed_bytes)
}

/// 扫描目录内的直接子文件，按 mtime 从旧到新排序。
fn scan_files(dirs: &[&Path]) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut files = Vec::new();
    for dir in dirs {
        let Ok(read) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in read.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            files.push((
                entry.path(),
                meta.len(),
                meta.modified().unwrap_or(UNIX_EPOCH),
            ));
        }
    }
    // 最旧优先：直播切片的 mtime 就是热度信号。
    files.sort_by_key(|(_, _, mtime)| *mtime);
    files
}

fn drop_file(path: &Path, len: u64, files: &mut u64, bytes: &mut u64) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => {
            *files += 1;
            *bytes += len;
            true
        }
        Err(_) => false,
    }
}

/// 起回收任务；每 [`GC_INTERVAL_SECS`] 秒扫描一轮。
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECS)).await;
        loop {
            tick(&state).await;
            tokio::time::sleep(Duration::from_secs(GC_INTERVAL_SECS)).await;
        }
    });
}

/// 一轮扫描：算上限 → 回收 → 顺带落盘实测（省一个定时器）。
async fn tick(state: &Arc<AppState>) {
    let config = state.snapshot();
    state.estimate.persist(false);
    let root = state.cache_dir.clone();
    let body_dir = root.join("seg_bytes");
    let index_dir = root.join("seg_url");
    let window = Duration::from_secs(config.stats.online_window_secs.max(1));
    let active = state.segments.active_channels(window) as u64;
    let working_channels = active.max(MIN_CONCURRENT_CHANNELS);
    let working = state.estimate.working_mb(working_channels);
    let space = fs_space(&root);
    if space.is_none() {
        tracing::warn!(dir = %root.display(), "读磁盘空间失败；按回退值回收");
    }
    let plan = plan(config.cache.limit_mb, space, working);
    let max_age = Duration::from_secs(config.cache.gc.max_age_secs);
    let limit = scan_limit(plan.cap_bytes / MB);
    // 目录遍历是阻塞系统调用：放到阻塞线程池，避免 1 vCPU 上抖动。
    let result = tokio::task::spawn_blocking(move || {
        sweep(
            &body_dir,
            &index_dir,
            plan.cap_bytes,
            max_age,
            limit,
            plan.emergency,
        )
    })
    .await;
    match result {
        Ok((files, bytes)) => {
            if plan.emergency {
                tracing::warn!(files, mb = bytes / MB, "可用空间低于预留，已紧急回收");
            }
            // 只有真正完成一轮扫描才更新“上次回收”。任务失败时保留旧值。
            state
                .gc_last_run_epoch
                .store(now_epoch(), std::sync::atomic::Ordering::Relaxed);
        }
        Err(error) => tracing::warn!(error = %error, "回收任务失败"),
    }
}

/// 面板用推荐摘要。
///
/// `ttl_source` 为 `manual`（显式配置优先）/ `measured`（有实测）/ `fallback`（回退值）；
/// `cap_source` 为 `manual` / `auto`。上限与回收共用 [`cap_plan`] 的结果。
pub fn recommendation(state: &AppState) -> serde_json::Value {
    let config = state.snapshot();
    let measured_ttl = state.estimate.ttl_secs();

    let ttl_manual = config.cache.ttl.segment_secs != 0;
    let ttl_source = if ttl_manual {
        "manual"
    } else if measured_ttl.is_some() {
        "measured"
    } else {
        "fallback"
    };
    let ttl_secs = if ttl_manual {
        config.cache.ttl.segment_secs
    } else {
        measured_ttl.unwrap_or(FALLBACK_TTL_SECS)
    };

    let window = Duration::from_secs(config.stats.online_window_secs.max(1));
    let channels = state.segments.active_channels(window) as u64;
    let working_channels = channels.max(MIN_CONCURRENT_CHANNELS);
    let working = state.estimate.working_mb(working_channels);
    let space = fs_space(&state.cache_dir);
    let cap = cap_plan(config.cache.limit_mb, space, working);
    let cap_source = if config.cache.limit_mb != 0 {
        "manual"
    } else {
        "auto"
    };

    let (segment_body_files, segment_body_bytes) = dir_usage(&state.cache_dir.join("seg_bytes"));
    let (_, segment_index_bytes) = segments::index_usage(&state.cache_dir.join("seg_url"));
    let segment_index_limit_bytes = (cap.cap_mb.saturating_mul(MB) as f64 * INDEX_RATIO) as u64;

    let last = state
        .gc_last_run_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    let gc_last_run_secs_ago = (last != 0).then(|| now_epoch().saturating_sub(last));

    serde_json::json!({
        "ttl_secs": ttl_secs,
        "ttl_source": ttl_source,
        "disk_cap_mb": cap.cap_mb,
        "cap_source": cap_source,
        "limited_by": cap.limited_by.as_str(),
        "channels": channels,
        "working_channels": working_channels,
        "working_mb": working,
        "budget_mb": cap.budget_mb,
        "ceiling_mb": cap.ceiling_mb,
        "gc_last_run_secs_ago": gc_last_run_secs_ago,
        "groups": state.estimate.group_count(),
        "updated_secs_ago": state.estimate.updated_secs_ago(),
        "disk_total_mb": space.map(|(total, _)| total),
        "disk_free_mb": space.map(|(_, free)| free),
        "segment_body_bytes": segment_body_bytes,
        "segment_index_bytes": segment_index_bytes,
        "segment_index_limit_bytes": segment_index_limit_bytes,
        "segment_cache_bytes": segment_body_bytes.saturating_add(segment_index_bytes),
        "segment_body_files": segment_body_files,
        "auto_tune": state.estimate.auto_tune(),
    })
}

/// 当前 Unix 秒。
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_plan_reports_limit_source() {
        let plan = cap_plan(0, Some((1024, 800)), Some(140));
        assert_eq!((plan.cap_mb, plan.limited_by), (140, LimitedBy::Working));
        assert_eq!(plan.budget_mb, Some(540));
        assert_eq!(plan.ceiling_mb, Some(716));

        let plan = cap_plan(0, Some((1024, 300)), Some(140));
        assert_eq!((plan.cap_mb, plan.limited_by), (140, LimitedBy::Working));

        let plan = cap_plan(0, Some((2048, 2000)), Some(3000));
        assert_eq!((plan.cap_mb, plan.limited_by), (1433, LimitedBy::Ceiling));
    }

    #[test]
    fn cap_plan_reports_budget_limit() {
        // 可用空间预算 140 MB < 工作集 200 MB，未触顶也未触底。
        let plan = cap_plan(0, Some((1024, 300)), Some(200));
        assert_eq!((plan.cap_mb, plan.limited_by), (140, LimitedBy::Budget));
        assert_eq!(plan.budget_mb, Some(140));
        assert_eq!(plan.ceiling_mb, Some(716));
    }

    #[test]
    fn cap_plan_fallback_participates_in_clamp() {
        let plan = cap_plan(0, Some((1024, 800)), None);
        assert_eq!((plan.cap_mb, plan.limited_by), (150, LimitedBy::NoData));

        let plan = cap_plan(0, Some((1024, 120)), None);
        assert_eq!((plan.cap_mb, plan.limited_by), (50, LimitedBy::Floor));
    }

    #[test]
    fn cap_plan_space_unknown_applies_floor() {
        let plan = cap_plan(0, None, Some(140));
        assert_eq!(
            (plan.cap_mb, plan.limited_by),
            (140, LimitedBy::SpaceUnknown)
        );
        assert_eq!(plan.budget_mb, None);
        assert_eq!(plan.ceiling_mb, None);

        let plan = cap_plan(0, None, Some(30));
        assert_eq!(
            (plan.cap_mb, plan.limited_by),
            (50, LimitedBy::SpaceUnknown)
        );

        let plan = cap_plan(0, None, None);
        assert_eq!(
            (plan.cap_mb, plan.limited_by),
            (150, LimitedBy::SpaceUnknown)
        );
    }

    #[test]
    fn manual_cap_wins() {
        let plan = cap_plan(200, Some((1024, 800)), Some(140));
        assert_eq!((plan.cap_mb, plan.limited_by), (200, LimitedBy::Manual));
        assert_eq!(plan.budget_mb, None);
        assert_eq!(plan.ceiling_mb, None);
    }

    /// 显式上限只读 `[cache].limit_mb`。
    #[test]
    fn explicit_cap_reads_limit_mb() {
        let cache = CacheCfg {
            limit_mb: 120,
            ..CacheCfg::default()
        };
        assert_eq!(explicit_cap_mb(&cache), 120);
    }

    #[test]
    fn scan_limit_clamps() {
        assert_eq!(scan_limit(140), 500);
        assert_eq!(scan_limit(20_000), 5_000);
        assert_eq!(scan_limit(4_000), 1_000);
    }

    #[test]
    fn plan_emergency_when_below_reserve() {
        assert!(!plan(0, Some((1024, 800)), Some(140)).emergency);
        assert!(plan(0, Some((1024, 80)), Some(140)).emergency);
        // statvfs 失败：用工作集上限，不紧急。
        let fallback = plan(0, None, Some(140));
        assert!(!fallback.emergency);
        assert_eq!(
            fallback.cap_bytes,
            140 * MB,
            "无空间数据时用工作集而非固定 150 MB"
        );
        // 显式值在读空间失败时仍然生效。
        assert_eq!(plan(200, None, Some(140)).cap_bytes, 200 * MB);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tv-buddy-gc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        dir
    }

    /// 造一个指定大小与 mtime 的文件。
    fn touch(path: &Path, bytes: usize, age: Duration) {
        std::fs::write(path, vec![b'x'; bytes]).expect("写文件");
        let when = SystemTime::now() - age;
        std::fs::File::open(path)
            .expect("打开")
            .set_modified(when)
            .expect("设置 mtime");
    }

    #[test]
    fn sweep_removes_by_age() {
        let dir = temp_dir("age");
        let index = temp_dir("age-index");
        for index in 0..5 {
            let age = if index < 3 {
                Duration::from_secs(3_600)
            } else {
                Duration::from_secs(10)
            };
            touch(&dir.join(format!("{index}.jpg")), 1_000, age);
        }
        let (files, bytes) = sweep(
            &dir,
            &index,
            1_000_000,
            Duration::from_secs(1_800),
            500,
            false,
        );
        assert_eq!(files, 3);
        assert_eq!(bytes, 3_000);
        assert_eq!(std::fs::read_dir(&dir).expect("读目录").count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&index);
    }

    #[test]
    fn sweep_removes_by_age_across_dirs() {
        let bytes_dir = temp_dir("multi-bytes");
        let url_dir = temp_dir("multi-url");
        touch(&bytes_dir.join("a.jpg"), 1_000, Duration::from_secs(3_600));
        touch(&url_dir.join("b.json"), 1_000, Duration::from_secs(3_600));
        touch(&url_dir.join("c.json"), 1_000, Duration::from_secs(5));
        let (files, _) = sweep(
            &bytes_dir,
            &url_dir,
            1_000_000,
            Duration::from_secs(1_800),
            500,
            false,
        );
        assert_eq!(files, 2, "两个目录一起按龄淘汰");
        assert_eq!(std::fs::read_dir(&url_dir).expect("读目录").count(), 1);
        let _ = std::fs::remove_dir_all(&bytes_dir);
        let _ = std::fs::remove_dir_all(&url_dir);
    }

    #[test]
    fn sweep_prunes_index_before_total() {
        let body = temp_dir("idx-body");
        let index = temp_dir("idx-index");
        for n in 0..10 {
            touch(
                &body.join(format!("b{n}.jpg")),
                1_000,
                Duration::from_secs(300),
            );
        }
        for n in 0..20 {
            let age = Duration::from_secs(10 * (20 - n));
            touch(&index.join(format!("{n}.json")), 1_000, age);
        }
        // cap 10 KB → 索引子预算 1 KB，总量 10 KB。
        let (files, _) = sweep(
            &body,
            &index,
            10_000,
            Duration::from_secs(86_400),
            500,
            false,
        );
        assert!(files >= 19, "至少删掉 19 个超额索引");
        let index_left = std::fs::read_dir(&index).expect("读索引目录").count();
        assert_eq!(index_left, 1, "索引保留 1 个文件");
        let total = std::fs::read_dir(&body).expect("读正文目录").count() as u64 * 1_000
            + index_left as u64 * 1_000;
        assert!(total <= 10_000, "总量应回收到上限内");
        let _ = std::fs::remove_dir_all(&body);
        let _ = std::fs::remove_dir_all(&index);
    }

    #[test]
    fn zero_max_age_skips_age_pruning() {
        let dir = temp_dir("zero-age");
        let index = temp_dir("zero-age-index");
        for n in 0..3 {
            touch(
                &dir.join(format!("{n}.jpg")),
                1_000,
                Duration::from_secs(3_600),
            );
        }
        let (files, _) = sweep(&dir, &index, 1_000_000, Duration::ZERO, 500, false);
        assert_eq!(files, 0, "max_age=0 表示不按时间删除");
        assert_eq!(std::fs::read_dir(&dir).expect("读目录").count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&index);
    }

    #[test]
    fn sweep_enforces_cap_lru() {
        let dir = temp_dir("lru");
        let index = temp_dir("lru-index");
        // 10 个文件，越新越大：最旧的先被删。
        for index in 0..10 {
            let age = Duration::from_secs(10 * (10 - index));
            touch(&dir.join(format!("{index}.jpg")), 1_000, age);
        }
        let (files, bytes) = sweep(&dir, &index, 3_000, Duration::from_secs(86_400), 500, false);
        assert_eq!(files, 7, "10KB 收到 3KB");
        assert_eq!(bytes, 7_000);
        // 留下的三个是最新的（7、8、9）。
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .expect("读目录")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, vec!["7.jpg", "8.jpg", "9.jpg"]);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&index);
    }

    #[test]
    fn emergency_ignores_age_and_halves() {
        let dir = temp_dir("emergency");
        let index = temp_dir("emergency-index");
        for index in 0..4 {
            touch(
                &dir.join(format!("{index}.jpg")),
                1_000,
                Duration::from_secs(5),
            );
        }
        // 未紧急：都新鲜 → 一个不删（超量按 cap 处理，这里 cap 足够）。
        let (files, _) = sweep(
            &dir,
            &index,
            100_000,
            Duration::from_secs(1_800),
            500,
            false,
        );
        assert_eq!(files, 0);
        // 紧急：忽略 max_age，删到 cap / 2 = 2KB。
        let (files, bytes) = sweep(&dir, &index, 4_000, Duration::from_secs(1_800), 500, true);
        assert_eq!(files, 2);
        assert_eq!(bytes, 2_000);
        assert_eq!(std::fs::read_dir(&dir).expect("读目录").count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&index);
    }

    #[test]
    fn scan_limit_bounds_work() {
        let dir = temp_dir("limit");
        let index = temp_dir("limit-index");
        for index in 0..20 {
            touch(
                &dir.join(format!("{index}.jpg")),
                1_000,
                Duration::from_secs(3_600),
            );
        }
        let (files, _) = sweep(&dir, &index, 1_000, Duration::from_secs(1_800), 5, false);
        assert_eq!(files, 5, "单次最多处理 scan_limit 个");
        assert_eq!(std::fs::read_dir(&dir).expect("读目录").count(), 15);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&index);
    }

    #[test]
    fn fs_space_missing_dir_is_none() {
        assert!(fs_space(Path::new("/definitely/not/here")).is_none());
    }

    #[test]
    fn fs_space_reports_positive_values() {
        let (total_mb, free_mb) = fs_space(&std::env::temp_dir()).expect("读到空间");
        assert!(total_mb > 0, "总容量应为正: {total_mb}");
        assert!(free_mb <= total_mb, "可用不应超过总量");
    }

    #[test]
    fn dir_usage_counts_files_and_bytes() {
        let dir = temp_dir("usage");
        touch(&dir.join("a.jpg"), 1_200, Duration::from_secs(1));
        std::fs::create_dir_all(dir.join("nested")).expect("建子目录");
        touch(&dir.join("nested/b.jpg"), 800, Duration::from_secs(1));
        assert_eq!(dir_usage(&dir), (2, 2_000));
        assert_eq!(dir_usage(&dir.join("missing")), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
