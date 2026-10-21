//! 面板接口。全部需要会话（登录端点除外）。

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    config,
    http::{error::ApiError, ext::client_ip_with_peer, ext::PanelAuth},
    state::AppState,
};

/// 未知面板接口：`404` JSON（S-EP-08）。
///
/// 注册在 `/{gid}/{cid}/*path` 之前，避免面板路径被播放通配路由吞掉。
pub async fn api_missing() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "接口不存在" })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

/// `POST /panel/api/login` → `{"token":"<uuid>"}`（S-AUTH-01）。
///
/// 失败次数超过窗口配额后返回 `429`，并带 `locked_for_secs`（S-AUTH-04、F-010 §4）。
pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> Result<Json<Value>, ApiError> {
    let cfg = state.snapshot();
    let ip = client_ip_with_peer(&headers, Some(peer));
    let window = Duration::from_secs(cfg.auth.auth_fail_window_secs.max(1));
    let lock = Duration::from_secs(cfg.auth.auth_fail_lock_secs.max(1));
    if let Some(secs) = state.login_guard.check(
        &ip,
        window,
        cfg.auth.auth_fail_free_per_window,
        cfg.auth.auth_fail_ban_threshold,
        lock,
    ) {
        return Err(login_locked(secs));
    }
    if !cfg.panel.enabled {
        return Err(ApiError::Forbidden("面板未启用".into()));
    }

    let user_ok = req.username.trim() == cfg.panel.username;
    let pass_ok = config::sha256_hex(&req.password) == cfg.panel.password_sha256;
    if !(user_ok && pass_ok) {
        state.login_guard.failure(&ip, window);
        return Err(ApiError::Unauthorized(
            json!({ "error": "用户名或密码错误" }),
        ));
    }

    state.login_guard.success(&ip);
    let token = state.sessions.issue();
    tracing::info!(user = %cfg.panel.username, "面板登录成功");
    Ok(Json(json!({ "token": token })))
}

/// 构造登录锁定的 `429` 响应。
fn login_locked(secs: u64) -> ApiError {
    ApiError::too_many(json!({
        "error": format!("登录失败次数过多，请 {secs}s 后再试"),
        "locked_for_secs": secs,
    }))
}

/// `POST /panel/api/logout`。
pub async fn logout(
    PanelAuth(token): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    state.sessions.revoke(&token);
    Json(json!({ "ok": true }))
}

/// `GET /panel/api/status`。
///
/// M0 只报本实现自身状态。完整字段面见 `docs/design/03-http-contract.md`（E3）。
pub async fn status(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.snapshot();
    let channels: usize = cfg.groups.iter().map(|group| group.channels.len()).sum();

    Json(json!({
        "name": "iptv-proxy",
        "version": state.version(),
        "uptime_secs": state.uptime_secs(),
        "bind": cfg.server.bind,
        "log_level": cfg.server.log_level,
        "timezone_offset_hours": cfg.server.timezone_offset_hours,
        "groups": cfg.groups.len(),
        "channels": channels,
        "sessions": state.sessions.len(),
        "cache_backend": cfg.cache.backend,
        "cache_disk_path": cfg.cache.disk_path,
        // 入口扩展名由后端计算，前端只查表（S-URL-01）。
        "entry_exts": { "hls": ".m3u8", "mpd": ".mpd" },
    }))
}

/// `GET /panel/api/config`——全量配置。
pub async fn get_config(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let cfg = state.snapshot();
    Json(serde_json::to_value(&*cfg).unwrap_or(Value::Null))
}

/// `PUT /panel/api/config`——全量保存（S-CFG-01）。
pub async fn put_config(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let config: config::Config = serde_json::from_value(body)
        .map_err(|err| ApiError::bad_request(format!("配置格式错误: {err}")))?;

    config::save(&state.config_path, &config)
        .map_err(|err| ApiError::internal(format!("保存失败: {err}")))?;
    state.replace(config);

    tracing::info!("配置已保存并生效");
    Ok(Json(json!({ "ok": true })))
}

/// `POST /panel/api/reload`——从磁盘重读配置。
pub async fn reload(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let config = config::load(&state.config_path)
        .map_err(|err| ApiError::internal(format!("重载失败: {err}")))?;
    state.replace(config);
    tracing::info!("配置已重载");
    Ok(Json(json!({ "ok": true })))
}
