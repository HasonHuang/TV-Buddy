//! 订阅聚合配置：读写 `[playlist]`。
//!
//! 聚合清单本身由 `GET /playlist.m3u` 提供；本模块只管理源与过滤规则。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::{
    config::model::PlaylistCfg,
    http::{error::ApiError, ext::PanelAuth},
    state::AppState,
};

/// `GET /panel/api/playlist`。
pub async fn get(PanelAuth(_): PanelAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!(state.snapshot().playlist))
}

/// `PUT /panel/api/playlist`——全量替换。
pub async fn update(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Json(mut cfg): Json<PlaylistCfg>,
) -> Result<Json<Value>, ApiError> {
    cfg.merge_ttl_secs = cfg.merge_ttl_secs.clamp(30, 86_400);
    if cfg.merge_position != "top" && cfg.merge_position != "bottom" {
        return Err(ApiError::bad_request("merge_position 只能是 top 或 bottom"));
    }
    for source in &mut cfg.sources {
        source.name = source.name.trim().to_string();
        source.url = source.url.trim().to_string();
        if source.id.trim().is_empty() {
            source.id = uuid::Uuid::new_v4().to_string();
        }
        if source.enabled
            && !(source.url.starts_with("http://") || source.url.starts_with("https://"))
        {
            return Err(ApiError::bad_request(format!(
                "源 {} 的 url 必须是 HTTP(S)",
                source.name
            )));
        }
    }
    state
        .mutate(|config| {
            config.playlist = cfg.clone();
            Ok(())
        })
        .map_err(|error| ApiError::internal(format!("{error:#}")))?;
    Ok(Json(json!({ "ok": true })))
}
