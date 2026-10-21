#!/usr/bin/env python3
"""DASH 回看端到端断言（容器内运行）。

用 ffmpeg 生成 DASH 源，起本实现与 `python3 -m http.server`，再检查回看清单与切片。
等价于 `scripts/smoke-dash-timeshift.sh`，供无 `bash`/`curl` 的镜像使用。

用法：python3 dash-smoke-container.py <iptv-proxy 路径>
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

BIN = sys.argv[1]
PORT = int(os.environ.get("PORT", "19940"))
UPORT = int(os.environ.get("UPORT", "19941"))
work = os.environ.get("WORK") or tempfile.mkdtemp(prefix="dash-")
procs = []
fail = 0


def check(name, expected, actual):
    global fail
    if expected == actual:
        print(f"ok   {name}")
    else:
        print(f"FAIL {name}（期望 {expected}，实际 {actual}）")
        fail = 1


def get(url, timeout=5):
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return response.status, response.headers, response.read()


try:
    subprocess.run(
        [
            "ffmpeg", "-loglevel", "error", "-f", "lavfi",
            "-i", "testsrc=size=320x240:rate=25", "-t", "16",
            "-c:v", "libx264", "-preset", "ultrafast", "-g", "50",
            "-f", "dash", "-seg_duration", "2",
            "-use_template", "1", "-use_timeline", "1",
            os.path.join(work, "out.mpd"),
        ],
        check=True,
    )
    procs.append(
        subprocess.Popen(
            [
                "python3", "-m", "http.server", str(UPORT),
                "--bind", "127.0.0.1", "--directory", work,
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    )
    for _ in range(50):
        try:
            if get(f"http://127.0.0.1:{UPORT}/out.mpd", timeout=1)[0] == 200:
                break
        except Exception:
            pass
        time.sleep(0.1)
    config = os.path.join(work, "config.toml")
    with open(config, "w", encoding="utf-8") as handle:
        handle.write(
            f"""[server]
bind = "127.0.0.1:{PORT}"
log_level = "warn"

[cache]
disk_path = "{work}/cache"

[[groups]]
id = "dg"
name = "DASH 组"
mode = "dash"
access = "open"
[[groups.channels]]
id = "c1"
name = "DASH 频道"
url = "http://127.0.0.1:{UPORT}/out.mpd"
dash_timeshift = true
dash_timeshift_hours = 4
"""
        )
    log_path = os.path.join(work, "server.log")
    with open(log_path, "wb") as log:
        procs.append(subprocess.Popen([BIN, config], stdout=log, stderr=subprocess.STDOUT))
        base = f"http://127.0.0.1:{PORT}"
        for _ in range(60):
            try:
                if get(f"{base}/health", timeout=1)[0] == 200:
                    break
            except Exception:
                pass
            time.sleep(0.1)

        now = int(time.time())
        url = f"{base}/dg/c1.m3u8?start={now - 60}&end={now + 60}"
        body = ""
        status = 0
        for _ in range(120):
            try:
                status, _, raw = get(url)
                body = raw.decode("utf-8", "replace")
            except Exception:
                status = 0
            if "#EXT-X-PLAYLIST-TYPE:VOD" in body:
                break
            time.sleep(0.5)
        check("DASH 回看清单 200", 200, status)
        check("VOD 类型", True, "#EXT-X-PLAYLIST-TYPE:VOD" in body)
        match = re.search(r"/dg/c1/timeshift/[0-9a-f]*[.][a-z]*", body)
        if not match:
            print("FAIL 找不到切片")
            fail = 1
        else:
            seg = match.group(0)
            seg_status, seg_headers, _ = get(base + seg)
            check("切片 200", 200, seg_status)
            check(
                "切片类型为图片",
                True,
                seg_headers.get("content-type", "").startswith("image/"),
            )

    if fail:
        print("--- server.log ---")
        with open(log_path, encoding="utf-8", errors="replace") as handle:
            sys.stdout.write(handle.read()[-2000:])
finally:
    for proc in reversed(procs):
        proc.terminate()
    for proc in reversed(procs):
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
    if not os.environ.get("KEEP"):
        shutil.rmtree(work, ignore_errors=True)

print("DASH 回看验证通过" if not fail else "DASH 回看验证失败")
sys.exit(fail)
