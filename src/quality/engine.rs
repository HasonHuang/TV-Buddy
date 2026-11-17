//! 周期与手动准入引擎：探测、判定、移出与结果提交。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;

use crate::{
    config::model::{Channel, Config, Group},
    quality::{
        admission::{url_hash_hex, Candidate, CandidateScope},
        gate::{probe_candidate, CandidateOutcome, Guard, RunMemo},
        rules::{effective_rules, Reason},
        state::{Entry, RunScope, FLAG_OK, FLAG_REACHABLE},
        store::{QualityResult, QualityStore},
    },
    state::AppState,
};

/// 任务触发方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTrigger {
    Schedule,
    Manual,
}

impl RunTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::Manual => "manual",
        }
    }
}

/// 一次任务参数。
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub run_id: String,
    pub trigger: RunTrigger,
    pub scope: RunScope,
    pub measure_only: bool,
}

/// 待移出频道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub gid: String,
    pub cid: String,
    pub url_hash: String,
}

/// 任务汇总。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: String,
    pub tested: usize,
    pub admitted: usize,
    pub removed: usize,
    pub rejected: usize,
    pub skipped: usize,
    pub hidden: usize,
    pub paused_by_guard: bool,
}

/// 判定结果。
#[derive(Debug, Clone, Default)]
pub struct EngineDecision {
    pub removals: Vec<Removal>,
    pub manual_entries: Vec<(String, String, Entry)>,
    pub results: Vec<QualityResult>,
    pub tested: usize,
    pub admitted: usize,
    pub removed: usize,
    pub rejected: usize,
    pub skipped: usize,
    pub hidden: usize,
    pub kept: usize,
    pub paused: bool,
}

/// 选出到期目标。
pub fn due_targets(
    config: &Config,
    results: &QualityStore,
    scope: &RunScope,
    now: i64,
    limit: usize,
    measure_only: bool,
    include_not_due: bool,
) -> Result<Vec<Candidate>> {
    let mut items = Vec::new();
    let mut order = 0usize;
    for group in &config.groups {
        if group.access == "archived" {
            continue;
        }
        if group.mode == "passthrough" && !config.quality.probe_passthrough {
            continue;
        }
        for channel in &group.channels {
            if channel.url.trim().is_empty() || !scope_matches(scope, group, channel) {
                continue;
            }
            let source = source_of(config, channel);
            if let Some(source) = source {
                if !source.enabled {
                    continue;
                }
                // 源门控关闭时，该源不参与周期移出，即使分组或频道覆盖开启规则。
                if source.quality.as_ref().and_then(|quality| quality.enabled) == Some(false) {
                    continue;
                }
            }
            let rules = effective_rules(config, group, channel);
            if !measure_only && !rules.enabled {
                continue;
            }
            let manual = channel.source_id.is_empty();
            let key = if manual {
                format!("channel:{}/{}", group.id, channel.id)
            } else {
                format!("sync:{}:{}", channel.source_id, channel.source_key)
            };
            let previous = results.get(&key)?;
            if let Some(previous) = previous.as_ref() {
                // 地址变化被门控暂停时，新地址还没写入配置。
                // 此时按旧地址重测会把刚通过的新地址判死，破坏暂停语义。
                if matches!(previous.action.as_str(), "held" | "removal_paused")
                    && previous.url_hash != url_hash_hex(&channel.url)
                {
                    continue;
                }
            }
            let next_check_at = previous.as_ref().map_or(0, |result| result.next_check_at);
            if !include_not_due && next_check_at > now {
                continue;
            }
            items.push((
                next_check_at,
                order,
                Candidate {
                    key,
                    scope: if manual {
                        CandidateScope::Manual
                    } else {
                        CandidateScope::Sync
                    },
                    source_id: channel.source_id.clone(),
                    source_key: channel.source_key.clone(),
                    gid: group.id.clone(),
                    cid: channel.id.clone(),
                    name: channel.name.clone(),
                    target_group: group.name.clone(),
                    url: channel.url.clone(),
                    existing_url: None,
                    headers: candidate_headers(group, channel),
                    proxy: config
                        .proxy_for(&group.id, &channel.id)
                        .or_else(|| source.and_then(|source| source.proxy.as_ref()))
                        .cloned(),
                    rules,
                    url_hash: url_hash_hex(&channel.url),
                    is_url_change: false,
                    sync_follow: channel.sync_follow,
                    offline_on_fail: config.offline_on_fail_for(&group.id, &channel.id),
                    interval_secs: config.quality.interval_hours.saturating_mul(3_600),
                    previous_first_fail_at: previous
                        .as_ref()
                        .and_then(|result| result.first_fail_at),
                },
            ));
            order += 1;
        }
    }
    items.sort_by_key(|(next, order, _)| (*next, *order));
    let mut targets: Vec<Candidate> = items
        .into_iter()
        .map(|(_, _, candidate)| candidate)
        .collect();
    if limit > 0 {
        targets.truncate(limit);
    }
    Ok(targets)
}

/// 移出提交前复核。
pub fn removal_allowed(config: &Config, removal: &Removal) -> bool {
    let Some(group) = config.group(&removal.gid) else {
        return false;
    };
    let Some(channel) = group
        .channels
        .iter()
        .find(|channel| channel.id == removal.cid)
    else {
        return false;
    };
    if channel.source_id.is_empty()
        || !channel.sync_follow
        || url_hash_hex(&channel.url) != removal.url_hash
    {
        return false;
    }
    let Some(source) = source_of(config, channel) else {
        return false;
    };
    if source.quality.as_ref().and_then(|quality| quality.enabled) == Some(false) {
        return false;
    }
    source.enabled && effective_rules(config, group, channel).enabled
}

/// 判定一组探测结果。
pub fn decide(outcomes: &[CandidateOutcome], measure_only: bool, guard: &Guard) -> EngineDecision {
    let now = crate::schedule::now_epoch();
    let mut decision = EngineDecision {
        tested: outcomes.len(),
        ..EngineDecision::default()
    };
    let sync_removals = outcomes
        .iter()
        .filter(|outcome| {
            outcome.candidate.scope == CandidateScope::Sync
                && outcome.candidate.sync_follow
                && !outcome.verdict.ok
                && outcome.verdict.reason != Reason::ProbeError
        })
        .count();
    let sync_tested = outcomes
        .iter()
        .filter(|outcome| {
            outcome.candidate.scope == CandidateScope::Sync
                && outcome.candidate.sync_follow
                && !matches!(outcome.verdict.reason, Reason::ProbeError | Reason::Unknown)
        })
        .count();
    let paused = !measure_only && guard_trip(guard, sync_tested, sync_removals);

    for outcome in outcomes {
        let candidate = &outcome.candidate;
        if measure_only {
            // 同步频道只测量，不写结果。写 kept 会覆盖 admitted 行并改写计数。
            if candidate.scope == CandidateScope::Manual {
                decision
                    .results
                    .push(result_for(candidate, outcome, "kept", now));
            }
            decision.kept += 1;
            continue;
        }
        match candidate.scope {
            CandidateScope::Manual => {
                if outcome.verdict.reason == Reason::ProbeError {
                    decision
                        .results
                        .push(result_for(candidate, outcome, "kept", now));
                    decision.kept += 1;
                    continue;
                }
                let action = if outcome.verdict.ok {
                    "kept"
                } else if manual_hide_due(candidate, now) {
                    "hidden"
                } else {
                    "kept"
                };
                if action == "hidden" {
                    decision.hidden += 1;
                } else {
                    decision.kept += 1;
                }
                // 宽限内失败不写 QualityState，否则订阅渲染会立即下线。
                if action == "hidden" || outcome.verdict.ok {
                    decision.manual_entries.push((
                        candidate.gid.clone(),
                        candidate.cid.clone(),
                        entry_for(candidate, outcome, now),
                    ));
                }
                decision
                    .results
                    .push(result_for(candidate, outcome, action, now));
            }
            CandidateScope::Sync => {
                if outcome.verdict.ok {
                    let action = if paused { "held" } else { "admitted" };
                    decision.admitted += 1;
                    decision
                        .results
                        .push(result_for(candidate, outcome, action, now));
                } else if !candidate.sync_follow {
                    // 固定频道保留管理员编辑的字段，不参与移出。
                    decision.kept += 1;
                    decision
                        .results
                        .push(result_for(candidate, outcome, "kept", now));
                } else if outcome.verdict.reason == Reason::ProbeError {
                    decision.skipped += 1;
                    decision
                        .results
                        .push(result_for(candidate, outcome, "skipped", now));
                } else if paused {
                    decision
                        .results
                        .push(result_for(candidate, outcome, "removal_paused", now));
                } else {
                    decision.removed += 1;
                    decision.removals.push(Removal {
                        gid: candidate.gid.clone(),
                        cid: candidate.cid.clone(),
                        url_hash: candidate.url_hash.clone(),
                    });
                    decision
                        .results
                        .push(result_for(candidate, outcome, "removed", now));
                }
            }
        }
    }
    decision.paused = paused;
    decision
}

pub async fn run(state: &Arc<AppState>, spec: RunSpec) -> Result<RunSummary> {
    let _admission = state.admission_lock.lock().await;
    let config = (*state.snapshot()).clone();
    let measure_only = spec.measure_only || !config.quality.enabled;
    let now = crate::schedule::now_epoch();
    let limit = if config.quality.max_channels_per_run == 0 {
        usize::MAX
    } else {
        usize::try_from(config.quality.max_channels_per_run).unwrap_or(usize::MAX)
    };
    let targets = due_targets(
        &config,
        &state.quality_store,
        &spec.scope,
        now,
        limit,
        measure_only,
        spec.trigger == RunTrigger::Manual,
    )?;
    state.quality.update_run(|run| {
        run.phase = "probing".into();
        run.total = targets.len();
        run.measure_only = measure_only;
        run.trigger = spec.trigger.as_str().into();
    });

    let guard = Guard {
        enabled: config.quality.guard_enabled,
        max_remove_ratio: config.quality.max_remove_ratio,
        min_remove_count: usize::try_from(config.quality.min_remove_count).unwrap_or(usize::MAX),
        base_channels: guard_base_channels(&config, &spec.scope),
    };
    let concurrency = usize::try_from(config.quality.concurrency.clamp(1, 32)).unwrap_or(1);
    let budget_bytes = config.quality.budget_mb_per_run.saturating_mul(1024 * 1024);
    let time_budget = config.quality.gate_time_budget_secs;
    let started = Instant::now();
    let memo = std::sync::Mutex::new(RunMemo::default());
    let mut outcomes = Vec::new();
    let mut bytes = 0u64;
    for chunk in targets.chunks(concurrency) {
        if state.quality.is_cancelled()
            || budget_reached(bytes, budget_bytes)
            || time_budget_reached(started, time_budget)
        {
            break;
        }
        let memo_ref = &memo;
        let config_ref = &config;
        let state_ref = state;
        let jobs = chunk.iter().cloned().map(|candidate| {
            let state = state_ref;
            let config = config_ref;
            let memo = memo_ref;
            async move { probe_candidate(state, config, &candidate, memo, true).await }
        });
        for outcome in futures_util::future::join_all(jobs).await {
            bytes = bytes.saturating_add(outcome.bytes);
            outcomes.push(outcome);
        }
        state.quality.update_run(|run| {
            run.tested = outcomes.len();
            run.bytes = bytes;
        });
    }

    let decision = decide(&outcomes, measure_only, &guard);
    let EngineDecision {
        removals,
        manual_entries,
        mut results,
        tested,
        admitted,
        removed,
        rejected,
        skipped,
        hidden,
        paused,
        kept,
    } = decision;
    for result in &mut results {
        result.run_id = spec.run_id.clone();
    }
    let summary = RunSummary {
        run_id: spec.run_id.clone(),
        tested,
        admitted,
        removed,
        rejected,
        skipped,
        hidden,
        paused_by_guard: paused,
    };
    let mut meta = vec![
        ("last_run_at".to_string(), now.to_string()),
        ("last_run_id".to_string(), spec.run_id.clone()),
        (
            "last_run_summary".to_string(),
            serde_json::json!({
                "tested": tested,
                "admitted": admitted,
                "removed": removed,
                "rejected": rejected,
                "skipped": skipped,
                "hidden": hidden,
                "kept": kept,
                "paused": paused,
            })
            .to_string(),
        ),
    ];
    meta.push((
        "guard_tripped_at".to_string(),
        if paused {
            now.to_string()
        } else {
            String::new()
        },
    ));

    let manual_entries = if removals.is_empty() || measure_only {
        state.commit_quality_results(&meta, results, |config, results| {
            revalidate_results(config, results);
            keep_existing_manual(config, manual_entries.clone())
        })?
    } else {
        let mut accepted = Vec::new();
        state.mutate_admission(&meta, |config| {
            for removal in &removals {
                if removal_allowed(config, removal) {
                    if let Some(group) = config.group_mut(&removal.gid) {
                        group.channels.retain(|channel| channel.id != removal.cid);
                    }
                } else if let Some(result) = results
                    .iter_mut()
                    .find(|result| result.gid == removal.gid && result.cid == removal.cid)
                {
                    result.action = "skipped".into();
                    result.reason = "source_changed".into();
                }
            }
            revalidate_results(config, &mut results);
            accepted = keep_existing_manual(config, manual_entries);
            Ok(results)
        })?;
        accepted
    };
    for (gid, cid, entry) in manual_entries {
        state.quality.put(&gid, &cid, entry);
    }
    state.quality.update_run(|run| {
        run.phase = if paused {
            "paused_by_guard".into()
        } else if run.cancelled {
            "cancelled".into()
        } else {
            "done".into()
        };
        run.admitted = admitted;
        run.removed = removed;
        run.rejected = rejected;
        run.skipped = skipped;
        run.hidden = hidden;
        run.paused_by_guard = paused;
        run.measure_only = measure_only;
        run.tested = tested;
        run.bytes = bytes;
    });
    state.quality.finish_run_id(&spec.run_id);
    Ok(summary)
}

fn budget_reached(bytes: u64, budget_bytes: u64) -> bool {
    budget_bytes > 0 && bytes >= budget_bytes.saturating_mul(9) / 10
}

/// 单轮墙钟是否耗尽。0 表示不限制。
fn time_budget_reached(started: Instant, budget_secs: u64) -> bool {
    budget_secs > 0 && started.elapsed() >= Duration::from_secs(budget_secs)
}

/// 提交前按最新配置复核。频道已删除、源已停用或地址已变化时不改结果状态。
///
/// 移出结果本身会让频道不再存在，因此要保留。否则结果表仍显示旧的
/// `admitted`，与配置不一致。
fn revalidate_results(config: &Config, results: &mut Vec<QualityResult>) {
    results.retain_mut(|result| {
        let is_removal = matches!(result.action.as_str(), "removed" | "source_removed");
        let Some(group) = config.group(&result.gid) else {
            return is_removal;
        };
        let Some(channel) = group
            .channels
            .iter()
            .find(|channel| channel.id == result.cid)
        else {
            return is_removal;
        };
        if result.scope == "manual" {
            return channel.source_id.is_empty();
        }
        let source_ok = config
            .playlist
            .sources
            .iter()
            .any(|source| source.id == result.source_id && source.enabled);
        if !source_ok || url_hash_hex(&channel.url) != result.url_hash {
            result.action = "skipped".into();
            result.reason = "source_changed".into();
            result.ok = false;
        }
        true
    });
}

fn keep_existing_manual(
    config: &Config,
    entries: Vec<(String, String, Entry)>,
) -> Vec<(String, String, Entry)> {
    entries
        .into_iter()
        .filter(|(gid, cid, _)| {
            config
                .group(gid)
                .and_then(|group| group.channels.iter().find(|channel| &channel.id == cid))
                .is_some_and(|channel| channel.source_id.is_empty())
        })
        .collect()
}

fn manual_hide_due(candidate: &Candidate, now: i64) -> bool {
    if !candidate.offline_on_fail {
        return false;
    }
    if candidate.rules.stale_keep_secs == 0 {
        return true;
    }
    let Some(first_fail) = candidate.previous_first_fail_at else {
        return false;
    };
    let grace = i64::try_from(candidate.rules.stale_keep_secs).unwrap_or(i64::MAX);
    now.saturating_sub(first_fail) >= grace
}

fn guard_trip(guard: &Guard, tested: usize, removals: usize) -> bool {
    guard.enabled
        && tested > 0
        && removals >= guard.effective_min_remove_count(removals)
        && removals as f64 / tested as f64 >= guard.max_remove_ratio
}

fn source_of<'a>(
    config: &'a Config,
    channel: &Channel,
) -> Option<&'a crate::config::model::PlaylistSource> {
    config
        .playlist
        .sources
        .iter()
        .find(|source| source.id == channel.source_id)
}

fn scope_matches(scope: &RunScope, group: &Group, channel: &Channel) -> bool {
    match scope {
        RunScope::All => true,
        RunScope::Source(id) => &channel.source_id == id,
        RunScope::Group(id) => &group.id == id,
        RunScope::Channel(gid, cid) => &group.id == gid && &channel.id == cid,
    }
}

/// 本轮范围内可移出的同步频道总数，用于小源安全阀阈值。
fn guard_base_channels(config: &Config, scope: &RunScope) -> usize {
    config
        .groups
        .iter()
        .filter(|group| group.access != "archived")
        .filter(|group| group.mode != "passthrough" || config.quality.probe_passthrough)
        .flat_map(|group| {
            group
                .channels
                .iter()
                .filter(move |channel| scope_matches(scope, group, channel))
                .filter(|channel| !channel.source_id.is_empty() && channel.sync_follow)
                .filter(|channel| {
                    source_of(config, channel).is_some_and(|source| {
                        source.enabled
                            && source.quality.as_ref().and_then(|quality| quality.enabled)
                                != Some(false)
                    }) && effective_rules(config, group, channel).enabled
                })
        })
        .count()
}

fn candidate_headers(group: &Group, channel: &Channel) -> Vec<(String, String)> {
    use crate::http::routes::stream::{merge_upstream_headers, user_agent};
    merge_upstream_headers(
        &group.headers,
        user_agent(&group.extra),
        &channel.headers,
        user_agent(&channel.extra),
    )
}

fn entry_for(candidate: &Candidate, outcome: &CandidateOutcome, now: i64) -> Entry {
    let ok = outcome.verdict.ok;
    Entry {
        url_hash: crate::quality::state::url_hash(&candidate.url),
        checked_at: now,
        first_fail_at: if ok {
            None
        } else if outcome.verdict.reason == Reason::ProbeError {
            candidate.previous_first_fail_at
        } else {
            candidate.previous_first_fail_at.or(Some(now))
        },
        last_ok_at: ok.then_some(now),
        next_check_at: now.saturating_add(candidate.interval_secs as i64),
        delay_ms: outcome.measurement.delay_ms.unwrap_or(0),
        speed_kib: outcome
            .measurement
            .speed_mib
            .map(|speed| (speed * 1024.0).clamp(0.0, u32::MAX as f64) as u32)
            .unwrap_or(0),
        width: outcome.measurement.width.unwrap_or(0),
        height: outcome.measurement.height.unwrap_or(0),
        flags: (if ok { FLAG_OK } else { 0 })
            | (if outcome.measurement.reachable {
                FLAG_REACHABLE
            } else {
                0
            }),
        reason: outcome.verdict.reason,
        consecutive_fails: 0,
    }
}

fn result_for(
    candidate: &Candidate,
    outcome: &CandidateOutcome,
    action: &str,
    now: i64,
) -> QualityResult {
    QualityResult {
        key: candidate.key.clone(),
        scope: if candidate.scope == CandidateScope::Manual {
            "manual"
        } else {
            "sync"
        }
        .into(),
        source_id: candidate.source_id.clone(),
        source_key: candidate.source_key.clone(),
        gid: candidate.gid.clone(),
        cid: candidate.cid.clone(),
        name: candidate.name.clone(),
        target_group: candidate.target_group.clone(),
        url_hash: candidate.url_hash.clone(),
        action: action.into(),
        reason: outcome.verdict.reason.as_str().into(),
        ok: outcome.verdict.ok,
        delay_ms: outcome.measurement.delay_ms,
        speed_kib: outcome
            .measurement
            .speed_mib
            .map(|speed| (speed * 1024.0).clamp(0.0, u32::MAX as f64) as u32),
        width: outcome.measurement.width,
        height: outcome.measurement.height,
        http_status: outcome.measurement.http_status,
        checked_at: now,
        next_check_at: now.saturating_add(candidate.interval_secs as i64),
        first_fail_at: if outcome.verdict.ok {
            None
        } else if outcome.verdict.reason == Reason::ProbeError {
            candidate.previous_first_fail_at
        } else {
            candidate.previous_first_fail_at.or(Some(now))
        },
        rule_hash: crate::quality::admission::rule_hash(&candidate.rules),
        run_id: String::new(),
        updated_at_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::model::{Channel, Group, PlaylistSource},
        quality::{
            admission::{Candidate, CandidateScope},
            gate::CandidateOutcome,
            rules::{Measurement, Verdict},
        },
    };
    use serde_json::json;

    fn candidate(key: &str, manual: bool, offline: bool) -> Candidate {
        Candidate {
            key: key.into(),
            scope: if manual {
                CandidateScope::Manual
            } else {
                CandidateScope::Sync
            },
            source_id: if manual { String::new() } else { "s1".into() },
            source_key: if manual { String::new() } else { key.into() },
            gid: "g".into(),
            cid: "c".into(),
            name: "c".into(),
            target_group: "组".into(),
            url: "http://up/live.m3u8".into(),
            existing_url: None,
            headers: Vec::new(),
            proxy: None,
            rules: crate::quality::rules::EffectiveRules::default(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
            is_url_change: false,
            sync_follow: true,
            offline_on_fail: offline,
            interval_secs: 6 * 3600,
            previous_first_fail_at: None,
        }
    }

    fn outcome(
        key: &str,
        manual: bool,
        offline: bool,
        ok: bool,
        reason: Reason,
    ) -> CandidateOutcome {
        CandidateOutcome {
            candidate: candidate(key, manual, offline),
            measurement: Measurement::default(),
            verdict: Verdict {
                ok,
                reachable: true,
                reason,
            },
            bytes: 0,
            duration_ms: 0,
        }
    }

    fn config_with_sync_channel() -> Config {
        let mut config = Config::default();
        config.quality.enabled = true;
        config.playlist.sources.push(PlaylistSource {
            id: "s1".into(),
            enabled: true,
            mode: "sync".into(),
            ..PlaylistSource::default()
        });
        config.groups.push(Group {
            id: "g".into(),
            origin: "sync".into(),
            channels: vec![Channel {
                id: "c".into(),
                url: "http://up/live.m3u8".into(),
                source_id: "s1".into(),
                source_key: "c".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        config
    }

    #[test]
    fn guard_pauses_sync_removals() {
        let outcomes: Vec<_> = (0..10)
            .map(|i| {
                outcome(
                    &format!("sync:s1:c{i}"),
                    false,
                    false,
                    false,
                    Reason::Unreachable,
                )
            })
            .collect();
        let guard = Guard {
            enabled: true,
            max_remove_ratio: 0.5,
            min_remove_count: 10,
            base_channels: 0,
        };
        let decision = decide(&outcomes, false, &guard);
        assert!(decision.paused);
        assert!(decision.removals.is_empty());
        assert_eq!(
            decision
                .results
                .iter()
                .filter(|item| item.action == "removal_paused")
                .count(),
            10
        );
    }

    #[test]
    fn measure_only_never_removes_or_hides() {
        let outcomes = vec![outcome(
            "channel:g/c",
            true,
            true,
            false,
            Reason::Unreachable,
        )];
        let decision = decide(&outcomes, true, &Guard::default());
        assert!(decision.removals.is_empty());
        assert!(decision.manual_entries.is_empty());
        assert_eq!(decision.results[0].action, "kept");
    }

    #[test]
    fn manual_hide_requires_offline_on_fail() {
        let decision = decide(
            &[outcome(
                "channel:g/c",
                true,
                true,
                false,
                Reason::Unreachable,
            )],
            false,
            &Guard::default(),
        );
        assert_eq!(decision.results[0].action, "hidden");
        assert_eq!(decision.manual_entries.len(), 1);
    }

    #[test]
    fn removal_revalidation_skips_changed_source_or_url() {
        let mut config = config_with_sync_channel();
        let removal = Removal {
            gid: "g".into(),
            cid: "c".into(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
        };
        assert!(removal_allowed(&config, &removal));
        config.groups[0].channels[0].url = "http://up/changed.m3u8".into();
        assert!(
            !removal_allowed(&config, &removal),
            "URL 变化时不得按旧结果移出"
        );
        config.groups[0].channels[0].url = "http://up/live.m3u8".into();
        config.playlist.sources[0].enabled = false;
        assert!(!removal_allowed(&config, &removal), "源停用后不得移出");
        config.playlist.sources[0].enabled = true;
        config.playlist.sources[0].quality = Some(crate::config::model::QualityOverride {
            enabled: Some(false),
            ..Default::default()
        });
        config.groups[0].quality = Some(crate::config::model::QualityOverride {
            enabled: Some(true),
            ..Default::default()
        });
        assert!(!removal_allowed(&config, &removal), "源门控关闭后不得移出");
    }

    #[test]
    fn periodic_probe_uses_source_proxy() {
        let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
        let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
        let mut config = config_with_sync_channel();
        config.playlist.sources[0].proxy = Some(crate::config::model::ProxyCfg {
            addr: "127.0.0.1:9".into(),
            ..Default::default()
        });
        let due =
            due_targets(&config, &store, &RunScope::All, 100, 0, false, false).expect("到期目标");
        assert_eq!(due.len(), 1);
        assert_eq!(
            due[0].proxy.as_ref().map(|proxy| proxy.addr.as_str()),
            Some("127.0.0.1:9"),
            "周期探测必须沿用源代理"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disabled_source_gate_skips_periodic_checks() {
        let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
        let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
        let mut config = config_with_sync_channel();
        config.playlist.sources[0].quality = Some(crate::config::model::QualityOverride {
            enabled: Some(false),
            ..Default::default()
        });
        config.groups[0].quality = Some(crate::config::model::QualityOverride {
            enabled: Some(true),
            ..Default::default()
        });
        let due =
            due_targets(&config, &store, &RunScope::All, 100, 0, false, false).expect("到期目标");
        assert!(due.is_empty(), "源门控关闭后不得周期移出");
        assert_eq!(
            guard_base_channels(&config, &RunScope::All),
            0,
            "源门控关闭的频道不计入安全阀基数"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_hide_waits_for_grace() {
        let now = crate::schedule::now_epoch();
        let mut candidate = candidate("channel:g/c", true, true);
        candidate.rules.stale_keep_secs = 60;
        candidate.previous_first_fail_at = Some(now - 30);
        let outcome = CandidateOutcome {
            candidate: candidate.clone(),
            measurement: Measurement::default(),
            verdict: Verdict {
                ok: false,
                reachable: false,
                reason: Reason::Unreachable,
            },
            bytes: 0,
            duration_ms: 0,
        };
        let decision = decide(&[outcome.clone()], false, &Guard::default());
        assert_eq!(decision.results[0].action, "kept", "宽限内不隐藏");
        assert_eq!(decision.hidden, 0);
        assert!(decision.manual_entries.is_empty(), "宽限内不写失败状态");

        let mut expired = outcome;
        expired.candidate.previous_first_fail_at = Some(now - 61);
        let decision = decide(&[expired], false, &Guard::default());
        assert_eq!(decision.results[0].action, "hidden", "超过宽限后隐藏");
        assert_eq!(decision.hidden, 1);
    }

    #[test]
    fn probe_error_does_not_start_fail_clock() {
        let decision = decide(
            &[outcome(
                "channel:g/c",
                true,
                true,
                false,
                Reason::ProbeError,
            )],
            false,
            &Guard::default(),
        );
        assert_eq!(decision.results[0].action, "kept");
        assert_eq!(
            decision.results[0].first_fail_at, None,
            "探测错误不得作为失败起点"
        );
    }

    #[test]
    fn manual_probe_error_does_not_hide() {
        let decision = decide(
            &[outcome(
                "channel:g/c",
                true,
                true,
                false,
                Reason::ProbeError,
            )],
            false,
            &Guard::default(),
        );
        assert_eq!(decision.results[0].action, "kept");
        assert!(decision.manual_entries.is_empty(), "探测错误不改手工状态");
    }

    #[test]
    fn manual_success_clears_hidden_state() {
        let decision = decide(
            &[outcome("channel:g/c", true, true, true, Reason::Pass)],
            false,
            &Guard::default(),
        );
        assert_eq!(decision.results[0].action, "kept");
        assert!(decision.manual_entries[0].2.ok());
    }

    #[test]
    fn removed_channel_result_is_not_written() {
        let mut config = config_with_sync_channel();
        config.groups.clear();
        let mut results = vec![QualityResult {
            gid: "g".into(),
            cid: "c".into(),
            scope: "sync".into(),
            source_id: "s1".into(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
            action: "admitted".into(),
            ok: true,
            ..QualityResult::default()
        }];
        revalidate_results(&config, &mut results);
        assert!(results.is_empty(), "频道已删除时不写 admitted 结果");
    }

    #[test]
    fn disabled_source_result_is_marked_skipped() {
        let mut config = config_with_sync_channel();
        config.playlist.sources[0].enabled = false;
        let mut results = vec![QualityResult {
            gid: "g".into(),
            cid: "c".into(),
            scope: "sync".into(),
            source_id: "s1".into(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
            action: "admitted".into(),
            ok: true,
            ..QualityResult::default()
        }];
        revalidate_results(&config, &mut results);
        assert_eq!(results[0].action, "skipped");
        assert_eq!(results[0].reason, "source_changed");
    }

    #[test]
    fn budget_reached_at_ninety_percent() {
        assert!(!budget_reached(0, 0), "0 表示不限制");
        assert!(!budget_reached(89, 100));
        assert!(budget_reached(90, 100));
    }

    #[test]
    fn time_budget_limits_run() {
        let started = Instant::now();
        assert!(!time_budget_reached(started, 0), "0 表示不限制");
        assert!(!time_budget_reached(started, 3600));
        let expired = Instant::now() - Duration::from_secs(11);
        assert!(time_budget_reached(expired, 10));
    }

    #[test]
    fn guard_ignores_manual_and_probe_errors_in_denominator() {
        let guard = Guard {
            enabled: true,
            max_remove_ratio: 0.5,
            min_remove_count: 10,
            base_channels: 0,
        };
        let mut outcomes: Vec<_> = (0..100)
            .map(|i| outcome(&format!("channel:g/c{i}"), true, false, true, Reason::Pass))
            .collect();
        outcomes.extend((0..10).map(|i| {
            outcome(
                &format!("sync:s1:c{i}"),
                false,
                false,
                false,
                Reason::Unreachable,
            )
        }));
        assert!(
            decide(&outcomes, false, &guard).paused,
            "手工频道不得稀释比例"
        );

        let mut outcomes: Vec<_> = (0..90)
            .map(|i| {
                outcome(
                    &format!("sync:s1:e{i}"),
                    false,
                    false,
                    false,
                    Reason::ProbeError,
                )
            })
            .collect();
        outcomes.extend((0..10).map(|i| {
            outcome(
                &format!("sync:s1:c{i}"),
                false,
                false,
                false,
                Reason::Unreachable,
            )
        }));
        assert!(
            decide(&outcomes, false, &guard).paused,
            "探测错误不得稀释比例"
        );
    }

    #[test]
    fn measure_only_does_not_overwrite_sync_results() {
        let outcomes = vec![
            outcome("sync:s1:c", false, false, false, Reason::Unreachable),
            outcome("channel:g/c", true, true, false, Reason::Unreachable),
        ];
        let decision = decide(&outcomes, true, &Guard::default());
        assert!(
            decision
                .results
                .iter()
                .all(|result| result.scope == "manual"),
            "仅测量不得覆盖同步准入结果"
        );
        assert_eq!(decision.results.len(), 1);
        assert!(decision.manual_entries.is_empty(), "仅测量不改手工状态");
    }

    #[test]
    fn pinned_sync_channel_is_kept_not_removed() {
        let mut outcomes = vec![outcome(
            "sync:s1:c",
            false,
            false,
            false,
            Reason::Unreachable,
        )];
        outcomes[0].candidate.sync_follow = false;
        let guard = Guard {
            enabled: true,
            max_remove_ratio: 0.5,
            min_remove_count: 1,
            base_channels: 0,
        };
        let decision = decide(&outcomes, false, &guard);
        assert!(decision.removals.is_empty(), "固定频道不得移出");
        assert_eq!(decision.removed, 0);
        assert_eq!(decision.results[0].action, "kept");
        assert!(!decision.paused, "固定频道失败不计入安全阀");
    }

    #[test]
    fn candidate_headers_include_user_agent() {
        let mut group = Group::default();
        group
            .extra
            .insert("user_agent".into(), json!("GroupUA/1.0"));
        group.headers = vec![json!("X-Group: yes")];
        let mut channel = Channel::default();
        channel
            .extra
            .insert("user_agent".into(), json!("ChannelUA/2.0"));
        channel.headers = vec![json!("X-Channel: yes")];
        let headers = candidate_headers(&group, &channel);
        assert!(headers.iter().any(
            |(name, value)| name.eq_ignore_ascii_case("user-agent") && value == "ChannelUA/2.0"
        ));
        assert!(headers.iter().any(|(name, _)| name == "X-Group"));
        assert!(headers.iter().any(|(name, _)| name == "X-Channel"));
    }

    #[test]
    fn paused_url_change_is_not_probed_on_old_url() {
        let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
        let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
        store
            .upsert(&QualityResult {
                key: "sync:s1:c".into(),
                scope: "sync".into(),
                source_id: "s1".into(),
                source_key: "c".into(),
                gid: "g".into(),
                cid: "c".into(),
                url_hash: url_hash_hex("http://up/new.m3u8"),
                action: "removal_paused".into(),
                reason: "unreachable".into(),
                ..QualityResult::default()
            })
            .expect("写结果");
        let config = config_with_sync_channel();
        let due =
            due_targets(&config, &store, &RunScope::All, 100, 0, false, false).expect("到期目标");
        assert!(due.is_empty(), "暂停的新地址尚不应按旧地址重测");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_run_includes_not_due_results() {
        let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
        let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
        store
            .upsert(&QualityResult {
                key: "sync:s1:c".into(),
                scope: "sync".into(),
                source_id: "s1".into(),
                source_key: "c".into(),
                gid: "g".into(),
                cid: "c".into(),
                url_hash: url_hash_hex("http://up/live.m3u8"),
                action: "removed".into(),
                next_check_at: 9_999_999_999,
                ..QualityResult::default()
            })
            .expect("写结果");
        let config = config_with_sync_channel();
        let due =
            due_targets(&config, &store, &RunScope::All, 100, 0, false, false).expect("到期目标");
        assert!(due.is_empty(), "未到期不进入周期校验");
        let forced =
            due_targets(&config, &store, &RunScope::All, 100, 0, false, true).expect("手动目标");
        assert_eq!(forced.len(), 1, "手动校验忽略下次检查时间");
        let _ = std::fs::remove_dir_all(dir);
    }
}
