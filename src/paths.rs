//! 路径前缀与分类工具。
//!
//! 本模块统一生成新前缀 URL。新前缀为 `/api`、`/api/panel`、`/sub`、
//! `/api/play`、`/media`、`/static`、`/page`。本模块也识别清单路径，并归一化
//! 已持久化的旧素材路径。

/// API 根前缀。
pub const API: &str = "/api";
/// 面板 API 前缀。
pub const PANEL_API: &str = "/api/panel";
/// 订阅前缀。它面向播放器，保持在 `/api` 之外，地址稳定。
pub const SUB_API: &str = "/sub";
/// 播放 API 前缀。
pub const PLAY_API: &str = "/api/play";
/// 媒体数据前缀。本前缀只提供切片与初始化段。
pub const MEDIA: &str = "/media";
/// 静态资源前缀。
pub const STATIC: &str = "/static";
/// 页面前缀。
pub const PAGE: &str = "/page";

/// 在 `prefix` 后拼接 `path`。
///
/// `path` 的全部前导 `/` 先删除。空 `path` 返回 `prefix` 本身。
fn join(prefix: &str, path: &str) -> String {
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return prefix.to_string();
    }
    format!("{prefix}/{path}")
}

/// 生成面板 API 路径。
pub fn panel(path: &str) -> String {
    join(PANEL_API, path)
}

/// 生成订阅 API 路径。
pub fn sub(path: &str) -> String {
    join(SUB_API, path)
}

/// 生成播放 API 路径。
pub fn play(path: &str) -> String {
    join(PLAY_API, path)
}

/// 生成媒体数据路径。
pub fn media(path: &str) -> String {
    join(MEDIA, path)
}

/// 生成静态资源路径。
pub fn static_path(path: &str) -> String {
    join(STATIC, path)
}

/// 生成页面路径。
pub fn page(path: &str) -> String {
    join(PAGE, path)
}

/// 拼接完整 URL。
///
/// `self_base` 末尾的 `/` 先删除。`slice` 与 `rest` 不带前导 `/`。
/// `slice` 为空时不产生双斜杠。
fn endpoint_url(
    prefix: &str,
    self_base: &str,
    gid: &str,
    cid: &str,
    slice: &str,
    rest: &str,
) -> String {
    let base = self_base.trim_end_matches('/');
    let slice = slice.trim_matches('/');
    let rest = rest.trim_start_matches('/');
    let mut path = String::from(prefix);
    path.push('/');
    path.push_str(gid);
    path.push('/');
    path.push_str(cid);
    if !slice.is_empty() {
        path.push('/');
        path.push_str(slice);
    }
    if !rest.is_empty() {
        path.push('/');
        path.push_str(rest);
    }
    format!("{base}{path}")
}

/// 生成播放清单 URL：`/api/play/<gid>/<cid>[/<slice>]/<rest>`。
pub fn play_url(self_base: &str, gid: &str, cid: &str, slice: &str, rest: &str) -> String {
    endpoint_url(PLAY_API, self_base, gid, cid, slice, rest)
}

/// 生成媒体数据 URL：`/media/<gid>/<cid>[/<slice>]/<rest>`。
pub fn media_url(self_base: &str, gid: &str, cid: &str, slice: &str, rest: &str) -> String {
    endpoint_url(MEDIA, self_base, gid, cid, slice, rest)
}

/// 判断 `rest` 是否为清单形状。
///
/// 真形状：`index.m3u8`、`<track>/index.txt`、`<track>/index.m3u8`、
/// `dash-timeshift/<track>/playlist[.txt|.m3u8]`。其余形状为媒体数据。
/// 本函数零分配。
pub fn is_manifest_rest(rest: &str) -> bool {
    let mut parts = rest.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("index.m3u8"), None, None, None) => true,
        (Some(track), Some(file), None, None) => {
            !track.is_empty() && matches!(file, "index.txt" | "index.m3u8")
        }
        (Some("dash-timeshift"), Some(track), Some(file), None) => {
            !track.is_empty() && matches!(file, "playlist" | "playlist.txt" | "playlist.m3u8")
        }
        _ => false,
    }
}

/// 归一化素材地址。
///
/// 本地 `/logo/*` 映射到 `/static/logo/*`。本地 `/assets/*` 映射到
/// `/static/assets/*`。其他值原样返回。
pub fn normalize_asset_url(path: &str) -> String {
    if path.starts_with("/logo/") || path.starts_with("/assets/") {
        format!("{STATIC}{path}")
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_rest_is_classified() {
        for rest in [
            "index.m3u8",
            "video/index.txt",
            "video_720p/index.txt",
            "video_720p/index.m3u8",
            "dash-timeshift/video_720p/playlist",
            "dash-timeshift/video_720p/playlist.txt",
            "dash-timeshift/video_720p/playlist.m3u8",
        ] {
            assert!(is_manifest_rest(rest), "{rest}");
        }
        for rest in [
            "",
            "/index.m3u8",
            "index.txt",
            "timeshift/a.ts",
            "mpd/tok/abc/init.mp4",
            "seg_000000000000.png",
            "video_720p/init.webp",
            "dash-timeshift/video_720p/12.m4s",
            "dash-timeshift/video_720p",
            "dash-timeshift//playlist",
            "a/b/c",
            "a/b/c/d",
            "video/index.txt/more",
        ] {
            assert!(!is_manifest_rest(rest), "{rest}");
        }
    }

    #[test]
    fn join_trims_all_leading_slashes() {
        assert_eq!(join("/api/panel", "//x"), "/api/panel/x");
        assert_eq!(join("/api/panel", "/x"), "/api/panel/x");
        assert_eq!(join("/api/panel", "x"), "/api/panel/x");
        assert_eq!(join("/api/panel", ""), "/api/panel");
        assert_eq!(join("/api/panel", "///"), "/api/panel");
    }

    #[test]
    fn asset_urls_are_normalized() {
        assert_eq!(normalize_asset_url("/logo/a.png"), "/static/logo/a.png");
        assert_eq!(
            normalize_asset_url("/assets/convert/x/index.m3u8"),
            "/static/assets/convert/x/index.m3u8"
        );
        assert_eq!(
            normalize_asset_url("/static/logo/a.png"),
            "/static/logo/a.png"
        );
        assert_eq!(normalize_asset_url("https://up/a.png"), "https://up/a.png");
    }

    #[test]
    fn subscription_urls_stay_outside_api() {
        assert_eq!(sub("/t/playlist.m3u"), "/sub/t/playlist.m3u");
        assert_eq!(sub("t/epg.xml"), "/sub/t/epg.xml");
        assert_eq!(sub(""), "/sub");
    }

    #[test]
    fn play_and_media_urls_use_distinct_prefixes() {
        assert_eq!(
            play_url("http://h", "g", "c", "l/1/sig", "index.m3u8"),
            "http://h/api/play/g/c/l/1/sig/index.m3u8"
        );
        assert_eq!(
            media_url("http://h", "g", "c", "l/1/sig", "seg.png"),
            "http://h/media/g/c/l/1/sig/seg.png"
        );
        assert_eq!(
            media_url("http://h/", "g", "c", "", "seg.png"),
            "http://h/media/g/c/seg.png"
        );
        assert_eq!(
            play_url("", "g", "c", "", "index.m3u8"),
            "/api/play/g/c/index.m3u8"
        );
    }
}
