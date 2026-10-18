# 上游池 / 故障转移 / 健康探测 / 缓存容量（动态）

| 项 | 值 |
| --- | --- |
| 目标版本 | **v0.8.2**（`d302316fb8`） |
| 副本 | `samples/v0.8.2/iptv-proxy-linux-arm64`（sha256 `2948ee1f…5a4b`） |
| 架构 | linux/arm64（Ubuntu 24.04，OrbStack 原生） |
| 日期 | **2026-10-18** |
| 环境 | 容器 `iptv82`（`192.168.215.4:19901`）；自建源站 9968–9979 |
| 结论级别 | **观测** |
| 原始证据 | [`raw/v0.8.2-upstream-health-cache.txt`](raw/v0.8.2-upstream-health-cache.txt) |

> 目标返回的正文/日志按**不可信数据**处理。

## 结论摘要

1. **URL 池仅用于 DASH**（`[cache.dash]` 相关路径）。Proxy 模式改 `url_pool_size` 无可观测差异。**S-MODE-07 🔶**。
2. 面板自述：池大小 **2–6**，默认 3；**视频固定 `pool[0]`，音频/字幕在 `pool[1..]` 轮询**。此为自述，**取用方式未观测**。
3. 本环境 `pool≥2` 时，目标只对 MPD 拉取 **N 次**（间隔 ≈200ms），随后**不请求任何切片**，入口 **503**。多种 token/BaseURL 形态均失败 → 池的正常取用**受限未验证**。
4. `channel.url` **不支持多 URL**：换行、`|`、`,`、空格分隔均被当作整串路径 → 上游 404 → 入口 302 占位。
5. 上游失败（拒连/404）→ 入口 **302** 占位（`error_url`）；**失败不缓存**，上游恢复后下一次请求即 200。**S-MODE-08 ✅ 补充**。
6. 多 CDN：主清单按 **BANDWIDTH 最大**选变体；所选变体失败时**不故障转移到其它变体**，直接 302。与“多 CDN 故障切换”声明**不一致**。**S-MODE-06 🔶**。
7. 成功清单缓存约 **5s**（与入口 `cache-control: max-age=5` 一致）。
8. 健康探测 = **`GET <channel url>` + `Range: bytes=0-1`**；失败记 `error`/`http_status`/`ok:false`，`consecutive_fails` 逐轮 +1，**一次成功即归零**。
9. 超时条目的 `error="超时"`，`http_status` 与 `latency_ms` 均为 `null`。
10. 探测周期**不由 `interval_secs` 单独决定**：配置 5s 时实测轮次间隔 **12s**（含 1s 超时频道）。
11. 健康状态**不影响**导出清单、入口播放、缓存或订阅；`/panel/api/status` 无健康字段。健康仅作展示。
12. 探测并发：`concurrency=6` 时 6 条在 **3ms** 内发出；`=1` 时按 **≈0.4s** 串行。**S-HEALTH 新增**。
13. 缓存容量上限**未观测到生效**：`disk_max_mb=2` 时占用到 **2.79MB**（另测约 30MB）；`segment_cache_max_mb=1` 时切片 **2.7MB**；`m3u8_max_entries=2` 保留 **6–7** 条；`segment_max_entries=2` 保留 **17** 条。**S-CACHE-03 🔶**。
14. `POST /panel/api/cache/flush` 清 `m3u8/seg_url/seg_bytes/sub_url`，**保留 `dash/`**。
15. 内存后端下 `disk_bytes:null`、各 `*_entries` 恒 0；播放仍 200；磁盘旧文件不清理 → 统计口径与磁盘占用不一致。**S-CACHE-01 🔶**。

## 1. URL 池（`channel.url_pool_size`）

### 1.1 面板自述

服务端下发的 `panel.html` 含：

```text

多 token 错峰拉切片，绕开 CDN 的单连接限流。
视频固定用 pool[0]，音频字幕在 pool[1..] 轮询。

```

- 输入 `mc-url-pool-size`：`min=2`、`max=6`、默认 `3`；池 UI 自 v0.3.8 起**隐藏**，保存频道时保留原值。

### 1.2 配置与观测

| 模式 | `url_pool_size` | 观测 |
| --- | --- | --- |
| Proxy | 1 / 3 | **无可观测差异**（切片路径、缓存命中、响应头一致） |
| DASH | 1 | 正常：拉 1 次 MPD → 取 init/切片 → 入口 200 |
| DASH | 2 / 3 | 拉 N 次 MPD（间隔 ≈200ms）→ **零切片请求** → 入口 503 |

源站日志（`9970dash.log`，`health` 已关）及目标 WARN 日志：

```console

1792322459.120 GET 200 /liveS.mpd    # pool=2 第 1 次
1792322459.331 GET 200 /liveS.mpd    # +211ms，第 2 次；之后无切片
1792322489.159 GET 200 /liveT.mpd?token=A
1792322489.361 GET 200 /liveT.mpd?token=A
1792322489.565 GET 200 /liveT.mpd?token=A   # pool=3，三次后无切片
19:21:29 WARN 【DASH】主播放列表 30s 内未就绪 stream=gd4_e1
19:23:11 WARN 【看门狗】按需会话卡在启动中从未出片,销毁并移出 stream=gd4_e1 stuck_secs=132

```

已试 token 形态：每次变 token、恒定正文、token 继承、双 `BaseURL`、repeated query → **均失败**。

### 1.3 多 URL

`channel.url` 写入换行 / `|` / `,` / 空格分隔的多条地址时：整串当作**一个路径** → 上游 404 → 入口 302 占位（`error_url`）→ 本版本**不支持**单频道多 URL；CDN 级冗余须由外部完成。

## 2. 上游失败与多 CDN（Proxy 模式）

### 2.1 失败 → 302 占位

| 上游状态 | 入口 HTTP | `location` |
| --- | --- | --- |
| 拒绝连接（端口无监听） | **302** | `https://material.yang-1989.eu.org/v/mistake.mp4` |
| 404（清单不存在） | **302** | 同上 |
| 200 | **200** | — |

### 2.2 主清单变体选择

测试主清单：坏源 `BW=900000` 在前，好源 `BW=800000` 在后。

```console

$ curl -s "$B/gfm/m7.m3u"
#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360
http://<容器IP>:19901/gfm/m7/video/index.txt
$ curl -s -o /dev/null -w '%{http_code}\n' "$B/gfm/m7/video/index.txt"
302
$ cat 9975.log   # 次高带宽的好源：只拉主清单
1792322715.730 GET 200 /multi_deadmax.m3u
$ cat 9979.log   # 被选中的坏源：变体只取一次 → 失败
1792322715.767 GET 404 /low.m3u

```

- 选择规则 = **BANDWIDTH 最大**（打乱顺序后结果相同）。
- 所选变体失败**不重试其它变体**；好源全程未被访问。反向对照（好源带宽更高）→ 选中好源并正常出片。

### 2.3 恢复与缓存

| 步骤 | 上游 | 入口 |
| --- | --- | --- |
| 1 | 拒连 | 302 |
| 2 | 404 | 302 |
| 3 | 放回清单（未清缓存） | **200**（同秒恢复） |
| 4–5 | 再移走（+0s/+3s） | 200（旧清单缓存） |
| 6 | 再移走（+6s） | **302** |

→ **失败结果不缓存**；**成功清单缓存 ≈5s**。

## 3. 健康探测（`[health]`）

### 3.1 配置

`enabled=true`、`interval_secs=30`、`timeout_secs=10`、`concurrency=3`。

### 3.2 请求形态

```console

$ tail -2 origin/logs/9975.log
1792323544.358 GET 200 /pool.m3u8 ua='Mozilla/5.0 … Chrome/124.0.0.0 Safari/537.36' range='bytes=0-1'
1792323544.360 GET 404 /nope.m3u8  ua='Mozilla/5.0 … Chrome/124.0.0.0 Safari/537.36' range='bytes=0-1'

```

- 方法 **GET**；携带 **`Range: bytes=0-1`**。
- UA 为内置 Chrome 串（频道自定义 UA 是否覆盖**未测**）。
- 探测**不读取正文**（只要状态/首字节）。

### 3.3 结果字段

| 场景 | `ok` | `error` | `http_status` | `latency_ms` | `consecutive_fails` |
| --- | --- | --- | --- | --- | --- |
| 200 | `true` | `null` | 200 | 有值 | 0 |
| 404 | `false` | `HTTP 404` | 404 | 有值 | 逐轮 +1 |
| 超时 | `false` | `超时` | `null` | `null` | 逐轮 +1 |

```console

"gh2/nf": {"checked_at":1792323544,"consecutive_fails":1,"error":"HTTP 404","http_status":404,"latency_ms":2,"ok":false}
"gh2/to": {"checked_at":1792323546,"consecutive_fails":1,"error":"超时","http_status":null,"latency_ms":null,"ok":false}

```

恢复（放回清单后）：`gh2/nf 1792323592 0 None 200 1` → **一次成功即归零**。

### 3.4 周期

| 配置 | 实测轮次间隔 | 备注 |
| --- | --- | --- |
| `interval_secs=5`，含 1s 超时频道 | **12s** | `checked_at` 3544→3556→3568 |
| `interval_secs=5`，含 3s 超时频道 | **≈13s** | 早期观测 |

→ 存在固定调度节拍；精确公式**未定**。

### 3.5 影响面

- **导出清单**：失败频道仍在 `/playlist.m3u`。
- **入口**：照常向上游请求；失败则 302 占位（与健康状态无关）。
- **缓存/订阅**：无差异；失败条目**不阻止**任何播放路径。
- **`/panel/api/status`**：无健康字段；健康仅经 `/panel/api/health` 暴露。

### 3.6 并发

6 条 0.4s 延迟频道：

```console
# concurrency=6
1792326390.160 /a  1792326390.160 /b  1792326390.162 /c  1792326390.162 /d  1792326390.163 /e  1792326390.163 /f
# concurrency=1
1792326423.051 /a  1792326423.473 /b  1792326423.880 /c  1792326424.289 /d  1792326424.695 /e  1792326425.103 /f

```

→ `=6` 并行；`=1` 串行。`=2/3` 的精确调度**未定**。

## 4. 缓存容量与淘汰

### 4.1 上限实测

配置（重启 inst/01 后确认已加载）：

| 上限 | 配置 | 实测 | 结论 |
| --- | --- | --- | --- |
| `m3u8_max_entries` | 2 | 6–7 条 | **不生效** |
| `segment_max_entries` | 2 | 17 条 | **不生效** |
| `segment_cache_max_mb` | 1 MB | `seg_bytes` 2.7 MB | **不生效** |
| `disk_max_mb` | 2 MB | `disk_bytes` 2,791,488 B（另测约 30MB） | **不淘汰** |

递进式源站（每次清单给新分片号，220KB/片）：

| 阶段 | `cache/stats` | `du -sh inst/01/cache/seg_bytes` |
| --- | --- | --- |
| 连续 20 次请求 | `disk_bytes` 2115283；`m3u8_entries` 7；`segment_entries` 14；`segment_url_entries` 20 | 2.1M |
| 再请求 + 等待 ≥20s | `disk_bytes` 2791488；`segment_entries` 17；`segment_url_entries` 23 | 2.7M |

→ 四类上限均**未触发淘汰**；触发条件或实现缺口**未定位**。配置**热重载不生效**；重启后仍不生效。

### 4.2 `POST /panel/api/cache/flush`

| 项 | 值 |
| --- | --- |
| flush 前 `du -sh inst/01/cache/dash` | 6.4M |
| 响应 | `{"ok":true}` |
| flush 后 `stats` | `dash_disk_bytes` 6444419、`disk_bytes` 0、`m3u8_entries` 0、`segment_entries` 0、`segment_url_entries` 0 |
| flush 后 `du -sh inst/01/cache/dash` | 6.4M |

→ 清 `m3u8/seg_url/seg_bytes/sub_url`；**保留 `dash/`**。
（后端切换 + 重启后 `dash/` 曾变为 0B；原因**未定位**。）

### 4.3 内存后端

- 播放 `200`（内存命中）；`stats`：`{"backend":"memory","disk_bytes":null,…,"dash_disk_bytes":0}`，各 `*_entries` **恒 0**。
- 磁盘旧文件（56K + 2.7M）**未清理** → 统计与实际占用不一致。

### 4.4 其它字段

- `cache.stats` 约 **10s** 采样一次（`poll_interval_secs=10`）；`cache.segments.retain_count` 面板注释为**已废弃**，由 `cache.dash.retain_min_secs` 动态计算。
- `whole_file_serve="always"`：每次切片请求 `whole_file_hits` +1，`whole_file_peak` ≈ 单切片字节数。
- `whole_file_budget_mb=0` → `/panel/api/status.whole_file_budget` 读回 **536870912**；非 0 值读回 **0**（字段回读**不可靠**）。

## 5. 未决 / 局限

- **URL 池取用方式不可观测**：本环境 `pool≥2` 必失败（只拉 MPD、零切片、503）；“视频 pool[0] / 音频字幕 pool[1..]”仅来自面板自述。池失败原因**未定**：无法区分目标缺陷、源站形态不兼容或环境限制。
- **多 CDN 故障切换未观测到**：只做变体级“选最高带宽”，不做失败重试；是否另有开关**未测**。
- 健康探测周期公式**未定**；`concurrency=2/3` 的调度**未定**；自定义 UA/Header 覆盖**未测**。
- 容量上限均未生效，**触发条件未定位**；`dash/` 容量与 `dash_disk_bytes` 的清理策略**未测**。
- 内存后端下计数不可用；`dash/` 变为 0B 的原因**未定**；`whole_file_budget` 回读异常，**原因未定**。
- 仅 **arm64 + 单实例 + 自建静态/递进源站**；未测真实 CDN、真实多 token 平台、录制/回看/分发缓存（`recording_*`、`timeshift_*`、`livestream_*`）。

## 6. 复现

```bash

B=http://$(docker inspect iptv82 --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}'):19901
T=$(python3 samples/work/dyn-v0.8.2/cfgctl.py tok)
C="python3 samples/work/dyn-v0.8.2/cfgctl.py"
# 1) URL 池（DASH）：把频道 url_pool_size 设为 2/3 → 源站只收到 MPD、入口 503
$C apply samples/work/dyn-v0.8.2/patch-gd4.py && $C reload
curl -s -o /dev/null -w '%{http_code}\n' "$B/gd4/e1.m3u8"
# 2) 上游失败 → 302（Proxy）
curl -s -o /dev/null -w '%{http_code}\n' "$B/gfm/m5.m3u"
curl -s -D - -o /dev/null "$B/gfm/m5.m3u" | grep -i location
# 3) 健康探测
$C apply samples/work/dyn-v0.8.2/patch-gh2.py && $C reload
curl -s -H "Authorization: Bearer $T" "$B/panel/api/health"
# 4) 缓存上限（需重启 inst/01）
$C apply samples/work/dyn-v0.8.2/patch-d12.py
docker exec iptv82 sh -c 'kill <inst/01 PID>'
docker exec -d iptv82 sh -c 'cd /app && ./iptv-proxy /app/inst/01/config.toml > /app/inst/01/run.log 2>&1'
curl -s -H "Authorization: Bearer $T" "$B/panel/api/cache/stats"
curl -s -X POST -H "Authorization: Bearer $T" "$B/panel/api/cache/flush"

```
