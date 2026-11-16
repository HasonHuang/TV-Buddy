//! 质量校验调度：周期、预算与手动入口。

use std::{sync::Arc, time::Duration};

use crate::{config::model::Config, state::AppState};

use super::{
    probe::{ProbeLimits, ProbeMode},
    rules::EffectiveRules,
    state::{RunError, RunMode, RunScope},
};

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
    // 预算小于单频道估算时仍跑一个。否则设置一个合法预算会让功能静默失效。
    by_budget.max(1).min(by_count)
}

/// 判断范围是否指向配置里存在的对象。
pub fn scope_exists(config: &Config, scope: &RunScope) -> bool {
    match scope {
        RunScope::All => true,
        RunScope::Source(id) => config
            .playlist
            .sources
            .iter()
            .any(|source| &source.id == id),
        RunScope::Group(id) => config.group(id).is_some(),
        RunScope::Channel(gid, cid) => config
            .group(gid)
            .is_some_and(|group| group.channels.iter().any(|channel| &channel.id == cid)),
    }
}

pub(crate) fn probe_mode(value: &str) -> ProbeMode {
    match value {
        "playlist" => ProbeMode::Playlist,
        "speed" | "full" => ProbeMode::Speed,
        _ => ProbeMode::Reachability,
    }
}

fn probe_rank(mode: ProbeMode) -> u8 {
    match mode {
        ProbeMode::Reachability => 0,
        ProbeMode::Playlist => 1,
        ProbeMode::Speed => 2,
    }
}

fn mode_at_least(configured: ProbeMode, required: ProbeMode) -> ProbeMode {
    if probe_rank(required) > probe_rank(configured) {
        required
    } else {
        configured
    }
}

/// 规则需要更深的探测数据时自动提升档位。
///
/// 设计见 `docs/design/13-availability-and-speedtest.md` §5.5：
/// 速率规则至少 `speed`，分辨率与广告规则至少 `playlist`。
pub(crate) fn required_probe_mode(config: &Config, rules: &EffectiveRules) -> ProbeMode {
    let mut mode = probe_mode(&config.quality.probe_mode);
    if rules.min_resolution_px.is_some() || rules.max_resolution_px.is_some() {
        mode = mode_at_least(mode, ProbeMode::Playlist);
    }
    if rules.min_speed_mib > 0.0
        || rules
            .resolution_speed_map
            .values()
            .any(|speed| *speed > 0.0)
    {
        mode = mode_at_least(mode, ProbeMode::Speed);
    }
    mode
}

pub(crate) fn probe_limits(config: &Config) -> ProbeLimits {
    ProbeLimits {
        timeout: Duration::from_secs(config.quality.timeout_secs.max(1)),
        speed_timeout: Duration::from_secs(config.quality.speed_timeout_secs.max(1)),
        max_playlist_bytes: (config.quality.max_playlist_kb as usize).saturating_mul(1024),
        max_sample_bytes: config.quality.sample_bytes_kb.saturating_mul(1024),
        max_sample_secs: config.quality.sample_secs,
        sample_segments: config.quality.sample_segments as usize,
    }
}

/// 全局关闭时手动校验只测量。
pub fn measure_only_for(config: &Config) -> bool {
    !config.quality.enabled
}

/// 下一次计划运行时间。关闭或配置非法时返回 `None`。
pub fn next_run_at(config: &Config, last_run: i64, now: i64) -> Option<i64> {
    if !config.quality.enabled {
        return None;
    }
    match config.quality.schedule.as_str() {
        "cron" => crate::cron::cron_preview(
            &config.quality.cron,
            now,
            config.server.timezone_offset_hours as i64,
            1,
        )
        .ok()
        .and_then(|times| times.first().copied()),
        "interval" => {
            if last_run <= 0 {
                return Some(now.saturating_add(60));
            }
            Some(
                last_run.saturating_add(
                    i64::try_from(config.quality.interval_hours)
                        .unwrap_or(i64::MAX / 3_600)
                        .saturating_mul(3_600),
                ),
            )
        }
        _ => None,
    }
}

/// 周期任务是否到期。`last_run` 为上次完成时间。
fn schedule_due(config: &Config, last_run: i64, now: i64) -> bool {
    match config.quality.schedule.as_str() {
        "cron" => crate::cron::cron_due(
            &config.quality.cron,
            now,
            config.server.timezone_offset_hours as i64,
            last_run,
        ),
        "interval" => next_run_at(config, last_run, now).is_some_and(|next| now >= next),
        _ => false,
    }
}

/// 清理过期结果。
pub fn cleanup_quality_results(state: &AppState) -> anyhow::Result<usize> {
    let config = state.snapshot();
    state.quality_store.cleanup(
        config.quality.result_retention_days,
        crate::schedule::now_epoch(),
    )
}

/// 发起一次手动校验。全局关闭时只测量。
pub async fn run_now(
    state: Arc<AppState>,
    scope: RunScope,
    _mode: RunMode,
) -> Result<(String, usize), RunError> {
    let config = (*state.snapshot()).clone();
    if !scope_exists(&config, &scope) {
        return Err(RunError::InvalidScope);
    }
    let run_id = format!("q-{}", uuid::Uuid::new_v4());
    let measure_only = measure_only_for(&config);
    if !state
        .quality
        .try_start_run(&run_id, "manual", 0, measure_only)
    {
        return Err(RunError::Busy);
    }
    let state_for_task = state.clone();
    let run_id_for_task = run_id.clone();
    let scope_for_task = scope.clone();
    tokio::spawn(async move {
        let spec = super::engine::RunSpec {
            run_id: run_id_for_task.clone(),
            trigger: super::engine::RunTrigger::Manual,
            scope: scope_for_task,
            measure_only,
        };
        if let Err(error) = super::engine::run(&state_for_task, spec).await {
            tracing::warn!(run_id = %run_id_for_task, %error, "质量校验失败");
            state_for_task.quality.update_run(|run| {
                run.error = Some(format!("{error:#}"));
                run.phase = "failed".into();
                run.finished = true;
            });
        }
    });
    Ok((run_id, 0))
}

/// 周期调度。启动后等待 60 秒，每 30 秒检查一次。
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let startup_due_at = crate::schedule::now_epoch();
        let mut last_cleanup = 0i64;
        loop {
            let config = (*state.snapshot()).clone();
            let now = crate::schedule::now_epoch();
            if now.saturating_sub(last_cleanup) >= 86_400 {
                if let Err(error) = cleanup_quality_results(&state) {
                    tracing::warn!(%error, "清理质量结果失败");
                }
                last_cleanup = now;
            }
            if !config.quality.enabled || next_run_at(&config, 0, now).is_none() {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            let last_run = state
                .quality_store
                .meta_get("last_run_at")
                .ok()
                .flatten()
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0);
            let due = if last_run <= 0 {
                now >= startup_due_at
            } else {
                schedule_due(&config, last_run, now)
            };
            if !due || state.quality.run_status().is_some_and(|run| !run.finished) {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            let run_id = format!("q-{}", uuid::Uuid::new_v4());
            if !state.quality.try_start_run(&run_id, "schedule", 0, false) {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            let spec = super::engine::RunSpec {
                run_id: run_id.clone(),
                trigger: super::engine::RunTrigger::Schedule,
                scope: RunScope::All,
                measure_only: false,
            };
            if let Err(error) = super::engine::run(&state, spec).await {
                tracing::warn!(%error, "周期质量校验失败");
                state.quality.update_run(|run| {
                    run.error = Some(format!("{error:#}"));
                    run.phase = "failed".into();
                    run.finished = true;
                });
            }
            let finished = crate::schedule::now_epoch();
            let _ = state
                .quality_store
                .meta_set("last_run_at", &finished.to_string());
            if let Some(next) = next_run_at(&(*state.snapshot()).clone(), finished, finished) {
                let _ = state
                    .quality_store
                    .meta_set("next_run_at", &next.to_string());
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{Channel, Config, Group};

    #[test]
    fn cron_is_due_after_previous_boundary() {
        let mut config = Config::default();
        config.quality.enabled = true;
        config.quality.schedule = "cron".into();
        config.quality.cron = "0 * * * *".into();
        let now = 1_700_000_000;
        let offset = config.server.timezone_offset_hours as i64;
        let prev = crate::cron::cron_prev(&config.quality.cron, now, offset).expect("上次触发");
        assert!(
            schedule_due(&config, prev.saturating_sub(1), now),
            "上次运行早于最近触发边界时应到期"
        );
        assert!(
            !schedule_due(&config, prev, now),
            "本小时已经运行过时不应重复触发"
        );
    }

    #[test]
    fn required_probe_mode_promotes_for_speed_and_resolution_rules() {
        let mut config = Config::default();
        let mut rules = EffectiveRules::default();
        assert_eq!(
            required_probe_mode(&config, &rules),
            ProbeMode::Reachability,
            "没有阈值时沿用配置档位"
        );
        rules.min_speed_mib = 1.0;
        assert_eq!(
            required_probe_mode(&config, &rules),
            ProbeMode::Speed,
            "速率规则至少测速"
        );
        rules.min_speed_mib = 0.0;
        rules.max_resolution_px = Some(1920 * 1080);
        assert_eq!(
            required_probe_mode(&config, &rules),
            ProbeMode::Playlist,
            "分辨率规则至少读清单"
        );
        config.quality.probe_mode = "playlist".into();
        rules.resolution_speed_map.insert(1280 * 720, 1.0);
        assert_eq!(
            required_probe_mode(&config, &rules),
            ProbeMode::Speed,
            "分辨率速率映射至少测速"
        );
    }

    #[test]
    fn next_run_interval_and_cron() {
        let mut config = Config::default();
        config.quality.enabled = true;
        config.quality.schedule = "interval".into();
        config.quality.interval_hours = 6;
        assert_eq!(next_run_at(&config, 100, 200), Some(100 + 6 * 3600));

        config.quality.schedule = "cron".into();
        config.quality.cron = "0 * * * *".into();
        let next = next_run_at(&config, 0, 1_700_000_000).expect("next");
        assert!(next > 1_700_000_000);
    }

    #[test]
    fn disabled_config_means_measure_only() {
        let mut config = Config::default();
        config.quality.enabled = false;
        assert!(measure_only_for(&config));
        config.quality.enabled = true;
        assert!(!measure_only_for(&config));
    }

    #[test]
    fn run_limit_follows_budget_and_count_cap() {
        assert_eq!(run_limit(64, 8.0, 0), 8);
        assert_eq!(run_limit(64, 8.0, 3), 3);
        assert_eq!(run_limit(0, 8.0, 5), 5);
        assert_eq!(run_limit(64, 0.0, 0), usize::MAX);
    }

    #[test]
    fn run_limit_keeps_one_channel_for_small_budget() {
        // 预算小于单频道估算时仍要跑一个，否则功能静默失效。
        assert_eq!(run_limit(4, 8.0, 0), 1);
        assert_eq!(run_limit(1, 8.0, 3), 1);
    }

    #[test]
    fn scope_exists_checks_config_ids() {
        let mut config = Config::default();
        config.groups.push(Group {
            id: "g".into(),
            channels: vec![Channel {
                id: "c".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        config
            .playlist
            .sources
            .push(crate::config::model::PlaylistSource {
                id: "s".into(),
                ..Default::default()
            });
        assert!(scope_exists(&config, &RunScope::All));
        assert!(scope_exists(&config, &RunScope::Source("s".into())));
        assert!(!scope_exists(&config, &RunScope::Source("x".into())));
        assert!(scope_exists(&config, &RunScope::Group("g".into())));
        assert!(!scope_exists(&config, &RunScope::Group("x".into())));
        assert!(scope_exists(
            &config,
            &RunScope::Channel("g".into(), "c".into())
        ));
        assert!(!scope_exists(
            &config,
            &RunScope::Channel("g".into(), "x".into())
        ));
    }
}
