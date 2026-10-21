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
        .and_then(|value| value.as_table())
        .cloned()
        .unwrap_or_default();
    let use_alias = !aliases.is_empty();
    let token = if config.auth.token_enabled {
        state.play_tokens.current()
    } else {
        String::new()
    };
    Json(json!({
        "aliases": aliases,
        "random_id_enabled": config.auth.random_id_enabled,
        "token": token,
        "token_enabled": config.auth.token_enabled,
        "use_alias": use_alias,
    }))
}

/// `GET /panel/api/security/token`——当前 Token 与宽限期内的旧 Token。
pub async fn token(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let config = state.snapshot();
    let (current, old) = state.play_tokens.snapshot();
    Json(json!({
        "enabled": config.auth.token_enabled,
        "ttl_hours": config.auth.token_ttl_hours,
        "current": current,
        "old": old,
    }))
}

/// `POST /panel/api/security/token/rotate`——换新 Token；旧值保留 1 小时宽限。
///
/// follower 的 Token 由主机接管，禁止本地轮换（F-013 §2）。
pub async fn token_rotate(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let config = state.snapshot();
    if config.share.enabled && config.share.role.eq_ignore_ascii_case("follower") {
        return Err(ApiError::conflict("token 由主机接管"));
    }
    let token = state.play_tokens.rotate();
    Ok(Json(json!({ "ok": true, "token": token })))
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
    let ip = body
        .get("ip")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if ip.is_empty() {
        return Err(ApiError::bad_request("ip 不能为空"));
    }
    let seconds = body.get("seconds").and_then(Value::as_u64).unwrap_or(0);
    let applied = state.ip_guard.ban(&ip, seconds);
    Ok(Json(json!({
        "ok": true,
        "ip": ip,
        "seconds": applied,
        "max_seconds": MAX_BAN_SECS,
        "default_seconds": DEFAULT_BAN_SECS,
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
        return Err(ApiError::bad_request("ip 或 ips 不能为空"));
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
