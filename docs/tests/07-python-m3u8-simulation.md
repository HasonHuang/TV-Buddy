# 07 · 用 Python 模拟不同速率的 m3u8 频道

## 1. 目的

本文说明如何用 Python 模拟四类 m3u8 频道：

| 类型 | 行为 | 验证什么 |
| --- | --- | --- |
| 高速 | 切片全速返回 | 速率上限与合格判定 |
| 慢速 | 切片低速返回 | `speed_low` 判定 |
| 指定速度 | 按设定速率返回 | 速率阈值边界 |
| 异常 | 清单或切片失败 | `unreachable` 与 `probe_error` 判定 |

模拟器只模拟 HTTP 传输与 HLS 清单。切片内容是零字节。模拟器不生成有效视频。

仓库的 `scripts/quality_mock_source.py` 提供固定的正常、慢速与 404 源。本文的脚本增加任意速率、切片卡住与切片中断。

测速验证使用 `speed` 档。只验证清单与切片可访问时，使用 `playlist` 档。需要验证 `full` 档的分辨率识别或兜底速率时，使用 §7.6 的真实切片夹具。模拟器的零字节切片不能用于分辨率识别。

## 2. 方法

### 2.1 清单与切片

探测器先读媒体清单。然后取倒数第 2 个切片起、向前共 `sample_segments` 个切片。默认 `sample_segments=2`。

探测器统计采样字节与采样耗时。速率按以下公式计算：

```
速率 MiB/s = 总字节 / 总耗时 / 1024 / 1024
```

模拟器返回 6 个切片。每个切片 2 MiB。每个切片声明 2 秒。本次测试使用直播清单。清单不带 `#EXT-X-ENDLIST`。

### 2.2 限速

限速分三步：

1. 把切片切成小块。
2. 每块写入后，按目标速率等待。
3. 小块的等待时间由频道汇总计算。

多个切片共享一个频道速率。探测器测得的总速率接近设定速率。

模拟器把 TCP 发送缓冲区设为 32 KiB。缓冲区过大时，内核会一次收下全部数据。测出的速率会偏高。

### 2.3 异常

模拟器提供 7 种异常：

| 异常 | 位置 | 预期结果 |
| --- | --- | --- |
| 清单 404 | 清单 | `unreachable` |
| 空清单 | 清单 | `probe_error` |
| 无切片清单 | 清单 | `probe_error` |
| HTML 正文 | 清单 | `probe_error` |
| 切片卡住 | 切片 | `probe_error`。等待 `speed_timeout_secs` 后超时 |
| 切片中断 | 切片 | `probe_error`。响应正文短于 `Content-Length` |
| 切片 500 | 切片 | `probe_error`。全部采样切片失败 |

## 3. 前置条件

| 项 | 要求 |
| --- | --- |
| Python | Python 3.8 或更高。只用标准库 |
| 端口 | `19930/tcp` 空闲。可用 `--port` 修改 |
| 网络 | tv-buddy 能访问模拟器地址。同机时用 `127.0.0.1` |
| 工具 | `curl` |
| 可选 | 一台已运行的 tv-buddy。版本来自 `v*` tag |
| 防火墙 | 跨机访问时，放行模拟器端口 |

如果 tv-buddy 在 Docker 中运行，`127.0.0.1` 指向容器本身。改用宿主机地址。Docker Desktop 使用 `host.docker.internal`。Linux 使用网桥网关地址。

先确认 Python：

```bash
python3 --version
```

预期输出 `Python 3.8` 或更高版本。

## 4. 模拟器脚本

把以下内容保存为 `m3u8_sim.py`。

```python
#!/usr/bin/env python3
"""模拟不同速率的 m3u8 频道。"""

import argparse
import re
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

MIB = 1024 * 1024


class Settings:
    segment_bytes = 2 * MIB
    segment_count = 6
    fast_mib = 0.0
    slow_mib = 0.3


class RateGate:
    """按频道汇总限速。多个切片共享一个速率。"""

    def __init__(self, rate_mib: float):
        self.rate = max(rate_mib, 0.01) * MIB
        self.chunk = max(1024, int(self.rate * 0.02))
        self.lock = threading.Lock()
        self.next_time = time.monotonic()

    def wait(self, size: int) -> None:
        with self.lock:
            start = max(time.monotonic(), self.next_time)
            self.next_time = start + size / self.rate
        delay = start - time.monotonic()
        if delay > 0:
            time.sleep(delay)


GATES = {}
GATES_LOCK = threading.Lock()


def gate_for(name: str, rate_mib: float) -> RateGate:
    with GATES_LOCK:
        gate = GATES.get(name)
        if gate is None:
            gate = RateGate(rate_mib)
            GATES[name] = gate
        return gate


def media_playlist(endlist: bool = False) -> bytes:
    lines = [
        "#EXTM3U",
        "#EXT-X-VERSION:3",
        "#EXT-X-TARGETDURATION:2",
        "#EXT-X-MEDIA-SEQUENCE:0",
    ]
    for index in range(Settings.segment_count):
        lines.append("#EXTINF:2.000000,")
        lines.append(f"seg{index}.ts")
    if endlist:
        lines.append("#EXT-X-ENDLIST")
    return ("\n".join(lines) + "\n").encode()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args) -> None:
        pass

    def setup(self) -> None:
        super().setup()
        try:
            self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 32768)
        except OSError:
            pass

    def send_body(self, body: bytes, content_type: str, status: int = 200) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def send_playlist(self, endlist: bool = False) -> None:
        self.send_body(media_playlist(endlist), "application/vnd.apple.mpegurl")

    def send_segment(self, name: str, rate_mib: float, mode: str = "ok") -> None:
        size = Settings.segment_bytes
        if mode == "bad":
            self.send_body(b"bad segment", "text/plain", 500)
            return
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.send_header("Content-Length", str(size))
        self.end_headers()
        if mode == "stall":
            time.sleep(60)
            return
        if mode == "reset":
            try:
                self.wfile.write(b"\0" * 65536)
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass
            self.close_connection = True
            return
        try:
            if rate_mib <= 0:
                self.wfile.write(b"\0" * size)
                self.wfile.flush()
                return
            gate = gate_for(name, rate_mib)
            sent = 0
            while sent < size:
                step = min(gate.chunk, size - sent)
                gate.wait(step)
                self.wfile.write(b"\0" * step)
                self.wfile.flush()
                sent += step
        except (BrokenPipeError, ConnectionResetError):
            pass

    def absolute_url(self, path: str) -> str:
        host = self.headers.get("Host")
        if not host:
            host = f"127.0.0.1:{self.server.server_address[1]}"
        return f"http://{host}{path}"

    def index(self) -> None:
        rows = [
            ("高速", "/fast/index.m3u8"),
            (f"慢速 {Settings.slow_mib} MiB/s", "/slow/index.m3u8"),
            ("指定 0.5 MiB/s", "/rate/0.5/index.m3u8"),
            ("指定 1.0 MiB/s", "/rate/1.0/index.m3u8"),
            ("指定 4.0 MiB/s", "/rate/4.0/index.m3u8"),
            ("404 清单", "/status/404.m3u8"),
            ("空清单", "/empty.m3u8"),
            ("无切片清单", "/noseg/index.m3u8"),
            ("HTML 正文", "/html.m3u8"),
            ("切片卡住", "/stall/index.m3u8"),
            ("切片中断", "/reset/index.m3u8"),
            ("切片 500", "/badseg/index.m3u8"),
        ]
        lines = ["<!doctype html><meta charset='utf-8'><h1>m3u8 模拟器</h1>"]
        lines.append("<p><a href='/source.m3u8'>源清单 source.m3u8</a></p><ul>")
        for label, path in rows:
            lines.append(f"<li>{label}: <a href='{path}'>{path}</a></li>")
        lines.append("</ul>")
        body = "".join(lines).encode()
        self.send_body(body, "text/html; charset=utf-8")

    def source(self) -> None:
        rows = [
            ("模拟-高速", "高速频道", "/fast/index.m3u8"),
            ("模拟-慢速", "慢速频道", "/slow/index.m3u8"),
            ("模拟-指定", "指定 0.5 MiB/s", "/rate/0.5/index.m3u8"),
            ("模拟-指定", "指定 1.0 MiB/s", "/rate/1.0/index.m3u8"),
            ("模拟-指定", "指定 4.0 MiB/s", "/rate/4.0/index.m3u8"),
            ("模拟-异常", "404 频道", "/status/404.m3u8"),
            ("模拟-异常", "空清单频道", "/empty.m3u8"),
            ("模拟-异常", "无切片频道", "/noseg/index.m3u8"),
            ("模拟-异常", "HTML 频道", "/html.m3u8"),
            ("模拟-异常", "切片卡住频道", "/stall/index.m3u8"),
            ("模拟-异常", "切片中断频道", "/reset/index.m3u8"),
            ("模拟-异常", "切片 500 频道", "/badseg/index.m3u8"),
        ]
        lines = ["#EXTM3U"]
        for group, title, path in rows:
            lines.append(f'#EXTINF:-1 group-title="{group}",{title}')
            lines.append(self.absolute_url(path))
        body = ("\n".join(lines) + "\n").encode()
        self.send_body(body, "application/vnd.apple.mpegurl")

    def do_GET(self) -> None:
        path = urlparse(self.path).path
        if path == "/":
            self.index()
            return
        if path == "/source.m3u8":
            self.source()
            return
        if path == "/empty.m3u8":
            self.send_body(b"", "application/vnd.apple.mpegurl")
            return
        if path == "/html.m3u8":
            self.send_body(b"<!doctype html><h1>not hls</h1>", "text/html")
            return
        if path == "/status/404.m3u8":
            self.send_body(b"not found", "text/plain", 404)
            return
        if path == "/noseg/index.m3u8":
            body = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n"
            self.send_body(body, "application/vnd.apple.mpegurl")
            return

        match = re.fullmatch(r"/rate/([0-9]+(?:\.[0-9]+)?)/(index\.m3u8|seg[0-9]+\.ts)", path)
        if match:
            rate_mib, file_name = match.groups()
            if file_name == "index.m3u8":
                self.send_playlist()
            else:
                self.send_segment(f"rate/{rate_mib}", float(rate_mib))
            return

        match = re.fullmatch(r"/(fast|slow|stall|reset|badseg)/(index\.m3u8|seg[0-9]+\.ts)", path)
        if match:
            name, file_name = match.groups()
            if file_name == "index.m3u8":
                self.send_playlist()
                return
            if name == "fast":
                self.send_segment(name, Settings.fast_mib)
            elif name == "slow":
                self.send_segment(name, Settings.slow_mib)
            else:
                mode = "bad" if name == "badseg" else name
                self.send_segment(name, 0.0, mode)
            return

        self.send_body(b"not found", "text/plain", 404)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bind", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=19930)
    parser.add_argument("--fast-mib", type=float, default=0.0)
    parser.add_argument("--slow-mib", type=float, default=0.3)
    parser.add_argument("--segment-bytes", type=int, default=2 * MIB)
    parser.add_argument("--segments", type=int, default=6)
    args = parser.parse_args()
    Settings.fast_mib = args.fast_mib
    Settings.slow_mib = args.slow_mib
    Settings.segment_bytes = args.segment_bytes
    Settings.segment_count = args.segments
    server = ThreadingHTTPServer((args.bind, args.port), Handler)
    server.daemon_threads = True
    print(f"索引: http://{args.bind}:{args.port}/")
    print(f"源清单: http://{args.bind}:{args.port}/source.m3u8")
    server.serve_forever()


if __name__ == "__main__":
    main()
```

## 5. 启动

执行：

```bash
python3 m3u8_sim.py --port 19930
```

预期输出：

```
索引: http://0.0.0.0:19930/
源清单: http://0.0.0.0:19930/source.m3u8
```

`0.0.0.0` 不是可连接地址。本机访问用 `127.0.0.1`。跨机访问用本机网卡地址。

后台运行：

```bash
nohup python3 m3u8_sim.py --port 19930 > m3u8_sim.log 2>&1 &
echo $!
```

预期：最后一行输出进程号。记下该进程号。

停止：

```bash
kill <进程号>
```

## 6. 自检

先自检模拟器。自检不经过 tv-buddy。自检证明模拟器本身符合设定。

### 6.1 源清单

执行：

```bash
curl -s http://127.0.0.1:19930/source.m3u8
```

预期：第一行是 `#EXTM3U`。清单有 12 个频道。每个频道有两行：`#EXTINF` 行与地址行。

### 6.2 高速频道

执行：

```bash
curl -s -o /dev/null -w 'speed=%{speed_download} bytes=%{size_download} time=%{time_total}\n' \
  http://127.0.0.1:19930/fast/seg5.ts
```

预期：

- `bytes=2097152`。
- `speed` 远大于 `1048576`。本机回环通常大于 `10485760`。
- `time` 小于 0.1 秒。

`curl` 的 `speed_download` 单位是字节/秒。除以 1048576 得到 MiB/s。

### 6.3 指定速度频道

执行：

```bash
for rate in 0.5 1.0 4.0; do
  printf '%s ' "$rate"
  curl -s -o /dev/null -w '%{speed_download}\n' \
    "http://127.0.0.1:19930/rate/$rate/seg5.ts"
done
```

预期：每行输出接近该行速率乘以 1048576。误差不超过 10%。

示例：

```
0.5 523000
1.0 1044000
4.0 4170000
```

### 6.4 慢速频道

执行：

```bash
curl -s -o /dev/null -w 'speed=%{speed_download} time=%{time_total}\n' --max-time 15 \
  http://127.0.0.1:19930/slow/seg5.ts
```

预期：

- `speed` 约等于 `314572`。这是 0.3 MiB/s。
- `time` 约等于 6.7 秒。

### 6.5 异常频道

逐项执行下表命令。命令在模拟器所在机器运行。

| 项 | 命令 | 预期 |
| --- | --- | --- |
| 清单 404 | `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19930/status/404.m3u8` | `404` |
| 空清单 | `curl -s -o /dev/null -w '%{http_code} %{size_download}\n' http://127.0.0.1:19930/empty.m3u8` | `200 0` |
| 无切片清单 | `curl -s http://127.0.0.1:19930/noseg/index.m3u8` | 没有 `#EXTINF` 行，没有 `seg` 行 |
| HTML 正文 | `curl -s -o /dev/null -w '%{content_type}\n' http://127.0.0.1:19930/html.m3u8` | 含 `text/html` |
| 切片卡住 | `curl -s -o /dev/null --max-time 3 http://127.0.0.1:19930/stall/seg5.ts; echo $?` | 退出码 `28`。3 秒内没有正文 |
| 切片中断 | `curl -s -o /dev/null http://127.0.0.1:19930/reset/seg5.ts; echo $?` | 退出码 `18`。正文只有 65536 字节 |
| 切片 500 | `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19930/badseg/seg5.ts` | `500` |

## 7. 在 tv-buddy 中验证

本节需要已运行的 tv-buddy。跳过本节时，本文只证明模拟器可用。

### 7.1 添加频道

打开面板。进入「分组管理」。选择一个分组。点击「新建频道」。

按下表添加 5 个频道。地址中的 `<模拟器地址>` 替换为模拟器地址。

| 频道 ID | 名称 | 地址 |
| --- | --- | --- |
| `sim-fast` | 模拟-高速 | `http://<模拟器地址>:19930/fast/index.m3u8` |
| `sim-slow` | 模拟-慢速 | `http://<模拟器地址>:19930/slow/index.m3u8` |
| `sim-0.5` | 模拟-0.5 | `http://<模拟器地址>:19930/rate/0.5/index.m3u8` |
| `sim-1.0` | 模拟-1.0 | `http://<模拟器地址>:19930/rate/1.0/index.m3u8` |
| `sim-404` | 模拟-404 | `http://<模拟器地址>:19930/status/404.m3u8` |

保存每个频道。手工频道默认「保留」。失败时只记录原因，不移出频道。

### 7.2 设置规则

进入「质量校验」→「规则」。

| 字段 | 值 |
| --- | --- |
| 质量校验 | 开 |
| 探测档位 | `测速` |
| 最低速率 | `0.8` MiB/s |
| 采样切片数 | `2` |
| 单切片采样上限 | `4096` KiB |
| 单切片采样时长 | `2` 秒 |
| 测速超时 | `10` 秒 |

点击「保存规则」。

`0.8` 是判定边界。0.5 MiB/s 频道判失败。1.0 MiB/s 频道判通过。不要用 `1.0` 作边界。协议开销会使实测值略低于 1.0。

### 7.3 运行

进入「质量校验」→「概览」。点击「立即校验全部」。

预期：运行开始。5 个频道通常小于 15 秒完成。加入切片卡住频道后，该频道占用约 20 秒。这是两个采样切片各自等待 10 秒超时的结果。

### 7.4 预期结果

进入「质量校验」→「结果」。找到 5 个模拟频道。

| 频道 | 预期速率 | 预期原因 | 预期动作 |
| --- | --- | --- | --- |
| 模拟-高速 | 远大于 0.8 MiB/s | 合格 | 准入或保留 |
| 模拟-慢速 | 约 0.3 MiB/s | 速度低 | 保留 |
| 模拟-0.5 | 约 0.5 MiB/s | 速度低 | 保留 |
| 模拟-1.0 | 约 1.0 MiB/s | 合格 | 准入或保留 |
| 模拟-404 | 无 | 不可达 | 保留 |

手工频道的动作由「手工频道默认处理」决定。默认值是「保留」。只要原因列符合本表，本项通过。

### 7.5 异常频道

重复 7.1 与 7.3，添加并运行其余异常频道：

| 频道 ID | 地址 | 预期原因 |
| --- | --- | --- |
| `sim-empty` | `http://<模拟器地址>:19930/empty.m3u8` | 探测错误 |
| `sim-noseg` | `http://<模拟器地址>:19930/noseg/index.m3u8` | 探测错误 |
| `sim-html` | `http://<模拟器地址>:19930/html.m3u8` | 探测错误 |
| `sim-stall` | `http://<模拟器地址>:19930/stall/index.m3u8` | 探测错误 |
| `sim-reset` | `http://<模拟器地址>:19930/reset/index.m3u8` | 探测错误 |
| `sim-500` | `http://<模拟器地址>:19930/badseg/index.m3u8` | 探测错误 |

### 7.6 生成真实 HLS 夹具

分辨率识别需要真实的视频切片。使用 `ffmpeg` 生成夹具。以下命令在宿主机执行。Docker 中的 tv-buddy 用 `host.docker.internal` 访问夹具服务。

标准 TS 夹具：

```bash
D=$(mktemp -d /tmp/tvb-ts-hls-XXXXXX)
ffmpeg -hide_banner -loglevel error \
  -f lavfi -i testsrc2=size=1280x720:rate=25 -t 6 \
  -c:v libx264 -pix_fmt yuv420p -g 50 \
  -f hls -hls_time 2 -hls_list_size 0 \
  -hls_segment_filename "$D/seg%d" "$D/media.m3u8"

cat > "$D/master.m3u8" <<'EOF'
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=8000000
media.m3u8
EOF

python3 -m http.server 19931 --directory "$D"
```

要点：

- 切片文件名是 `seg0`、`seg1` 等，没有 `.ts` 扩展名。
- 主清单没有 `RESOLUTION`。
- 服务地址是 `http://<宿主机地址>:19931/master.m3u8`。

fMP4 夹具：

```bash
F=$(mktemp -d /tmp/tvb-fmp4-hls-XXXXXX)
ffmpeg -hide_banner -loglevel error \
  -f lavfi -i testsrc2=size=1280x720:rate=25 -t 6 \
  -c:v libx264 -pix_fmt yuv420p -g 50 \
  -f hls -hls_time 2 -hls_list_size 0 \
  -hls_segment_type fmp4 \
  -hls_fmp4_init_filename init.mp4 \
  -hls_segment_filename "$F/seg%d.m4s" "$F/media.m3u8"

cat > "$F/master.m3u8" <<'EOF'
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=8000000
media.m3u8
EOF

grep '#EXT-X-MAP' "$F/media.m3u8"
python3 -m http.server 19932 --directory "$F"
```

预期：`grep` 输出 `#EXT-X-MAP:URI="init.mp4"`。服务地址是 `http://<宿主机地址>:19932/master.m3u8`。

### 7.7 兜底速率验收

1. 启动模拟器：`python3 m3u8_sim.py --port 19930`。
2. 添加 `sim-0.5` 与 `sim-1.0` 两个频道。地址见 §7.1。
3. 设置规则：

| 字段 | 值 |
| --- | --- |
| 探测档位 | `测速` |
| 最低速率 | `0` MiB/s |
| 兜底速率 | `0.8` MiB/s |
| 采样切片数 | `2` |
| 单切片采样上限 | `4096` KiB |
| 单切片采样时长 | `2` 秒 |

4. 运行质量校验。预期：`sim-0.5` 判速率低；`sim-1.0` 合格。模拟清单没有 `RESOLUTION`，两条结果都走兜底速率。
5. 把兜底速率改为 `0`，最低速率改为 `0.8`。再运行。预期原因不变。`0` 表示使用最低速率。

### 7.8 分辨率识别验收

1. 按 §7.6 启动标准 TS 夹具，端口 `19931`。
2. 添加频道 `fixture-ts`。地址是 `http://<宿主机地址>:19931/master.m3u8`。
3. 设置规则：

| 字段 | 值 |
| --- | --- |
| 探测档位 | `完整` |
| 最低速率 | `0` MiB/s |
| 兜底速率 | `0.8` MiB/s |
| 启用分辨率识别 | 开 |
| 单次超时 | `5` 秒 |
| 并发 | `1` |
| 单轮上限 | `0` |
| 最低测速门槛 | `0` MiB/s |
| 采样大小 | `512` KiB |

4. 运行质量校验。预期：`fixture-ts` 的分辨率显示 `1280x720`。原因按速率判定。主清单没有 `RESOLUTION`，切片地址没有扩展名。分辨率来自已下载切片。
5. 关闭「启用分辨率识别」。再运行。预期：分辨率显示 `—`。原因按兜底速率判定。不出现「探测错误」。
6. 按 §7.6 启动 fMP4 夹具，端口 `19932`。添加频道 `fixture-fmp4`，地址是 `http://<宿主机地址>:19932/master.m3u8`。
7. 开启分辨率识别。运行质量校验。预期：分辨率显示 `—`。原因按兜底速率判定。不出现「探测错误」。
8. 可选：在未安装 `ffprobe` 的容器中运行。预期：分辨率显示 `—`。原因按兜底速率判定。不出现「探测错误」。

## 8. 参数

| 参数 | 默认值 | 作用 |
| --- | --- | --- |
| `--bind` | `0.0.0.0` | 监听地址 |
| `--port` | `19930` | 监听端口 |
| `--fast-mib` | `0` | 高速频道限速。`0` 表示不限速 |
| `--slow-mib` | `0.3` | 慢速频道速率 |
| `--segment-bytes` | `2097152` | 每个切片的字节数 |
| `--segments` | `6` | 每个清单的切片数。建议不小于 3 |

修改参数后重启模拟器。正在运行的探测不会读取新参数。

## 9. 常见问题

### 9.1 实测速率低于设定值

原因一：同一频道还有其他客户端。限速按频道汇总。多个客户端会分摊速率。

原因二：采样时长太短。低速频道在采样窗口内读到的字节少。字节少时，启动开销占比大。把「单切片采样时长」调到 3 秒，或把 `--segment-bytes` 调大。

原因三：CPU 或网络繁忙。降低目标速率，再测一次。

### 9.2 实测速率高于设定值

原因一：TCP 发送缓冲区过大。脚本已设为 32 KiB。确认没有修改 `setup()`。

原因二：目标速率高于链路能力。降低速率。

原因三：切片太小。高速率下，2 MiB 切片在几毫秒内完成。测量误差变大。把 `--segment-bytes` 调到 8388608。

### 9.3 探测结果是「不可达」

检查清单地址。`curl` 直接访问清单。确认服务返回 `200` 与 `#EXTM3U`。

检查 tv-buddy 到模拟器的网络。Docker 中的 tv-buddy 不能用 `127.0.0.1` 访问宿主机。

### 9.4 探测结果是「探测错误」

检查采样切片数。`sample_segments` 必须大于 0。建议清单的切片数大于 `sample_segments`。

检查 `#EXT-X-ENDLIST`。正常模拟频道不应带它。

检查 `speed_timeout_secs`。低速频道需要更长的超时。保持默认值 `10`。

### 9.5 端口被占用

执行：

```bash
lsof -iTCP:19930 -sTCP:LISTEN
```

预期：无输出时端口可用。有输出时换一个端口：

```bash
python3 m3u8_sim.py --port 19931
```

### 9.6 分辨率显示为 `—`

可能原因：

- 未开启分辨率识别。完整档按测速执行。
- 未安装 `ffprobe`。检查面板的工具状态。
- 采样大小太小。把 `ffprobe_sample_kb` 调到 `1024` 或 `2048`。
- 夹具是 fMP4。本版不支持需要初始化段的 fMP4 HLS。使用 TS 夹具。
- 切片是零字节。模拟器不能用于分辨率识别。使用 §7.6 的夹具。

分辨率未知不是探测错误。判定使用兜底速率。

## 10. 记录模板

复制以下内容到 `docs/tests/runs/<日期>.md`：

```markdown
## 07 · 用 Python 模拟不同速率的 m3u8 频道

- 执行日期：
- 执行人：
- tv-buddy 版本：
- 模拟器命令：
- 模拟器地址与端口：
- 夹具命令：
- 探测档位：
- 采样参数：sample_segments= / sample_secs= / sample_bytes_kb=
- 判定阈值：min_speed_mib= / fallback_speed_mib=
- ffprobe 设置：开关= / ffprobe_sample_kb= / ffprobe_timeout_secs=
- 自检结果：
  - 高速：____ MiB/s
  - 慢速：____ MiB/s
  - 指定 0.5：____ MiB/s
  - 指定 1.0：____ MiB/s
  - 指定 4.0：____ MiB/s
- 服务结果：
  - 高速：通过 / 不通过
  - 慢速：通过 / 不通过
  - 指定 0.5：通过 / 不通过
  - 指定 1.0：通过 / 不通过
  - 指定 4.0：通过 / 不通过
  - 清单 404：通过 / 不通过
  - 空清单：通过 / 不通过
  - 无切片清单：通过 / 不通过
  - HTML 正文：通过 / 不通过
  - 切片卡住：通过 / 不通过
  - 切片中断：通过 / 不通过
  - 切片 500：通过 / 不通过
  - 兜底 0.5 频道：通过 / 不通过
  - 兜底 1.0 频道：通过 / 不通过
  - TS 夹具分辨率：____（预期 `1280x720`）
  - TS 夹具原因：____
  - fMP4 夹具分辨率：____（预期 `—`）
  - fMP4 夹具原因：____（预期不是探测错误）
- 实际现象：
- 结论：通过 / 不通过 / 未验证
- 缺口：
```

## 11. 失败时怎么报

第 1 步：收集证据。

```bash
D=/tmp/m3u8-sim-fail-$(date +%Y%m%d-%H%M)
mkdir -p "$D"
cp m3u8_sim.log "$D"/ 2>/dev/null
curl -s http://127.0.0.1:19930/source.m3u8 > "$D/source.m3u8"
curl -s http://127.0.0.1:19930/slow/index.m3u8 > "$D/slow.m3u8"
curl -s http://127.0.0.1:19930/rate/1.0/index.m3u8 > "$D/rate-1.0.m3u8"
for rate in 0.5 1.0 4.0; do
  curl -s -o /dev/null -w "$rate %{speed_download}\n" \
    "http://127.0.0.1:19930/rate/$rate/seg5.ts"
done > "$D/speed.txt" 2>&1
tar -czf "$D.tar.gz" -C /tmp "$(basename "$D")"
echo "$D.tar.gz"
```

预期：最后一行输出 tar 包路径。

第 2 步：在 `docs/tests/runs/<日期>.md` 写三项：

- 结论：`不通过`。
- 实际现象：写数字与日志原文。一行。
- 证据：写 tar 包路径。

第 3 步：把 tar 包路径交给开发者。开发者在 `PROGRESS.md` 立一条缺口。

必须包含的证据：

- 模拟器的完整命令。
- `curl` 的速率输出。
- 探测结果的截图或 JSON。
- tv-buddy 日志中的探测错误行。
