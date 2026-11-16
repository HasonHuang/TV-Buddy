//! 质量校验接口：状态、结果、来源、规则、手动运行与预览。
//!
//! 契约见 [`docs/design/16-quality-admission-system.md`](../../../../docs/design/16-quality-admission-system.md)。

use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};

use crate::{
    config::model::{Channel, Group},
    http::{error::ApiError, ext::PanelAuth},
    quality::{
        admission::source_layer,
        rules::effective_rules,
        schedule::{measure_only_for, run_now, scope_exists},
        state::{RunError, RunMode, RunScope, RunState},
        store::{QualityFilter, QualityResult},
    },
    state::AppState,
};

/// 手动校验请求。
#[derive(Debug, Clone, PartialEq)]
struct RunRequest {
    scope: RunScope,
    mode: RunMode,
}

impl RunRequest {
    /// 从 JSON 请求体解析。`scope` 缺省为 `all`，`run_mode` 缺省为 `quick`。
    fn from_json(body: &Value) -> Result<Self, ApiError> {
        let scope = match body.get("scope").and_then(Value::as_str).unwrap_or("all") {
            "all" => RunScope::All,
            "source" => RunScope::Source(required_id(body, "source")?),
            "group" => RunScope::Group(required_id(body, "group")?),
            "channel" => {
                let id = required_id(body, "channel")?;
                let Some((gid, cid)) = id.split_once('/') else {
                    return Err(ApiError::bad_request("scope=channel 的 id 需为 gid/cid"));
                };
                if gid.is_empty() || cid.is_empty() {
                    return Err(ApiError::bad_request("scope=channel 的 id 需为 gid/cid"));
                }
                RunScope::Channel(gid.to_string(), cid.to_string())
            }
            other => return Err(ApiError::bad_request(format!("未知 scope: {other}"))),
        };
        let mode = match body
            .get("run_mode")
            .and_then(Value::as_str)
            .unwrap_or("quick")
        {
            "quick" => RunMode::Quick,
            "full" => RunMode::Full,
            other => return Err(ApiError::bad_request(format!("未知 run_mode: {other}"))),
        };
        Ok(Self { scope, mode })
    }
}

fn required_id(body: &Value, scope: &str) -> Result<String, ApiError> {
    body.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ApiError::bad_request(format!("scope={scope} 需要 id")))
}

fn run_state_label(run: &RunState) -> &str {
    if !run.phase.is_empty() {
        return &run.phase;
    }
    if run.cancelled {
        "cancelled"
    } else if run.finished {
        "done"
    } else if run.tested == 0 {
        "queued"
    } else {
        "probing"
    }
}

fn run_json(run: &RunState) -> Value {
    json!({
        "run_id": run.run_id,
        "trigger": run.trigger,
        "phase": run.phase,
        "started_at": run.started_at,
        "total": run.total,
        "tested": run.tested,
        "valid": run.valid,
        "admitted": run.admitted,
        "removed": run.removed,
        "rejected": run.rejected,
        "skipped": run.skipped,
        "hidden": run.hidden,
        "bytes": run.bytes,
        "cancelled": run.cancelled,
        "finished": run.finished,
        "paused_by_guard": run.paused_by_guard,
        "measure_only": run.measure_only,
        "status": run_state_label(run),
        "error": run.error,
    })
}

/// `GET /panel/api/quality/status`。
pub async fn status(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let run = state.quality.run_status();
    let now = crate::schedule::now_epoch();
    let counts = state
        .quality_store
        .counts(now.saturating_sub(7 * 86_400))
        .unwrap_or_default();
    let next_run_at = state
        .quality_store
        .meta_get("next_run_at")
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    Json(json!({
        "enabled": config.quality.enabled,
        "quality_enabled": config.quality.enabled,
        "running": run.as_ref().is_some_and(|run| !run.finished),
        "run": run.as_ref().map(run_json),
        "schedule": {
            "mode": config.quality.schedule,
            "interval_hours": config.quality.interval_hours,
            "cron": config.quality.cron,
            "next_run_at": next_run_at,
        },
        "counts": {
            "admitted": counts.admitted,
            "removed_7d": counts.removed_7d,
            "sources": config.playlist.sources.iter().filter(|source| source.mode == "sync").count(),
        },
        "guard": {
            "enabled": config.quality.guard_enabled,
            "max_remove_ratio": config.quality.max_remove_ratio,
            "min_remove_count": config.quality.min_remove_count,
            "tripped": state.quality_store.meta_get("guard_tripped_at").ok().flatten().is_some_and(|value| !value.is_empty()),
        },
    }))
}

/// 结果查询参数。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ResultsQuery {
    filter: QualityFilter,
}

impl ResultsQuery {
    fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        let mut filter = QualityFilter::default();
        for (key, value) in pairs {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            match *key {
                "source_id" => filter.source_id = Some(value.into()),
                "gid" => filter.gid = Some(value.into()),
                "cid" => filter.cid = Some(value.into()),
                "action" => filter.action = Some(value.into()),
                "reason" => filter.reason = Some(value.into()),
                "q" => filter.q = Some(value.into()),
                "limit" => filter.limit = value.parse().unwrap_or(0),
                "offset" => filter.offset = value.parse().unwrap_or(0),
                _ => {}
            }
        }
        if filter.limit == 0 {
            filter.limit = 50;
        }
        Self { filter }
    }
}

fn result_json(item: &QualityResult) -> Value {
    let resolution = match (item.width, item.height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => format!("{width}x{height}"),
        _ => String::new(),
    };
    json!({
        "key": item.key,
        "scope": item.scope,
        "source_id": item.source_id,
        "source_key": item.source_key,
        "gid": item.gid,
        "cid": item.cid,
        "name": item.name,
        "target_group": item.target_group,
        "url_hash": item.url_hash,
        "action": item.action,
        "reason": item.reason,
        "ok": item.ok,
        "delay_ms": item.delay_ms,
        "speed_mib": item.speed_kib.map(|speed| (f64::from(speed) / 1024.0 * 100.0).round() / 100.0),
        "resolution": resolution,
        "http_status": item.http_status,
        "checked_at": item.checked_at,
        "next_check_at": item.next_check_at,
        "first_fail_at": item.first_fail_at,
        "rule_hash": item.rule_hash,
    })
}

/// `GET /panel/api/quality/results`。
pub async fn results(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let pairs: Vec<(&str, &str)> = query
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let query = ResultsQuery::from_pairs(&pairs);
    match state.quality_store.list(&query.filter) {
        Ok((total, items)) => Json(json!({
            "total": total,
            "items": items.iter().map(result_json).collect::<Vec<_>>(),
        })),
        Err(_) => Json(json!({ "total": 0, "items": [] })),
    }
}

/// `GET /panel/api/quality/sources`。
pub async fn sources(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let statuses = state.sources.statuses();
    let items: Vec<Value> = config
        .playlist
        .sources
        .iter()
        .filter(|source| source.mode == "sync")
        .map(|source| {
            let status = statuses.get(&source.id).cloned().unwrap_or_default();
            json!({
                "id": source.id,
                "name": source.name,
                "enabled": source.enabled,
                "channels": config
                    .groups
                    .iter()
                    .flat_map(|group| group.channels.iter())
                    .filter(|channel| channel.source_id == source.id)
                    .count(),
                "gate_enabled": source.quality.as_ref().and_then(|quality| quality.enabled).unwrap_or(true),
                "phase": status.gate_phase,
                "tested": status.gate_tested,
                "admitted": status.gate_admitted,
                "rejected": status.gate_rejected,
                "removed": status.gate_removed,
                "paused": status.gate_paused,
                "pause_reason": status.gate_pause_reason,
                "truncated": status.gate_truncated,
                "cursor": status.gate_cursor,
                "remaining": status.gate_remaining,
                "last_at": status.gate_last_at,
            })
        })
        .collect();
    Json(json!({ "items": items }))
}

/// `GET /panel/api/quality/effective`。
pub async fn effective(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let config = state.snapshot();
    let gid = query.get("gid").map(String::as_str);
    let cid = query.get("cid").map(String::as_str);
    let group = gid.and_then(|gid| config.group(gid));
    let channel = group.and_then(|group| {
        cid.and_then(|cid| group.channels.iter().find(|channel| channel.id == cid))
    });
    let empty_group = Group::default();
    let empty_channel = Channel::default();
    let group = group.unwrap_or(&empty_group);
    let channel = channel.unwrap_or(&empty_channel);
    let rules = effective_rules(&config, group, channel);
    Ok(Json(json!({
        "rules": {
            "enabled": rules.enabled,
            "min_speed_mib": rules.min_speed_mib,
            "max_delay_ms": rules.max_delay_ms,
            "min_resolution_px": rules.min_resolution_px,
            "max_resolution_px": rules.max_resolution_px,
            "supply": rules.supply,
            "host_cache": rules.host_cache,
        },
        "source_layer": source_layer(&config, group, channel),
    })))
}

/// `POST /panel/api/quality/run`。
pub async fn run(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let request = RunRequest::from_json(&body)?;
    let config = state.snapshot();
    if !scope_exists(&config, &request.scope) {
        return Err(ApiError::bad_request("范围内没有可校验的频道"));
    }
    let measure_only = measure_only_for(&config);
    match run_now(state.clone(), request.scope, request.mode).await {
        Ok((run_id, queued)) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "ok": true,
                "run_id": run_id,
                "queued": queued,
                "measure_only": measure_only,
            })),
        )),
        Err(RunError::Busy) => Err(ApiError::conflict_code("run_active", "已有质量校验在运行")),
        Err(RunError::Disabled) => Err(ApiError::bad_request("质量校验未启用")),
        Err(RunError::InvalidScope) => Err(ApiError::bad_request("范围内没有可校验的频道")),
    }
}

/// `GET /panel/api/quality/runs/:run_id`。
pub async fn run_status(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let run = state
        .quality
        .run_status()
        .filter(|run| run.run_id == run_id)
        .ok_or_else(|| ApiError::not_found("未找到该次校验"))?;
    Ok(Json(run_json(&run)))
}

/// `POST /panel/api/quality/cancel`。
pub async fn cancel(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let running = state.quality.run_status().is_some_and(|run| !run.finished);
    if running {
        state.quality.cancel_run();
    }
    Json(json!({ "ok": true, "running": running }))
}

/// `POST /panel/api/quality/preview`。按最近结果预览将移出的频道。不写状态。
pub async fn preview(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let filter = QualityFilter {
        limit: 500,
        ..QualityFilter::default()
    };
    let items = state
        .quality_store
        .list(&filter)
        .map(|(_, items)| items)
        .unwrap_or_default();
    let mut would_remove = Vec::new();
    for item in items {
        if item.ok || item.reason == "probe_error" {
            continue;
        }
        if !matches!(item.action.as_str(), "admitted" | "removal_paused") {
            continue;
        }
        let Some(group) = config.group(&item.gid) else {
            continue;
        };
        let Some(channel) = group.channels.iter().find(|channel| channel.id == item.cid) else {
            continue;
        };
        let source_enabled = config
            .playlist
            .sources
            .iter()
            .find(|source| source.id == item.source_id)
            .is_none_or(|source| source.enabled);
        if source_enabled && effective_rules(&config, group, channel).enabled {
            would_remove.push(result_json(&item));
        }
    }
    let total = would_remove.len();
    Json(json!({ "would_remove": would_remove, "total": total }))
}

/// 调度预览请求。
#[derive(Debug, Clone, PartialEq)]
struct SchedulePreviewRequest {
    mode: String,
    interval_hours: u64,
    cron: String,
}

impl SchedulePreviewRequest {
    fn from_json(body: &Value) -> Result<Self, ApiError> {
        let mode = body
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("interval")
            .to_string();
        if mode != "interval" && mode != "cron" {
            return Err(ApiError::bad_request("mode 只能是 interval 或 cron"));
        }
        let interval_hours = body
            .get("interval_hours")
            .and_then(Value::as_u64)
            .unwrap_or(6);
        let cron = body
            .get("cron")
            .and_then(Value::as_str)
            .unwrap_or("0 */6 * * *")
            .to_string();
        if mode == "cron" && crate::cron::parse_cron(&cron).is_err() {
            return Err(ApiError::bad_request("cron 不是有效的 5 段表达式"));
        }
        Ok(Self {
            mode,
            interval_hours,
            cron,
        })
    }
}

/// `POST /panel/api/quality/schedule/preview`。
pub async fn schedule_preview(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let request = SchedulePreviewRequest::from_json(&body)?;
    let config = state.snapshot();
    let now = crate::schedule::now_epoch();
    let offset = config.server.timezone_offset_hours as i64;
    let times = if request.mode == "cron" {
        crate::cron::cron_preview(&request.cron, now, offset, 3)
            .map_err(|error| ApiError::bad_request(format!("{error:#}")))?
    } else {
        (1..=3)
            .map(|index| now + (request.interval_hours * index) as i64 * 3_600)
            .collect()
    };
    Ok(Json(json!({ "times": times })))
}

#[cfg(test)]
mod tests {
    use crate::quality::state::{RunMode, RunScope};

    use super::*;

    #[test]
    fn run_request_parses_scope_and_mode() {
        let body = serde_json::json!({ "scope": "channel", "id": "g1/c1", "run_mode": "quick" });
        let request = RunRequest::from_json(&body).expect("parse");
        assert_eq!(request.scope, RunScope::Channel("g1".into(), "c1".into()));
        assert_eq!(request.mode, RunMode::Quick);
    }

    #[test]
    fn run_request_rejects_missing_id() {
        let body = serde_json::json!({ "scope": "group" });
        assert!(RunRequest::from_json(&body).is_err());
    }

    #[test]
    fn results_query_parses_filters_and_default_limit() {
        let query = ResultsQuery::from_pairs(&[
            ("source_id", "s1"),
            ("action", "removed"),
            ("limit", "50"),
            ("offset", "100"),
        ]);
        assert_eq!(query.filter.source_id.as_deref(), Some("s1"));
        assert_eq!(query.filter.action.as_deref(), Some("removed"));
        assert_eq!(query.filter.limit, 50);
        assert_eq!(query.filter.offset, 100);

        let empty = ResultsQuery::from_pairs(&[]);
        assert_eq!(empty.filter.limit, 50);
    }

    #[test]
    fn schedule_preview_rejects_bad_cron() {
        let body = serde_json::json!({ "mode": "cron", "cron": "bad" });
        assert!(SchedulePreviewRequest::from_json(&body).is_err());
    }

    #[test]
    fn run_json_reports_derived_status() {
        let mut run = RunState {
            run_id: "q-1".into(),
            trigger: "manual".into(),
            phase: "queued".into(),
            started_at: 1,
            total: 3,
            tested: 0,
            valid: 0,
            admitted: 0,
            removed: 0,
            rejected: 0,
            skipped: 0,
            hidden: 0,
            bytes: 0,
            cancelled: false,
            finished: false,
            paused_by_guard: false,
            measure_only: false,
            error: None,
        };
        assert_eq!(run_json(&run)["status"], "queued");
        run.phase = "probing".into();
        run.tested = 1;
        assert_eq!(run_json(&run)["status"], "probing");
        run.phase = "done".into();
        run.finished = true;
        assert_eq!(run_json(&run)["status"], "done");
    }
}
