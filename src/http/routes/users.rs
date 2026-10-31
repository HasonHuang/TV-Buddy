//! 用户与设备管理。
//!
//! 功能对齐 mpd-hls 的 `/users`
//! （`docs/design/09-users-and-devices.md` §3、§4）。
//! 用户与令牌的增删改只允许管理员。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    http::{error::ApiError, ext::PanelUser},
    state::AppState,
    users::{model::now_ms, AccessToken, Purpose, Role, User},
};

fn require_admin(user: &PanelUser) -> Result<(), ApiError> {
    if user.is_admin {
        Ok(())
    } else {
        Err(ApiError::Forbidden("需要管理员权限".into()))
    }
}

/// 对外的用户视图：不含口令散列。
fn user_view(user: &User) -> Value {
    json!({
        "id": user.id,
        "username": user.username,
        "role": user.role.as_str(),
        "allowed_group_ids": user.allowed_group_ids,
        "channel_filter_enabled": user.channel_filter_enabled,
        "allowed_channel_ids": user.allowed_channel_ids,
        "web_ui_access": user.web_ui_access,
        "created_at_ms": user.created_at_ms,
        "updated_at_ms": user.updated_at_ms,
    })
}

/// 对外的令牌视图。
fn token_view(token: &AccessToken) -> Value {
    json!({
        "token": token.token,
        "user_id": token.user_id,
        "purpose": match token.purpose {
            Purpose::Device => "device",
            Purpose::WebUiLinks => "web_ui_links",
        },
        "label": token.label,
        "created_at_ms": token.created_at_ms,
        "expires_at_ms": token.expires_at_ms,
        "last_seen_at_ms": token.last_seen_at_ms,
        "playlist_url": format!("/sub/{}/playlist.m3u", token.token),
        "epg_url": format!("/sub/{}/epg.xml", token.token),
    })
}

#[derive(Debug, Deserialize)]
pub struct CreateReq {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub allowed_group_ids: Vec<String>,
    #[serde(default)]
    pub channel_filter_enabled: bool,
    #[serde(default)]
    pub allowed_channel_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub web_ui_access: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct UpdateReq {
    pub username: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub allowed_group_ids: Vec<String>,
    #[serde(default)]
    pub channel_filter_enabled: bool,
    #[serde(default)]
    pub allowed_channel_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub web_ui_access: bool,
}

#[derive(Debug, Deserialize)]
pub struct PasswordReq {
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenReq {
    #[serde(default)]
    pub label: Option<String>,
    /// 有效期（秒）。缺省或 `null` 表示永不过期。
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct TokenUpdateReq {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

fn parse_role(raw: Option<String>, fallback: Role) -> Role {
    match raw.as_deref() {
        Some("admin") => Role::Admin,
        Some("user") => Role::User,
        _ => fallback,
    }
}

/// `GET /panel/api/users`。
pub async fn list(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    let users: Vec<Value> = state.users.list_users().iter().map(user_view).collect();
    Ok(Json(json!({ "users": users })))
}

/// `POST /panel/api/users`。
pub async fn create(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateReq>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    if req.username.trim().is_empty() {
        return Err(ApiError::bad_request("用户名不能为空"));
    }
    if req.password.is_empty() {
        return Err(ApiError::bad_request("密码不能为空"));
    }
    let role = parse_role(req.role, Role::User);
    if role == Role::User && req.allowed_group_ids.is_empty() {
        return Err(ApiError::bad_request("普通用户至少需要一个可见分组"));
    }
    let hash = crate::users::password::hash(&req.password)
        .map_err(|error| ApiError::internal(format!("口令散列失败: {error}")))?;
    let now = now_ms();
    let user = User {
        id: String::new(),
        username: req.username.trim().to_string(),
        role,
        password_hash: hash,
        allowed_group_ids: req.allowed_group_ids,
        channel_filter_enabled: req.channel_filter_enabled,
        allowed_channel_ids: req.allowed_channel_ids,
        web_ui_access: req.web_ui_access || role == Role::Admin,
        created_at_ms: now,
        updated_at_ms: now,
    };
    let created = state
        .users
        .create_user(user)
        .map_err(|error| ApiError::bad_request(format!("{error}")))?;
    tracing::info!(user = %created.username, "新建用户");
    Ok(Json(json!({ "ok": true, "user": user_view(&created) })))
}

/// `PUT /panel/api/users/:id`。
pub async fn update(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateReq>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    let Some(existing) = state.users.find_user(&id) else {
        return Err(ApiError::not_found("用户不存在"));
    };
    let role = parse_role(req.role, existing.role);
    if role == Role::User && req.allowed_group_ids.is_empty() {
        return Err(ApiError::bad_request("普通用户至少需要一个可见分组"));
    }
    let next = User {
        username: req.username.trim().to_string(),
        role,
        allowed_group_ids: req.allowed_group_ids,
        channel_filter_enabled: req.channel_filter_enabled,
        allowed_channel_ids: req.allowed_channel_ids,
        web_ui_access: req.web_ui_access || role == Role::Admin,
        ..existing
    };
    let saved = state
        .users
        .update_user(next)
        .map_err(|error| ApiError::bad_request(format!("{error}")))?;
    tracing::info!(user = %saved.username, "更新用户");
    Ok(Json(json!({ "ok": true, "user": user_view(&saved) })))
}

/// `PUT /panel/api/users/:id/password`。
pub async fn set_password(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<PasswordReq>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    if req.password.is_empty() {
        return Err(ApiError::bad_request("密码不能为空"));
    }
    let Some(existing) = state.users.find_user(&id) else {
        return Err(ApiError::not_found("用户不存在"));
    };
    let hash = crate::users::password::hash(&req.password)
        .map_err(|error| ApiError::internal(format!("口令散列失败: {error}")))?;
    let next = User {
        password_hash: hash,
        ..existing
    };
    state
        .users
        .update_user(next)
        .map_err(|error| ApiError::internal(format!("{error}")))?;
    // 改密后踢掉该用户的全部面板会话。
    state.sessions.revoke_user(&id);
    tracing::info!(user_id = %id, "重置用户口令");
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /panel/api/users/:id`。
pub async fn remove(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    let Some(target) = state.users.find_user(&id) else {
        return Err(ApiError::not_found("用户不存在"));
    };
    // 不允许删掉最后一个管理员。
    if target.role.is_admin() {
        let admins = state
            .users
            .list_users()
            .iter()
            .filter(|user| user.role.is_admin())
            .count();
        if admins <= 1 {
            return Err(ApiError::bad_request("不能删除最后一个管理员"));
        }
    }
    let removed = state
        .users
        .delete_user(&id)
        .map_err(|error| ApiError::internal(format!("{error}")))?;
    state.sessions.revoke_user(&id);
    tracing::info!(user = %target.username, "删除用户");
    Ok(Json(json!({ "ok": true, "removed": removed })))
}

/// `GET /panel/api/users/channel-options`——分组与频道选项，供勾选。
pub async fn channel_options(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    let cfg = state.snapshot();
    let groups: Vec<Value> = cfg
        .groups
        .iter()
        .map(|group| {
            json!({
                "id": group.id,
                "name": group.name,
                "channels": group.channels.iter()
                    .map(|channel| json!({
                        "id": channel.id,
                        "name": channel.name,
                        "key": format!("{}/{}", group.id, channel.id),
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "groups": groups })))
}

/// `GET /panel/api/users/:id/tokens`。
pub async fn list_tokens(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    let tokens: Vec<Value> = state
        .users
        .list_tokens(&id)
        .iter()
        .map(token_view)
        .collect();
    Ok(Json(json!({ "tokens": tokens })))
}

/// `POST /panel/api/users/:id/tokens`。
pub async fn create_token(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TokenReq>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    let ttl_ms = req
        .ttl_secs
        .filter(|secs| *secs > 0)
        .map(|secs| secs * 1000);
    let token = state
        .users
        .create_token(&id, Purpose::Device, req.label, ttl_ms)
        .map_err(|error| ApiError::bad_request(format!("{error}")))?;
    Ok(Json(json!({ "ok": true, "token": token_view(&token) })))
}

/// `PUT /panel/api/users/:id/tokens/:token`。
pub async fn update_token(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    Path((id, token)): Path<(String, String)>,
    Json(req): Json<TokenUpdateReq>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    if !state
        .users
        .list_tokens(&id)
        .iter()
        .any(|item| item.token == token)
    {
        return Err(ApiError::not_found("令牌不存在"));
    }
    let ttl_ms = req
        .ttl_secs
        .filter(|secs| *secs > 0)
        .map(|secs| secs * 1000);
    let saved = state
        .users
        .update_token(&token, req.label, ttl_ms)
        .map_err(|error| ApiError::bad_request(format!("{error}")))?;
    Ok(Json(json!({ "ok": true, "token": token_view(&saved) })))
}

/// `DELETE /panel/api/users/:id/tokens/:token`。
pub async fn revoke_token(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    Path((id, token)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&actor)?;
    if !state
        .users
        .list_tokens(&id)
        .iter()
        .any(|item| item.token == token)
    {
        return Err(ApiError::not_found("令牌不存在"));
    }
    state
        .users
        .revoke_token(&token)
        .map_err(|error| ApiError::internal(format!("{error}")))?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /panel/api/me`——当前登录者与自己的订阅地址。
///
/// 普通用户也能调。返回的 `groups` 只含该用户可见的分组与频道，
/// 供面板「我的订阅」页直接渲染，不需要额外的管理权限。
pub async fn me(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let Some(user) = state.users.find_user(&actor.user_id) else {
        return Err(ApiError::need_login());
    };
    let token = ensure_web_token(&state, &user)?;
    let config = state.snapshot();
    let base = crate::http::ext::base_url(&headers);
    let groups: Vec<Value> = crate::playlist::user_view(&config, &user, &base, &token)
        .iter()
        .map(|group| {
            json!({
                "id": group.id,
                "name": group.name,
                "channels": group.channels.iter().map(|channel| json!({
                    "id": channel.id,
                    "name": channel.name,
                    "url": channel.url,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({
        "user": user_view(&user),
        "token": token,
        "playlist_link": format!("/sub/{token}/playlist.m3u"),
        "epg_link": format!("/sub/{token}/epg.xml"),
        "groups": groups,
    })))
}

/// 面板内使用的播放令牌。
///
/// 每个用户一个，`Purpose::WebUiLinks`，不过期。没有就建一个。
/// 这样面板里的播放地址与订阅地址始终有效，不需要用户手工建设备令牌。
fn ensure_web_token(state: &AppState, user: &User) -> Result<String, ApiError> {
    if let Some(existing) = state
        .users
        .list_tokens(&user.id)
        .into_iter()
        .find(|token| token.purpose == Purpose::WebUiLinks)
    {
        return Ok(existing.token);
    }
    state
        .users
        .create_token(
            &user.id,
            Purpose::WebUiLinks,
            Some("面板链接".to_string()),
            None,
        )
        .map(|token| token.token)
        .map_err(|error| ApiError::internal(format!("签发面板令牌失败: {error}")))
}
