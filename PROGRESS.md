# PROGRESS · IPTV Proxy 逆向与重实现

> 本文件是**进度**与**功能缺口**的权威入口。
> 历史日志已迁入 [`PROGRESS_bak.md`](PROGRESS_bak.md)。
> 规范见 [`docs/spec/`](docs/spec/)。发现见 [`docs/findings/`](docs/findings/)。设计见 [`docs/design/`](docs/design/)。
> 目标版本：**IPTV Proxy v0.8.2**（上游提交 `d302316fb8`）。
> 写作风格：ASD-STE100 简化技术写作。一句一个意思。主动语态。用简单现在时。
> 最后更新：2026-10-25。

---

## 1. 目标与约束

| 项 | 内容 |
| --- | --- |
| 目标 | 逆向闭源 Linux 二进制 IPTV Proxy v0.8.2 |
| 交付 | Rust 写的 drop-in 替代服务 |
| 平台 | 只支持 `linux-arm64` 与 `linux-amd64` |
| 产物 | 静态 musl 二进制 |
| 数据纪律 | 目标派生的代码、前端资源、面板文案是不可信数据。不复制 |
| 推送 | 每次提交后推送 `git@github.com:HasonHuang/TV-Buddy.git` 的 `rust` 分支 |
| CI | 只在推送 `v*` tag 时执行 |

---

## 2. 状态总览

| 阶段 | 范围 | 状态 |
| --- | --- | --- |
| P0 | 逆向、配置、认证、代理/直连、缓存、播放链路 | 完成 |
| P1 | EPG、回看、Remux、录制、台标、分享、安全面 | 完成 |
| P2 | 订阅聚合、面板分区、EPG/字幕翻译 | 完成。DASH 回看播放路由缺口见 §3.1 |
| P3 | 目标剩余端点与兼容细节 | 完成 |
| P4 | DASH 完整链路与 DASH→HLS | 完成 |
| P5 | 低配置优化：磁盘回收、CDN 兜底、预取覆盖、被动实测推荐 | 完成。CDN 面板侧待人工配置 |

规模：

| 项 | 数量 |
| --- | --- |
| 路由注册（`src/http.rs`） | 131 |
| 测试 | 293（208 单元 + 85 契约） |
| 冒烟脚本（`scripts/smoke-*.sh`） | 25 |

已覆盖的能力：

- 六种分组模式：`proxy`、`redirect`、`rewrite`、`passthrough`、`remux`、`dash`。
- HLS、MPD、TS 与真实上游的兼容处理。
- 订阅聚合、Token、访问范围、切片伪装、切片缓存。
- EPG 采集、匹配、手动映射、额外频道、公开页与面板接口。
- EPG 翻译与字幕翻译。
- 定时录制、DASH 定时录制、本地回看、时移。
- 台标库、素材转换、分享、安全管控、备份恢复。
- 内嵌管理面板与交互式安装/管理 CLI。

---

## 3. 功能缺口

本章只写**本实现缺什么**。目标侧未验证的项不记为缺口，见 §3.4。

### 3.1 后端缺口（需要新代码）

| # | 缺口 | 现状 | 做法 |
| --- | --- | --- | --- |
| B1 ✅ | 概览页实时指标 | **已完成**（2026-10-27）。见 §3.1.2 | 新增 `src/metrics.rs` |
| B2 ✅ | `online_viewers` | **已完成**。见 §3.1.2 | 按客户端 IP 的滑动窗口 |
| B3 ✅ | 频道行绿点与 `异常` 徽章 | **已完成**。见 §3.1.2 | 新增 `src/health.rs`，前端读 `entries` |

B4 已完成。见 §3.1.1。

### 3.1.1 B4 完成记录（2026-10-26）

新增模块 `src/dash_timeshift.rs`。做法如下：

1. 按 MPD 逐轨道抓取切片原字节。不转封装，不调用 ffmpeg。
2. 落盘布局与目标一致：`cache/dash_timeshift/{gid}_{cid}/{index.json, init/, segments/}`。
3. 切片名主干 = 上游 MPD 段号。`#EXT-X-MEDIA-SEQUENCE` 取首条段号。
4. `#EXT-X-PROGRAM-DATE-TIME` 取切片到达本机的时间，UTC。
5. 清单按 `playseek` 区间筛选。窗口 = 已录制区间，不取墙钟。
6. 错误分流按目标实测：
   - 缺 `playseek` 或格式错 → `400 missing playseek`。
   - 区间不重叠、反向、相等、未知轨道、非 `dash` 组 → `404 no segments in range`。
   - 切片未命中 → `404 segment not found`。
7. 切片扩展名白名单：5 项图片扩展名 + 磁盘原名 `m4s`。init 用 `mp4`。
8. 旧 `timeshift` 模块删掉 DASH 分支（原用 ffmpeg 转 HLS），避免两条路径重复录制。

对拍结果：同一份动态 MPD 与同一时间窗，目标与本实现逐项比对 18 项，差异 1 项。差异是音轨名，见 §3.3。

质量门：302 项测试通过（215 单元 + 87 契约）。`smoke-dash-timeshift.sh` 13 项全部通过。

### 3.1.2 B1 + B2 + B3 完成记录（2026-10-27）

目标 `/panel/api/status`、`/panel/api/cache/stats`、`/panel/api/health` 已实测（容器 `rea82v09`）。

新增两个模块：

| 模块 | 职责 |
| --- | --- |
| `src/metrics.rs` | 出网字节、在线观众窗口、进程 RSS 与 CPU、磁盘占用采样 |
| `src/health.rs` | 逐频道上游探测与结果缓存 |

实现要点：

1. `/panel/api/status` 补齐目标字段：`mem_rss_mb`、`cpu_seconds`、`cpu_cores`、`disk_cache_mb`、`recording_mb`、`low_resource_mode`、`poll_interval_secs`、`traffic_in_bytes`、`traffic_out_bytes`、`whole_file_*`、`online_viewers`、`active_sessions`、`group_count`、`channel_count`。
2. `traffic_in_bytes` 固定 0。目标同样恒 0，不修（S-BUG-01）。
3. `traffic_out_bytes` 由中间件累计。响应体边发边计数，不缓冲正文。
4. `online_viewers` = 窗口内**带播放 Token 的不同客户端 IP 数**。实测依据：
   - 同 IP 重复播放不增；
   - 换 `X-Forwarded-For` 增 1；
   - `token_enabled=false` 时恒 0。
5. `active_sessions` = 面板会话数。实测：登录 1 次为 1，播放请求不改变。
6. `entry_exts` 是 `gid → ".m3u8" | ".mpd"` 的映射，不是固定两项。
7. `/panel/api/cache/stats` 补齐目标字段：`disk_bytes`、`m3u8_entries`、`segment_entries`、`segment_url_entries`、`sub_url_entries`、`dash_disk_bytes`、`timeshift_disk_bytes`、`recording_disk_bytes`、`livestream_disk_bytes`。
8. `/panel/api/health` 返回 `{enabled, entries{"<gid>/<cid>": {checked_at, consecutive_fails, error, http_status, latency_ms, ok}}}`。
9. 探测按 `[health]` 的 `interval_secs`、`timeout_secs`、`concurrency` 运行。连接失败时 `error="连接失败"`，`latency_ms=null`。两者与目标实测一致。
10. 前端接线：
    - 分组页频道行按 `entries` 画绿点与 `异常`。
    - 系统页新增「频道探测」卡片。
11. 顺带对齐 `GET /panel/api/security/token` 与 `POST /panel/api/security/token/rotate` 的目标字段：
    - 前者补 `current_value`、`status.current/old`（掩码值 + 剩余秒数）。
    - 后者补 `new_token`、`expires_in_hours`。
    - 旧字段保留，面板与旧调用方不受影响。

对拍结果：三张接口的字段集合比对，目标字段缺失 0 项。本实现另外保留若干附加字段，供本地面板使用。

质量门：309 项测试通过（222 单元 + 87 契约）。`pnpm build` 通过。

### 3.2 面板未接线（端点已在，前端无调用）

下表由静态检索得出。前端用模板字面量拼出的路径已人工核对，不计入缺口。

已接线（2026-10-27）：

| 端点 | 面板位置 |
| --- | --- |
| `GET /panel/api/dash/status` | 系统页「DASH 会话」卡片 |
| `GET /panel/api/dash/tracks/:gid/:cid` | 分组页 DASH 频道的「轨道」按钮（弹窗） |
| `POST /panel/api/dash/scan-url` | 系统页「DASH 会话」卡片的「扫描轨道」 |
| `GET /panel/api/groups/:gid/export` | 分组卡片工具栏「导出」 |
| `POST /panel/api/resolver/remove` | 系统页「解析脚本」的「删除」 |
| `POST /panel/api/security/ip/unban-batch` | 安全页「批量解封」 |
| `POST /panel/api/security/ip/ban`（多地址） | 安全页支持逗号或换行分隔多个地址 |

仍未接线：

| 端点 | 面板应做的事 |
| --- | --- |
| `POST /panel/api/assets/convert` | 素材转换：ffmpeg 可用时转换上游地址。前端只显示 ffmpeg 状态 |
| `GET /panel/api/epg/match-report` | 显示匹配报告。前端只调台标的 `/logo/match-report`。**注意**：目标是 `text/plain` 表格，面板 JS 期望 JSON。见 §3.3 |
| `GET/POST /panel/api/epg/{scripts,script,script-delete}` | 管理 EPG 采集脚本 |
| `GET /panel/api/epg/suggest/:gid/:cid` | 频道匹配建议 |
| `GET /panel/api/epg/source-channels/:id` | 列出某个源的频道 |
| `GET /panel/api/epg/tz-preview/:id` | 时区预览 |
| `GET /panel/api/epg/viewer-channels` | 公开查看器的频道集合 |
| `GET /panel/api/epg/translate/{models,probe}` | 列出翻译模型、探测引擎 |
| `GET /panel/api/link-info` | 审计播放链接与 Token 剩余期 |
| `GET /panel/api/subs/status` | 显示订阅源状态。前端只调 `/subs/preview` 与 `/subs/groups` |
| `GET /panel/api/logo/list` | 带鉴权的台标列表。前端走公开的 `/logo/api/list` 并自行补报告 |
| 分组工具栏「多选」 | 目标有多选与批量操作。本实现没有 |

### 3.3 已登记差异与不实现项

| 项 | 结论 | 依据 |
| --- | --- | --- |
| `<P>` 算法 | 目标算法未解。不改用其它哈希。四条可观测性质全部对齐 | F-017 §5.2、`06` §12 |
| 扩展名轮换顺序 | 只保证取自 `[cache.disguise].formats`。目标序列未解 | F-018、F-019 |
| DASH 音轨名 | 目标是 `audio-{语言}`（如 `audio-en`），同语言加 `-1`；无语言才用 `audio-{序号}`。本实现统一用 `audio-{序号}` | F-016 §5.1 |
| DASH 文字轨出口 | 目标在实测夹具里不把文字轨写进 master。本实现输出 `SUBTITLES` 组 | F-016 §5.1 |
| Remux 后缀 | 本实现轮换 5 种。目标是 `.png/.gif/.bmp/.jpg/.webp` 轮换 | F-014 |
| `out_groups`/`dropped_groups` 顺序 | 目标是哈希序。本实现是稳定排序 | F-020 |
| `match-report` 形状 | 保留目标的 `text/plain` 表格。面板 JS 期望 JSON。冲突未解 | F-013 |
| 缓存容量上限 | 不实现。目标配置存在，但未观测到生效 | F-015 |
| 旧数据迁移 `./var/dash` | 不实现。只有 v0.8.8 有这个行为 | F-015 |
| DASH 多 CDN 故障切换 | 未复刻。只做顺序回退 | F-015 |
| CENC/ClearKey 解密 | 不实现。按原字节代理 | F-009 |
| `[auth]` 的 `l1/l2` 限流、违规计数、升级封禁（13 个键） | 键面与默认值对齐目标。**不消费**。触发条件未复现 | F-010 |
| `[auth].whitelist`、`max_sessions_per_ip`、`session_ttl_secs` | 不消费。目标未观测到行为。界面标"未消费" | F-010 |
| `random_id_enabled` | 只回显。行为未观测 | F-018 |
| `[share].share_token` | 只回显。没有消费点 | F-016 |
| `cache.backend=memory` | 只回显。实际仍落盘 | F-015 |
| `[subtitle].style.*` 样式注入 | 保留键面。运行时不注入 VTT | `05` §4.1 |
| 播放侧 `compat_mode`、`srt_ua_keywords` | 不实现。目标未测出行为 | D19 |
| `[cache].segment_cache_max_mb`、`m3u8_max_entries`、`segment_max_entries` | 不消费。目标同样未生效。默认值非 0，生效会静默压容 | S-CACHE-03、`04` §11.5 |
| 面板语言（i18n） | 已定：不做。目标是真 i18n，我们只有中文文案 | `07` §3.C4 |
| 素材「未被引用」判定 | 本实现自定：配置里没有字符串指向 `/assets/<文件名>`。只标不删 | `05` §4.1 |

> `[cdn]`、`[cache.gc]`、分组 `prefetch` 是本实现新增的配置面。目标 v0.8.2 没有这些键。默认值不改变既有行为。

### 3.4 待补目标证据

| ID | 项 | 当前结论 |
| --- | --- | --- |
| V-01 | URL 池回退顺序 | 目标 `url_pool_size>1` 时曾失败并产生零切片。无法继续对拍 |
| V-08 | `<P>` 算法 | **已收口**。不再追求字节一致。只对齐可观测性质 |
| V-08a | 切片名文法与占位分流 | **已对齐**。文法、取值校验、`Content-Type` 白名单、入口后缀白名单都已对拍 |
| V-10 | 面板逐屏视觉与交互 | API 对拍 22/22 差异 0。13 个一级分区已对齐。逐屏视觉对拍未做 |
| — | `dash-timeshift/status` 的 `hours` | 本实现对 `dash_timeshift` 频道也返回 `timeshift_hours`。目标用哪个键未验证 |
| — | U-06 其余 5 种台标效果 | 只有 `breathe` 逐字节一致。其余为近似 |

---

## 4. 质量门

2026-10-25 实测：

| 项 | 结果 |
| --- | --- |
| `cargo fmt --check` | 通过 |
| `cargo test --offline --all-targets` | 293 项通过（208 单元 + 85 契约） |
| `cargo clippy --offline --all-targets` | 0 error，27 条历史 warning |
| 冒烟脚本 | 25 个全部通过。`smoke-record-dash` 需要 ffmpeg，且要求端口 19970/19971 空闲 |
| `pnpm build` | 通过（含 `tsc --noEmit`） |
| 发布校验 | 上一轮双架构产物通过 `verify-release.sh`。本轮未重跑 |
| CI | 只在 `v*` tag 触发。fmt/clippy/test、CLI 冒烟、双架构校验已配置 |

---

## 5. 关键设计结论

EPG：

- `out_id = tvg_id || channel.id`。
- 匹配顺序：手工映射 → `tvg_id` → 名称。
- 名称规范化：删除全部空白，转小写。
- 手工映射存 `<cache>/epg/mappings.json`。额外频道存 `<cache>/epg/extra.json`。
- 源正文缓存到 `<cache>/epg/raw/<source_id>.xml`。`regenerate` 只读 raw 缓存。
- `public_viewer_enabled=false` 时，公开 `/epg` 返回 `404`。

DASH：

- 轨道命名：`video_{高}p`、`audio-{n}`、`text-{n}`。
- 媒体清单路径：`/{gid}/{cid}/{track}/index.txt`。
- 媒体窗口取 `[cache.dash].playlist_min_secs`。分组 `mpd_keep_seconds > 0` 时优先。
- 直播清单按 `start_time_offset_secs` 输出 `#EXT-X-START:TIME-OFFSET=-…`。
- CENC/ClearKey 不解密，按原字节代理。

伪装与切片：

- `<S>` = `sha256(上游切片绝对 URL)` 的前 32 位十六进制。
- `<P>` = 16 位十六进制，只依赖 `(gid,cid)`。算法用域标签加长度前缀。
- 切片名文法：`<P>_<S>[.<ext>]`。扩展名可省。最多一个点。恰好一个下划线。
- `<P>` 与 `<S>` 必须定长小写十六进制。
- 切片 `Content-Type` 用 5 项白名单。其余一律 `video/mp2t`。
- 入口后缀白名单：`.m3u8`、`.mpd`、`.txt`、无后缀。四者正文相同。
- 占位分流：切片校验失败 → `error_url`。分组或频道不存在、非 `proxy` 模式子路径 → `not_found_url`。
- 切片正文缓存与扩展名解耦。同一 `(P,S)` 只存一份。

订阅：

- `group_map` 是选择集。`group_mode` 未知值按 `keep`。
- 源正文落盘 `<cache>/sub_url/`。状态在 `<cache>/subs/status.json`。
- `ttl_secs=0` 按 3600 秒。源级 TTL 优先于 `merge_ttl_secs`。
- 聚合按 URL 去重，覆盖同源与跨源。

缓存与低配：

- 推荐 TTL = `clamp(ceil60(W + 2T), 300, 900)`。取各分组最大值。无实测时回退 600 秒。
- 磁盘上限 = `clamp(min(工作集, 可用空间×0.8 − 100MB), 50MB, 总容量×0.7)`。
- 工作集 = `round10(3 × N × seg_mb × 1.5)`，按十进制 MB 计算。
- `statvfs` 取 `f_bavail` 与 `f_frsize`。失败时回退 150 MB。
- GC 淘汰顺序：超龄 → 超量 mtime LRU → 可用空间低于预留时紧急清理。
- 磁盘索引按登记时间截断。`per_channel_max=0` 时用内置 4096。
- 预取合并顺序与 `proxy_for` 相同：频道 > 分组 > 全局。
- 估算统计落盘 `<cache>/cache_estimate.json`。带版本号。落盘由 GC tick 节流。
- `[cdn].enabled=false`（默认）时，切片响应头、空体状态码与 `self_base` 推导逐字节等于旧行为。

哈希选型：

- 全部保持 SHA-256。不做非加密哈希替换。
- 理由：一次清单刷新只算 1 个 `<P>` 加 7–20 个 `<S>`，合计 2–15 µs。1 vCPU 下不可观测。
- 详见 `06` §12。

---

## 6. 推进顺序

1. **面板未接线**（§3.2）：只差接线，可分批做。
2. **T0 与真机验收**：在 CDN 面板按 `06` §2 配五处。在目标 VPS 上跑 S4/S6。
3. 每项完成后运行完整质量门。更新本文件与对应 `docs/`。提交并立即推送 `rust` 分支。

---

## 7. 文档入口

| 主题 | 路径 |
| --- | --- |
| 目标规范 | [`docs/spec/08-target-spec.md`](docs/spec/08-target-spec.md) |
| 验证矩阵 | [`docs/spec/07-baseline-and-verification.md`](docs/spec/07-baseline-and-verification.md) |
| 全量覆盖对比与规划 | [`docs/design/07-coverage-and-plan.md`](docs/design/07-coverage-and-plan.md) |
| HTTP 契约 | [`docs/design/03-http-contract.md`](docs/design/03-http-contract.md) |
| 配置与存储 | [`docs/design/04-config-and-storage.md`](docs/design/04-config-and-storage.md) |
| 低配置优化 | [`docs/design/06-low-spec-optimization.md`](docs/design/06-low-spec-optimization.md) |
| 低配置示例 | [`docs/design/config-low-spec.toml`](docs/design/config-low-spec.toml) |
| 架构 | [`docs/design/02-architecture.md`](docs/design/02-architecture.md) |
| 面板字段清单 | [`docs/design/05-frontend.md`](docs/design/05-frontend.md) |
| 发现索引 | [`docs/findings/README.md`](docs/findings/README.md) |
| 历史进度 | [`PROGRESS_bak.md`](PROGRESS_bak.md) |

---

## 8. 归档

2026-10-25，本文件按 ASD-STE100 重写。旧内容全部迁入 [`PROGRESS_bak.md`](PROGRESS_bak.md)。

归档含：

- P0 至 P4 的逐项完成记录。
- 每轮实现、对拍、测试与发布细节。
- 旧任务编号与验证命令。
- 2026-10-25 之前的续接快照。

本文件只保留：目标与约束、状态总览、功能缺口、质量门、设计结论、推进顺序、文档入口。
