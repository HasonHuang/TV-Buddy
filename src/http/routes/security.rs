//! 安全面：播放 Token 与 IP 统计/封禁。
//!
//! 响应形状为本实现自定（目标只登记了路由存在性，F-010）。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::{
    auth::ip_guard::{DEFAULT_BAN_SECS, MAX_BAN_SECS},
    http::{error::ApiError, ext::PanelAuth},
    state::AppState,
};

/// `GET /panel/api/link-info`——播放链接与别名审计（S-GAP-04）。
///
/// 形状与目标一致：`aliases`、`random_id_enabled`、`token`、`token_enabled`、`use_alias`。
/// Token 关闭时返回空串。
pub async fn link_info(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let aliases = config
        .auth
        .extra
        .get("aliases")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let use_alias = !aliases.is_empty();
    // 全局播放令牌已废弃（`docs/design/09-users-and-devices.md` §9.2）。
    let token = String::new();
    Json(json!({
        "aliases": aliases,
        "random_id_enabled": config.auth.random_id_enabled,
        "token": token,
        "token_enabled": config.auth.token_enabled,
        "use_alias": use_alias,
    }))
}

/// `GET /panel/api/security/ip/stats`。
pub async fn ip_stats(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let entries = state.ip_guard.snapshot();
    let (total, banned) = state.ip_guard.summary();
    Json(json!({
        "count": total,
        "banned": banned,
        "entries": entries
            .into_iter()
            .map(|entry| json!({
                "ip": entry.ip,
                "requests": entry.requests,
                "failures": entry.failures,
                "banned": entry.banned,
                "remaining_secs": entry.remaining_secs,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// `POST /panel/api/security/ip/ban`——`{"ip","seconds"}`；`seconds` 缺省 1 小时，上限 30 天。
pub async fn ip_ban(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    // 目标用 `{"ips":[…],"secs":N}`；本实现同时接受单数 `ip` 与 `seconds`。
    let mut targets: Vec<String> = Vec::new();
    if let Some(ip) = body.get("ip").and_then(Value::as_str) {
        targets.push(ip.trim().to_string());
    }
    if let Some(ips) = body.get("ips").and_then(Value::as_array) {
        for ip in ips.iter().filter_map(Value::as_str) {
            targets.push(ip.trim().to_string());
        }
    }
    targets.retain(|ip| !ip.is_empty());
    targets.dedup();
    if targets.is_empty() {
        return Err(ApiError::bad_request("Missing ips array"));
    }
    let seconds = body
        .get("secs")
        .and_then(Value::as_u64)
        .or_else(|| body.get("seconds").and_then(Value::as_u64))
        .unwrap_or(0);
    let mut results = Vec::with_capacity(targets.len());
    let mut applied = 0;
    for ip in &targets {
        let secs = state.ip_guard.ban(ip, seconds);
        applied = secs;
        results.push(json!({ "ip": ip, "ok": true }));
    }
    Ok(Json(json!({
        "ok": true,
        "results": results,
        "secs": applied,
        "max_seconds": MAX_BAN_SECS,
        "default_seconds": DEFAULT_BAN_SECS,
        // 单条封禁的旧字段。
        "ip": targets.first().cloned().unwrap_or_default(),
        "seconds": applied,
    })))
}

/// `POST /panel/api/security/ip/unban`——`{"ip"}` 或 `{"ips":[…]}`。
pub async fn ip_unban(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut targets: Vec<String> = Vec::new();
    if let Some(ip) = body.get("ip").and_then(Value::as_str) {
        targets.push(ip.trim().to_string());
    }
    if let Some(ips) = body.get("ips").and_then(Value::as_array) {
        for ip in ips.iter().filter_map(Value::as_str) {
            targets.push(ip.trim().to_string());
        }
    }
    targets.retain(|ip| !ip.is_empty());
    if targets.is_empty() {
        // 目标原文（F-021 §3）。
        return Err(ApiError::bad_request("Missing ip"));
    }
    let mut removed = 0usize;
    for ip in &targets {
        if state.ip_guard.unban(ip) {
            removed += 1;
        }
    }
    Ok(Json(
        json!({ "ok": true, "removed": removed, "requested": targets.len() }),
    ))
}
