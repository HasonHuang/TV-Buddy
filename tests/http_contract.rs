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
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use iptv_proxy::{config, http, logging::RingBuffer, state::AppState};
use reqwest::{redirect::Policy, Client, Response as HttpResponse};

const LIVE: &str = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\nseg2.ts\n#EXT-X-ENDLIST\n";
const MASTER: &str = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1920x1080\nv0/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nv1/index.m3u8\n";
const VARIANT: &str = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nv1.ts\n#EXT-X-ENDLIST\n";
const SEGMENT: &[u8] = b"segment-bytes";

/// 伪上游：按路径返回清单或切片。
async fn upstream_file(Path(path): Path<String>) -> Response {
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
    let dir = std::env::temp_dir().join(format!("iptv-proxy-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("建临时目录");

    let upstream = serve(Router::new().route("/*path", get(upstream_file))).await;

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
"#,
        dir = dir.display()
    );
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, toml).expect("写配置");
    let cfg = config::load(&config_path).expect("加载配置");

    let state = Arc::new(AppState::new(
        config_path,
        cfg,
        Arc::new(RingBuffer::default()),
    ));
    let base = serve(http::router(state)).await;

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
    }
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
    let prefix = format!("{}/{}/c1/", fixture.base, if url.contains("/pm/") { "pm" } else { "pg" });
    assert!(url.starts_with(&prefix), "应为 {prefix} 下的绝对地址：{url}");
    let tail = url.rsplit('/').next().expect("取文件名");
    let (hash, ext) = tail.split_once('.').expect("应含扩展名");
    assert!(["jpg", "png"].contains(&ext), "扩展名应取自配置：{ext}");
    let (left, right) = hash.split_once('_').expect("应含下划线");
    assert_eq!(left.len(), 16, "前缀长度：{left}");
    assert!(left.chars().all(|c| c.is_ascii_hexdigit()), "前缀应为十六进制：{left}");
    assert_eq!(right.len(), 32, "后缀长度：{right}");
    assert!(right.chars().all(|c| c.is_ascii_hexdigit()), "后缀应为十六进制：{right}");
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
    assert_eq!(header(&response, "content-type"), "text/plain; charset=utf-8");
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
    assert!(body.contains(&format!(
        "{}/pm/c1/video/index.txt",
        fixture.base
    )));
}

#[tokio::test]
async fn master_upstream_variant_playlist_and_index_alias() {
    let fixture = fixture().await;
    let response = fetch(&fixture, "/pm/c1/video/index.txt").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "content-type"), "text/plain; charset=utf-8");
    assert_eq!(
        header(&response, "cache-control"),
        "public, max-age=6, s-maxage=6"
    );
    let body = response.text().await.expect("读正文");
    let segments = media_lines(&body);
    assert_eq!(segments.len(), 1);
    assert_disguised(&fixture, segments[0]);

    // `index.m3u8` 是入口别名。
    let response = fetch(&fixture, &format!("/pm/c1/index.m3u8?token={}", fixture.token)).await;
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
    assert_eq!(header(&response, "location"), "http://placeholder.test/Failure.mp4");
    let response = fetch(&fixture, &format!("/pg/ghost.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(header(&response, "location"), "http://placeholder.test/404.mp4");

    // 未知分组同理。
    let response = fetch(&fixture, &format!("/ghost/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(header(&response, "location"), "http://placeholder.test/404.mp4");

    // 其它未知路径不看 Token。
    let response = fetch(&fixture, "/nope/nope/nope").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(header(&response, "location"), "http://placeholder.test/404.mp4");

    // 错 Token。
    let response = fetch(&fixture, "/pg/c1.m3u8?token=deadbeef").await;
    assert_eq!(header(&response, "location"), "http://placeholder.test/Failure.mp4");

    // open 组免 Token。
    let response = fetch(&fixture, "/op/c1.m3u8").await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn archived_entry_redirects_to_not_found() {
    let fixture = fixture().await;
    let response = fetch(&fixture, &format!("/ar/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(header(&response, "location"), "http://placeholder.test/404.mp4");

    let response = fetch(&fixture, "/ar/playlist.m3u").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn passthrough_requires_token() {
    let fixture = fixture().await;
    let response = fetch(&fixture, "/pt/c1.m3u8").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(header(&response, "location"), "http://placeholder.test/Failure.mp4");

    let response = fetch(&fixture, &format!("/pt/c1.m3u8?token={}", fixture.token)).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(header(&response, "location"), format!("{}/live.m3u8", fixture.upstream));
}

#[tokio::test]
async fn unknown_panel_api_is_404() {
    let fixture = fixture().await;
    let response = fetch(&fixture, "/panel/api/nope").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
