//! 运行指标：出网字节、在线观众、进程资源与磁盘占用。
//!
//! 字段口径见 `docs/findings/dynamic/11-security-and-metrics.md` §6。要点：
//!
//! - `traffic_in_bytes` 目标恒为 0（S-BUG-01）。本模块不采集，只输出 0。
//! - `online_viewers` 只在**带播放 Token** 的播放请求上计数。取
//!   `[stats].online_window_secs` 窗口内的不同 Token 数。
//! - 进程内存与 CPU 读 `/proc`。非 Linux 平台返回 0。
//! - 磁盘占用按目录遍历，结果缓存 30 秒。

use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

/// 磁盘采样间隔。
const DISK_TTL: Duration = Duration::from_secs(30);
/// `whole_file_budget_mb=0` 时目标报的字节数。
const DEFAULT_WHOLE_FILE_BUDGET: u64 = 512 * 1024 * 1024;

/// 磁盘采样结果。
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskSample {
    pub cache_mb: u64,
    pub recording_mb: u64,
}

struct CachedDisk {
    at: Instant,
    sample: DiskSample,
}

#[derive(Default)]
pub struct Metrics {
    out_bytes: AtomicU64,
    /// 在线记录：(时间, 观众键, 频道键)。频道键形如 `gid/cid`。
    viewers: Mutex<VecDeque<(i64, String, String)>>,
    disk: Mutex<Option<CachedDisk>>,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次出网字节。
    pub fn add_out(&self, bytes: u64) {
        self.out_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn out_bytes(&self) -> u64 {
        self.out_bytes.load(Ordering::Relaxed)
    }

    /// 记一次在线观看，并关联频道键（`gid/cid`）。只保留窗口内的记录。
    pub fn viewer(&self, token: &str, channel: &str) {
        let now = crate::schedule::now_epoch();
        let mut viewers = self.viewers.lock().unwrap_or_else(|e| e.into_inner());
        viewers.push_back((now, token.to_string(), channel.to_string()));
        // 保留 1 小时，足够覆盖任何窗口；过旧记录在这里裁掉。
        while let Some((ts, _, _)) = viewers.front() {
            if now - *ts > 3600 {
                viewers.pop_front();
            } else {
                break;
            }
        }
    }

    /// 窗口内的不同观众数。
    pub fn online(&self, window_secs: u64) -> usize {
        let now = crate::schedule::now_epoch();
        let window = window_secs.max(1) as i64;
        let viewers = self.viewers.lock().unwrap_or_else(|e| e.into_inner());
        let mut seen: Vec<&str> = viewers
            .iter()
            .filter(|(ts, _, _)| now - *ts <= window)
            .map(|(_, token, _)| token.as_str())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    }

    /// 窗口内每频道的不同观众数，按人数降序、频道键升序排列。
    ///
    /// 只含有人的频道。人数为 0 的频道不出现。
    pub fn online_by_channel(&self, window_secs: u64) -> Vec<(String, usize)> {
        let now = crate::schedule::now_epoch();
        let window = window_secs.max(1) as i64;
        let viewers = self.viewers.lock().unwrap_or_else(|e| e.into_inner());
        let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
        for (ts, token, channel) in viewers.iter() {
            if now - *ts <= window {
                map.entry(channel.as_str())
                    .or_default()
                    .push(token.as_str());
            }
        }
        let mut out: Vec<(String, usize)> = map
            .into_iter()
            .map(|(channel, mut tokens)| {
                tokens.sort_unstable();
                tokens.dedup();
                (channel.to_string(), tokens.len())
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// 磁盘占用（MB）。`cache_dir` 为缓存根，录制目录在其下。
    pub fn disk(&self, cache_dir: &Path) -> DiskSample {
        let mut guard = self.disk.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = guard.as_ref() {
            if cached.at.elapsed() < DISK_TTL {
                return cached.sample;
            }
        }
        let recordings = dir_bytes(&cache_dir.join("recordings"));
        let total = dir_bytes(cache_dir);
        let sample = DiskSample {
            cache_mb: total.saturating_sub(recordings) / (1024 * 1024),
            recording_mb: recordings / (1024 * 1024),
        };
        *guard = Some(CachedDisk {
            at: Instant::now(),
            sample,
        });
        sample
    }
}

/// 目录总字节数。读不到按 0。
fn dir_bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            total += dir_bytes(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

/// 进程常驻内存（MB）。
pub fn rss_mb() -> u64 {
    let Ok(text) = fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let pages: u64 = text
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    pages * page_size() / (1024 * 1024)
}

/// 进程累计 CPU 秒数。
pub fn cpu_seconds() -> f64 {
    let Ok(text) = fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    // 字段以 `comm` 之后的 `)` 为界，避免进程名里的空格干扰。
    let Some(rest) = text.rsplit_once(')').map(|(_, rest)| rest) else {
        return 0.0;
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` 从 state 开始；utime 是第 14 个总字段，即这里的下标 11。
    let ticks: u64 = fields
        .get(11)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
        + fields
            .get(12)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
    ticks as f64 / clock_ticks()
}

/// 可用于工作的 CPU 核数。
pub fn cpu_cores() -> u64 {
    std::thread::available_parallelism()
        .map(|value| value.get() as u64)
        .unwrap_or(1)
}

/// `whole_file_budget_mb` 换算成字节。0 表示目标默认 512 MB。
pub fn whole_file_budget(mb: u64) -> u64 {
    if mb == 0 {
        DEFAULT_WHOLE_FILE_BUDGET
    } else {
        mb.saturating_mul(1024 * 1024)
    }
}

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    // Linux 常见页大小。避免引入 libc 依赖。
    4096
}

#[cfg(not(target_os = "linux"))]
fn page_size() -> u64 {
    4096
}

#[cfg(target_os = "linux")]
fn clock_ticks() -> f64 {
    // 用户态 HZ 在常见 Linux 发行版上为 100。
    100.0
}

#[cfg(not(target_os = "linux"))]
fn clock_ticks() -> f64 {
    100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_counts_distinct_tokens_in_window() {
        let metrics = Metrics::new();
        metrics.viewer("a", "g1/c1");
        metrics.viewer("a", "g1/c1");
        metrics.viewer("b", "g1/c1");
        assert_eq!(metrics.online(90), 2);
        // 窗口为 0 按 1 秒算，仍然计入刚写入的记录。
        assert_eq!(metrics.online(0), 2);
    }

    #[test]
    fn expired_viewers_fall_out_of_window() {
        let metrics = Metrics::new();
        metrics.viewer("old", "g1/c1");
        {
            let mut viewers = metrics.viewers.lock().unwrap();
            if let Some(front) = viewers.front_mut() {
                front.0 = crate::schedule::now_epoch() - 600;
            }
        }
        assert_eq!(metrics.online(90), 0);
        assert_eq!(metrics.online(3600), 1);
    }

    #[test]
    fn online_by_channel_groups_and_sorts() {
        let metrics = Metrics::new();
        metrics.viewer("a", "g1/c1");
        metrics.viewer("a", "g1/c1");
        metrics.viewer("b", "g1/c1");
        metrics.viewer("c", "g2/d1");
        let rows = metrics.online_by_channel(90);
        assert_eq!(
            rows,
            vec![("g1/c1".to_string(), 2), ("g2/d1".to_string(), 1)]
        );
    }

    #[test]
    fn out_bytes_accumulate() {
        let metrics = Metrics::new();
        metrics.add_out(10);
        metrics.add_out(5);
        assert_eq!(metrics.out_bytes(), 15);
    }

    #[test]
    fn whole_file_budget_defaults_to_512_mb() {
        assert_eq!(whole_file_budget(0), 536_870_912);
        assert_eq!(whole_file_budget(8), 8 * 1024 * 1024);
    }

    #[test]
    fn dir_bytes_walks_nested_dirs() {
        let dir = std::env::temp_dir().join(format!("metrics-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("a"), [0u8; 3]).unwrap();
        fs::write(dir.join("nested/b"), [0u8; 4]).unwrap();
        assert_eq!(dir_bytes(&dir), 7);
        let _ = fs::remove_dir_all(dir);
    }
}
