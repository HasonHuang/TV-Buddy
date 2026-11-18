# 2026-11-18 · 质量兜底速率与 ffprobe 分辨率识别

- 执行日期：2026-11-18
- 执行人：Claude（本地容器实测）
- tv-buddy 版本：`local`（从 HEAD `5da1d03` 本地构建）
- 构建命令：`docker build --target build -t tvb-build:test .`
- 运行方式：从 `tvb-build:test` 取 `/out/bin/tv-buddy`，在 Alpine 容器中运行
- 容器：`tvb-qtest`。映射 `127.0.0.1:19902` → `9900/tcp`
- 模拟器命令：`python3 m3u8_sim.py --port 19930`
- 模拟器地址：宿主机 `19930/tcp`。容器经 `host.docker.internal` 访问
- 夹具服务：`python3 -m http.server 19931 --directory <TS 目录>`、`python3 -m http.server 19932 --directory <fMP4 目录>`
- 宿主机工具：`ffmpeg 9.0.2`、`ffprobe 9.0.2`（`/opt/homebrew/bin`）

## 1. 构建

执行：

```bash
docker build --target build -t tvb-build:test .
```

预期：镜像构建成功，产物 `/out/bin/tv-buddy` 存在。

实际：`BUILD_EXIT=0`。产物大小 11.9 MiB。

结论：通过。

## 2. 夹具

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

实际：

- 切片文件名是 `seg0`、`seg1`、`seg2`。没有 `.ts` 扩展名。
- 媒体清单是 `#EXTINF:2.000000` 加 `seg0` 等地址。
- 主清单只有 `BANDWIDTH=8000000`。没有 `RESOLUTION`。

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

实际：`grep` 输出 `#EXT-X-MAP:URI="init.mp4"`。

结论：通过。

## 3. 配置

面板 `PUT /panel/api/config`。关键值：

| 字段 | 值 |
| --- | --- |
| `quality.enabled` | `true` |
| `quality.probe_mode` | `full` |
| `quality.ffprobe` | `true`（步骤 5 改为 `false`） |
| `quality.ffprobe_timeout_secs` | `5` |
| `quality.probe_concurrency` | `1` |
| `quality.max_ffprobe_per_run` | `0` |
| `quality.ffprobe_min_speed_mib` | `0` |
| `quality.ffprobe_sample_kb` | `512` |
| `quality.sample_segments` | `2` |
| `quality.sample_secs` | `2.0` |
| `quality.sample_bytes_kb` | `4096` |
| `quality.interval_secs` | `0` |
| `rules.min_speed_mib` | `0` |
| `rules.fallback_speed_mib` | `0.8` |
| `rules.resolution_speed_map` | `{}` |
| `rules.max_delay_ms` | `0` |

## 4. 端到端结果

| 步 | 预期 | 实际 | 结论 |
| --- | --- | --- | --- |
| 1 | Linux 二进制构建成功 | `tvb-build:test` 构建成功，产物 11.9 MiB | 通过 |
| 2 | TS 夹具无扩展名切片与无 `RESOLUTION` 主清单 | 切片是 `seg0` 等，主清单无 `RESOLUTION` | 通过 |
| 3 | 模拟器与容器可互通 | 模拟器 200。容器 4 个频道全部探测 | 通过 |
| 4 | 分辨率显示 `WxH` | `fixture-ts` 显示 `1280x720` | 通过 |
| 5 | 关闭 ffprobe 后分辨率 `—`，使用兜底 | 4 个频道分辨率都是 `—`。原因按兜底速率 | 通过 |
| 6 | fMP4 分辨率 `—`，使用兜底，无 `probe_error` | 分辨率 `—`。原因 `pass`。`probe_error` 为 `false` | 通过 |
| 7 | 两个二进制下载并可用 | 两个都下载。`ffprobe -version` 输出 `7.0.2-static` | 通过 |
| 8 | 1440、1024、720、460 px 无重叠 | 无浏览器 | 未验证 |

### 4.1 步骤 4：开启 ffprobe

运行 `POST /panel/api/quality/run`，范围 `group=qtest`，模式 `full`。

| 频道 | 分辨率 | 速率 MiB/s | 原因 | `probe_error` |
| --- | --- | --- | --- | --- |
| TS夹具 | `1280x720` | 724.66 | `pass` | `false` |
| fMP4夹具 | `—` | 740.61 | `pass` | `false` |
| 模拟-0.5 | `—` | 0.49 | `speed_low` | `false` |
| 模拟-1.0 | `—` | 1.00 | `pass` | `false` |

说明：

- TS 主清单没有 `RESOLUTION`。分辨率来自已下载切片。
- 模拟-0.5 的分辨率未知。阈值取兜底 `0.8`。速率 0.49 判 `speed_low`。
- 模拟-1.0 的速率 1.00 不低于兜底 `0.8`。判 `pass`。
- 结果行的 `rule_hash` 是 `9356472b3a11fb75c671f84d08b21e47014af4f55fbaf639704b60529ace583d`。

### 4.2 步骤 5：关闭 ffprobe

运行同一范围。4 个频道都完成。

| 频道 | 分辨率 | 速率 MiB/s | 原因 | `probe_error` |
| --- | --- | --- | --- | --- |
| TS夹具 | `—` | 408.47 | `pass` | `false` |
| fMP4夹具 | `—` | 445.05 | `pass` | `false` |
| 模拟-0.5 | `—` | 0.49 | `speed_low` | `false` |
| 模拟-1.0 | `—` | 1.00 | `pass` | `false` |

说明：

- 关闭 ffprobe 后，`probe_mode=full` 按 `speed` 执行。
- 分辨率全部未知。判定使用兜底 `0.8`。
- `rule_hash` 是 `73e98613bf318d786fb9a832c3c4a6916a798f3d88b4441ed12b7f6d7b21923a`。与步骤 4 不同。`ffprobe_ready` 进入哈希。

### 4.3 步骤 6：fMP4 夹具

fMP4 频道在步骤 4 中已运行。媒体清单含 `#EXT-X-MAP:URI="init.mp4"`。

实际：

- 采集只取媒体切片 `seg1.m4s`。不含 `init.mp4`。
- `ffprobe` 无法解析。分辨率保持 `—`。
- 原因 `pass`。判定使用兜底 `0.8`。
- `probe_error` 为 `false`。

结论：通过。

### 4.4 步骤 7：面板安装

执行 `POST /panel/api/tools/ffmpeg/install`。

实际：

- 任务 `phase=done`。
- `downloaded=51062264`。`total=51062264`。
- `ffmpeg` 行：`source=managed`、`path=/data/tools/ffmpeg`、版本 `7.0.2-static`。
- `ffprobe` 行：`source=managed`、`path=/data/tools/ffprobe`。
- 容器内执行 `/data/tools/ffprobe -version`。输出 `ffprobe version 7.0.2-static`。
- 补测删除。`POST /panel/api/tools/ffmpeg/remove` 返回 `removed=true`。两个文件都消失。状态行回退系统版本。

结论：通过。

## 5. 规则页宽度

本机没有浏览器。没有做视觉检查。以下 4 行都记未验证。

| 宽度 px | 预期 | 实际 | 结论 | 缺少条件 |
| --- | --- | --- | --- | --- |
| 1440 | 无重叠、无横向滚动 | 未检查 | 未验证 | 无浏览器 |
| 1024 | 无重叠、无横向滚动 | 未检查 | 未验证 | 无浏览器 |
| 720 | 无重叠、无横向滚动 | 未检查 | 未验证 | 无浏览器 |
| 460 | 无重叠、无横向滚动 | 未检查 | 未验证 | 无浏览器 |

前端构建通过。构建结果不能替代视觉检查。

## 6. 结论

- 步骤 1–7：通过。
- 步骤 8：未验证。缺少浏览器。
- 总体判定：通过（核心行为）。规则页宽度检查未验证。
- 缺口：规则页在 1440、1024、720、460 px 的版式没有人工检查。
