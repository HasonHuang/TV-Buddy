//! 用户订阅入口：`/sub/:token/playlist.m3u`。
//!
//! 令牌来自 `access_tokens`。无效或过期一律 `404`，不区分原因，
//! 避免探测令牌是否存在。
//!
//! 清单按用户的可见分组与频道过滤
//! （`docs/design/09-users-and-devices.md` §5）。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{http::routes::play::base_url, state::AppState};

fn missing() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// `GET /sub/:token/playlist.m3u`。
pub async fn playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let Some(access) = state.users.find_token(&token) else {
        return missing();
    };
    let Some(user) = state.users.find_user(&access.user_id) else {
        return missing();
    };
    state.users.touch_token(&token);

    let config = state.snapshot();
    let body = crate::playlist::render_user(&config, &user, &base_url(&headers), &token);
    (
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.apple.mpegurl; charset=utf-8",
            ),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"playlist.m3u\"",
            ),
        ],
        body,
    )
        .into_response()
}
