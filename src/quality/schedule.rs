//! 质量校验调度：目标选择、预算与周期运行。

use std::{sync::Arc, time::Duration};

use futures_util::future::join_all;
use tokio::sync::Semaphore;

use crate::{
    config::model::{Channel, Config},
    state::AppState,
};

use super::{
    probe::{probe, ProbeLimits, ProbeMode},
    rules::{effective_rules, evaluate, Action, Reason},
    state::{
        url_hash, Entry, RunError, RunMode, RunScope, FLAG_OK, FLAG_REACHABLE,
    },
};

/// 选出需要校验的频道。`force` 时忽略质量启用状态，仅供手动干跑。
pub fn target_channels(config: &Config, force: bool) -> Vec<(String, String, Channel)> {
    let mut targets = Vec::new();
    for group in &config.groups {
        if group.access == "archived" {
            continue;
        }
        if group.mode == "passthrough" && !config.quality.probe_passthrough {
            continue;
        }
        for channel in &group.channels {
            if !force && !effective_rules(config, group, channel).enabled {
                continue;
            }
            targets.push((group.id.clone(), channel.id.clone(), channel.clone()));
        }
    }
    targets
}

/// 单轮可处理的频道数。
pub fn run_limit(budget_mb: u64, per_channel_mb: f64, max_channels: u64) -> usize {
    let by_count = if max_channels == 0 {
        usize::MAX
    } else {
        usize::try_from(max_channels).unwrap_or(usize::MAX)
    };
    if budget_mb == 0 || per_channel_mb <= 0.0 {
        return by_count;
    }
    let by_budget = (budget_mb as f64 / per_channel_mb).floor() as usize;
    by_budget.min(by_count)
}

fn probe_mode(value: &str) -> ProbeMode {
    match value {
        "playlist" => ProbeMode::Playlist,
        "speed" | "full" => ProbeMode::Speed,
        _ => ProbeMode::Reachability,
    }
}

fn probe_limits(config: &Config) -> ProbeLimits {
    ProbeLimits {
        timeout: Duration::from_secs(config.quality.timeout_secs.max(1)),
        speed_timeout: Duration::from_secs(config.quality.speed_timeout_secs.max(1)),
        max_playlist_bytes: (config.quality.max_playlist_kb as usize).saturating_mul(1024),
        max_sample_bytes: config.quality.sample_bytes_kb.saturating_mul(1024),
        max_sample_secs: config.quality.sample_secs,
        sample_segments: config.quality.sample_segments as usize,
    }
}

fn scope_matches(scope: &RunScope, gid: &str, cid: &str) -> bool {
    match scope {
        RunScope::All => true,
        RunScope::Source(source_id) => {
            // 源范围在 target 过滤时按 channel.source_id 处理；这里只占位。
            let _ = source_id;
            true
        }
        RunScope::Group(group_id) => group_id == gid,
        RunScope::Channel(want_gid, want_cid) => want_gid == gid && want_cid == cid,
    }
}

fn targets_for_scope(
    config: &Config,
    scope: &RunScope,
    force: bool,
) -> Vec<(String, String, Channel)> {
    target_channels(config, force)
        .into_iter()
        .filter(|(gid, cid, _)| scope_matches(scope, gid, cid))
        .filter(|(_, _, channel)| match scope {
            RunScope::Source(source_id) => &channel.source_id == source_id,
            _ => true,
        })
        .collect()
}

/// 校验一组频道。调用方先占用运行状态。
async fn run_targets(
    state: Arc<AppState>,
    targets: Vec<(String, String, Channel)>,
    config: Config,
    force: bool,
) {
    let config = Arc::new(config);
    let limits = probe_limits(&config);
    let mode = probe_mode(&config.quality.probe_mode);
    let concurrency = config.quality.concurrency.clamp(1, 32) as usize;
    let interval = config.quality.interval_secs;
    let budget_bytes = config.quality.budget_mb_per_run.saturating_mul(1024 * 1024);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut tested = 0usize;
    let mut valid = 0usize;
    let mut hidden = 0usize;
    let mut bytes = 0u64;

    for chunk in targets.chunks(concurrency) {
        if state.quality.is_cancelled() {
            break;
        }
        if budget_bytes > 0 && bytes >= budget_bytes.saturating_mul(9) / 10 {
            break;
        }
        let mut jobs = Vec::new();
        for (gid, cid, channel) in chunk.iter().cloned() {
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let state = state.clone();
            let config = config.clone();
            let limits = limits.clone();
            jobs.push(tokio::spawn(async move {
                let _permit = permit;
                probe_one(&state, &config, &gid, &cid, &channel, mode, &limits, interval, force)
                    .await
            }));
        }
        for job in join_all(jobs).await {
            let Ok((gid, cid, entry, out, hidden_one)) = job else {
                continue;
            };
            bytes = bytes.saturating_add(out.bytes);
            tested += 1;
            if entry.ok() {
                valid += 1;
            }
            if hidden_one {
                hidden += 1;
            }
            state.quality.put(&gid, &cid, entry);
            state.quality.update_run(|run| {
                run.tested = tested;
                run.valid = valid;
                run.hidden = hidden;
                run.bytes = bytes;
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn probe_one(
    state: &Arc<AppState>,
    config: &Config,
    gid: &str,
    cid: &str,
    channel: &Channel,
    mode: ProbeMode,
    limits: &ProbeLimits,
    interval: u64,
    _force: bool,
) -> (String, String, Entry, super::probe::ProbeOutput, bool) {
    let now = crate::schedule::now_epoch();
    let Some(group) = config.group(gid) else {
        let entry = missing_entry(channel, now, interval);
        return (
            gid.to_string(),
            cid.to_string(),
            entry,
            empty_output(),
            false,
        );
    };
    let rules = effective_rules(config, group, channel);
    let previous = state.quality.get(gid, cid);
    let url = match crate::resolver::resolve(state, gid, cid, channel).await {
        Ok(Some(url)) => url,
        Ok(None) => channel.url.clone(),
        Err(error) => {
            let mut entry = missing_entry(channel, now, interval);
            entry.first_fail_at = previous.as_ref().and_then(|entry| entry.first_fail_at);
            entry.consecutive_fails = previous.as_ref().map_or(0, |entry| entry.consecutive_fails);
            entry.last_ok_at = previous.as_ref().and_then(|entry| entry.last_ok_at);
            tracing::debug!(gid, cid, %error, "质量探测地址解析失败");
            return (
                gid.to_string(),
                cid.to_string(),
                entry,
                empty_output(),
                false,
            );
        }
    };
    let headers = crate::http::routes::stream::channel_headers(state, gid, cid);
    let client = state.probe_client_for(gid, cid);
    let out = probe(
        &client,
        &url,
        &headers,
        mode,
        rules.max_resolution_px,
        limits,
    )
    .await;
    let verdict = evaluate(&rules, &out.measurement);

    let entry = build_entry(
        channel,
        previous.as_ref(),
        now,
        interval,
        &out,
        verdict.ok,
        verdict.reason,
        verdict.reachable,
    );

    if !out.measurement.probe_error {
        state.health.record(
            gid,
            cid,
            out.measurement.reachable,
            out.measurement.http_status,
            if out.measurement.reachable {
                None
            } else {
                out.measurement.error.clone()
            },
            out.measurement.delay_ms.map(u64::from),
        );
    }

    let hidden = !verdict.ok
        && rules.action == Action::Hide
        && !rules.retain
        && verdict.reason != Reason::ProbeError;
    (gid.to_string(), cid.to_string(), entry, out, hidden)
}

fn empty_output() -> super::probe::ProbeOutput {
    super::probe::ProbeOutput {
        measurement: super::rules::Measurement {
            reachable: false,
            probe_error: true,
            http_status: None,
            delay_ms: None,
            speed_mib: None,
            width: None,
            height: None,
            error: None,
        },
        bytes: 0,
        duration_ms: 0,
    }
}

fn missing_entry(channel: &Channel, now: i64, interval: u64) -> Entry {
    Entry {
        url_hash: url_hash(&channel.url),
        checked_at: now,
        first_fail_at: None,
        last_ok_at: None,
        next_check_at: now + interval_or_hour(interval) as i64,
        delay_ms: 0,
        speed_kib: 0,
        width: 0,
        height: 0,
        flags: 0,
        reason: Reason::ProbeError,
        consecutive_fails: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_entry(
    channel: &Channel,
    previous: Option<&Entry>,
    now: i64,
    interval: u64,
    out: &super::probe::ProbeOutput,
    ok: bool,
    reason: Reason,
    reachable: bool,
) -> Entry {
    let previous = previous.cloned().unwrap_or_default();
    let probe_error = reason == Reason::ProbeError;
    let failed = !ok && !probe_error;
    let consecutive = if ok {
        0
    } else if probe_error {
        previous.consecutive_fails
    } else {
        previous.consecutive_fails.saturating_add(1)
    };
    let first_fail_at = if ok {
        None
    } else if probe_error {
        previous.first_fail_at
    } else {
        previous.first_fail_at.or(Some(now))
    };
    let next_check_at = if failed && consecutive >= 3 {
        let backoff = 60u64.saturating_mul(1u64 << u32::from(consecutive.min(20)));
        now + interval.max(backoff.min(86_400)) as i64
    } else {
        now + interval_or_hour(interval) as i64
    };
    let mut flags = 0;
    if ok {
        flags |= FLAG_OK;
    }
    if reachable {
        flags |= FLAG_REACHABLE;
    }
    Entry {
        url_hash: url_hash(&channel.url),
        checked_at: now,
        first_fail_at,
        last_ok_at: if ok {
            Some(now)
        } else {
            previous.last_ok_at
        },
        next_check_at,
        delay_ms: out.measurement.delay_ms.unwrap_or(0),
        speed_kib: (out.measurement.speed_mib.unwrap_or(0.0) * 1024.0) as u32,
        width: out.measurement.width.unwrap_or(0),
        height: out.measurement.height.unwrap_or(0),
        flags,
        reason,
        consecutive_fails: consecutive.min(u16::MAX),
    }
}

fn interval_or_hour(interval: u64) -> u64 {
    if interval == 0 {
        3600
    } else {
        interval
    }
}

/// 发起一次手动校验。
pub async fn run_now(
    state: Arc<AppState>,
    scope: RunScope,
    _mode: RunMode,
    force: bool,
) -> Result<String, RunError> {
    let config = (*state.snapshot()).clone();
    if !config.quality.enabled && !force {
        return Err(RunError::Disabled);
    }
    let mut targets = targets_for_scope(&config, &scope, force);
    targets.truncate(run_limit(
        config.quality.budget_mb_per_run,
        8.0,
        config.quality.max_channels_per_run,
    ));
    if targets.is_empty() {
        return Err(RunError::InvalidScope);
    }
    let run_id = format!("q-{}", uuid::Uuid::new_v4());
    if !state.quality.try_start_run(&run_id, targets.len()) {
        return Err(RunError::Busy);
    }
    let state_for_task = state.clone();
    let run_id_for_task = run_id.clone();
    tokio::spawn(async move {
        run_targets(state_for_task.clone(), targets, config, force).await;
        state_for_task.quality.update_run(|run| {
            run.finished = true;
            run.run_id = run_id_for_task;
        });
        state_for_task.quality.finish_run();
    });
    Ok(run_id)
}

/// 周期调度。启动后等待 60 秒，每 60 秒检查一次。
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            let config = (*state.snapshot()).clone();
            if !config.quality.enabled || config.quality.interval_secs == 0 {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            let now = crate::schedule::now_epoch();
            let mut due: Vec<(String, String, Channel)> = target_channels(&config, false)
                .into_iter()
                .filter(|(gid, cid, _)| {
                    state
                        .quality
                        .get(gid, cid)
                        .is_none_or(|entry| entry.next_check_at <= now)
                })
                .collect();
            due.sort_by_key(|(gid, cid, _)| {
                state
                    .quality
                    .get(gid, cid)
                    .map_or(0, |entry| entry.next_check_at)
            });
            let limit = run_limit(
                config.quality.budget_mb_per_run,
                8.0,
                config.quality.max_channels_per_run,
            );
            due.truncate(limit);
            if due.is_empty() {
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
            let run_id = format!("q-{}", uuid::Uuid::new_v4());
            if !state.quality.try_start_run(&run_id, due.len()) {
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
            run_targets(state.clone(), due, config, false).await;
            state.quality.finish_run();
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{Channel, Config, Group, QualityOverride};

    #[test]
    fn targets_skip_archived_and_passthrough_unless_enabled() {
        let mut config = Config::default();
        config.quality.enabled = true;
        config.groups.push(Group {
            id: "archived".into(),
            access: "archived".into(),
            channels: vec![Channel {
                id: "c".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        config.groups.push(Group {
            id: "pass".into(),
            mode: "passthrough".into(),
            channels: vec![Channel {
                id: "c".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        config.groups.push(Group {
            id: "proxy".into(),
            mode: "proxy".into(),
            channels: vec![Channel {
                id: "c".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        let targets = target_channels(&config, false);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0, "proxy");
    }

    #[test]
    fn targets_respect_scope_and_disabled_overrides() {
        let mut config = Config::default();
        config.quality.enabled = true;
        config.groups.push(Group {
            id: "g".into(),
            quality: Some(QualityOverride {
                enabled: Some(false),
                ..QualityOverride::default()
            }),
            channels: vec![Channel {
                id: "c".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        assert!(target_channels(&config, false).is_empty());
        assert_eq!(target_channels(&config, true).len(), 1);
    }


    #[test]
    fn build_entry_tracks_first_failure_and_recovers() {
        let channel = Channel { id: "c".into(), url: "http://up/c.m3u8".into(), ..Channel::default() };
        let down = super::super::probe::ProbeOutput {
            measurement: super::super::rules::Measurement {
                reachable: false,
                probe_error: false,
                http_status: None,
                delay_ms: None,
                speed_mib: None,
                width: None,
                height: None,
                error: Some("timeout".into()),
            },
            bytes: 0,
            duration_ms: 0,
        };
        let now = 1000;
        let first = build_entry(&channel, None, now, 900, &down, false, Reason::Unreachable, false);
        assert_eq!(first.first_fail_at, Some(now));
        assert_eq!(first.consecutive_fails, 1);

        let second = build_entry(&channel, Some(&first), now + 900, 900, &down, false, Reason::Unreachable, false);
        assert_eq!(second.first_fail_at, Some(now), "重复失败不重置起点");
        assert_eq!(second.consecutive_fails, 2);

        let ok = super::super::probe::ProbeOutput {
            measurement: super::super::rules::Measurement {
                reachable: true,
                probe_error: false,
                http_status: Some(200),
                delay_ms: Some(10),
                speed_mib: Some(2.0),
                width: Some(1920),
                height: Some(1080),
                error: None,
            },
            bytes: 10,
            duration_ms: 10,
        };
        let recovered = build_entry(&channel, Some(&second), now + 1800, 900, &ok, true, Reason::Pass, true);
        assert_eq!(recovered.first_fail_at, None);
        assert_eq!(recovered.consecutive_fails, 0);
        assert_eq!(recovered.last_ok_at, Some(now + 1800));
        assert!(recovered.ok());

        let probe_error = build_entry(&channel, Some(&second), now + 1800, 900, &down, false, Reason::ProbeError, true);
        assert_eq!(probe_error.first_fail_at, second.first_fail_at);
        assert_eq!(probe_error.consecutive_fails, second.consecutive_fails);
    }

    #[test]
    fn run_limit_follows_budget_and_count_cap() {
        assert_eq!(run_limit(64, 8.0, 0), 8);
        assert_eq!(run_limit(64, 8.0, 3), 3);
        assert_eq!(run_limit(0, 8.0, 5), 5);
        assert_eq!(run_limit(64, 0.0, 0), usize::MAX);
    }
}
