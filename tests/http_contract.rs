//! 路由级集成测试：起真实 HTTP 服务，用 `reqwest` 观测对外契约。
//!
//! 覆盖 `docs/design/03-http-contract.md` §3–§4 的关键形状。
//! 伪上游也由 axum 提供，避免外部依赖。

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{Path, Query},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use iptv_proxy::{config, epg, http, logging::RingBuffer, state::AppState};
use reqwest::{redirect::Policy, Client, Response as HttpResponse};

const LIVE_BODY: &str = "#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nseg2.ts\n#EXT-X-ENDLIST\n";
const LIVE: &str = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nseg2.ts\n#EXT-X-ENDLIST\n";
const MASTER: &str = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1920x1080\nv0/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nv1/index.m3u8\n";
const VARIANT: &str =
    "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nv1.ts\n#EXT-X-ENDLIST\n";
const SEGMENT: &[u8] = b"segment-bytes";

/// 伪上游：按路径返回清单或切片。
async fn upstream_file(
    Path(path): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    incoming: axum::http::HeaderMap,
) -> Response {
    // 回看与自定义头：回显成注释行，便于断言透传。
    if path == "live.m3u8" {
        let seek = query.get("playseek");
        let token = incoming
            .get("x-token")
            .and_then(|value| value.to_str().ok());
        if seek.is_some() || token.is_some() {
            let mut extra = String::new();
            if let Some(seek) = seek {
                extra.push_str(&format!("# playseek={seek}\n"));
            }
            if let Some(token) = token {
                extra.push_str(&format!("# x-token={token}\n"));
            }
            let body = format!("#EXTM3U\n{extra}{LIVE_BODY}");
            return (
                [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
                body,
            )
                .into_response();
        }
    }
    let (mime, body): (&str, &[u8]) = match path.as_str() {
        "live.m3u8" => ("application/vnd.apple.mpegurl", LIVE.as_bytes()),
        "master.m3u8" => ("application/vnd.apple.mpegurl", MASTER.as_bytes()),
        "v0/index.m3u8" => ("application/vnd.apple.mpegurl", VARIANT.as_bytes()),
        "seg1.ts" | "seg2.ts" | "v0/v1.ts" => ("video/mp2t", SEGMENT),
        _ => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    ([(header::CONTENT_TYPE, mime)], body.to_vec()).into_response()
}

/// 测试夹具：临时目录、伪上游、本实现服务、播放 Token。
struct Fixture {
    dir: PathBuf,
    base: String,
    upstream: String,
    token: String,
    client: Client,
    state: Arc<AppState>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定端口");
    let addr = listener.local_addr().expect("取端口");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn wait_ready(client: &Client, base: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(response) = client.get(format!("{base}/health")).send().await {
            if response.status() == StatusCode::OK {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("服务未就绪：{base}");
}

async fn fixture() -> Fixture {
    fixture_with(false).await
}

/// `epg_enabled` 为真时，伪上游额外提供 `/epg.xml`，配置里打开 EPG。
async fn fixture_with(epg_enabled: bool) -> Fixture {
    let dir = std::env::temp_dir().join(format!("iptv-proxy-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("建临时目录");

    let mut upstream_app = Router::new().route("/*path", get(upstream_file));
    if epg_enabled {
        let body = epg_document();
        upstream_app = upstream_app.route(
            "/epg.xml",
            get(move || {
                let body = body.clone();
                async move { ([(header::CONTENT_TYPE, "application/xml")], body) }
            }),
        );
    }
    let upstream = serve(upstream_app).await;

    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
log_level = "error"
not_found_url = "http://placeholder.test/404.mp4"
error_url = "http://placeholder.test/mistake.mp4"

[auth]
token_enabled = true
token_failure_url = "http://placeholder.test/Failure.mp4"

[cache]
backend = "disk"
disk_path = "{dir}/cache"
[cache.disguise]
enabled = true
formats = ["jpg", "png"]

[[groups]]
id = "pg"
name = "代理组"
mode = "proxy"
access = "public"
[[groups.channels]]
id = "c1"
name = "媒体清单频道"
url = "{upstream}/live.m3u8"
headers = ["X-Token: abc123"]

[[groups]]
id = "pm"
name = "主清单组"
mode = "proxy"
access = "public"
[[groups.channels]]
id = "c1"
name = "主清单频道"
url = "{upstream}/master.m3u8"

[[groups]]
id = "ar"
name = "归档组"
mode = "proxy"
access = "archived"
[[groups.channels]]
id = "c1"
name = "归档频道"
url = "{upstream}/live.m3u8"

[[groups]]
id = "op"
name = "开放组"
mode = "proxy"
access = "open"
[[groups.channels]]
id = "c1"
name = "开放频道"
url = "{upstream}/live.m3u8"

[[groups]]
id = "pt"
name = "透传组"
mode = "passthrough"
access = "public"
[[groups.channels]]
id = "c1"
name = "透传频道"
url = "{upstream}/live.m3u8"
{epg_section}
"#,
        dir = dir.display(),
        epg_section = if epg_enabled {
            format!(
                r#"
[epg]
enabled = true
keep_days = 3
future_days = 1
keep_yesterday = false
match_by_name = true

[[epg.sources]]
id = "s1"
name = "本地源"
kind = "xml"
url = "{upstream}/epg.xml"
"#
            )
        } else {
            String::new()
        }
    );
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, toml).expect("写配置");
    let cfg = config::load(&config_path).expect("加载配置");

    let state = Arc::new(AppState::new(
        config_path,
        cfg,
        Arc::new(RingBuffer::default()),
    ));
    let base = serve(http::router(state.clone())).await;

    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .expect("建客户端");
    wait_ready(&client, &base).await;

    let body = client
        .get(format!("{base}/playlist.m3u"))
        .send()
        .await
        .expect("取订阅")
        .text()
        .await
        .expect("读订阅");
    let token = body
        .split("token=")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_hexdigit()).next())
        .filter(|value| value.len() == 48)
        .expect("订阅应含 48 位 Token")
        .to_string();

    Fixture {
        dir,
        base,
        upstream,
        token,
        client,
        state,
    }
}

/// 生成带动态时间窗的 XMLTV：一条在窗口内，一条在 1990 年。
fn epg_document() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("取时间")
        .as_secs() as i64;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="src">
  <channel id="cctv1"><display-name lang="zh">媒体清单频道</display-name></channel>
  <channel id="unused"><display-name>未匹配频道</display-name></channel>
  <programme start="{start}" stop="{stop}" channel="cctv1">
    <title lang="zh">新闻</title>
    <desc>早间新闻</desc>
  </programme>
  <programme start="19900101000000 +0000" stop="19900101010000 +0000" channel="cctv1">
    <title>老节目</title>
  </programme>
  <programme start="{start}" stop="{stop}" channel="unused">
    <title>未匹配节目</title>
  </programme>
</tv>"#,
        start = xmltv_time(now - 1800),
        stop = xmltv_time(now + 1800),
    )
}

/// Unix 秒 → `YYYYMMDDHHMMSS +0000`。
fn xmltv_time(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}{:02}{:02}{:02} +0000",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// 1970-01-01 起的天数 → 公历。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

async fn fetch(fixture: &Fixture, path: &str) -> HttpResponse {
    fixture
        .client
        .get(format!("{}{}", fixture.base, path))
        .send()
        .await
        .expect("发请求")
}

fn header(response: &HttpResponse, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// 取正文里的非标签行。
fn media_lines(body: &str) -> Vec<&str> {
    body.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// 断言伪装切片地址形状：绝对地址 + `<16hex>_<32hex>.<ext>`。
fn assert_disguised(fixture: &Fixture, url: &str) {
    let prefix = format!(
        "{}/{}/c1/",
        fixture.base,
        if url.contains("/pm/") { "pm" } else { "pg" }
    );
    assert!(
        url.starts_with(&prefix),
        "应为 {prefix} 下的绝对地址：{url}"
    );
    let tail = url.rsplit('/').next().expect("取文件名");
    let (hash, ext) = tail.split_once('.').expect("应含扩展名");
    assert!(["jpg", "png"].contains(&ext), "扩展名应取自配置：{ext}");
    let (left, right) = hash.split_once('_').expect("应含下划线");
    assert_eq!(left.len(), 16, "前缀长度：{left}");
    assert!(
        left.chars().all(|c| c.is_ascii_hexdigit()),
        "前缀应为十六进制：{left}"
    );
    assert_eq!(right.len(), 32, "后缀长度：{right}");
    assert!(
        right.chars().all(|c| c.is_ascii_hexdigit()),
        "后缀应为十六进制：{right}"
    );
}

#[tokio::test]
async fn health_and_total_playlist() {
    let fixture = fixture().await;

    let response = fetch(&fixture, "/health").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.expect("读正文"), "ok");

    let response = fetch(&fixture, "/playlist.m3u").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(&response, "content-type"),
        "application/x-mpegurl; charset=utf-8"
    );
    assert_eq!(
        header(&response, "content-disposition"),
        "attachment; filename=\"playlist.m3u\""
    );
    let body = response.text().await.expect("读正文");
    assert!(body.starts_with("#EXTM3U"));
    // 归档组不导出；公开、开放、透传组导出。
    assert!(body.contains("/pg/c1.m3u8?token="));
    // 直通组在订阅里直接给上游地址。
    assert!(body.contains(&format!("{}/live.m3u8", fixture.upstream)));
    assert!(!body.contains("/pt/"));
    assert!(!body.contains("/ar/"));
}

#[tokio::test]
async fn media_upstream_entry_is_rewritten_media_playlist() {
    let fixture = fixture().await;
    let response = fetch(&fixture, &format!("/pg/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(&response, "content-type"),
        "text/plain; charset=utf-8"
    );
    assert_eq!(
        header(&response, "cache-control"),
        "public, max-age=4, s-maxage=4"
    );
    let body = response.text().await.expect("读正文");
    assert!(body.contains("#EXT-X-TARGETDURATION:4"));
    assert!(!body.contains("#EXT-X-STREAM-INF"));
    let segments = media_lines(&body);
    assert_eq!(segments.len(), 2);
    for url in segments {
        assert_disguised(&fixture, url);
    }
}

#[tokio::test]
async fn media_upstream_variant_path_redirects_to_error_url() {
    let fixture = fixture().await;
    let response = fetch(&fixture, "/pg/c1/video/index.txt").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        header(&response, "location"),
        "http://placeholder.test/mistake.mp4"
    );
}

#[tokio::test]
async fn master_upstream_entry_is_synthetic_master_playlist() {
    let fixture = fixture().await;
    let response = fetch(&fixture, &format!("/pm/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(&response, "content-type"),
        "application/vnd.apple.mpegurl"
    );
    assert_eq!(header(&response, "cache-control"), "no-cache, no-store");
    let body = response.text().await.expect("读正文");
    assert!(body.contains("#EXT-X-VERSION:6"));
    assert!(body.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
    assert!(body.contains("#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1920x1080"));
    assert!(body.contains(&format!("{}/pm/c1/video/index.txt", fixture.base)));
}

#[tokio::test]
async fn master_upstream_variant_playlist_and_index_alias() {
    let fixture = fixture().await;
    let response = fetch(&fixture, "/pm/c1/video/index.txt").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(&response, "content-type"),
        "text/plain; charset=utf-8"
    );
    assert_eq!(
        header(&response, "cache-control"),
        "public, max-age=6, s-maxage=6"
    );
    let body = response.text().await.expect("读正文");
    let segments = media_lines(&body);
    assert_eq!(segments.len(), 1);
    assert_disguised(&fixture, segments[0]);

    // `index.m3u8` 是入口别名。
    let response = fetch(
        &fixture,
        &format!("/pm/c1/index.m3u8?token={}", fixture.token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(&response, "content-type"),
        "application/vnd.apple.mpegurl"
    );
}

#[tokio::test]
async fn segment_served_from_cache_with_hit_headers() {
    let fixture = fixture().await;
    let response = fetch(&fixture, "/pm/c1/video/index.txt").await;
    let body = response.text().await.expect("读正文");
    let url = media_lines(&body)[0].to_string();
    let path = url
        .strip_prefix(&fixture.base)
        .expect("切片应为绝对地址")
        .to_string();

    let response = fetch(&fixture, &path).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "cache-control"), "public, max-age=300");
    assert!(header(&response, "content-type").starts_with("image/"));
    assert_eq!(response.bytes().await.expect("读字节").as_ref(), SEGMENT);

    let response = fetch(&fixture, &path).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "cache-control"), "private, max-age=300");
}

#[tokio::test]
async fn token_and_unknown_path_semantics() {
    let fixture = fixture().await;

    // 未知频道：无 Token → failure；合法 Token → not_found。
    let response = fetch(&fixture, "/pg/ghost.m3u8").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        header(&response, "location"),
        "http://placeholder.test/Failure.mp4"
    );
    let response = fetch(&fixture, &format!("/pg/ghost.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        header(&response, "location"),
        "http://placeholder.test/404.mp4"
    );

    // 未知分组同理。
    let response = fetch(&fixture, &format!("/ghost/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(
        header(&response, "location"),
        "http://placeholder.test/404.mp4"
    );

    // 其它未知路径不看 Token。
    let response = fetch(&fixture, "/nope/nope/nope").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        header(&response, "location"),
        "http://placeholder.test/404.mp4"
    );

    // 错 Token。
    let response = fetch(&fixture, "/pg/c1.m3u8?token=deadbeef").await;
    assert_eq!(
        header(&response, "location"),
        "http://placeholder.test/Failure.mp4"
    );

    // open 组免 Token。
    let response = fetch(&fixture, "/op/c1.m3u8").await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn archived_entry_redirects_to_not_found() {
    let fixture = fixture().await;
    let response = fetch(&fixture, &format!("/ar/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        header(&response, "location"),
        "http://placeholder.test/404.mp4"
    );

    let response = fetch(&fixture, "/ar/playlist.m3u").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn passthrough_requires_token() {
    let fixture = fixture().await;
    let response = fetch(&fixture, "/pt/c1.m3u8").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        header(&response, "location"),
        "http://placeholder.test/Failure.mp4"
    );

    let response = fetch(&fixture, &format!("/pt/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        header(&response, "location"),
        format!("{}/live.m3u8", fixture.upstream)
    );
}

#[tokio::test]
async fn unknown_panel_api_is_404() {
    let fixture = fixture().await;
    let response = fetch(&fixture, "/panel/api/nope").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn epg_disabled_returns_404() {
    let fixture = fixture().await;
    for path in ["/epg.xml", "/epg.xml.gz", "/epg"] {
        let response = fetch(&fixture, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn epg_enabled_serves_xmltv_and_filters() {
    let fixture = fixture_with(true).await;

    let summary = epg::refresh(&fixture.state).await.expect("采集应成功");
    assert_eq!(summary.channels, 1, "只保留匹配到的频道");
    assert_eq!(summary.kept, 1, "只保留时间窗内的节目");

    let response = fetch(&fixture, "/epg.xml").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(&response, "content-type"),
        "application/xml; charset=utf-8"
    );
    let body = response.text().await.expect("读正文");
    assert!(body.contains("generator-info-name=\"iptv-proxy\""));
    assert!(body.contains("<channel id=\"cctv1\">"));
    assert!(body.contains("<title lang=\"zh\">新闻</title>"));
    assert!(!body.contains("老节目"));
    assert!(!body.contains("未匹配节目"));

    let response = fetch(&fixture, "/epg.xml.gz").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "content-type"), "application/gzip");
    let bytes = response.bytes().await.expect("读字节");
    assert_eq!(&bytes[..2], &[0x1f, 0x8b], "应为 gzip 魔数");

    let response = fetch(&fixture, "/epg").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(header(&response, "content-type").starts_with("text/html"));

    // 面板接口需要会话。
    let response = fetch(&fixture, "/panel/api/epg/status").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn playseek_is_forwarded_to_upstream() {
    let fixture = fixture().await;
    let seek = "20261020080000-20261020090000";

    // 代理：上游收到 playseek，响应带注释行。
    let response = fetch(
        &fixture,
        &format!("/pg/c1.m3u8?token={}&playseek={seek}", fixture.token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("读正文");
    assert!(body.contains(&format!("# playseek={seek}")), "{body}");

    // 透传：跳转地址带 playseek。
    let response = fetch(
        &fixture,
        &format!("/pt/c1.m3u8?token={}&playseek={seek}", fixture.token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        header(&response, "location"),
        format!("{}/live.m3u8?playseek={seek}", fixture.upstream)
    );
}

#[tokio::test]
async fn single_segment_unknown_path_is_404() {
    let fixture = fixture().await;
    for path in ["/status", "/foo"] {
        let response = fetch(&fixture, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn epg_api_exposes_channels_programmes_and_search() {
    let fixture = fixture_with(true).await;
    epg::refresh(&fixture.state).await.expect("采集应成功");

    let response = fetch(&fixture, "/epg/api/channels").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(body["count"], 1);
    assert_eq!(body["channels"][0]["id"], "cctv1");

    let response = fetch(&fixture, "/epg/api/programmes/cctv1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(body["count"], 1);
    assert_eq!(body["programmes"][0]["title"], "新闻");

    let response = fetch(&fixture, "/epg/api/search?q=新闻").await;
    let body: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(body["count"], 1);

    let response = fetch(&fixture, "/epg/api/search?q=不存在").await;
    let body: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(body["count"], 0);

    let disabled = fixture_with(false).await;
    let response = fetch(&disabled, "/epg/api/channels").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// 用默认口令登录，返回会话 Token。
async fn login(fixture: &Fixture) -> String {
    let response = fixture
        .client
        .post(format!("{}/panel/api/login", fixture.base))
        .header("content-type", "application/json")
        .body(r#"{"username":"admin","password":"admin"}"#)
        .send()
        .await
        .expect("登录请求");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    body["token"].as_str().expect("应返回 token").to_string()
}

#[tokio::test]
async fn whoami_reports_forwarded_ip() {
    let fixture = fixture().await;

    let response = fixture
        .client
        .get(format!("{}/whoami", fixture.base))
        .header("x-forwarded-for", "1.2.3.4, 5.6.7.8")
        .send()
        .await
        .expect("请求 whoami");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(body["ip"], "1.2.3.4");

    let response = fetch(&fixture, "/unban/whoami").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(body["ip"], "unknown");
}

#[tokio::test]
async fn group_import_adds_skips_and_overwrites() {
    let fixture = fixture().await;
    let token = login(&fixture).await;
    let body = serde_json::json!([{
        "id": "imp",
        "name": "导入组",
        "mode": "proxy",
        "access": "public",
        "channels": [{
            "id": "c1",
            "name": "导入频道",
            "url": format!("{}/live.m3u8", fixture.upstream)
        }]
    }]);

    let post = |path: &str, body: &serde_json::Value| {
        fixture
            .client
            .post(format!("{}{path}", fixture.base))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(body.to_string())
            .send()
    };

    // 未登录 → 401。
    let response = fixture
        .client
        .post(format!("{}/panel/api/groups/import", fixture.base))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 首次导入。
    let response = post("/panel/api/groups/import", &body).await.expect("请求");
    assert_eq!(response.status(), StatusCode::OK);
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["imported"], 1);
    assert_eq!(result["skipped"], 0);

    let playlist = fetch(&fixture, "/playlist.m3u")
        .await
        .text()
        .await
        .expect("读正文");
    assert!(playlist.contains("/imp/c1.m3u8?token="));

    // 重复导入 → 跳过。
    let response = post("/panel/api/groups/import", &body).await.expect("请求");
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["imported"], 0);
    assert_eq!(result["skipped"], 1);

    // 强制导入 → 覆盖。
    let mut renamed = body.clone();
    renamed[0]["name"] = serde_json::json!("覆盖组");
    let response = post("/panel/api/groups/import/force", &renamed)
        .await
        .expect("请求");
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["imported"], 1);
    let playlist = fetch(&fixture, "/playlist.m3u")
        .await
        .text()
        .await
        .expect("读正文");
    assert!(playlist.contains("覆盖组"));

    // 非数组 → 400。
    let response = post("/panel/api/groups/import", &serde_json::json!({}))
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn m3u_import_creates_groups_and_channels() {
    let fixture = fixture().await;
    let token = login(&fixture).await;

    let post = |path: &str, body: &serde_json::Value| {
        fixture
            .client
            .post(format!("{}{path}", fixture.base))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(body.to_string())
            .send()
    };

    let text = "#EXTM3U\n\
        #EXTINF:-1 tvg-id=\"m1\" tvg-name=\"频道一\" group-title=\"导入A\",频道一\n\
        http://up/1.m3u8\n\
        #EXTINF:-1 group-title=\"导入A\",频道二\n\
        http://up/2.m3u8\n\
        #EXTINF:-1 tvg-id=\"d1\" group-title=\"导入D\",DASH\n\
        http://up/3.mpd\n";

    let body = serde_json::json!({ "text": text, "mode": "append", "group_mode": "auto" });
    let response = post("/panel/api/groups/import/m3u", &body)
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::OK);
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["parsed_groups"], 2);
    assert_eq!(result["parsed_channels"], 3);
    assert_eq!(result["added_groups"], 2);
    assert_eq!(result["skipped_groups"], 0);

    // 落库检查：auto 按扩展名判定模式，tvg-id 作为频道 id。
    let response = fixture
        .client
        .get(format!("{}/panel/api/config", fixture.base))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("取配置");
    let config: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    let groups = config["groups"].as_array().expect("分组数组");
    let group_a = groups
        .iter()
        .find(|group| group["id"] == "A")
        .expect("应有分组 A");
    assert_eq!(group_a["mode"], "rewrite");
    assert_eq!(group_a["format"], "hls");
    assert_eq!(group_a["channels"].as_array().expect("频道").len(), 2);
    assert_eq!(group_a["channels"][0]["id"], "m1");
    assert_eq!(group_a["channels"][0]["name"], "频道一");
    let group_d = groups
        .iter()
        .find(|group| group["id"] == "D")
        .expect("应有分组 D");
    assert_eq!(group_d["mode"], "dash");
    assert_eq!(group_d["format"], "mpd");

    // append 再导入 → 全部跳过。
    let response = post("/panel/api/groups/import/m3u", &body)
        .await
        .expect("请求");
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["skipped_groups"], 2);

    // merge 补一个新频道。
    let merge = serde_json::json!({
        "text": "#EXTM3U\n#EXTINF:-1 tvg-id=\"m3\" group-title=\"导入A\",频道三\nhttp://up/4.m3u8\n",
        "mode": "merge",
        "group_mode": "auto"
    });
    let response = post("/panel/api/groups/import/m3u", &merge)
        .await
        .expect("请求");
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["merged_channels"], 1);

    // replace 整体替换。
    let response = post("/panel/api/groups/import/m3u", &merge)
        .await
        .expect("请求");
    let _ = response;
    let replace = serde_json::json!({
        "text": "#EXTM3U\n#EXTINF:-1 tvg-id=\"m9\" group-title=\"导入A\",新频道\nhttp://up/9.m3u8\n",
        "mode": "replace",
        "group_mode": "auto"
    });
    let response = post("/panel/api/groups/import/m3u", &replace)
        .await
        .expect("请求");
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["replaced_groups"], 1);

    // 非法 mode → 400。
    let bad = serde_json::json!({ "text": text, "mode": "upsert" });
    let response = post("/panel/api/groups/import/m3u", &bad)
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn logo_library_upload_serve_and_delete() {
    let fixture = fixture().await;
    let token = login(&fixture).await;

    // PNG 魔数的 base64。
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let body = serde_json::json!({
        "items": [{ "name": "cctv1.png", "data": format!("data:image/png;base64,{png}") }]
    });
    let upload = |body: &serde_json::Value| {
        fixture
            .client
            .post(format!("{}/panel/api/logo/upload", fixture.base))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(body.to_string())
            .send()
    };

    // 未登录 → 401。
    let response = fixture
        .client
        .post(format!("{}/panel/api/logo/upload", fixture.base))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = upload(&body).await.expect("请求");
    assert_eq!(response.status(), StatusCode::OK);
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["added"][0]["file"], "cctv1.png");
    assert_eq!(result["failed"].as_array().expect("失败列表").len(), 0);

    // 公开列表。
    let response = fetch(&fixture, "/logo/api/list").await;
    assert_eq!(response.status(), StatusCode::OK);
    let list: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(list["count"], 1);
    assert_eq!(list["logos"][0]["url"], "/logo/cctv1.png");

    // 公开取图。
    let response = fetch(&fixture, "/logo/cctv1.png").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "content-type"), "image/png");
    assert_eq!(header(&response, "cache-control"), "public, max-age=86400");
    let bytes = response.bytes().await.expect("读字节");
    assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4e, 0x47], "应为 PNG 魔数");

    // 缺文件与越界名 → 404。
    for path in [
        "/logo/missing.png",
        "/logo/..%2F..%2Fconfig.toml",
        "/logo/evil.exe",
    ] {
        let response = fetch(&fixture, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }

    // 删除。
    let response = fixture
        .client
        .post(format!("{}/panel/api/logo/delete", fixture.base))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(serde_json::json!({ "file": "cctv1.png" }).to_string())
        .send()
        .await
        .expect("请求");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        fetch(&fixture, "/logo/cctv1.png").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn backup_and_restore_roundtrip() {
    let fixture = fixture().await;
    let token = login(&fixture).await;
    let auth = format!("Bearer {token}");

    // 备份。
    let response = fixture
        .client
        .get(format!("{}/panel/api/backup?include=all", fixture.base))
        .header("authorization", &auth)
        .send()
        .await
        .expect("备份请求");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "content-type"), "application/gzip");
    assert!(header(&response, "content-disposition").contains("iptv-proxy-backup-"));
    let data = response.bytes().await.expect("读字节");
    assert_eq!(&data[..2], &[0x1f, 0x8b], "应为 gzip 魔数");

    // 改动配置：导入一个分组。
    let import = serde_json::json!([{ "id": "tmp", "name": "临时组", "mode": "proxy" }]);
    let response = fixture
        .client
        .post(format!("{}/panel/api/groups/import", fixture.base))
        .header("content-type", "application/json")
        .header("authorization", &auth)
        .body(import.to_string())
        .send()
        .await
        .expect("导入请求");
    assert_eq!(response.status(), StatusCode::OK);
    let groups = |fixture: &Fixture| {
        let request = fixture
            .client
            .get(format!("{}/panel/api/config", fixture.base))
            .header("authorization", &auth);
        request.send()
    };
    let config: serde_json::Value = serde_json::from_str(
        &groups(&fixture)
            .await
            .expect("取配置")
            .text()
            .await
            .expect("读正文"),
    )
    .expect("解析 JSON");
    assert!(
        config["groups"]
            .as_array()
            .expect("分组")
            .iter()
            .any(|group| group["id"] == "tmp"),
        "导入后应有临时组"
    );

    // 恢复。
    let response = fixture
        .client
        .post(format!("{}/panel/api/restore", fixture.base))
        .header("authorization", &auth)
        .header("content-type", "application/gzip")
        .body(data)
        .send()
        .await
        .expect("恢复请求");
    assert_eq!(response.status(), StatusCode::OK);
    let result: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("读正文")).expect("解析 JSON");
    assert_eq!(result["ok"], true);
    assert_eq!(result["config_reloaded"], true);

    // 恢复后临时组消失，原分组仍在。
    let config: serde_json::Value = serde_json::from_str(
        &groups(&fixture)
            .await
            .expect("取配置")
            .text()
            .await
            .expect("读正文"),
    )
    .expect("解析 JSON");
    let ids: Vec<&str> = config["groups"]
        .as_array()
        .expect("分组")
        .iter()
        .filter_map(|group| group["id"].as_str())
        .collect();
    assert!(!ids.contains(&"tmp"), "恢复后不应有临时组：{ids:?}");
    assert!(ids.contains(&"pg"), "恢复后应保留原分组：{ids:?}");

    // 非法内容 → 400。
    let response = fixture
        .client
        .post(format!("{}/panel/api/restore", fixture.base))
        .header("authorization", &auth)
        .header("content-type", "application/gzip")
        .body("not gzip")
        .send()
        .await
        .expect("恢复请求");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn channel_headers_are_sent_upstream() {
    let fixture = fixture().await;
    let response = fetch(&fixture, &format!("/pg/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("读正文");
    assert!(body.contains("# x-token=abc123"), "{body}");
}
