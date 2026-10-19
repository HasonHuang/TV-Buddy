//! 错误到响应的映射。
//!
//! 面板面统一返回 JSON `{"error": "..."}`。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

#[derive(Debug)]
pub enum ApiError {
    /// 未登录或会话失效。
    Unauthorized(Value),
    Forbidden(String),
    BadRequest(String),
    /// M1 起使用（订阅与频道接口）。
    #[allow(dead_code)]
    NotFound(String),
    /// 资源冲突，如分组 id 重复。
    Conflict(String),
    Internal(String),
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// 未登录（默认文案）。
    pub fn need_login() -> Self {
        Self::Unauthorized(json!({ "error": "未登录或登录已失效" }))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            ApiError::Unauthorized(body) => (StatusCode::UNAUTHORIZED, body),
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, json!({ "error": message })),
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, json!({ "error": message })),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, json!({ "error": message })),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, json!({ "error": message })),
            ApiError::Internal(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": message }),
            ),
        };
        (status, Json(body)).into_response()
    }
}
