#!/usr/bin/env bash
# 质量校验端到端冒烟：探测三种频道、按规则隐藏、关闭后恢复。
#
# 依赖：curl、jq、python3，以及已构建的 `target/debug/tv-buddy`。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

for tool in curl jq python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "缺少 $tool，请先安装" >&2
    exit 1
  fi
done

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19931}
MEDIA_PORT=${MEDIA_PORT:-19932}
if [ ! -x "$BIN" ]; then
  echo "未找到可执行文件 $BIN，请先运行 cargo build" >&2
  exit 1
fi

WORK=$(mktemp -d)
DB="$WORK/tv-buddy.db"
CFG="$WORK/config.json"
B="http://127.0.0.1:$PORT"
M="http://127.0.0.1:$MEDIA_PORT"

MEDIA_PID=""
PID=""
cleanup() {
  [ -n "$PID" ] && kill "$PID" 2>/dev/null || true
  [ -n "$MEDIA_PID" ] && kill "$MEDIA_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

fail=0
check() {
  if [ "$2" = "$3" ]; then
    printf 'ok   %s\n' "$1"
  else
    printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"
    fail=1
  fi
}

# 本地测试源：正常 HLS、限速 HLS、404。
python3 - "$MEDIA_PORT" >"$WORK/media.log" 2>&1 <<'PY' &
import http.server
import sys
import time

port = int(sys.argv[1])
M = f"http://127.0.0.1:{port}"
ok_body = b"\0" * (2 * 1024 * 1024)


def media(seg1: str, seg2: str) -> bytes:
    return (
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n"
        f"#EXTINF:2,\n{seg1}\n#EXTINF:2,\n{seg2}\n#EXT-X-ENDLIST\n"
    ).encode()


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def send_body(self, body: bytes, content_type: str):
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/ok.m3u8":
            body = media(f"{M}/ok-seg1.ts", f"{M}/ok-seg2.ts")
            self.send_body(body, "application/vnd.apple.mpegurl")
        elif path == "/slow.m3u8":
            body = media(f"{M}/slow-seg1.ts", f"{M}/slow-seg2.ts")
            self.send_body(body, "application/vnd.apple.mpegurl")
        elif path == "/ok-seg1.ts":
            # 分块发完 2 MiB，避免毫秒级完成时耗时被记为 0。
            self.send_response(200)
            self.send_header("Content-Type", "video/mp2t")
            self.send_header("Content-Length", str(len(ok_body)))
            self.end_headers()
            step = len(ok_body) // 16
            for start in range(0, len(ok_body), step):
                self.wfile.write(ok_body[start:start + step])
                self.wfile.flush()
                time.sleep(0.002)
        elif path in ("/ok-seg2.ts", "/slow-seg2.ts"):
            self.send_body(b"\0" * 1024, "video/mp2t")
        elif path == "/slow-seg1.ts":
            # 每 100 ms 发 1 KiB，约 10 KiB/s，明显低于 1 MiB/s。
            self.send_response(200)
            self.send_header("Content-Type", "video/mp2t")
            self.send_header("Content-Length", str(1024 * 1024))
            self.end_headers()
            try:
                for _ in range(1000):
                    self.wfile.write(b"\0" * 1024)
                    self.wfile.flush()
                    time.sleep(0.1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        else:
            self.send_error(404)


server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY
MEDIA_PID=$!

# Python 服务把端口写在脚本里；这里只替换成运行时端口。
sleep 0.2
if ! curl -fsS "$M/ok.m3u8" >/dev/null 2>&1; then
  echo "测试源启动失败，日志见 $WORK/media.log" >&2
  exit 1
fi

# 最小配置：只指定监听与缓存目录。其余字段由默认值补齐。
cat > "$CFG" <<JSON
{
  "server": { "bind": "127.0.0.1:$PORT", "log_level": "warn" },
  "cache": { "disk_path": "$WORK/cache" }
}
JSON

"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" >"$WORK/server.log" 2>&1 &
PID=$!

for _ in $(seq 1 60); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 0.1
done

login() {
  curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
    -d '{"username":"admin","password":"admin"}' | jq -r '.token // ""'
}

T=$(login)
AUTH="Authorization: Bearer $T"
check "登录拿到 token" "yes" "$([ -n "$T" ] && echo yes || echo no)"

getcfg() { curl -s -H "$AUTH" "$B/panel/api/config" > "$1"; }
putcfg() {
  curl -s -o "$WORK/put.out" -w '%{http_code}' -X PUT "$B/panel/api/config" \
    -H "$AUTH" -H 'content-type: application/json' --data-binary "@$1"
}

# 1) 写入三个频道与质量规则。宽限设为 0，便于立即观察隐藏效果。
getcfg "$WORK/base.json"
jq \
  --arg ok "$M/ok.m3u8" \
  --arg slow "$M/slow.m3u8" \
  --arg bad "$M/404.m3u8" '
  .quality.enabled = true
  | .quality.probe_mode = "speed"
  | .quality.interval_secs = 3600
  | .quality.timeout_secs = 5
  | .quality.speed_timeout_secs = 5
  | .quality.sample_bytes_kb = 4096
  | .quality.sample_secs = 2
  | .quality.sample_segments = 2
  | .quality.stale_keep_secs = 0
  | .quality.rules.min_speed_mib = 1.0
  | .quality.rules.action = "mark"
  | .groups = [{
      id: "qsmoke",
      name: "质量冒烟",
      mode: "proxy",
      access: "public",
      format: "hls",
      channels: [
        { id: "ok", name: "OK频道", url: $ok },
        { id: "slow", name: "慢速频道", url: $slow },
        { id: "bad", name: "坏频道", url: $bad }
      ]
    }]
  ' "$WORK/base.json" > "$WORK/cfg.json"
check "写入质量配置 200" "200" "$(putcfg "$WORK/cfg.json")"

# 2) 手动校验全部频道。force 允许在配置刚写入时立即执行。
CODE=$(curl -s -o "$WORK/run.json" -w '%{http_code}' -X POST "$B/panel/api/quality/run" \
  -H "$AUTH" -H 'content-type: application/json' \
  -d '{"scope":"all","run_mode":"quick","force":true}')
check "发起校验 202" "202" "$CODE"
RUN_ID=$(jq -r '.run_id // ""' "$WORK/run.json")
check "返回 run_id" "yes" "$([ -n "$RUN_ID" ] && echo yes || echo no)"
check "queued 为频道数" "3" "$(jq -r '.queued' "$WORK/run.json")"

# 3) 轮询到完成。慢速源采样 2 秒，留足余量。
STATUS='{}'
for _ in $(seq 1 200); do
  STATUS=$(curl -s -H "$AUTH" "$B/panel/api/quality/status")
  if [ "$(echo "$STATUS" | jq -r '.run.finished // false')" = "true" ]; then
    break
  fi
  sleep 0.25
done
check "校验完成" "true" "$(echo "$STATUS" | jq -r '.run.finished // false')"
check "校验频道数" "3" "$(echo "$STATUS" | jq -r '.run.tested // 0')"

RESULTS=$(curl -s -H "$AUTH" "$B/panel/api/quality/results")
reason() { echo "$RESULTS" | jq -r --arg cid "$1" '.items[] | select(.cid == $cid) | .reason'; }
check "正常源判定 pass" "pass" "$(reason ok)"
check "慢速源判定 speed_low" "speed_low" "$(reason slow)"
check "404 源判定 unreachable" "$(reason bad)" "unreachable"
check "慢速源结果可达" "true" \
  "$(echo "$RESULTS" | jq -r '.items[] | select(.cid == "slow") | .reachable')"

# 4) mark 动作不改变订阅。三个频道都应出现。
PTOK=$(curl -s -H "$AUTH" "$B/panel/api/me" | jq -r '.token // ""')
playlist() { curl -s "$B/sub/$PTOK/playlist.m3u"; }
PL=$(playlist)
for name in "OK频道" "慢速频道" "坏频道"; do
  check "mark 时清单含 $name" "1" "$(echo "$PL" | grep -c "$name" || true)"
done

# 5) 切换到 hide，两个失败频道应从清单消失。每次 PUT 都要用最新 revision。
getcfg "$WORK/live.json"
jq '.quality.rules.action = "hide"' "$WORK/live.json" > "$WORK/hide.json"
check "切换到 hide 200" "200" "$(putcfg "$WORK/hide.json")"
PL=$(playlist)
check "hide 时清单含正常频道" "1" "$(echo "$PL" | grep -c "OK频道" || true)"
check "hide 时清单不含慢速频道" "0" "$(echo "$PL" | grep -c "慢速频道" || true)"
check "hide 时清单不含 404 频道" "0" "$(echo "$PL" | grep -c "坏频道" || true)"

# 隐藏按当前规则重算，因此重新取一次结果。
RESULTS=$(curl -s -H "$AUTH" "$B/panel/api/quality/results")
check "慢速结果 hidden=true" "true" \
  "$(echo "$RESULTS" | jq -r '.items[] | select(.cid == "slow") | .hidden')"
check "坏源结果 hidden=true" "true" \
  "$(echo "$RESULTS" | jq -r '.items[] | select(.cid == "bad") | .hidden')"

# 预览只统计隐藏项，不改清单。
PREVIEW=$(curl -s -X POST -H "$AUTH" "$B/panel/api/quality/preview")
check "预览隐藏数" "2" "$(echo "$PREVIEW" | jq -r '.total')"

# 6) 关闭质量校验后，清单恢复原状。
getcfg "$WORK/live.json"
jq '.quality.enabled = false' "$WORK/live.json" > "$WORK/off.json"
check "关闭质量校验 200" "200" "$(putcfg "$WORK/off.json")"
PL=$(playlist)
for name in "OK频道" "慢速频道" "坏频道"; do
  check "关闭后清单恢复 $name" "1" "$(echo "$PL" | grep -c "$name" || true)"
done

if [ "$fail" = "0" ]; then
  echo "smoke-quality: PASS"
else
  echo "smoke-quality: FAIL"
  echo "--- server.log ---"
  tail -40 "$WORK/server.log"
  exit 1
fi
