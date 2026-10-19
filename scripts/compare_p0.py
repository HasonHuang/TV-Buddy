#!/usr/bin/env python3
"""P0 对拍：本实现 ↔ 目标 v0.8.2。

对比**可观测契约**：状态码、响应头、正文形状。
不比较 token 值、伪装前缀/后缀值、缓存文件名等内部值。

用法：
    TARGET_BASE=http://127.0.0.1:19899 OUR_BASE=http://127.0.0.1:19898 \
        python3 scripts/compare_p0.py
"""

import hashlib
import os
import re
import sys
import urllib.error
import urllib.request

TARGET = os.environ.get("TARGET_BASE", "http://127.0.0.1:19899")
OURS = os.environ.get("OUR_BASE", "http://127.0.0.1:19898")

SEG_RE = re.compile(r"^https?://[^/]+/([^/]+)/([^/]+)/([0-9a-f]{16})_([0-9a-f]{32})\.([a-z0-9]+)$")
ENTRY_RE = re.compile(r"^http://[^/]+/([^/]+)/([^/?]+)\.(\w+)(\?token=([0-9a-f]{48}))?$")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """不要跟随跳转：302 本身就是要对比的结果。"""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def get(base, path, headers=None):
    request = urllib.request.Request(base + path, headers=headers or {})
    try:
        with OPENER.open(request, timeout=20) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers), error.read()


def header(headers, name):
    for key, value in headers.items():
        if key.lower() == name.lower():
            return value
    return ""


def lines_of(body):
    return body.decode("utf-8", "replace").splitlines()


def tags_of(body):
    return [line for line in lines_of(body) if line.startswith("#")]


def tail(value):
    return value.rstrip("/").split("/")[-1] if value else ""


def token_of(base):
    _, _, body = get(base, "/playlist.m3u")
    found = re.search(rb"token=([0-9a-f]{48})", body)
    return found.group(1).decode() if found else ""


def playlist_shape(body):
    entries = []
    for line in lines_of(body):
        matched = ENTRY_RE.match(line)
        if matched:
            entries.append(
                (matched.group(1), matched.group(2), matched.group(3), bool(matched.group(5)))
            )
    return entries


def slice_shape(body):
    """切片形状：全部匹配伪装路径 + 条数。不比较前缀与后缀的值。"""
    candidates = [line for line in lines_of(body) if line and not line.startswith("#")]
    matched = [SEG_RE.match(line) for line in candidates]
    return (all(matched), len(matched))


def segment_path(base, token=""):
    """取媒体清单首条切片路径（相对本服务）。"""
    _, _, body = get(base, "/pm/c1/video/index.txt" + (f"?token={token}" if token else ""))
    for line in lines_of(body):
        matched = SEG_RE.match(line)
        if matched:
            return "/" + "/".join(line.split("/")[3:])
    return None


# --- 用例：接受 `(base, token)`，返回可比较的值 -------------------------------------


def c_health(base, _token):
    status, _, body = get(base, "/health")
    return status, body.decode().strip()


def c_total_playlist(base, _token):
    status, headers, body = get(base, "/playlist.m3u")
    return (
        status,
        header(headers, "content-type"),
        header(headers, "content-disposition"),
        playlist_shape(body),
    )


def c_total_tags(base, _token):
    _, _, body = get(base, "/playlist.m3u")
    tags = tags_of(body)
    return tags[:1], [line for line in tags[1:] if not line.startswith("#EXTINF")]


def c_group_playlists(base, _token):
    result = []
    for gid in ["pg", "hd", "op", "ar", "ghost"]:
        status, headers, body = get(base, f"/{gid}/playlist.m3u")
        result.append((gid, status, header(headers, "content-type"), len(playlist_shape(body))))
    return result


def c_entry_media(base, token):
    """上游是媒体清单：入口直接下发媒体清单。"""
    status, headers, body = get(base, f"/pg/c1.m3u8?token={token}")
    return (
        status,
        header(headers, "content-type"),
        header(headers, "cache-control"),
        tags_of(body),
        slice_shape(body),
    )


def c_entry_master(base, token):
    """上游是主清单：入口自造单变体主清单。"""
    status, headers, body = get(base, f"/pm/c1.m3u8?token={token}")
    return (
        status,
        header(headers, "content-type"),
        header(headers, "cache-control"),
        tags_of(body),
        slice_shape(body),
    )


def c_index_media(base, token):
    """上游是媒体清单时，变体清单路径不存在。"""
    status, headers, _ = get(base, f"/pg/c1/video/index.txt?token={token}")
    return status, tail(header(headers, "location"))


def c_index_master(base, token):
    """上游是主清单时，变体清单存在。"""
    status, headers, body = get(base, f"/pm/c1/video/index.txt?token={token}")
    return (
        status,
        header(headers, "content-type"),
        header(headers, "cache-control"),
        tags_of(body),
        slice_shape(body),
    )


def c_index_alias(base, token):
    """`/{gid}/{cid}/index.m3u8` 与入口同义。"""
    status, headers, body = get(base, f"/pg/c1/index.m3u8?token={token}")
    return status, header(headers, "content-type"), len(tags_of(body))


def c_segment_cold(base, token):
    """伪装扩展名取值不比对（轮换算法属内部实现），只比对 MIME 大类。"""
    path = segment_path(base, token)
    status, headers, _ = get(base, path)
    mime = header(headers, "content-type")
    return status, mime.split("/")[0], header(headers, "cache-control")


def c_segment_hit(base, token):
    path = segment_path(base, token)
    get(base, path)
    status, headers, _ = get(base, path)
    return status, header(headers, "cache-control")


def c_segment_bytes(base, token):
    """切片字节应等于上游文件。"""
    path = segment_path(base, token)
    _, _, body = get(base, path)
    return hashlib.sha256(body).hexdigest()[:16]


def c_segment_range(base, token):
    path = segment_path(base, token)
    status, headers, body = get(base, path, {"Range": "bytes=0-9"})
    return status, header(headers, "content-length") == str(len(body))


def c_unknown_with_token(base, token):
    result = []
    for path in ["/pg/ghost.m3u8", "/ghost/c1.m3u8"]:
        status, headers, _ = get(base, f"{path}?token={token}")
        result.append((path, status, tail(header(headers, "location"))))
    return result


def c_unknown_without_token(base, _token):
    result = []
    for path in ["/pg/ghost.m3u8", "/ghost/c1.m3u8"]:
        status, headers, _ = get(base, path)
        result.append((path, status, tail(header(headers, "location"))))
    return result


def c_archived(base, token):
    status, headers, _ = get(base, f"/ar/c1.m3u8?token={token}")
    return status, tail(header(headers, "location"))


def c_token_missing(base, _token):
    status, headers, _ = get(base, "/pg/c1.m3u8")
    bad_status, bad_headers, _ = get(base, "/pg/c1.m3u8?token=deadbeef")
    return (
        status,
        tail(header(headers, "location")),
        bad_status,
        tail(header(bad_headers, "location")),
    )


def c_token_open(base, token):
    without = get(base, "/op/c1.m3u8")[0]
    wrong = get(base, "/op/c1.m3u8?token=deadbeef")[0]
    with_token = get(base, f"/op/c1.m3u8?token={token}")[0]
    return without, wrong, with_token


def c_redirect(base, token):
    status, headers, _ = get(base, f"/rg/c1.m3u8?token={token}")
    return status, tail(header(headers, "location"))


def c_rewrite(base, token):
    status, headers, body = get(base, f"/rw/c1.m3u8?token={token}")
    targets = [line for line in lines_of(body) if line and not line.startswith("#")]
    return (
        status,
        header(headers, "content-type"),
        header(headers, "cache-control"),
        len(targets),
        all(line.startswith("http") for line in targets),
    )


def c_passthrough(base, token):
    status, headers, _ = get(base, f"/pt/c1.m3u8?token={token}")
    without = get(base, "/pt/c1.m3u8")[0]
    return status, tail(header(headers, "location")), without


def c_unknown_path(base, _token):
    status, headers, _ = get(base, "/nope/nope/nope")
    return status, tail(header(headers, "location"))


CASES = [
    ("健康检查", c_health),
    ("总订阅：状态/类型/形状", c_total_playlist),
    ("总订阅：首行与头行", c_total_tags),
    ("分组订阅：五种 access", c_group_playlists),
    ("入口·媒体清单：类型/缓存/标签/切片", c_entry_media),
    ("入口·主清单：类型/缓存/标签/切片", c_entry_master),
    ("变体清单·媒体清单上游 → 跳错误页", c_index_media),
    ("变体清单·主清单上游：类型/缓存/切片", c_index_master),
    ("别名 index.m3u8", c_index_alias),
    ("切片：类型/首写缓存头", c_segment_cold),
    ("切片：命中缓存头", c_segment_hit),
    ("切片：字节一致", c_segment_bytes),
    ("切片：Range 被忽略", c_segment_range),
    ("未知频道/分组 + 合法 token", c_unknown_with_token),
    ("未知频道/分组 + 无 token", c_unknown_without_token),
    ("归档组入口", c_archived),
    ("Token：缺失与错误", c_token_missing),
    ("Token：open 组免校验", c_token_open),
    ("模式 redirect", c_redirect),
    ("模式 rewrite", c_rewrite),
    ("模式 passthrough", c_passthrough),
    ("未知路径", c_unknown_path),
]


def main():
    target_token = token_of(TARGET)
    ours_token = token_of(OURS)
    print(f"目标 token 长度 {len(target_token)}；本实现 token 长度 {len(ours_token)}")
    print()
    print(f"{'用例':<38}{'结果':<6}目标")
    diffs = 0
    for name, function in CASES:
        try:
            target_value = function(TARGET, target_token)
            ours_value = function(OURS, ours_token)
        except Exception as error:  # noqa: BLE001
            print(f"{name:<38}{'错误':<6}{error}")
            diffs += 1
            continue
        same = target_value == ours_value
        print(f"{name:<38}{'一致' if same else '差异':<6}{target_value}")
        if not same:
            print(f"{'':<38}{'':<6}本实现 {ours_value}")
            diffs += 1
    print()
    print(f"用例总数 {len(CASES)}；差异 {diffs}")
    return 1 if diffs else 0


if __name__ == "__main__":
    sys.exit(main())
