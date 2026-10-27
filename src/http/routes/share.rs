//! 分享端点：`GET /api/share/pull`。
//!
//! 门禁为双重校验（F-013）：
//! 1. `key` 必须等于 `[share].key`；
//! 2. 源 IP 必须在 `[share].trust_ips`。
//!
//! 任一失败 → `403`。`[share].enabled` 为假 → `404`（形状自定）。

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use tracing::warn;

use crate::{http::ext::client_ip_with_peer, share, state::AppState};

#[derive(Debug, Deserialize)]
pub struct PullQuery {
    #[serde(default)]
    pub key: String,
}

/// `GET /api/share/pull?key=<share.key>`。
pub async fn pull(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(query): Query<PullQuery>,
    headers: HeaderMap,
) -> Response {
    let share_cfg = state.snapshot().share.clone();
    if !share_cfg.enabled {
        return (StatusCode::NOT_FOUND, "share disabled").into_response();
    }
    if share_cfg.key.is_empty() || query.key != share_cfg.key {
        warn!("【共享】分享请求被拒:密钥不匹配");
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    let ip = client_ip_with_peer(&headers, Some(peer));
    if !share::ip_allowed(&ip, &share_cfg.trust_ips) {
        warn!("【共享】分享请求被拒:源 IP 不在白名单 来源={ip}");
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    let payload = share::payload(&state, Duration::from_secs(share_cfg.poll_secs.max(5) * 2));
    Json(payload).into_response()
}
