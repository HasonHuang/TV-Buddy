//! CDN 侧端点：「立即估算」。
//!
//! 被动实测（`docs/design/06-low-spec-optimization.md` §3.2）在首次播放前没有数据，
//! 装完想立刻拿到推荐 TTL 时点一次本接口：取前 3 个非归档分组各第 1 个频道，
//! 抓媒体清单与首个切片的 `Content-Length`，喂给统计后由被动数据接管。
//!
//! 复用 `routes::stream` 的抓取函数（已提为 `pub(crate)`），不另起一套客户端。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{
    http::{error::ApiError, ext::PanelAuth, routes::stream},
    proxy,
    state::AppState,
};

/// 一次估算最多探测的分组数。
const MAX_GROUPS: usize = 3;

/// 串行化：同一时刻只允许一次估算在跑。
static ESTIMATE_LOCK: Mutex<()> = Mutex::const_new(());

/// `POST /panel/api/cdn/estimate`。
///
/// 全部失败时仍返回 `200`（`recommend.source` 回落 `fallback`）；并发请求返回 `409`。
pub async fn estimate(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let Ok(_guard) = ESTIMATE_LOCK.try_lock() else {
        return Err(ApiError::conflict("已有估算正在进行，请稍后再试"));
    };

    let config = state.snapshot();
    let targets: Vec<(String, String, String)> = config
        .groups
        .iter()
        .filter(|group| group.access != "archived")
        .filter_map(|group| {
            group
                .channels
                .first()
                .map(|channel| (group.id.clone(), channel.id.clone(), channel.url.clone()))
        })
        .take(MAX_GROUPS)
        .collect();

    let probes = targets
        .iter()
        .map(|(gid, cid, url)| probe(&state, gid, cid, url));
    let results = futures_util::future::join_all(probes).await;

    let mut probed = 0usize;
    let mut failed: Vec<Value> = Vec::new();
    for ((gid, cid, _), result) in targets.iter().zip(results) {
        match result {
            Ok(()) => probed += 1,
            Err(error) => {
                tracing::debug!(gid = %gid, cid = %cid, "估算探测失败: {error:#}");
                failed.push(json!({ "gid": gid, "cid": cid, "error": format!("{error:#}") }));
            }
        }
    }
    if probed > 0 {
        // 一次性估算的结果立刻落盘，避免重启后丢失。
        state.estimate.persist(true);
    }
    tracing::info!(
        probed,
        failed = failed.len(),
        "估算完成（分组 {probed}/{})",
        targets.len()
    );

    Ok(Json(json!({
        "ok": true,
        "groups": probed,
        "failed": failed,
        "recommend": crate::cache_gc::recommendation(&state),
    })))
}

/// 探测一个频道：清单（必要时跟随主清单）→ 窗口统计 → 首个切片长度。
async fn probe(state: &Arc<AppState>, gid: &str, cid: &str, url: &str) -> anyhow::Result<()> {
    if url.trim().is_empty() {
        anyhow::bail!("频道没有上游地址");
    }
    let (playlist, base) = fetch_media_playlist(state, gid, cid, url).await?;
    if proxy::media::window_stats(&playlist).is_none() {
        anyhow::bail!("清单里没有 #EXTINF");
    }
    state.estimate.probe_playlist(gid, &playlist);
    // 首个切片只读响应头，不读正文。
    if let Some(first) = first_segment(&playlist, &base) {
        if let Some(len) = head_length(state, gid, cid, &first).await {
            state.estimate.probe_segment(gid, len);
        }
    }
    Ok(())
}

/// 抓媒体清单；上游是主清单时跟随 `BANDWIDTH` 最大的变体，返回 `(正文, 基址)`。
async fn fetch_media_playlist(
    state: &Arc<AppState>,
    gid: &str,
    cid: &str,
    url: &str,
) -> anyhow::Result<(String, String)> {
    let text = stream::fetch_playlist(state, url, gid, cid).await?;
    if !proxy::media::is_master(&text) {
        return Ok((text, url.to_string()));
    }
    let Some((_, variant)) = proxy::media::best_variant(&text, url) else {
        anyhow::bail!("主清单里没有可用变体");
    };
    let media = stream::fetch_playlist(state, &variant, gid, cid).await?;
    Ok((media, variant))
}

/// 媒体清单里第一个切片地址（跳过标签与空行）。
fn first_segment(body: &str, base: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Ok(url) = url::Url::parse(base).and_then(|base| base.join(trimmed)) {
            return Some(url.to_string());
        }
    }
    None
}

/// 只读响应头拿切片长度；缺失或失败返回 `None`。
async fn head_length(state: &Arc<AppState>, gid: &str, cid: &str, url: &str) -> Option<u64> {
    let mut request = state.http_for(gid, cid).get(url);
    for (name, value) in stream::channel_headers(state, gid, cid) {
        request = request.header(name, value);
    }
    let response = request.send().await.ok()?.error_for_status().ok()?;
    response.content_length()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_segment_skips_tags_and_resolves_relative() {
        let body = "#EXTM3U\n#EXT-X-TARGETDURATION:8\n#EXTINF:8.0,\nseg/1.ts\n";
        assert_eq!(
            first_segment(body, "http://up.example/live/index.m3u8").as_deref(),
            Some("http://up.example/live/seg/1.ts")
        );
        assert!(first_segment("#EXTM3U\n#EXTINF:8.0,\n", "http://up.example/a.m3u8").is_none());
        assert_eq!(
            first_segment(
                "#EXTM3U\n#EXTINF:8.0,\nhttp://cdn.example/x.jpg?sig=1\n",
                "http://up.example/a.m3u8"
            )
            .as_deref(),
            Some("http://cdn.example/x.jpg?sig=1")
        );
    }
}
