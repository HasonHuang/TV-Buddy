//! 用户订阅入口：`/sub/:token/playlist.m3u`、`/sub/:token/epg.xml[.gz]`。
//!
//! 令牌来自 `access_tokens`。无效或过期一律 `404`，不区分原因，
//! 避免探测令牌是否存在。
//!
//! 清单与节目单都按用户的可见分组与频道过滤
//! （`docs/design/09-users-and-devices.md` §5）。

use std::{collections::HashSet, sync::Arc};

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{
    config::model::PlaylistSource, http::ext::base_url, playlist, state::AppState, users::User,
};

fn missing() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// 全局订阅已停用。保留路由是为了给出明确原因，而不是落进播放通配路由。
pub async fn gone() -> Response {
    (
        StatusCode::NOT_FOUND,
        "全局订阅已停用，请使用 /sub/<令牌>/playlist.m3u",
    )
        .into_response()
}

/// 令牌 → 用户。无效令牌或已删用户都返回 `None`。
fn resolve(state: &AppState, token: &str) -> Option<User> {
    let access = state.users.find_token(token)?;
    let user = state.users.find_user(&access.user_id)?;
    state.users.touch_token(token);
    Some(user)
}

/// `GET /sub/:token/playlist.m3u`。
pub async fn playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let Some(user) = resolve(&state, &token) else {
        return missing();
    };

    let config = state.snapshot();
    let base = base_url(&headers);
    let local = playlist::render_user(&config, &user, &base, &token);
    let extra = aggregate_sources(&state, &config).await;
    let body = playlist::compose(local, extra, &config.playlist.merge_position);
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

/// 抓取并合并聚合源（F-013 §5）。
///
/// 单个源失败只记日志；本地订阅照常返回。
/// 外部源条目不属于任何本地分组，无法映射到 `allowed_group_ids`，
/// 所以对所有用户一视同仁。
async fn aggregate_sources(state: &Arc<AppState>, config: &crate::config::Config) -> String {
    let mut extra = String::new();
    let mut seen = HashSet::new();
    for source in config
        .playlist
        .sources
        .iter()
        .filter(|source| source.enabled && !source.url.trim().is_empty())
    {
        let key = format!("sub:{}:{}", source.id, source.url);
        let ttl = std::time::Duration::from_secs(crate::subs::effective_ttl_secs(source.ttl_secs));
        let body = match state.sources.get_with_ttl(&key, ttl) {
            Some(body) => body,
            None => match fetch_source(state, source).await {
                Ok(text) => text,
                Err(error) => {
                    tracing::warn!(url = %source.url, error = %error, "抓取聚合源失败");
                    state.sources.mark_error(&source.id, error);
                    continue;
                }
            },
        };
        let (piece, channels) =
            playlist::aggregate_seen(source, &config.playlist.global_exclude, &body, &mut seen);
        extra.push_str(&piece);
        state.sources.set_channels(&source.id, channels);
    }
    extra
}

/// 抓取单个聚合源；成功时写入缓存与状态。
async fn fetch_source(state: &Arc<AppState>, source: &PlaylistSource) -> Result<String, String> {
    let response = state
        .http
        .get(&source.url)
        .send()
        .await
        .map_err(|error| format!("抓取失败: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("上游返回 {}", response.status()));
    }
    let text = response
        .text()
        .await
        .map_err(|error| format!("读取失败: {error}"))?;
    state
        .sources
        .put(&format!("sub:{}:{}", source.id, source.url), text.clone());
    state.sources.mark_success(&source.id);
    Ok(text)
}

/// `GET /sub/:token/epg.xml`。
pub async fn epg(State(state): State<Arc<AppState>>, Path(token): Path<String>) -> Response {
    user_epg(&state, &token, false)
}

/// `GET /sub/:token/epg.xml.gz`。
pub async fn epg_gz(State(state): State<Arc<AppState>>, Path(token): Path<String>) -> Response {
    user_epg(&state, &token, true)
}

/// 只含该用户可见频道的节目单。
///
/// `epg.xml` 的频道 id 是 `out_id`（小写 `tvg_id`，没有则小写 `channel.id`），
/// 不是 `gid/cid`。所以先按可见范围算出 `out_id` 集合，再过滤。
fn user_epg(state: &Arc<AppState>, token: &str, gzip: bool) -> Response {
    let Some(user) = resolve(state, token) else {
        return missing();
    };
    let config = state.snapshot();
    if !config.epg.enabled {
        return missing();
    }
    let Some(document) = crate::epg::load_generated_cached(state) else {
        return missing();
    };

    let allowed: HashSet<String> = config
        .groups
        .iter()
        .filter(|group| user.can_see_group(&group.id))
        .flat_map(|group| {
            group
                .channels
                .iter()
                .filter(|channel| user.can_see_channel(&group.id, &channel.id))
                .map(crate::epg::channel_out_id)
                .collect::<Vec<_>>()
        })
        .collect();

    let filtered = document.as_ref().clone().retain_channels(&allowed);
    let xml = crate::epg::render(&filtered.channels, &filtered.programmes);

    if gzip {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        if std::io::Write::write_all(&mut encoder, xml.as_bytes()).is_err() {
            return missing();
        }
        match encoder.finish() {
            Ok(bytes) => ([(header::CONTENT_TYPE, "application/gzip")], bytes).into_response(),
            Err(_) => missing(),
        }
    } else {
        (
            [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
            xml,
        )
            .into_response()
    }
}
