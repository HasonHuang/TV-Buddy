//! 同步门控：目标选择、判定与计划过滤。

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    config::model::{Config, PlaylistSource},
    http::routes::stream as stream_headers,
    quality::{
        admission::{candidate_rules, url_hash_hex, Candidate, CandidateScope, ProbeSignature},
        probe::{probe, ProbeGates, ProbeMode, ProbeOutput},
        rules::{effective_rules, evaluate, EffectiveRules, Measurement, Reason, Verdict},
        schedule::{probe_limits, required_probe_mode, run_limit},
        store::QualityResult,
    },
    state::AppState,
    subscription::Plan,
};

/// 安全阀参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guard {
    pub enabled: bool,
    pub max_remove_ratio: f64,
    pub min_remove_count: usize,
    /// 本轮可移出的同步频道总数。用于小源自适应阈值。
    pub base_channels: usize,
}

impl Default for Guard {
    fn default() -> Self {
        Self {
            enabled: false,
            max_remove_ratio: 0.5,
            min_remove_count: 10,
            base_channels: 0,
        }
    }
}

impl Guard {
    /// 数量阈值。小源全部可移出频道都失败时按 30% 降低，但不少于 1。
    pub(crate) fn effective_min_remove_count(&self, removals: usize) -> usize {
        if self.base_channels < 3 || removals < self.base_channels {
            return self.min_remove_count;
        }
        let adaptive = self.base_channels.saturating_mul(3).div_ceil(10).max(1);
        self.min_remove_count.min(adaptive)
    }
}

/// 门控选中的目标。
#[derive(Debug, Clone, Default)]
pub struct GateTargets {
    pub url_changed: Vec<Candidate>,
    pub new: Vec<Candidate>,
    /// 本轮全部新候选的稳定键，包含被截断的部分。
    pub new_all: Vec<String>,
    /// 规则关闭、无需探测的候选。
    pub kept: Vec<Candidate>,
    pub cursor_after: Option<String>,
    pub truncated: bool,
    pub remaining: usize,
}

/// 本轮统计。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GateStats {
    pub tested: usize,
    pub admitted: usize,
    pub rejected: usize,
    pub removed: usize,
    pub source_removed: usize,
}

/// 一个候选的探测结果。
#[derive(Debug, Clone)]
pub struct CandidateOutcome {
    pub candidate: Candidate,
    pub measurement: Measurement,
    pub verdict: Verdict,
    pub bytes: u64,
    pub duration_ms: u64,
}

/// 门控结果。
#[derive(Debug, Clone)]
pub enum GateOutcome {
    Apply {
        plan: Plan,
        results: Vec<QualityResult>,
        stats: GateStats,
        cursor: Option<String>,
        truncated: bool,
        remaining: usize,
    },
    Paused {
        reason: String,
        results: Vec<QualityResult>,
        stats: GateStats,
    },
}

/// 选出本轮门控目标。
pub fn select_targets(
    config: &Config,
    source: &PlaylistSource,
    plan: &Plan,
    cursor: Option<&str>,
    max_new: usize,
) -> GateTargets {
    let mut url_changed = Vec::new();
    let mut new = Vec::new();
    let mut kept = Vec::new();

    let paired = crate::subscription::pair_source_channels(config, &source.id, plan);
    for (planned, existing) in plan.channels.iter().zip(paired.iter()) {
        let existing = *existing;
        let planned_group = plan
            .groups
            .iter()
            .find(|group| group.target == planned.target);
        let target = planned_group.and_then(|group| {
            crate::subscription::find_target_group(&config.groups, source, group)
                .map(|index| &config.groups[index])
        });
        let rules = candidate_rules(config, source, existing, target);
        let gid = existing
            .map(|(group, _)| group.id.clone())
            .unwrap_or_default();
        let cid = existing
            .map(|(_, channel)| channel.id.clone())
            .unwrap_or_default();
        let proxy = existing
            .and_then(|(group, channel)| config.proxy_for(&group.id, &channel.id))
            .or_else(|| target.and_then(|group| group.proxy.as_ref()))
            .or(source.proxy.as_ref())
            .cloned();
        let is_url_change = existing
            .is_some_and(|(_, channel)| url_hash_hex(&channel.url) != url_hash_hex(&planned.url));
        let header_group = existing.map(|(group, _)| group).or(target);
        let channel_agent =
            existing.and_then(|(_, channel)| stream_headers::user_agent(&channel.extra));
        let headers = match header_group {
            Some(group) => stream_headers::merge_upstream_headers(
                &group.headers,
                stream_headers::user_agent(&group.extra),
                &planned.headers,
                channel_agent,
            ),
            None => header_pairs(&planned.headers),
        };
        let candidate = Candidate {
            key: planned.key.clone(),
            scope: CandidateScope::Sync,
            source_id: source.id.clone(),
            source_key: planned.key.clone(),
            gid,
            cid,
            name: planned.name.clone(),
            target_group: planned_group
                .map(|group| group.name.clone())
                .unwrap_or_default(),
            url: planned.url.clone(),
            existing_url: existing.map(|(_, channel)| channel.url.clone()),
            headers,
            proxy,
            rules,
            url_hash: url_hash_hex(&planned.url),
            is_url_change,
            sync_follow: existing.is_none_or(|(_, channel)| channel.sync_follow),
            offline_on_fail: existing
                .is_some_and(|(group, channel)| config.offline_on_fail_for(&group.id, &channel.id)),
            interval_secs: config.quality.interval_hours.saturating_mul(3_600),
            previous_first_fail_at: None,
        };

        if !candidate.rules.enabled {
            kept.push(candidate);
        } else if existing.is_some_and(|(_, channel)| !channel.sync_follow) {
            // 固定频道不跟随上游字段，apply 不会更新或删除它。
            // 探测与移出只会产生无法执行的结果，并污染安全阀统计。
            kept.push(candidate);
        } else if is_url_change {
            url_changed.push(candidate);
        } else if existing.is_none() {
            new.push(candidate);
        }
    }

    new.sort_by(|left, right| left.key.cmp(&right.key));
    let new_all = new.iter().map(|item| item.key.clone()).collect();
    let start = cursor
        .and_then(|cursor| new.iter().position(|item| item.key.as_str() > cursor))
        .unwrap_or(0);
    let end = if max_new == 0 {
        new.len()
    } else {
        start.saturating_add(max_new).min(new.len())
    };
    let truncated = end < new.len();
    let remaining = new.len().saturating_sub(end);
    let cursor_after = truncated.then(|| new[end - 1].key.clone());
    let new = new[start..end].to_vec();

    GateTargets {
        url_changed,
        new,
        new_all,
        kept,
        cursor_after,
        truncated,
        remaining,
    }
}

/// 应用门控判定。
pub fn decide(
    plan: Plan,
    targets: &GateTargets,
    outcomes: &[CandidateOutcome],
    guard: &Guard,
    probe: &ProbeSignature,
) -> GateOutcome {
    let by_key: HashMap<&str, &CandidateOutcome> = outcomes
        .iter()
        .map(|outcome| (outcome.candidate.key.as_str(), outcome))
        .collect();
    let now = crate::schedule::now_epoch();
    let probe_error = targets.url_changed.iter().any(|candidate| {
        by_key
            .get(candidate.key.as_str())
            .is_some_and(|outcome| outcome.verdict.reason == Reason::ProbeError)
    });
    if probe_error {
        let results = outcomes
            .iter()
            .map(|outcome| {
                let action = if outcome.verdict.ok {
                    "held"
                } else {
                    "removal_paused"
                };
                result_for(&outcome.candidate, outcome, action, now, probe)
            })
            .collect();
        return GateOutcome::Paused {
            reason: "probe_error".into(),
            results,
            stats: stats_for(outcomes, targets),
        };
    }

    let removals = targets
        .url_changed
        .iter()
        .filter(|candidate| {
            by_key
                .get(candidate.key.as_str())
                .is_some_and(|outcome| !outcome.verdict.ok)
        })
        .count();
    let tested = outcomes
        .iter()
        .filter(|outcome| !matches!(outcome.verdict.reason, Reason::ProbeError | Reason::Unknown))
        .count();
    if guard_trips(guard, tested, removals) {
        let results = outcomes
            .iter()
            .map(|outcome| {
                let action = if outcome.verdict.ok {
                    "held"
                } else {
                    "removal_paused"
                };
                result_for(&outcome.candidate, outcome, action, now, probe)
            })
            .collect();
        return GateOutcome::Paused {
            reason: "guard".into(),
            results,
            stats: stats_for(outcomes, targets),
        };
    }

    let mut keep: HashSet<String> = plan
        .channels
        .iter()
        .map(|channel| channel.key.clone())
        .collect();
    for candidate in &targets.new {
        let failed = by_key
            .get(candidate.key.as_str())
            .is_some_and(|outcome| !outcome.verdict.ok);
        if failed || !by_key.contains_key(candidate.key.as_str()) {
            keep.remove(&candidate.key);
        }
    }
    for key in &targets.new_all {
        let selected = targets.new.iter().any(|candidate| &candidate.key == key);
        let kept = targets.kept.iter().any(|candidate| &candidate.key == key);
        if !selected && !kept {
            keep.remove(key);
        }
    }
    for candidate in &targets.url_changed {
        if by_key
            .get(candidate.key.as_str())
            .is_some_and(|outcome| !outcome.verdict.ok)
        {
            keep.remove(&candidate.key);
        }
    }

    let mut results = Vec::with_capacity(outcomes.len());
    for outcome in outcomes {
        let action = if outcome.candidate.is_url_change {
            if outcome.verdict.ok {
                "admitted"
            } else {
                "removed"
            }
        } else if outcome.verdict.ok {
            "admitted"
        } else {
            "rejected"
        };
        results.push(result_for(&outcome.candidate, outcome, action, now, probe));
    }

    GateOutcome::Apply {
        plan: plan.retain_keys(&keep),
        results,
        stats: stats_for(outcomes, targets),
        cursor: targets.cursor_after.clone(),
        truncated: targets.truncated,
        remaining: targets.remaining,
    }
}

fn guard_trips(guard: &Guard, tested: usize, removals: usize) -> bool {
    guard.enabled
        && tested > 0
        && removals >= guard.effective_min_remove_count(removals)
        && removals as f64 / tested as f64 >= guard.max_remove_ratio
}

fn stats_for(outcomes: &[CandidateOutcome], targets: &GateTargets) -> GateStats {
    GateStats {
        tested: outcomes.len(),
        admitted: outcomes.iter().filter(|outcome| outcome.verdict.ok).count(),
        rejected: outcomes
            .iter()
            .filter(|outcome| !outcome.candidate.is_url_change && !outcome.verdict.ok)
            .count(),
        removed: targets
            .url_changed
            .iter()
            .filter(|candidate| {
                outcomes.iter().any(|outcome| {
                    outcome.candidate.key == candidate.key
                        && !outcome.verdict.ok
                        && outcome.verdict.reason != Reason::ProbeError
                })
            })
            .count(),
        source_removed: 0,
    }
}

fn result_for(
    candidate: &Candidate,
    outcome: &CandidateOutcome,
    action: &str,
    now: i64,
    probe: &ProbeSignature,
) -> QualityResult {
    let measurement = &outcome.measurement;
    QualityResult {
        key: format!("sync:{}:{}", candidate.source_id, candidate.source_key),
        scope: "sync".into(),
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
        delay_ms: measurement.delay_ms,
        speed_kib: measurement
            .speed_mib
            .map(|speed| (speed * 1024.0).clamp(0.0, u32::MAX as f64) as u32),
        width: measurement.width,
        height: measurement.height,
        http_status: measurement.http_status,
        checked_at: now,
        next_check_at: if action == "admitted" {
            now.saturating_add(candidate.interval_secs as i64)
        } else {
            0
        },
        first_fail_at: if outcome.verdict.ok {
            None
        } else {
            candidate.previous_first_fail_at.or(Some(now))
        },
        rule_hash: crate::quality::admission::rule_hash(&candidate.rules, probe),
        run_id: String::new(),
        updated_at_ms: 0,
    }
}

fn header_pairs(headers: &[serde_json::Value]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|value| {
            if let Some(text) = value.as_str() {
                let (name, value) = text.split_once(':')?;
                let name = name.trim();
                return (!name.is_empty()).then(|| (name.to_string(), value.trim().to_string()));
            }
            let table = value.as_object()?;
            let name = table.get("name")?.as_str()?.trim();
            let value = table.get("value")?.as_str()?.trim();
            (!name.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

/// 单轮测量缓存。按 URL 哈希去重；可选按 Host 共享。
#[derive(Debug, Default)]
pub struct RunMemo {
    by_url: HashMap<u64, Measurement>,
    by_host: HashMap<u64, Measurement>,
}

impl RunMemo {
    pub fn get(&self, url_hash: u64) -> Option<&Measurement> {
        self.by_url.get(&url_hash)
    }

    pub fn get_host(&self, host_key: u64) -> Option<&Measurement> {
        self.by_host.get(&host_key)
    }

    pub fn insert(
        &mut self,
        url_hash: u64,
        host_key: Option<u64>,
        measurement: Measurement,
        host_cache: bool,
    ) {
        self.by_url
            .entry(url_hash)
            .or_insert_with(|| measurement.clone());
        if host_cache {
            if let Some(host_key) = host_key {
                self.by_host.entry(host_key).or_insert(measurement);
            }
        }
    }
}

/// 门控墙钟是否耗尽。0 表示不限制。
pub fn gate_expired(started: Instant, budget_secs: u64) -> bool {
    budget_secs > 0 && started.elapsed() >= Duration::from_secs(budget_secs)
}

/// 探测计划中的新候选与 URL 变化频道，并应用门控判定。
pub async fn filter_plan(
    state: &Arc<AppState>,
    config: &Config,
    source: &PlaylistSource,
    plan: Plan,
) -> GateOutcome {
    let source_enabled = source
        .quality
        .as_ref()
        .and_then(|quality| quality.enabled)
        .unwrap_or(true);
    if !config.quality.enabled || !source_enabled {
        return GateOutcome::Apply {
            plan,
            results: Vec::new(),
            stats: GateStats::default(),
            cursor: None,
            truncated: false,
            remaining: 0,
        };
    }

    let cursor = state
        .sources
        .statuses()
        .get(&source.id)
        .map(|status| status.gate_cursor.clone())
        .filter(|cursor| !cursor.is_empty());
    // 整轮只检测一次 ffprobe 可用性。
    let probe = ProbeSignature::from_config(config, state.ffprobe_ready().await);
    let gates = ProbeGates {
        probe_semaphore: Arc::new(tokio::sync::Semaphore::new(
            config.quality.probe_concurrency as usize,
        )),
        ffprobe_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        max_ffprobe_per_run: config.quality.max_ffprobe_per_run,
    };
    let max_new = run_limit(
        config.quality.budget_mb_per_run,
        8.0,
        config.quality.max_channels_per_run,
    );
    let mut targets = select_targets(config, source, &plan, cursor.as_deref(), max_new);
    let guard = Guard {
        enabled: config.quality.guard_enabled,
        max_remove_ratio: config.quality.max_remove_ratio,
        min_remove_count: usize::try_from(config.quality.min_remove_count).unwrap_or(usize::MAX),
        base_channels: config
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .channels
                    .iter()
                    .filter(|channel| channel.source_id == source.id && channel.sync_follow)
                    .filter(|channel| effective_rules(config, group, channel).enabled)
            })
            .count(),
    };
    let started = Instant::now();
    let budget_secs = config.quality.gate_time_budget_secs;
    let memo = std::sync::Mutex::new(RunMemo::default());
    let mut outcomes = Vec::new();

    for candidate in &targets.url_changed {
        if gate_expired(started, budget_secs) {
            return paused_outcome("budget", &outcomes, &targets, &probe);
        }
        let mut candidate = candidate.clone();
        candidate.previous_first_fail_at = previous_first_fail(state, &candidate);
        let outcome = probe_candidate(state, config, &candidate, &memo, false, &gates).await;
        let probe_error = outcome.measurement.probe_error;
        outcomes.push(outcome);
        if probe_error {
            return decide(plan, &targets, &outcomes, &guard, &probe);
        }
    }

    let mut processed = 0usize;
    for candidate in targets.new.clone() {
        if state.quality.is_cancelled() || gate_expired(started, budget_secs) {
            break;
        }
        let mut candidate = candidate;
        candidate.previous_first_fail_at = previous_first_fail(state, &candidate);
        let outcome = probe_candidate(state, config, &candidate, &memo, false, &gates).await;
        outcomes.push(outcome);
        processed += 1;
    }
    if processed < targets.new.len() {
        targets.truncated = true;
        let start = cursor
            .as_deref()
            .and_then(|cursor| targets.new_all.iter().position(|key| key.as_str() > cursor))
            .unwrap_or(0);
        targets.remaining = targets
            .new_all
            .len()
            .saturating_sub(start)
            .saturating_sub(processed);
        targets.cursor_after = processed
            .checked_sub(1)
            .map(|index| targets.new[index].key.clone());
        targets.new.truncate(processed);
    }

    decide(plan, &targets, &outcomes, &guard, &probe)
}

fn previous_first_fail(state: &AppState, candidate: &Candidate) -> Option<i64> {
    state
        .quality_store
        .get(&sync_key(candidate))
        .ok()
        .flatten()
        .and_then(|result| result.first_fail_at)
}

/// 单轮测量缓存的键。探测档位、代理、请求头与分辨率上限都会改变测量结果。
fn memo_fingerprint(url: &str, candidate: &Candidate, mode: ProbeMode) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let rank = match mode {
        ProbeMode::Reachability => 0u8,
        ProbeMode::Playlist => 1,
        ProbeMode::Speed => 2,
        ProbeMode::Full => 3,
    };
    rank.hash(&mut hasher);
    candidate.headers.hash(&mut hasher);
    candidate.rules.max_resolution_px.hash(&mut hasher);
    if let Some(proxy) = &candidate.proxy {
        proxy.kind.hash(&mut hasher);
        proxy.addr.hash(&mut hasher);
        proxy.user.hash(&mut hasher);
        proxy.pass.hash(&mut hasher);
    }
    hasher.finish()
}

/// Host 缓存键。同一 origin 但代理或请求头不同时不得共享测速值。
fn host_cache_key(url: &str, candidate: &Candidate) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parsed.scheme().hash(&mut hasher);
    host.hash(&mut hasher);
    parsed.port_or_known_default().hash(&mut hasher);
    candidate.headers.hash(&mut hasher);
    if let Some(proxy) = &candidate.proxy {
        proxy.kind.hash(&mut hasher);
        proxy.addr.hash(&mut hasher);
        proxy.user.hash(&mut hasher);
        proxy.pass.hash(&mut hasher);
    }
    Some(hasher.finish())
}

/// Full 档启用 ffprobe 时，Host 缓存会跳过逐频道分辨率识别，因此必须停用。
fn should_use_host_cache(
    candidate: &Candidate,
    mode: ProbeMode,
    ffprobe: bool,
    dash_fallback: bool,
) -> bool {
    candidate.rules.host_cache && !dash_fallback && !(mode == ProbeMode::Full && ffprobe)
}

/// Host 缓存命中时只复用速率与延迟。可达性仍逐频道探测。
fn host_cache_probe_mode(rules: &EffectiveRules) -> ProbeMode {
    if rules.min_resolution_px.is_some()
        || rules.max_resolution_px.is_some()
        || !rules.resolution_speed_map.is_empty()
    {
        ProbeMode::Playlist
    } else {
        ProbeMode::Reachability
    }
}

fn merge_host_measurement(mut current: Measurement, cached: &Measurement) -> Measurement {
    if current.reachable && !current.probe_error {
        if current.speed_mib.is_none() {
            current.speed_mib = cached.speed_mib;
        }
        if current.delay_ms.is_none() {
            current.delay_ms = cached.delay_ms;
        }
    }
    current
}

async fn resolved_probe_url(
    state: &Arc<AppState>,
    config: &Config,
    candidate: &Candidate,
) -> anyhow::Result<Option<String>> {
    let Some(group) = config.group(&candidate.gid) else {
        return Ok(None);
    };
    let Some(channel) = group
        .channels
        .iter()
        .find(|channel| channel.id == candidate.cid)
    else {
        return Ok(None);
    };
    let url = crate::resolver::resolve(state, &candidate.gid, &candidate.cid, channel).await?;
    Ok(url.filter(|url| !url.trim().is_empty()))
}

fn reachability_only_verdict(measurement: &Measurement) -> Verdict {
    if measurement.probe_error {
        Verdict {
            ok: false,
            reachable: measurement.reachable,
            reason: Reason::ProbeError,
        }
    } else if !measurement.reachable {
        Verdict {
            ok: false,
            reachable: false,
            reason: Reason::Unreachable,
        }
    } else {
        Verdict {
            ok: true,
            reachable: true,
            reason: Reason::Pass,
        }
    }
}

fn dash_url_change(candidate: &Candidate, config: &Config) -> bool {
    candidate.is_url_change
        && crate::quality::probe::is_dash(&candidate.url)
        && required_probe_mode(config, &candidate.rules) != ProbeMode::Reachability
}

/// 探测一个候选。`resolve` 时先执行频道的解析脚本。
pub(crate) async fn probe_candidate(
    state: &Arc<AppState>,
    config: &Config,
    candidate: &Candidate,
    memo: &std::sync::Mutex<RunMemo>,
    resolve: bool,
    gates: &ProbeGates,
) -> CandidateOutcome {
    let probe_url = if resolve {
        match resolved_probe_url(state, config, candidate).await {
            Ok(Some(url)) => url,
            Ok(None) => candidate.url.clone(),
            Err(error) => {
                let measurement = Measurement {
                    reachable: true,
                    probe_error: true,
                    error: Some(format!("解析脚本失败: {error:#}")),
                    ..Measurement::default()
                };
                let verdict = evaluate(&candidate.rules, &measurement);
                return CandidateOutcome {
                    candidate: candidate.clone(),
                    measurement,
                    verdict,
                    bytes: 0,
                    duration_ms: 0,
                };
            }
        }
    } else {
        candidate.url.clone()
    };

    let dash_fallback = dash_url_change(candidate, config);
    let probe_mode = if dash_fallback {
        ProbeMode::Reachability
    } else {
        required_probe_mode(config, &candidate.rules)
    };
    let host_key = host_cache_key(&probe_url, candidate);
    let key = memo_fingerprint(&probe_url, candidate, probe_mode);
    let (url_cached, host_cached) = {
        let memo = memo.lock().unwrap_or_else(|error| error.into_inner());
        let url_cached = memo.get(key).cloned();
        let host_cached =
            should_use_host_cache(candidate, probe_mode, config.quality.ffprobe, dash_fallback)
                .then(|| {
                    host_key
                        .and_then(|key| memo.get_host(key))
                        .filter(|measurement| {
                            measurement.reachable
                                && !measurement.probe_error
                                && measurement.speed_mib.is_some()
                        })
                        .cloned()
                })
                .flatten();
        (url_cached, host_cached)
    };
    let output = match (url_cached, host_cached) {
        (Some(measurement), _) => ProbeOutput {
            measurement,
            bytes: 0,
            duration_ms: 0,
        },
        (None, Some(cached)) => {
            let client = state.probe_client_for_candidate(candidate.proxy.as_ref());
            let mut output = probe(
                &client,
                &probe_url,
                &candidate.headers,
                host_cache_probe_mode(&candidate.rules),
                candidate.rules.max_resolution_px,
                &probe_limits(config, state.ffprobe(), state.ffprobe_ready().await),
                gates,
            )
            .await;
            output.measurement = merge_host_measurement(output.measurement, &cached);
            output
        }
        (None, None) => {
            let client = state.probe_client_for_candidate(candidate.proxy.as_ref());
            let output = probe(
                &client,
                &probe_url,
                &candidate.headers,
                probe_mode,
                candidate.rules.max_resolution_px,
                &probe_limits(config, state.ffprobe(), state.ffprobe_ready().await),
                gates,
            )
            .await;
            memo.lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    key,
                    host_key,
                    output.measurement.clone(),
                    candidate.rules.host_cache,
                );
            output
        }
    };
    let verdict = if dash_fallback {
        reachability_only_verdict(&output.measurement)
    } else {
        evaluate(&candidate.rules, &output.measurement)
    };
    CandidateOutcome {
        candidate: candidate.clone(),
        measurement: output.measurement,
        verdict,
        bytes: output.bytes,
        duration_ms: output.duration_ms,
    }
}

fn paused_outcome(
    reason: &str,
    outcomes: &[CandidateOutcome],
    targets: &GateTargets,
    probe: &ProbeSignature,
) -> GateOutcome {
    let now = crate::schedule::now_epoch();
    let results = outcomes
        .iter()
        .map(|outcome| {
            let action = if outcome.verdict.ok {
                "held"
            } else {
                "removal_paused"
            };
            result_for(&outcome.candidate, outcome, action, now, probe)
        })
        .collect();
    GateOutcome::Paused {
        reason: reason.into(),
        results,
        stats: stats_for(outcomes, targets),
    }
}

fn sync_key(candidate: &Candidate) -> String {
    format!("sync:{}:{}", candidate.source_id, candidate.source_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::Group;
    use crate::quality::rules::EffectiveRules;
    use crate::subs::Trace;
    use crate::subscription::{PlannedChannel, PlannedGroup};

    fn source() -> PlaylistSource {
        PlaylistSource {
            id: "s1".into(),
            mode: "sync".into(),
            ..PlaylistSource::default()
        }
    }

    fn probe() -> ProbeSignature {
        ProbeSignature::from_config(&Config::default(), false)
    }

    fn plan_with(keys: &[&str]) -> Plan {
        let mut groups = Vec::new();
        let mut channels = Vec::new();
        for key in keys {
            if groups.is_empty() {
                groups.push(PlannedGroup {
                    original: "体育".into(),
                    target: "group:体育".into(),
                    name: "体育".into(),
                    merge_id: String::new(),
                    count: keys.len(),
                    quality: None,
                });
            }
            channels.push(PlannedChannel {
                key: (*key).into(),
                id_base: (*key).into(),
                name: (*key).into(),
                url: "http://up/live.m3u8".into(),
                source_group: "体育".into(),
                target: "group:体育".into(),
                tvg_id: None,
                tvg_name: None,
                tvg_logo: None,
                headers: Vec::new(),
                dash: false,
            });
        }
        Plan {
            channels,
            groups,
            duplicates: 0,
            trace: Trace {
                total: keys.len(),
                after_group: keys.len(),
                after_global_exclude: keys.len(),
                after_include: keys.len(),
                after_exclude: keys.len(),
            },
        }
    }

    fn config_with_existing(keys: &[&str]) -> Config {
        let mut config = Config::default();
        config.quality.enabled = true;
        config.playlist.sources.push(source());
        config.groups.push(Group {
            id: "g".into(),
            origin: "sync".into(),
            source_id: "s1".into(),
            source_group: "体育".into(),
            channels: keys
                .iter()
                .map(|key| crate::config::model::Channel {
                    id: (*key).into(),
                    name: (*key).into(),
                    url: "http://up/live.m3u8".into(),
                    source_id: "s1".into(),
                    source_key: (*key).into(),
                    ..Default::default()
                })
                .collect(),
            ..Group::default()
        });
        config
    }

    fn candidate(key: &str, url_change: bool) -> Candidate {
        Candidate {
            key: key.into(),
            scope: CandidateScope::Sync,
            source_id: "s1".into(),
            source_key: key.into(),
            gid: if url_change {
                "g".into()
            } else {
                String::new()
            },
            cid: if url_change {
                key.into()
            } else {
                String::new()
            },
            name: key.into(),
            target_group: "体育".into(),
            url: "http://up/new.m3u8".into(),
            existing_url: url_change.then(|| "http://up/old.m3u8".into()),
            headers: Vec::new(),
            proxy: None,
            rules: EffectiveRules::default(),
            url_hash: url_hash_hex("http://up/new.m3u8"),
            is_url_change: url_change,
            sync_follow: true,
            offline_on_fail: false,
            interval_secs: 6 * 3600,
            previous_first_fail_at: None,
        }
    }

    fn outcome(key: &str, ok: bool, reason: Reason, url_change: bool) -> CandidateOutcome {
        CandidateOutcome {
            candidate: candidate(key, url_change),
            measurement: Measurement {
                reachable: !matches!(reason, Reason::Unreachable | Reason::ProbeError),
                probe_error: reason == Reason::ProbeError,
                http_status: None,
                delay_ms: None,
                speed_mib: None,
                width: None,
                height: None,
                error: None,
            },
            verdict: Verdict {
                ok,
                reachable: true,
                reason,
            },
            bytes: 0,
            duration_ms: 0,
        }
    }

    #[test]
    fn memo_fingerprint_separates_probe_settings() {
        let base = candidate("c1", true);
        let mut changed = base.clone();
        changed.headers = vec![("X-Test".into(), "a".into())];
        assert_ne!(
            memo_fingerprint(&base.url, &base, ProbeMode::Speed),
            memo_fingerprint(&base.url, &changed, ProbeMode::Speed),
            "请求头不同不得共享测量"
        );
        changed.headers.clear();
        changed.proxy = Some(crate::config::model::ProxyCfg {
            addr: "127.0.0.1:9".into(),
            ..Default::default()
        });
        assert_ne!(
            memo_fingerprint(&base.url, &base, ProbeMode::Speed),
            memo_fingerprint(&base.url, &changed, ProbeMode::Speed),
            "代理不同不得共享测量"
        );
        changed.proxy = None;
        changed.rules.max_resolution_px = Some(1280);
        assert_ne!(
            memo_fingerprint(&base.url, &base, ProbeMode::Speed),
            memo_fingerprint(&base.url, &changed, ProbeMode::Speed),
            "分辨率上限不同不得共享测量"
        );
        assert_ne!(
            memo_fingerprint(&base.url, &base, ProbeMode::Reachability),
            memo_fingerprint(&base.url, &base, ProbeMode::Speed),
            "探测档位不同不得共享测量"
        );
    }

    #[test]
    fn host_cache_key_separates_proxy_and_headers() {
        let base = candidate("c1", true);
        let key = host_cache_key(&base.url, &base);
        assert!(key.is_some());
        let mut changed = base.clone();
        changed.proxy = Some(crate::config::model::ProxyCfg {
            addr: "127.0.0.1:9".into(),
            ..Default::default()
        });
        assert_ne!(key, host_cache_key(&base.url, &changed));
        changed.proxy = None;
        changed.headers = vec![("X-Test".into(), "a".into())];
        assert_ne!(key, host_cache_key(&base.url, &changed));
    }

    #[test]
    fn host_cache_probes_reachability_and_merges_speed() {
        let cached = Measurement {
            reachable: true,
            speed_mib: Some(4.0),
            delay_ms: Some(20),
            ..Measurement::default()
        };
        let current = Measurement {
            reachable: true,
            delay_ms: Some(80),
            ..Measurement::default()
        };
        let merged = merge_host_measurement(current.clone(), &cached);
        assert_eq!(merged.speed_mib, Some(4.0), "速率可复用 Host 缓存");
        assert_eq!(merged.delay_ms, Some(80), "延迟用本频道实测值");
        let failed = Measurement {
            reachable: false,
            error: Some("404".into()),
            ..Measurement::default()
        };
        let merged = merge_host_measurement(failed.clone(), &cached);
        assert_eq!(merged, failed, "不可达时不得套用缓存成功值");
        assert_eq!(
            host_cache_probe_mode(&EffectiveRules::default()),
            ProbeMode::Reachability
        );
    }

    #[test]
    fn gate_candidate_merges_group_headers() {
        let mut config = config_with_existing(&["c1"]);
        config.groups[0].headers = vec![serde_json::json!("Referer: https://example.test/")];
        config.groups[0]
            .extra
            .insert("user_agent".into(), serde_json::json!("GateUA/1.0"));
        let mut plan = plan_with(&["c1"]);
        plan.channels[0].url = "http://up/new.m3u8".into();
        let targets = select_targets(&config, &source(), &plan, None, 10);
        let headers = &targets.url_changed[0].headers;
        assert!(headers
            .iter()
            .any(|(name, value)| name == "Referer" && value == "https://example.test/"));
        assert!(headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("user-agent") && value == "GateUA/1.0"));
    }

    #[test]
    fn pinned_channel_url_change_is_not_targeted() {
        let mut config = config_with_existing(&["c1"]);
        config.groups[0].channels[0].sync_follow = false;
        let mut plan = plan_with(&["c1"]);
        plan.channels[0].url = "http://up/new.m3u8".into();
        let targets = select_targets(&config, &source(), &plan, None, 10);
        assert!(targets.url_changed.is_empty(), "固定频道不探测 URL 变化");
        assert!(targets.new.is_empty());
        assert_eq!(targets.kept.len(), 1, "固定频道原样保留");
    }

    #[test]
    fn guard_excludes_probe_errors_from_denominator() {
        let mut targets = GateTargets {
            url_changed: (0..10)
                .map(|index| candidate(&format!("c{index}"), true))
                .collect(),
            ..Default::default()
        };
        targets.new = (0..90)
            .map(|index| candidate(&format!("n{index}"), false))
            .collect();
        let mut outcomes: Vec<_> = (0..10)
            .map(|index| outcome(&format!("c{index}"), false, Reason::Unreachable, true))
            .collect();
        outcomes.extend(
            (0..90).map(|index| outcome(&format!("n{index}"), false, Reason::ProbeError, false)),
        );
        let guard = Guard {
            enabled: true,
            max_remove_ratio: 0.5,
            min_remove_count: 10,
            base_channels: 0,
        };
        assert!(
            matches!(
                decide(plan_with(&[]), &targets, &outcomes, &guard, &probe()),
                GateOutcome::Paused { .. }
            ),
            "探测错误不得稀释安全阀比例"
        );
    }

    #[test]
    fn guard_adaptive_min_protects_small_sources() {
        let guard = Guard {
            enabled: true,
            max_remove_ratio: 0.5,
            min_remove_count: 10,
            base_channels: 8,
        };
        assert!(guard_trips(&guard, 8, 8), "8 个频道全挂应触发安全阀");
        assert!(!guard_trips(&guard, 8, 4), "部分失败仍按原数量阈值");
        let tiny = Guard {
            base_channels: 2,
            ..guard
        };
        assert!(!guard_trips(&tiny, 2, 2), "不足 3 个频道不启用自适应");
    }

    #[test]
    fn dash_url_change_uses_reachability() {
        let mut config = Config::default();
        config.quality.probe_mode = "speed".into();
        let mut candidate = candidate("c1", true);
        candidate.url = "http://up/live.mpd".into();
        assert!(dash_url_change(&candidate, &config));
        candidate.is_url_change = false;
        assert!(!dash_url_change(&candidate, &config), "新候选仍按原档位");
        let reachable = Measurement {
            reachable: true,
            ..Measurement::default()
        };
        assert_eq!(reachability_only_verdict(&reachable).reason, Reason::Pass);
        let down = Measurement {
            reachable: false,
            ..Measurement::default()
        };
        assert_eq!(reachability_only_verdict(&down).reason, Reason::Unreachable);
        let error = Measurement {
            reachable: true,
            probe_error: true,
            ..Measurement::default()
        };
        assert_eq!(reachability_only_verdict(&error).reason, Reason::ProbeError);
    }

    #[test]
    fn gate_budget_zero_never_expires() {
        assert!(!gate_expired(Instant::now(), 0));
    }

    #[test]
    fn run_memo_dedupes_url_and_host() {
        let mut memo = RunMemo::default();
        let measurement = Measurement {
            reachable: true,
            ..Measurement::default()
        };
        assert!(memo.get(1).is_none());
        memo.insert(1, None, measurement.clone(), false);
        assert_eq!(memo.get(1), Some(&measurement));
        memo.insert(2, Some(7), measurement.clone(), true);
        assert_eq!(memo.get(2), Some(&measurement));
        assert_eq!(memo.get_host(7), Some(&measurement));
    }

    #[test]
    fn new_failed_candidate_is_rejected_and_plan_keeps_passing() {
        let targets = GateTargets {
            new: vec![candidate("new1", false), candidate("new2", false)],
            ..Default::default()
        };
        let plan = plan_with(&["new1", "new2"]);
        let out = decide(
            plan,
            &targets,
            &[
                outcome("new1", true, Reason::Pass, false),
                outcome("new2", false, Reason::SpeedLow, false),
            ],
            &Guard {
                enabled: true,
                max_remove_ratio: 0.5,
                min_remove_count: 10,
                base_channels: 0,
            },
            &probe(),
        );
        match out {
            GateOutcome::Apply { plan, stats, .. } => {
                assert_eq!(plan.channels.len(), 1);
                assert_eq!(plan.channels[0].key, "new1");
                assert_eq!(stats.rejected, 1);
            }
            _ => panic!("应应用"),
        }
    }

    #[test]
    fn url_changed_probe_error_pauses_sync() {
        let targets = GateTargets {
            url_changed: vec![candidate("c1", true)],
            ..Default::default()
        };
        let out = decide(
            plan_with(&["c1"]),
            &targets,
            &[outcome("c1", false, Reason::ProbeError, true)],
            &Guard::default(),
            &probe(),
        );
        assert!(matches!(out, GateOutcome::Paused { .. }));
    }

    #[test]
    fn duplicate_count_change_pairs_existing_channel() {
        let config = config_with_existing(&["id:same#abcd"]);
        let mut plan = plan_with(&["id:same"]);
        plan.channels[0].url = "http://up/new.m3u8".into();
        let targets = select_targets(&config, &source(), &plan, None, 10);
        assert!(targets.new.is_empty(), "重复数变化不得当成新候选");
        assert_eq!(targets.url_changed.len(), 1, "应识别为 URL 变化");
        assert_eq!(targets.url_changed[0].gid, "g");
        assert_eq!(targets.url_changed[0].cid, "id:same#abcd");
        assert!(targets.url_changed[0].is_url_change);
    }

    #[test]
    fn truncation_keeps_admitted_and_advances_cursor() {
        let config = config_with_existing(&["new:1"]);
        let plan = plan_with(&["new:1", "new:2", "new:3", "new:4"]);
        let targets = select_targets(&config, &source(), &plan, Some("new:1"), 2);
        assert_eq!(
            targets
                .new
                .iter()
                .map(|item| item.key.as_str())
                .collect::<Vec<_>>(),
            vec!["new:2", "new:3"]
        );
        assert!(targets.truncated);
        assert_eq!(targets.cursor_after.as_deref(), Some("new:3"));
        assert_eq!(targets.remaining, 1);
        assert!(
            !targets.new.iter().any(|item| item.key == "new:1"),
            "已准入且未变化的频道不参与门控"
        );
    }

    #[test]
    fn unprocessed_new_candidates_are_not_admitted() {
        let targets = GateTargets {
            new: vec![candidate("new1", false)],
            new_all: vec!["new1".into(), "new2".into()],
            truncated: true,
            ..Default::default()
        };
        let plan = plan_with(&["new1", "new2"]);
        let out = decide(
            plan,
            &targets,
            &[outcome("new1", true, Reason::Pass, false)],
            &Guard::default(),
            &probe(),
        );
        match out {
            GateOutcome::Apply { plan, .. } => {
                assert_eq!(plan.channels.len(), 1);
                assert_eq!(plan.channels[0].key, "new1", "未处理的新候选不得准入");
            }
            _ => panic!("应应用"),
        }
    }

    #[test]
    fn guard_disabled_removes_failed_url_change() {
        let targets = GateTargets {
            url_changed: vec![candidate("c1", true)],
            ..Default::default()
        };
        let out = decide(
            plan_with(&["c1"]),
            &targets,
            &[outcome("c1", false, Reason::Unreachable, true)],
            &Guard::default(),
            &probe(),
        );
        match out {
            GateOutcome::Apply { plan, stats, .. } => {
                assert!(plan.channels.is_empty());
                assert_eq!(stats.removed, 1);
            }
            _ => panic!("应应用"),
        }
    }

    #[test]
    fn guard_trips_at_exact_threshold() {
        let targets = GateTargets {
            url_changed: (0..10).map(|i| candidate(&format!("c{i}"), true)).collect(),
            ..Default::default()
        };
        let outcomes: Vec<_> = (0..10)
            .map(|i| outcome(&format!("c{i}"), false, Reason::Unreachable, true))
            .collect();
        let guard = Guard {
            enabled: true,
            max_remove_ratio: 0.5,
            min_remove_count: 10,
            base_channels: 0,
        };
        assert!(matches!(
            decide(plan_with(&[]), &targets, &outcomes, &guard, &probe()),
            GateOutcome::Paused { .. }
        ));
    }

    #[test]
    fn guard_does_not_trip_below_min_count() {
        let targets = GateTargets {
            url_changed: (0..9).map(|i| candidate(&format!("c{i}"), true)).collect(),
            ..Default::default()
        };
        let outcomes: Vec<_> = (0..9)
            .map(|i| outcome(&format!("c{i}"), false, Reason::Unreachable, true))
            .collect();
        let guard = Guard {
            enabled: true,
            max_remove_ratio: 0.5,
            min_remove_count: 10,
            base_channels: 0,
        };
        assert!(matches!(
            decide(plan_with(&[]), &targets, &outcomes, &guard, &probe()),
            GateOutcome::Apply { .. }
        ));
    }

    #[test]
    fn full_ffprobe_skips_host_cache() {
        let mut candidate = candidate("sync:s1:c", false);
        candidate.rules.host_cache = true;
        assert!(!should_use_host_cache(
            &candidate,
            ProbeMode::Full,
            true,
            false
        ));
        assert!(should_use_host_cache(
            &candidate,
            ProbeMode::Full,
            false,
            false
        ));
        assert!(should_use_host_cache(
            &candidate,
            ProbeMode::Speed,
            true,
            false
        ));
    }
}
