//! 质量校验接口：状态、结果、手动运行与预览。
//!
//! 契约见 [`docs/design/13-availability-and-speedtest.md`](../../../../docs/design/13-availability-and-speedtest.md)。
//! P1 只存内存结果。`ffprobe` 未接入，状态固定为未安装。

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};

use crate::{
    config::model::{Channel, Config, Group},
    http::{error::ApiError, ext::PanelAuth},
    playlist::quality_hidden,
    quality::{
        schedule::{run_now, scope_exists},
        state::{Entry, QualityState, RunError, RunMode, RunScope, RunState},
    },
    state::AppState,
};

/// 手动校验请求。
#[derive(Debug, Clone, PartialEq)]
struct RunRequest {
    scope: RunScope,
    mode: RunMode,
    force: bool,
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
        let force = body.get("force").and_then(Value::as_bool).unwrap_or(false);
        Ok(Self { scope, mode, force })
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

fn run_state_label(run: &RunState) -> &'static str {
    if run.cancelled {
        "cancelled"
    } else if run.finished {
        "done"
    } else if run.tested == 0 {
        "queued"
    } else {
        "running"
    }
}

fn run_json(run: &RunState) -> Value {
    json!({
        "run_id": run.run_id,
        "started_at": run.started_at,
        "total": run.total,
        "tested": run.tested,
        "valid": run.valid,
        "hidden": run.hidden,
        "bytes": run.bytes,
        "cancelled": run.cancelled,
        "finished": run.finished,
        "status": run_state_label(run),
        "error": run.error,
    })
}

/// `GET /panel/api/quality/status`。
pub async fn status(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let run = state.quality.run_status();
    let rules = &config.quality.rules;
    Json(json!({
        "enabled": config.quality.enabled,
        "probe_mode": config.quality.probe_mode,
        "interval_secs": config.quality.interval_secs,
        "running": run.as_ref().is_some_and(|run| !run.finished),
        "run": run.as_ref().map(run_json),
        "rules": {
            "action": rules.action,
            "retain": rules.retain,
            "supply": rules.supply,
            "min_speed_mib": rules.min_speed_mib,
            "max_delay_ms": rules.max_delay_ms,
            "min_resolution": rules.min_resolution,
            "max_resolution": rules.max_resolution,
            "stale_keep_secs": config.quality.stale_keep_secs,
        },
        "ffprobe": { "installed": false, "required": false },
        "entries": state.quality.snapshot().len(),
    }))
}

/// `GET /panel/api/quality/results`。支持 `gid`、`cid`、`limit`。
pub async fn results(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let config = state.snapshot();
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(200)
        .min(1000);
    let gid_filter = query.get("gid").map(String::as_str);
    let cid_filter = query.get("cid").map(String::as_str);
    let mut items = Vec::new();
    for group in &config.groups {
        if gid_filter.is_some_and(|gid| gid != group.id) {
            continue;
        }
        for channel in &group.channels {
            if cid_filter.is_some_and(|cid| cid != channel.id) {
                continue;
            }
            let Some(entry) = state.quality.get(&group.id, &channel.id) else {
                continue;
            };
            items.push(entry_json(&config, &state.quality, group, channel, &entry));
        }
    }
    let total = items.len();
    items.truncate(limit);
    Json(json!({ "total": total, "items": items }))
}

fn entry_json(
    config: &Config,
    quality: &QualityState,
    group: &Group,
    channel: &Channel,
    entry: &Entry,
) -> Value {
    let interval = config.quality.interval_secs;
    let stale = interval > 0
        && crate::schedule::now_epoch() - entry.checked_at > (interval as i64).saturating_mul(2);
    let resolution = if entry.width > 0 && entry.height > 0 {
        format!("{}x{}", entry.width, entry.height)
    } else {
        String::new()
    };
    let speed_mib = (f64::from(entry.speed_kib) / 1024.0 * 100.0).round() / 100.0;
    json!({
        "gid": group.id,
        "cid": channel.id,
        "name": channel.name,
        "source_id": channel.source_id,
        "checked_at": entry.checked_at,
        "next_check_at": entry.next_check_at,
        "last_ok_at": entry.last_ok_at,
        "first_fail_at": entry.first_fail_at,
        "ok": entry.ok(),
        "reachable": entry.reachable(),
        "reason": entry.reason.as_str(),
        "delay_ms": entry.delay_ms,
        "speed_mib": speed_mib,
        "width": entry.width,
        "height": entry.height,
        "resolution": resolution,
        "consecutive_fails": entry.consecutive_fails,
        "flags": entry.flags,
        "hidden": quality_hidden(config, Some(quality), group, channel),
        "stale": stale,
    })
}

/// `POST /panel/api/quality/run`。
pub async fn run(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let request = RunRequest::from_json(&body)?;
    let config = state.snapshot();
    if !config.quality.enabled && !request.force {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "质量校验未启用", "code": "quality_disabled" })),
        ));
    }
    if !scope_exists(&config, &request.scope) {
        return Err(ApiError::not_found("范围不存在"));
    }
    match run_now(
        state.clone(),
        request.scope,
        request.mode,
        request.force,
    )
    .await
    {
        Ok((run_id, queued)) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "ok": true, "run_id": run_id, "queued": queued })),
        )),
        Err(RunError::Busy) => Err(ApiError::conflict_code(
            "run_active",
            "已有质量校验在运行",
        )),
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
pub async fn cancel(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let running = state
        .quality
        .run_status()
        .is_some_and(|run| !run.finished);
    if running {
        state.quality.cancel_run();
    }
    Json(json!({ "ok": true, "running": running }))
}

/// `POST /panel/api/quality/preview`。统计当前结果中会被隐藏的频道，不写状态。
pub async fn preview(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let config = state.snapshot();
    let mut hidden = Vec::new();
    for group in &config.groups {
        for channel in &group.channels {
            if !quality_hidden(&config, Some(&state.quality), group, channel) {
                continue;
            }
            let reason = state
                .quality
                .get(&group.id, &channel.id)
                .map_or("unknown", |entry| entry.reason.as_str());
            hidden.push(json!({
                "gid": group.id,
                "cid": channel.id,
                "name": channel.name,
                "reason": reason,
            }));
        }
    }
    let total = hidden.len();
    Json(json!({ "hidden": hidden, "total": total }))
}

#[cfg(test)]
mod tests {
    use crate::quality::state::{RunMode, RunScope};

    use super::*;

    #[test]
    fn run_request_parses_scope_and_mode() {
        let body = serde_json::json!({ "scope": "group", "id": "sports", "run_mode": "full", "force": true });
        let request = RunRequest::from_json(&body).expect("parse");
        assert_eq!(request.scope, RunScope::Group("sports".into()));
        assert_eq!(request.mode, RunMode::Full);
        assert!(request.force);
    }

    #[test]
    fn run_request_rejects_missing_id() {
        let body = serde_json::json!({ "scope": "group" });
        assert!(RunRequest::from_json(&body).is_err());
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
        run.tested = 1;
        assert_eq!(run_json(&run)["status"], "running");
        run.finished = true;
        assert_eq!(run_json(&run)["status"], "done");
        run.cancelled = true;
        assert_eq!(run_json(&run)["status"], "cancelled");
    }
}
