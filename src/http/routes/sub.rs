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
    let local = playlist::render_user(&config, &user, &base, &token, Some(&state.health));
    let extra = aggregate_sources(&state, &config, &user).await;
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
            // 每个用户的清单都不同，CDN 不能共享。
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        body,
    )
        .into_response()
}

/// `GET /sub/:token/:gid/playlist.m3u`——单个分组的订阅。
///
/// 分组不存在、已归档或对该用户不可见时返回 `404`，与无效令牌不区分。
pub async fn group_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((token, gid)): Path<(String, String)>,
) -> Response {
    let Some(user) = resolve(&state, &token) else {
        return missing();
    };

    let config = state.snapshot();
    let base = base_url(&headers);
    let Some(body) =
        playlist::render_user_group(&config, &user, &base, &token, Some(&state.health), &gid)
    else {
        return missing();
    };
    // 文件名只保留安全字符，避免把路径段原样写进响应头。
    let filename: String = gid
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    (
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.apple.mpegurl; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{filename}.m3u\""),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
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
///
/// 去重只对照该用户可见的本地频道。用全部频道会让受限用户或归档组
/// 压掉自己看不到的聚合副本。
async fn aggregate_sources(
    state: &Arc<AppState>,
    config: &crate::config::Config,
    user: &User,
) -> String {
    let mut extra = String::new();
    // 只把该用户可见的本地地址放进去重集合。
    let mut seen: HashSet<String> = HashSet::new();
    for group in &config.groups {
        if group.access == "archived" || !user.can_see_group(&group.id) {
            continue;
        }
        for channel in &group.channels {
            if !user.can_see_channel(&group.id, &channel.id) {
                continue;
            }
            let url = channel.url.trim();
            if !url.is_empty() {
                seen.insert(url.to_string());
            }
        }
    }
    let statuses = state.sources.statuses();
    for source in
        config.playlist.sources.iter().filter(|source| {
            source.enabled && source.mode != "sync" && !source.url.trim().is_empty()
        })
    {
        let key = format!("sub:{}:{}", source.id, source.url);
        let ttl = std::time::Duration::from_secs(crate::subs::effective_ttl_secs(source.ttl_secs));
        let body = match state.sources.get_with_ttl(&key, ttl) {
            Some(body) => body,
            None => {
                // 失败冷却：每个订阅请求都重试慢源会堆积连接。
                let failed_recently = statuses.get(&source.id).is_some_and(|status| {
                    status.error.is_some() && now_secs().saturating_sub(status.last_try) < 60
                });
                if failed_recently {
                    continue;
                }
                match fetch_source(state, source).await {
                    Ok(text) => text,
                    Err(error) => {
                        tracing::warn!(url = %source.url, error = %error, "抓取聚合源失败");
                        state.sources.mark_error(&source.id, error);
                        continue;
                    }
                }
            }
        };
        let (piece, channels) =
            playlist::aggregate_seen(source, &config.playlist.global_exclude, &body, &mut seen);
        extra.push_str(&piece);
        state.sources.set_channels(&source.id, channels);
    }
    extra
}

/// 抓取单个聚合源；成功时写入缓存与状态。
///
/// 聚合源用固定短超时：订阅请求同步等待，不能套用托管源的长超时。
async fn fetch_source(state: &Arc<AppState>, source: &PlaylistSource) -> Result<String, String> {
    let text = crate::subscription::fetch_bounded(
        state,
        source,
        std::time::Duration::from_secs(crate::subscription::AGGREGATE_FETCH_TIMEOUT_SECS),
        crate::subscription::MAX_BODY_BYTES,
    )
    .await
    .map_err(|error| format!("{error:#}"))?;
    state
        .sources
        .put(&format!("sub:{}:{}", source.id, source.url), text.clone());
    state.sources.mark_success(&source.id);
    Ok(text)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
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
            Ok(bytes) => (
                [
                    (header::CONTENT_TYPE, "application/gzip"),
                    (header::CACHE_CONTROL, "private, no-store"),
                ],
                bytes,
            )
                .into_response(),
            Err(_) => missing(),
        }
    } else {
        (
            [
                (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
                (header::CACHE_CONTROL, "private, no-store"),
            ],
            xml,
        )
            .into_response()
    }
}
