//! 逐频道上游健康探测。
//!
//! 证据：F-021 §7。目标 `/panel/api/health` 返回：
//!
//! ```json
//! {"enabled":true,"entries":{"g2/c1":{"checked_at":1792222076,
//!  "consecutive_fails":0,"error":null,"http_status":200,"latency_ms":4,"ok":true}}}
//! ```
//!
//! 周期取 `[health].interval_secs`，超时取 `timeout_secs`，并发取 `concurrency`。
//! 探测只读，不改配置，也不影响播放链路。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::state::AppState;

/// 一条频道的探测结果。
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// 最近一次探测时间（Unix 秒）。
    pub checked_at: i64,
    /// 连续失败次数。
    pub consecutive_fails: u32,
    /// 失败原因。成功时为 `null`。
    pub error: Option<String>,
    /// HTTP 状态码。连接失败时为 `null`。
    pub http_status: Option<u16>,
    /// 往返耗时（毫秒）。连接失败时为 `null`（目标实测）。
    pub latency_ms: Option<u64>,
    /// 本次是否成功。
    pub ok: bool,
}

#[derive(Default)]
pub struct Health {
    entries: Mutex<HashMap<String, Entry>>,
}

fn key(gid: &str, cid: &str) -> String {
    format!("{gid}/{cid}")
}

impl Health {
    pub fn new() -> Self {
        Self::default()
    }

    /// 全部结果。键为 `"<gid>/<cid>"`。
    pub fn snapshot(&self) -> HashMap<String, Entry> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 单频道结果。面板画绿点与 `异常` 徽章用。
    pub fn get(&self, gid: &str, cid: &str) -> Option<Entry> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&key(gid, cid))
            .cloned()
    }

    fn record(
        &self,
        gid: &str,
        cid: &str,
        ok: bool,
        status: Option<u16>,
        error: Option<String>,
        latency_ms: Option<u64>,
    ) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let entry = entries.entry(key(gid, cid)).or_insert(Entry {
            checked_at: 0,
            consecutive_fails: 0,
            error: None,
            http_status: None,
            latency_ms: None,
            ok: false,
        });
        entry.checked_at = crate::schedule::now_epoch();
        entry.http_status = status;
        entry.latency_ms = latency_ms;
        entry.ok = ok;
        if ok {
            entry.consecutive_fails = 0;
            entry.error = None;
        } else {
            entry.consecutive_fails = entry.consecutive_fails.saturating_add(1);
            entry.error = error;
        }
    }
}

/// 后台探测任务。`[health].enabled=false` 时不做任何事。
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            let config = state.snapshot();
            if !config.health.enabled {
                drop(config);
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            let timeout = Duration::from_secs(config.health.timeout_secs.max(1));
            let concurrency = config.health.concurrency.clamp(1, 32) as usize;
            let interval = Duration::from_secs(config.health.interval_secs.max(10));
            let targets: Vec<(String, String, String)> = config
                .groups
                .iter()
                .flat_map(|group| {
                    group
                        .channels
                        .iter()
                        .map(|channel| (group.id.clone(), channel.id.clone(), channel.url.clone()))
                        .collect::<Vec<_>>()
                })
                .collect();
            drop(config);

            let mut chunk = targets.into_iter();
            loop {
                let batch: Vec<(String, String, String)> =
                    chunk.by_ref().take(concurrency).collect();
                if batch.is_empty() {
                    break;
                }
                let mut handles = Vec::with_capacity(batch.len());
                for (gid, cid, url) in batch {
                    let state = state.clone();
                    handles.push(tokio::spawn(async move {
                        let started = Instant::now();
                        let result = state
                            .http_for(&gid, &cid)
                            .get(&url)
                            .timeout(timeout)
                            .send()
                            .await;
                        let latency_ms = started.elapsed().as_millis() as u64;
                        match result {
                            Ok(response) => {
                                let status = response.status().as_u16();
                                let ok = response.status().is_success();
                                let error = (!ok).then(|| format!("HTTP {status}"));
                                state.health.record(
                                    &gid,
                                    &cid,
                                    ok,
                                    Some(status),
                                    error,
                                    Some(latency_ms),
                                );
                            }
                            Err(_) => {
                                // 目标对连接层失败统一写 `连接失败`，且 `latency_ms` 为 null。
                                state.health.record(
                                    &gid,
                                    &cid,
                                    false,
                                    None,
                                    Some("连接失败".to_string()),
                                    None,
                                );
                            }
                        }
                    }));
                }
                for handle in handles {
                    let _ = handle.await;
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_accumulate_and_reset() {
        let health = Health::new();
        health.record("g", "c", false, Some(500), Some("HTTP 500".into()), Some(3));
        health.record("g", "c", false, None, Some("timeout".into()), None);
        let entry = health.get("g", "c").expect("应有记录");
        assert_eq!(entry.consecutive_fails, 2);
        assert!(!entry.ok);
        health.record("g", "c", true, Some(200), None, Some(2));
        let entry = health.get("g", "c").expect("应有记录");
        assert_eq!(entry.consecutive_fails, 0);
        assert!(entry.ok);
        assert!(entry.error.is_none());
    }

    #[test]
    fn missing_channel_returns_none() {
        let health = Health::new();
        assert!(health.get("x", "y").is_none());
        assert!(health.snapshot().is_empty());
    }
}
