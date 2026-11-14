//! 指标历史：SQLite 采样与窗口聚合。
//!
//! 采样任务每 10 秒写一行，保留 7 天。查询按窗口聚合，返回约 30–170 个点。
//! 原始行只含两张趋势图需要的四项：主机网卡入/出速率与主机/进程 CPU。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::{db::SharedConn, state::AppState};

/// 采样间隔（秒）。
pub const SAMPLE_SECS: u64 = 10;
/// 数据保留时长（秒）：7 天。
pub const RETENTION_SECS: i64 = 7 * 24 * 3600;
/// 清理间隔（秒）。
const PURGE_SECS: u64 = 3600;

/// 一条采样。
#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    pub ts: i64,
    pub net_rx: f64,
    pub net_tx: f64,
    pub cpu_host: f64,
    pub cpu_proc: f64,
}

/// 趋势窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    M5,
    M10,
    H1,
    H24,
    D7,
}

impl Range {
    /// 解析查询参数。未知值返回 `None`。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "5m" => Some(Self::M5),
            "10m" => Some(Self::M10),
            "1h" => Some(Self::H1),
            "24h" => Some(Self::H24),
            "7d" => Some(Self::D7),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::M5 => "5m",
            Self::M10 => "10m",
            Self::H1 => "1h",
            Self::H24 => "24h",
            Self::D7 => "7d",
        }
    }

    /// 查询时间范围（秒）。
    pub fn window_secs(self) -> i64 {
        match self {
            Self::M5 => 300,
            Self::M10 => 600,
            Self::H1 => 3600,
            Self::H24 => 86_400,
            Self::D7 => 7 * 86_400,
        }
    }

    /// 聚合桶宽（秒）。5m/10m 直接使用 10 秒原始点。
    pub fn bucket_secs(self) -> i64 {
        match self {
            Self::M5 | Self::M10 => SAMPLE_SECS as i64,
            Self::H1 => 30,
            Self::H24 => 600,
            Self::D7 => 3600,
        }
    }
}

/// 历史表。
#[derive(Clone)]
pub struct Store {
    conn: SharedConn,
}

impl Store {
    /// 打开历史表。建表是幂等的；失败时中止启动，避免功能静默不可用。
    pub fn new(conn: SharedConn) -> anyhow::Result<Self> {
        {
            let guard = conn.lock().unwrap_or_else(|error| error.into_inner());
            guard
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS metrics_sample (
                         ts INTEGER PRIMARY KEY NOT NULL,
                         net_rx REAL NOT NULL,
                         net_tx REAL NOT NULL,
                         cpu_host REAL NOT NULL,
                         cpu_proc REAL NOT NULL
                     );",
                )
                .context("建指标历史表失败")?;
        }
        Ok(Self { conn })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 写一条采样。同一秒重复写入时覆盖。
    pub fn insert(&self, sample: &Sample) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO metrics_sample
                 (ts, net_rx, net_tx, cpu_host, cpu_proc)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                sample.ts,
                sample.net_rx,
                sample.net_tx,
                sample.cpu_host,
                sample.cpu_proc
            ],
        )?;
        Ok(())
    }

    /// 删除 `before` 之前的行。
    pub fn purge(&self, before: i64) -> rusqlite::Result<usize> {
        self.lock()
            .execute("DELETE FROM metrics_sample WHERE ts < ?1", params![before])
    }

    /// 按窗口聚合。`now` 由调用方给出，测试可注入固定时间。
    fn query(&self, range: Range, now: i64) -> rusqlite::Result<Vec<Sample>> {
        let bucket = range.bucket_secs();
        let since = now - range.window_secs();
        let conn = self.lock();
        let mut statement = conn.prepare(
            "SELECT (ts / ?1) * ?1 AS bucket,
                    AVG(net_rx), AVG(net_tx), AVG(cpu_host), AVG(cpu_proc)
             FROM metrics_sample
             WHERE ts >= ?2
             GROUP BY bucket
             ORDER BY bucket",
        )?;
        let rows = statement.query_map(params![bucket, since], |row| {
            Ok(Sample {
                ts: row.get(0)?,
                net_rx: row.get(1)?,
                net_tx: row.get(2)?,
                cpu_host: row.get(3)?,
                cpu_proc: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// 查询最近一个窗口的聚合点。
    pub fn history(&self, range: Range) -> rusqlite::Result<Vec<Sample>> {
        self.query(range, crate::schedule::now_epoch())
    }
}

/// 启动采样任务。
///
/// 第一次 tick 只建立基线，不写库。之后每 10 秒写一行，每小时清理过期行。
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(SAMPLE_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous: Option<(Instant, u64, u64, f64)> = None;
        let mut purged_at = Instant::now();
        // 启动后先清一次，之后每小时一次。
        let mut purge_due = true;
        loop {
            ticker.tick().await;
            let now = Instant::now();
            let (rx, tx) = crate::metrics::net_bytes();
            let cpu = crate::metrics::cpu_seconds();
            let cores = crate::metrics::cpu_cores().max(1) as f64;
            let host = state.metrics.host_cpu_percent();
            if let Some((last_at, last_rx, last_tx, last_cpu)) = previous {
                let seconds = now.duration_since(last_at).as_secs_f64();
                if seconds > 0.0 {
                    let sample = Sample {
                        ts: crate::schedule::now_epoch(),
                        net_rx: rx.saturating_sub(last_rx) as f64 / seconds,
                        net_tx: tx.saturating_sub(last_tx) as f64 / seconds,
                        cpu_host: host,
                        cpu_proc: (cpu - last_cpu).max(0.0) / seconds / cores * 100.0,
                    };
                    if let Err(error) = state.history.insert(&sample) {
                        tracing::warn!(%error, "写指标采样失败");
                    }
                }
            }
            previous = Some((now, rx, tx, cpu));
            if purge_due || purged_at.elapsed() >= Duration::from_secs(PURGE_SECS) {
                purge_due = false;
                purged_at = Instant::now();
                let before = crate::schedule::now_epoch() - RETENTION_SECS;
                match state.history.purge(before) {
                    Ok(removed) if removed > 0 => {
                        tracing::info!(removed, "已清理过期指标采样");
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, "清理指标采样失败"),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn store() -> Store {
        let conn = Connection::open_in_memory().unwrap();
        Store::new(Arc::new(Mutex::new(conn))).unwrap()
    }

    fn sample(ts: i64, value: f64) -> Sample {
        Sample {
            ts,
            net_rx: value,
            net_tx: value * 2.0,
            cpu_host: value,
            cpu_proc: value / 2.0,
        }
    }

    #[test]
    fn parses_ranges() {
        assert_eq!(Range::parse("5m"), Some(Range::M5));
        assert_eq!(Range::parse("7d"), Some(Range::D7));
        assert_eq!(Range::parse("bad"), None);
        assert_eq!(Range::M5.bucket_secs(), 10);
        assert_eq!(Range::H1.bucket_secs(), 30);
        assert_eq!(Range::D7.window_secs(), 604_800);
    }

    #[test]
    fn aggregates_by_bucket() {
        let store = store();
        // 同一 30 秒桶内两条，另有一条在下一桶。
        store.insert(&sample(1_000, 10.0)).unwrap();
        store.insert(&sample(1_010, 20.0)).unwrap();
        store.insert(&sample(1_050, 30.0)).unwrap();
        store.insert(&sample(900, 99.0)).unwrap();
        // 固定 now，避免依赖真实时钟。
        let points = store.query(Range::H1, 1_100).unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].ts, 900);
        assert_eq!(points[1].ts, 990);
        assert!((points[1].net_rx - 15.0).abs() < 1e-9);
        assert_eq!(points[2].ts, 1_050);
        assert!((points[2].net_rx - 30.0).abs() < 1e-9);
    }

    #[test]
    fn purges_old_rows() {
        let store = store();
        store.insert(&sample(100, 1.0)).unwrap();
        store.insert(&sample(200, 1.0)).unwrap();
        assert_eq!(store.purge(150).unwrap(), 1);
        let points = store.query(Range::D7, 250).unwrap();
        assert_eq!(points.len(), 1);
    }
}
