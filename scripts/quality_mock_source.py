#!/usr/bin/env python3
"""质量准入验收用的伪订阅源。

监听本机端口，提供：
- `/ok.m3u8`：正常清单，切片快速返回。
- `/slow.m3u8`：慢速清单，切片每 100 ms 发 1 KiB。
- `/dead.m3u8`：固定 404。
- `/source.m3u8`：三频道源清单。
- `/control?slow=ok|slow`：切换慢速频道地址，用于恢复测试。

容器通过 `host.docker.internal` 访问本机。`--public-base` 必须与容器视角一致。
"""

import argparse
import http.server
import json
import time
import urllib.parse

STATE = {"slow": "slow"}


def media_playlist(base: str, segment: str) -> bytes:
    return (
        "#EXTM3U\n"
        "#EXT-X-VERSION:3\n"
        "#EXT-X-TARGETDURATION:2\n"
        "#EXTINF:2,\n"
        f"{base}/{segment}\n"
        "#EXT-X-ENDLIST\n"
    ).encode()


class Handler(http.server.BaseHTTPRequestHandler):
    public_base = "http://host.docker.internal:19932"

    def log_message(self, *args):
        pass

    def send_body(self, body: bytes, content_type: str, status: int = 200):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_fast_segment(self):
        body = b"\0" * (2 * 1024 * 1024)
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        step = len(body) // 16
        try:
            for start in range(0, len(body), step):
                self.wfile.write(body[start : start + step])
                self.wfile.flush()
                time.sleep(0.002)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def send_slow_segment(self):
        # 约 10 KiB/s，明显低于验收规则里的 1 MiB/s。
        size = 1024 * 1024
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.send_header("Content-Length", str(size))
        self.end_headers()
        try:
            for _ in range(size // 1024):
                self.wfile.write(b"\0" * 1024)
                self.wfile.flush()
                time.sleep(0.1)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        if path == "/ok.m3u8":
            self.send_body(
                media_playlist(self.public_base, "ok-seg.ts"),
                "application/vnd.apple.mpegurl",
            )
        elif path == "/ok2.m3u8":
            # 恢复后的慢速频道用独立地址，避免被源同步按同址去重。
            self.send_body(
                media_playlist(self.public_base, "ok2-seg.ts"),
                "application/vnd.apple.mpegurl",
            )
        elif path == "/slow.m3u8":
            self.send_body(
                media_playlist(self.public_base, "slow-seg.ts"),
                "application/vnd.apple.mpegurl",
            )
        elif path == "/ok-seg.ts":
            self.send_fast_segment()
        elif path == "/ok2-seg.ts":
            self.send_fast_segment()
        elif path == "/slow-seg.ts":
            self.send_slow_segment()
        elif path == "/dead.m3u8":
            self.send_body(b"not found", "text/plain", status=404)
        elif path == "/source.m3u8":
            slow = STATE["slow"]
            slow_path = "slow.m3u8" if slow == "slow" else "ok2.m3u8"
            body = (
                "#EXTM3U\n"
                '#EXTINF:-1 tvg-id="normal" group-title="质量",正常频道\n'
                f"{self.public_base}/ok.m3u8\n"
                '#EXTINF:-1 tvg-id="slow" group-title="质量",慢速频道\n'
                f"{self.public_base}/{slow_path}\n"
                '#EXTINF:-1 tvg-id="dead" group-title="质量",不可达频道\n'
                f"{self.public_base}/dead.m3u8\n"
            ).encode()
            self.send_body(body, "application/vnd.apple.mpegurl")
        elif path == "/control":
            query = urllib.parse.parse_qs(parsed.query)
            value = query.get("slow", ["slow"])[0]
            if value not in ("ok", "slow"):
                self.send_body(b'{"error":"slow must be ok or slow"}', "application/json", 400)
                return
            STATE["slow"] = value
            self.send_body(
                json.dumps({"ok": True, "slow": value}).encode(),
                "application/json",
            )
        else:
            self.send_body(b"not found", "text/plain", status=404)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=19932)
    parser.add_argument(
        "--public-base",
        default="http://host.docker.internal:19932",
        help="容器访问本机时使用的基础地址",
    )
    args = parser.parse_args()
    Handler.public_base = args.public_base.rstrip("/")
    server = http.server.ThreadingHTTPServer(("0.0.0.0", args.port), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()
