//! 旧路径改写：把已弃用的路径映射到新前缀。
//!
//! 本中间件在最外层运行。它只改写路径，保留查询串。
//! 新前缀不再改写，避免二次映射。

use axum::{
    http::{
        uri::{PathAndQuery, Uri},
        Method,
    },
    middleware::Next,
    response::Response,
    Router,
};

use crate::paths;

/// 订阅文件名的旧写法。
const SUBSCRIPTION_FILE: &str = "playlist.m3u";

/// 按 §8.2 的顺序改写旧路径。
///
/// 返回 `None` 表示不改写。新前缀与首页不改写。
pub fn rewrite(method: &Method, path: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    if let Some(rewritten) = rewrite_prefix(method, path) {
        return Some(rewritten);
    }
    if is_reserved(path) {
        return None;
    }
    rewrite_playback(path)
}

/// 中间件：改写请求路径，然后交给内层。
///
/// 本层必须在 IP 管控、功能开关与管理员检查之前运行。
pub async fn middleware(mut request: axum::extract::Request, next: Next) -> Response {
    if let Some(uri) = rewritten_uri(request.method(), request.uri()) {
        *request.uri_mut() = uri;
    }
    next.run(request).await
}

/// 用外层路由包住内层路由。
///
/// `Router::layer` 在路由匹配之后运行。旧路径改写必须先于匹配。
/// 因此本层放在 `fallback_service` 之外。
pub(crate) fn wrap(inner: Router) -> Router {
    Router::new()
        .fallback_service(inner)
        .layer(axum::middleware::from_fn(middleware))
}

/// 生成改写后的 URI。查询串原样保留。
fn rewritten_uri(method: &Method, uri: &Uri) -> Option<Uri> {
    let path = rewrite(method, uri.path())?;
    let mut parts = uri.clone().into_parts();
    let query = parts.path_and_query.as_ref().and_then(PathAndQuery::query);
    let path_and_query = match query {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    parts.path_and_query = Some(path_and_query.parse().ok()?);
    Uri::from_parts(parts).ok()
}

/// 前缀规则。按规格顺序匹配，命中即停。
fn rewrite_prefix(method: &Method, path: &str) -> Option<String> {
    let get = method == Method::GET || method == Method::HEAD;
    if get && path == "/health" {
        return Some(format!("{}/health", paths::API));
    }
    if let Some(rest) = segment_rest(path, "/panel/api") {
        return Some(join_prefix(paths::PANEL_API, rest));
    }
    if let Some(rest) = segment_rest(path, "/subs") {
        return Some(join_prefix(paths::PANEL_API, &format!("/subs{rest}")));
    }
    if let Some(rest) = segment_rest(path, "/epg/translate") {
        return Some(join_prefix(
            paths::PANEL_API,
            &format!("/epg/translate{rest}"),
        ));
    }
    if let Some(rest) = segment_rest(path, "/subtitle") {
        return Some(join_prefix(paths::PANEL_API, &format!("/subtitle{rest}")));
    }
    // v2026.60 曾用 `/api/sub`。现以 `/sub` 为准，旧前缀保留为别名。
    if let Some(rest) = segment_rest(path, "/api/sub") {
        return Some(join_prefix(paths::SUB_API, rest));
    }
    if let Some(rest) = segment_rest(path, "/l") {
        return Some(join_prefix(paths::PLAY_API, &format!("/token{rest}")));
    }
    if let Some(rest) = segment_rest(path, "/logo") {
        return Some(join_prefix(paths::STATIC, &format!("/logo{rest}")));
    }
    if let Some(rest) = segment_rest(path, "/__assets") {
        return Some(join_prefix(paths::STATIC, &format!("/panel{rest}")));
    }
    if let Some(rest) = segment_rest(path, "/assets") {
        return Some(join_prefix(paths::STATIC, &format!("/assets{rest}")));
    }
    if get && path == "/unban/whoami" {
        return Some(format!("{}/unban/whoami", paths::API));
    }
    if method == Method::POST && path == "/unban" {
        return Some(format!("{}/unban", paths::API));
    }
    if get && path == "/unban" {
        return Some(format!("{}/unban", paths::PAGE));
    }
    if get && (path == "/panel" || path == "/panel/") {
        return Some(format!("{}{path}", paths::PAGE));
    }
    None
}

/// 去前缀，按路径分段匹配。
///
/// `/sub` 不匹配 `/subs/status`。空后缀匹配前缀本身。
fn segment_rest<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    if rest.is_empty() || rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}

/// 拼前缀与后缀。后缀为空时只返回前缀。
fn join_prefix(prefix: &str, rest: &str) -> String {
    if rest.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}{rest}")
    }
}

/// 新前缀与首页不参与改写。
fn is_reserved(path: &str) -> bool {
    path == "/"
        || path == "/sub"
        || path.starts_with("/sub/")
        || path.starts_with("/api/")
        || path.starts_with("/media/")
        || path.starts_with("/static/")
        || path.starts_with("/page/")
}

/// 播放形状规则。按规格顺序匹配，命中即停。
fn rewrite_playback(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').skip(1).collect();
    match parts.as_slice() {
        // `/<gid>/playlist.m3u` 是固定指路，不改写。
        [gid, file] if !gid.is_empty() && *file == SUBSCRIPTION_FILE => None,
        // `/<gid>/<file>`：旧入口路径。入口扩展名白名单由处理器判定，
        // 非入口扩展名也要到达处理器并返回旧的占位 302。
        [gid, file] if !gid.is_empty() => Some(paths::play(&format!("/{gid}/{file}"))),
        // 签名切片：`/<gid>/<cid>/l/<exp>/<sig>/<rest>`。
        [gid, cid, "l", exp, sig, rest @ ..]
            if !gid.is_empty()
                && !cid.is_empty()
                && !exp.is_empty()
                && !sig.is_empty()
                && !rest.is_empty() =>
        {
            let rest = rest.join("/");
            let base = if paths::is_manifest_rest(&rest) {
                paths::PLAY_API
            } else {
                paths::MEDIA
            };
            Some(format!("{base}/{gid}/{cid}/l/{exp}/{sig}/{rest}"))
        }
        // 通用切片：`/<gid>/<cid>/<rest>`。
        [gid, cid, rest @ ..] if !gid.is_empty() && !cid.is_empty() && !rest.is_empty() => {
            let rest = rest.join("/");
            let base = if paths::is_manifest_rest(&rest) {
                paths::PLAY_API
            } else {
                paths::MEDIA
            };
            Some(format!("{base}/{gid}/{cid}/{rest}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Method;

    #[test]
    fn legacy_prefixes_are_rewritten() {
        let get = Method::GET;
        let post = Method::POST;
        assert_eq!(rewrite(&get, "/health").as_deref(), Some("/api/health"));
        assert_eq!(
            rewrite(&get, "/panel/api/status").as_deref(),
            Some("/api/panel/status")
        );
        assert_eq!(rewrite(&get, "/panel/api").as_deref(), Some("/api/panel"));
        assert_eq!(
            rewrite(&get, "/subs/status").as_deref(),
            Some("/api/panel/subs/status")
        );
        assert_eq!(
            rewrite(&get, "/epg/translate/status").as_deref(),
            Some("/api/panel/epg/translate/status")
        );
        assert_eq!(
            rewrite(&get, "/subtitle/list").as_deref(),
            Some("/api/panel/subtitle/list")
        );
        assert_eq!(
            rewrite(&get, "/api/sub/t/playlist.m3u").as_deref(),
            Some("/sub/t/playlist.m3u")
        );
        assert_eq!(
            rewrite(&get, "/api/sub/t/g/playlist.m3u").as_deref(),
            Some("/sub/t/g/playlist.m3u")
        );
        assert_eq!(
            rewrite(&get, "/l/t/g/c.m3u8").as_deref(),
            Some("/api/play/token/t/g/c.m3u8")
        );
        assert_eq!(
            rewrite(&get, "/logo/a.png").as_deref(),
            Some("/static/logo/a.png")
        );
        assert_eq!(
            rewrite(&get, "/assets/x/index.m3u8").as_deref(),
            Some("/static/assets/x/index.m3u8")
        );
        assert_eq!(
            rewrite(&get, "/__assets/x.js").as_deref(),
            Some("/static/panel/x.js")
        );
        assert_eq!(
            rewrite(&get, "/unban/whoami").as_deref(),
            Some("/api/unban/whoami")
        );
        assert_eq!(rewrite(&get, "/unban").as_deref(), Some("/page/unban"));
        assert_eq!(rewrite(&post, "/unban").as_deref(), Some("/api/unban"));
        assert_eq!(rewrite(&get, "/panel").as_deref(), Some("/page/panel"));
        assert_eq!(rewrite(&get, "/panel/").as_deref(), Some("/page/panel/"));
    }

    #[test]
    fn head_uses_the_same_rewrite_as_get() {
        let head = Method::HEAD;
        assert_eq!(rewrite(&head, "/health").as_deref(), Some("/api/health"));
        assert_eq!(rewrite(&head, "/panel").as_deref(), Some("/page/panel"));
        assert_eq!(rewrite(&head, "/unban").as_deref(), Some("/page/unban"));
        assert_eq!(
            rewrite(&head, "/unban/whoami").as_deref(),
            Some("/api/unban/whoami")
        );
        // POST 规则不受影响。
        let post = Method::POST;
        assert_eq!(rewrite(&post, "/health"), None);
        assert_eq!(rewrite(&post, "/unban").as_deref(), Some("/api/unban"));
    }

    #[test]
    fn playback_shapes_are_split_by_manifest_predicate() {
        let get = Method::GET;
        assert_eq!(
            rewrite(&get, "/g/c/index.m3u8").as_deref(),
            Some("/api/play/g/c/index.m3u8")
        );
        assert_eq!(
            rewrite(&get, "/g/c/l/1/sig/index.txt").as_deref(),
            Some("/media/g/c/l/1/sig/index.txt")
        );
        assert_eq!(
            rewrite(&get, "/g/c/l/1/sig").as_deref(),
            Some("/media/g/c/l/1/sig")
        );
        assert_eq!(
            rewrite(&get, "/g/c/l/1/sig/seg.png").as_deref(),
            Some("/media/g/c/l/1/sig/seg.png")
        );
        assert_eq!(
            rewrite(&get, "/g/c/seg.png").as_deref(),
            Some("/media/g/c/seg.png")
        );
        assert_eq!(
            rewrite(&get, "/g/c.m3u8").as_deref(),
            Some("/api/play/g/c.m3u8")
        );
        // 非入口扩展名也改写。扩展名白名单由处理器判定，旧响应保持不变。
        assert_eq!(
            rewrite(&get, "/g/c.mp4").as_deref(),
            Some("/api/play/g/c.mp4")
        );
        assert_eq!(
            rewrite(&get, "/g/c.ts").as_deref(),
            Some("/api/play/g/c.ts")
        );
        assert_eq!(
            rewrite(&get, "/g/c/video/index.txt").as_deref(),
            Some("/api/play/g/c/video/index.txt")
        );
        assert_eq!(
            rewrite(&get, "/g/c/dash-timeshift/v/playlist.m3u8").as_deref(),
            Some("/api/play/g/c/dash-timeshift/v/playlist.m3u8")
        );
        assert_eq!(
            rewrite(&get, "/g/c/dash-timeshift/v/12.m4s").as_deref(),
            Some("/media/g/c/dash-timeshift/v/12.m4s")
        );
    }

    #[test]
    fn new_prefixes_and_guidance_paths_are_not_rewritten() {
        let get = Method::GET;
        for path in [
            "/api/share/pull",
            "/sub/t/playlist.m3u",
            "/sub/t/g/playlist.m3u",
            "/media/g/c/seg.png",
            "/static/logo/a.png",
            "/page/panel",
            "/",
        ] {
            assert_eq!(rewrite(&get, path), None, "{path}");
        }
        assert_eq!(rewrite(&get, "/g/playlist.m3u"), None);
    }

    #[test]
    fn rewritten_uri_keeps_the_query() {
        let uri = "/g/c/index.m3u8?token=abc&x=1"
            .parse::<axum::http::Uri>()
            .unwrap();
        let rewritten = rewritten_uri(&Method::GET, &uri).unwrap();
        assert_eq!(rewritten.path(), "/api/play/g/c/index.m3u8");
        assert_eq!(rewritten.query(), Some("token=abc&x=1"));
        assert_eq!(
            rewritten_uri(
                &Method::GET,
                &"/api/health?x=1".parse::<axum::http::Uri>().unwrap()
            ),
            None
        );
    }

    #[tokio::test]
    async fn rewrite_runs_before_route_matching() {
        use axum::{
            body::Body,
            http::{Request as HttpRequest, StatusCode, Uri},
            routing::get,
            Router,
        };
        use tower::ServiceExt;

        async fn echo_uri(uri: Uri) -> String {
            uri.to_string()
        }

        let inner = Router::new().route("/api/health", get(echo_uri));
        // 外层包装：改写先于内层路由匹配。
        let app = wrap(inner);
        for (target, expected) in [
            ("/health", "/api/health"),
            ("/health?x=1", "/api/health?x=1"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri(target)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{target}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(&body[..], expected.as_bytes(), "{target}");
        }
    }
}
