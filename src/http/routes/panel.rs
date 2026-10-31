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
    // 升级阶梯只在「IP 管控」打开时启用。默认关闭时锁定时长仍是固定的
    // `auth_fail_lock_secs`，与改动前一致。
    let checked = if cfg.auth.ip_guard_enabled {
        state.login_guard.check_escalating(
            &ip,
            window,
            cfg.auth.auth_fail_free_per_window,
            cfg.auth.auth_fail_ban_threshold,
            lock,
            Duration::from_secs(cfg.auth.auth_fail_ban_duration_secs.max(1)),
            Duration::from_secs(cfg.auth.auth_fail_ban_repeat_duration_secs.max(1)),
            Duration::from_secs(cfg.auth.auth_fail_ban_escalate_window_secs.max(1)),
        )
    } else {
        state.login_guard.check(
            &ip,
            window,
            cfg.auth.auth_fail_free_per_window,
            cfg.auth.auth_fail_ban_threshold,
            lock,
        )
    };
    if let Some(secs) = checked {
        return Err(login_locked(secs));
    }
    if !cfg.panel.enabled {
        return Err(ApiError::Forbidden("面板未启用".into()));
    }

    // 账号来自 `users.db`。口令用 argon2id 校验。
    let Some(user) = state.users.find_by_username(req.username.trim()) else {
        state.login_guard.failure(&ip, window);
        return Err(ApiError::Unauthorized(
            json!({ "error": "用户名或密码错误" }),
        ));
    };
    let verdict = crate::users::password::verify_any(&req.password, &user.password_hash);
    if verdict == crate::users::password::Verdict::Fail {
        state.login_guard.failure(&ip, window);
        return Err(ApiError::Unauthorized(
            json!({ "error": "用户名或密码错误" }),
        ));
    }
    if !user.web_ui_access && !user.role.is_admin() {
        return Err(ApiError::Forbidden("该账号不允许登录面板".into()));
    }
    // 旧 `sha256:` 散列在首次登录时升级成 argon2id。
    if verdict == crate::users::password::Verdict::OkNeedsRehash {
        if let Ok(hash) = crate::users::password::hash(&req.password) {
            let mut next = user.clone();
            next.password_hash = hash;
            if let Err(error) = state.users.update_user(next) {
                tracing::warn!(%error, "口令散列升级失败");
            }
        }
    }

    state.login_guard.success(&ip);
    let token = state
        .sessions
        .issue(&user.id, &user.username, user.role.is_admin());
    tracing::info!(user = %user.username, "面板登录成功");
    Ok(Json(json!({
        "token": token,
        "username": user.username,
        "role": user.role.as_str(),
    })))
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
/// 字段面按目标 v0.8.2 实测（F-021 §6）。要点：
/// - `traffic_in_bytes` 目标恒为 0（S-BUG-01），本实现固定输出 0。
/// - `online_viewers` 只统计窗口内带播放 Token 的请求。
/// - `entry_exts` 是 `gid → ".m3u8" | ".mpd"` 的映射，不是固定两项。
pub async fn status(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.snapshot();
    let channels: usize = cfg.groups.iter().map(|group| group.channels.len()).sum();
    let disk = state.metrics.disk(&state.cache_dir);
    let entry_exts: serde_json::Map<String, Value> = cfg
        .groups
        .iter()
        .map(|group| {
            let ext = if group.format == "mpd" {
                ".mpd"
            } else {
                ".m3u8"
            };
            (group.id.clone(), json!(ext))
        })
        .collect();

    Json(json!({
        "uptime_secs": state.uptime_secs(),
        "cache_backend": cfg.cache.backend,
        "active_sessions": state.sessions.len(),
        "online_viewers": state.metrics.online(cfg.stats.online_window_secs),
        "group_count": cfg.groups.len(),
        "channel_count": channels,
        "version": state.version(),
        "mem_rss_mb": crate::metrics::rss_mb(),
        "cpu_seconds": crate::metrics::cpu_seconds(),
        "disk_cache_mb": disk.cache_mb,
        "recording_mb": disk.recording_mb,
        "low_resource_mode": cfg.server.low_resource_mode,
        // 目标没有对应配置键，固定 10。
        "poll_interval_secs": 10,
        // 目标缺陷：回源流量恒 0，不修（S-BUG-01）。
        "traffic_in_bytes": 0,
        "traffic_out_bytes": state.metrics.out_bytes(),
        // 整文件缓存未实现，计数恒 0；预算按配置换算。
        "whole_file_committed": 0,
        "whole_file_budget": crate::metrics::whole_file_budget(
            cfg.cache.dash.whole_file_budget_mb,
        ),
        "whole_file_peak": 0,
        "whole_file_hits": 0,
        "whole_file_fallbacks": 0,
        "entry_exts": entry_exts,
        "cpu_cores": crate::metrics::cpu_cores(),
        // 面板功能入口开关。侧栏按它渲染（`[features]` 段）。
        "features": {
            "recording": cfg.features.recording,
            "livestream": cfg.features.livestream,
            "logo": cfg.features.logo,
            "playlist": cfg.features.playlist,
            "assets": cfg.features.assets,
            "users": cfg.features.users,
            "epg": cfg.features.epg,
            "stats": cfg.features.stats,
        },
        // 面板功能入口开关。侧栏按它渲染。
        "features": {
            "recording": cfg.features.recording,
            "livestream": cfg.features.livestream,
            "logo": cfg.features.logo,
            "playlist": cfg.features.playlist,
            "assets": cfg.features.assets,
            "users": cfg.features.users,
            "epg": cfg.features.epg,
            "stats": cfg.features.stats,
        },
        // 以下为本实现附加字段，目标没有。便于本地面板排障。
        "name": "tv-buddy",
        "bind": cfg.server.bind,
        "log_level": cfg.server.log_level,
        "timezone_offset_hours": cfg.server.timezone_offset_hours,
        "cache_disk_path": cfg.cache.disk_path,
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
