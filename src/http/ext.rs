//! 请求提取器：面板会话、客户端地址。

use std::sync::Arc;

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};

use crate::{http::error::ApiError, state::AppState};

/// 已认证的面板请求。携带原始 token，便于注销。
pub struct PanelAuth(pub String);

#[async_trait::async_trait]
impl FromRequestParts<Arc<AppState>> for PanelAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // 目标同时接受 `Authorization: Bearer <token>` 与 `X-Token: <token>`（S-AUTH-01）。
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(|value| value.trim().to_string())
            .or_else(|| {
                parts
                    .headers
                    .get("x-token")
                    .and_then(|value| value.to_str().ok())
                    .map(|value| value.trim().to_string())
            })
            .filter(|token| !token.is_empty())
            .ok_or_else(ApiError::need_login)?;

        if state.sessions.verify(&token) {
            Ok(PanelAuth(token))
        } else {
            Err(ApiError::need_login())
        }
    }
}
