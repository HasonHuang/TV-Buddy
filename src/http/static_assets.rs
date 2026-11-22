//! 内嵌前端资源的读取与响应。

use axum::{
    body::Body,
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

/// 编译期嵌入 `frontend/dist`。前端未构建时由 `build.rs` 写占位页。
#[derive(RustEmbed)]
#[folder = "frontend/dist"]
struct Assets;

fn asset_response(name: &str, cache_control: &'static str) -> Option<Response> {
    let file = Assets::get(name)?;
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let mut response = Response::new(Body::from(file.data.into_owned()));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, mime.to_string().parse().ok()?);
    // 页面不缓存；播放面的缓存窗口在别的层实现（S-DIS-06）。
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(cache_control),
    );
    Some(response)
}

fn page(name: &str) -> Response {
    asset_response(name, "no-cache").unwrap_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("缺少前端资源 {name}。请先构建前端：scripts/build-frontend.sh"),
        )
            .into_response()
    })
}

pub async fn panel_page() -> Response {
    page("panel.html")
}

pub async fn unban_page() -> Response {
    page("unban.html")
}

/// `/static/panel/*path`（旧 `/__assets/*path`）——Vite 构建产物。
///
/// 文件名带内容哈希，可以长缓存。
pub async fn asset(Path(path): Path<String>) -> Response {
    asset_response(&path, "public, max-age=31536000, immutable")
        .unwrap_or_else(|| (StatusCode::NOT_FOUND, "not found").into_response())
}
