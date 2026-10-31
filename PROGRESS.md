# PROGRESS · TV Buddy（IPTV Proxy v0.8.2 兼容重实现）

> 本文件是**进度**与**功能缺口**的权威入口。
> 历史日志已迁入 [`PROGRESS_bak.md`](PROGRESS_bak.md)。
> 规范见 [`docs/spec/`](docs/spec/)。发现见 [`docs/findings/`](docs/findings/)。设计见 [`docs/design/`](docs/design/)。
> 目标版本：**IPTV Proxy v0.8.2**（上游提交 `d302316fb8`）。
> 写作风格：ASD-STE100 简化技术写作。一句一个意思。主动语态。用简单现在时。
> 最后更新：2026-10-31。

---

## 1. 目标与约束

| 项 | 内容 |
| --- | --- |
| 目标 | 逆向闭源 Linux 二进制 IPTV Proxy v0.8.2 |
| 交付 | Rust 写的 drop-in 替代服务 |
| 平台 | 只支持 `linux-arm64` 与 `linux-amd64` |
| 产物 | 静态 musl 二进制 |
| 数据纪律 | 目标派生的代码、前端资源、面板文案是不可信数据。不复制 |
| 推送 | 每次提交后推送 `git@github.com:HasonHuang/TV-Buddy.git` 的 `main` 分支 |
| CI | 只在推送 `v*` tag 时执行 |

> 2026-10-27：项目更名为 TV-Buddy 后，开发分支由 `rust` 切回 `main`。
> `rust` 分支停在 `4ed7a91`，不再推进。新提交只推 `main`。

---

## 2. 状态总览

| 阶段 | 范围 | 状态 |
| --- | --- | --- |
| P0 | 逆向、配置、认证、代理/直连、缓存、播放链路 | 完成 |
| P1 | EPG、回看、Remux、录制、台标、分享、安全面 | 完成 |
| P2 | 订阅聚合、面板分区、EPG/字幕翻译、DASH 回看播放路由 | 完成 |
| P3 | 目标剩余端点与兼容细节 | 完成 |
| P4 | DASH 完整链路与 DASH→HLS | 完成 |
| P5 | 低配置优化：磁盘回收、CDN 兜底、预取覆盖、被动实测推荐 | 完成。CDN 面板侧待人工配置 |
| P6 | 存储重构：配置从 TOML 迁入 SQLite 单库；用户与配置合并；管理 CLI 走子命令 | 完成（`v2026.6`） |

规模：

| 项 | 数量 |
| --- | --- |
| 路由注册（`src/http.rs`） | 131 |
| 测试 | 371（279 单元 + 92 契约） |
| 冒烟脚本（`scripts/smoke-*.sh`） | 26 |

已覆盖的能力：

- 六种分组模式：`proxy`、`redirect`、`rewrite`、`passthrough`、`remux`、`dash`。
- HLS、MPD、TS 与真实上游的兼容处理。
- 订阅聚合、Token、访问范围、切片伪装、切片缓存。
- EPG 采集、匹配、手动映射、额外频道、公开页与面板接口。
- EPG 翻译与字幕翻译。
- 定时录制、DASH 定时录制、本地回看、时移。
- 台标库、素材转换、分享、安全管控、备份恢复。
- 内嵌管理面板与交互式安装/管理 CLI。
- 单文件存储：配置、用户、令牌在 `tv-buddy.db`。运行时不再读写 TOML。

---

## 3. 功能缺口

本章只写**本实现缺什么**。目标侧未验证的项不记为缺口，见 §3.4。

### 3.1 后端缺口（已清空）

当前没有需要新代码的后端缺口。B1–B4 的完成记录留在 §3.1.1 与 §3.1.2。

尚未实现的可选能力见 §6 的优先级清单。

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

对拍结果：同一份动态 MPD 与同一时间窗，目标与本实现逐项比对 18 项，差异 1 项。差异是音轨名，已于 2026-10-28 收口（P3-1）。

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

### 3.1.3 P1 完成记录（2026-10-27）

`url_pool_size>1` 与多 `<BaseURL>` 的场景原来只有一处缺陷：池大小没有生效。
`choose_urls()` 用地址条数当池大小，`4` 个 `<BaseURL>` 配 `url_pool_size=2` 时仍会用满 4 个。
抓取循环也有问题。每个地址先跑满 `retry_max`（默认 8，退避到 2 秒）才换下一个地址。

改法是三点：

1. `choose_urls()` 把池大小当候选上限。N = min(地址数, `url_pool_size`, 6)。
   视频固定 `pool[0]`；音频/字幕在 `pool[1..N]` 轮询。
2. 新增纯函数 `attempt_plan()`。池启用时首轮逐址各试一次，失败立即换址，轮末才退避。
   总尝试数 = max(`retry_max+1`, 成员数)，不随成员数相乘膨胀。
3. 视频轨记住最近成功的上游目录前缀，300 秒内优先从该地址起步。
   看门狗重建时清空。键用目录前缀而不是整条 URL：切片名每个都不同，
   直接比 URL 永远不相等。

两项配套约束：

- 池启用时首轮单址超时降到 3 秒。黑洞型上游不报错，只会挂满 `segment_timeout_secs`。
- `url_pool_size<=1` 或单 `<BaseURL>` 时走旧路径，抓取序列逐项不变。

对拍结果：无法对拍。目标在 `pool>=2` 时只拉 MPD 后零切片、入口 503（F-015 §1.2）。
本实现不复刻该缺陷。语义记为自定，写在 `07` §E。

质量门：322 项测试通过（234 单元 + 88 契约）。

### 3.1.4 P2-1 完成记录（2026-10-28）

`[auth]` 的三个键原来只回显、不参与判定：`whitelist`、`max_sessions_per_ip`、`session_ttl_secs`。

新增模块 `src/auth/session_guard.rs`。规则是自定的，因为目标未观测到任何生效行为（F-010）：

| 项 | 规则 |
| --- | --- |
| 会话 | 一个会话 = `(客户端 IP, 频道)` |
| 续期 | 每次播放请求续期。超过 `session_ttl_secs` 没有请求即过期 |
| 超限 | 同一 IP 的会话数超过 `max_sessions_per_ip` 时，踢掉最久未用的那路 |
| 被踢方 | 在 TTL 内再请求同一频道 → `302 limit_url` |
| 白名单 | 精确地址、IPv4 CIDR、`*`。命中后豁免封禁与并发上限 |
| 开关 | 三项都只在 `ip_guard_enabled = true` 时生效 |

挂载点两处：

1. `src/http/guard.rs` 的中间件：白名单豁免封禁。
2. `src/http/routes/stream.rs` 的 `entry` 与 `wildcard`：播放会话判定。

为什么默认行为不变：`ip_guard_enabled` 默认 `false`，`session_limit()` 直接返回 `None`，
中间件的白名单判定也短路。所以默认配置下的请求路径与改动前逐项相同。

不做的事：

- `ban_url` 仍未消费。封禁仍返回 `403`，与今天一致。见 §3.1.5。
- 会话只在单进程内存里。多进程部署各自计数，与目标一致（目标也没有共享存储）。

质量门：331 项测试通过（242 单元 + 89 契约）。

### 3.1.5 P2-2 完成记录（2026-10-28）

`[auth]` 里与限流、违规、升级封禁相关的 13 个键原来只回显。目标侧没有复现过触发条件
（F-010），所以规则同样是自定的。

新增模块 `src/auth/rate_limit.rs`：

| 项 | 规则 |
| --- | --- |
| 计数范围 | **只计播放入口请求**（`/{gid}/{cid}` 的清单入口），不计切片 |
| L2 | 突发上限，默认 8 次 / 30 秒 |
| L1 | 持续上限，默认 30 次 / 30 秒 |
| 超限 | 冷却 `cooldown_secs` 秒，返回 `429` + `Retry-After`，并记一次违规 |
| 违规 | 在 `violation_window_secs` 内有效；每满 `violation_decay_secs` 失效一条 |
| 升级 | 有效违规达到 `ban_threshold` → 封禁 `ban_duration_secs`，计数清零 |

限流只限入口的理由：切片的频率由码率决定，限切片等于限画质；限入口等于限换台频率，
那才是滥用信号。按默认值算，正常播放不会触发。

登录锁也加了阶梯。原来固定 300 秒，现在：

| 触发次序 | 时长 | 键 |
| --- | --- | --- |
| 第 1 次 | 300 秒 | `auth_fail_lock_secs` |
| 第 2 次 | 1 小时 | `auth_fail_ban_duration_secs` |
| 第 3 次及以后 | 30 天 | `auth_fail_ban_repeat_duration_secs` |

第 2 次起要求在 `auth_fail_ban_escalate_window_secs` 窗口内重复触发。

全部新行为都在 `ip_guard_enabled = true` 时生效。该键默认 `false`，所以默认配置下
限流不计数、登录锁仍是固定 300 秒。

仍未消费：`ban_url`。封禁仍返回 `403`。

质量门：338 项测试通过（248 单元 + 90 契约）。

### 3.1.6 P2-3 完成记录（2026-10-28）

`[auth].random_id_enabled` 原来只回显。

目标侧唯一的正面证据是**否定**的：开启后入口路径与切片的 16 位前缀都没变（F-010 §4）。
所以它不作用于播放路径。按「random id」的字面含义与那条证据，落到导入环节：

| 项 | 规则 |
| --- | --- |
| 关闭（默认） | 导入 M3U 时频道 id 取 `tvg-id`；缺失时 `g-<6hex>` |
| 开启 | 忽略 `tvg-id`，每个新频道取 `r-<8hex>` |
| 已有频道 | 不变。所以切片前缀、索引文件名、已下发地址都不受影响 |

这与观测一致：开关只改变**新导入**频道的 id，既有频道的播放路径逐项不变。

质量门：340 项测试通过（249 单元 + 91 契约）。

### 3.1.7 P2-5 完成记录（2026-10-28）

`[cache].backend` 原来只回显，实际仍写磁盘。

新增 `src/cache_memory.rs`。`backend = "memory"` 时：

| 项 | 规则 |
| --- | --- |
| 切片正文 | 留在进程内的字典，键为 `gid/cid/suffix`，不写 `seg_bytes` |
| 上限 | `[cache].memory_max_mb`；`0` 按 512 MB 处理 |
| 淘汰 | 最近最少使用。单条超过上限时不存 |
| 清空 | `POST /panel/api/cache/flush` 一并清空，计入返回的释放字节 |
| 重启 | 全部丢失。这与磁盘后端不同，面板文案已写明 |

不做的事：索引与清单仍按原路径处理。`backend` 只影响切片正文。

质量门：347 项测试通过（255 单元 + 92 契约）。

### 3.2 面板接线（已完成）

原清单由静态检索得出。2026-10-27 全部接线完成，逐项落点如下。

| 端点 | 面板位置 |
| --- | --- |
| `GET /panel/api/dash/status` | 系统页「DASH 会话」卡片 |
| `GET /panel/api/dash/tracks/:gid/:cid` | 分组页 DASH 频道的「轨道」按钮（弹窗） |
| `POST /panel/api/dash/scan-url` | 系统页「DASH 会话」卡片的「扫描轨道」 |
| `GET /panel/api/groups/:gid/export` | 分组卡片工具栏「导出」 |
| `POST /panel/api/resolver/remove` | 系统页「解析脚本」的「删除」 |
| `POST /panel/api/security/ip/unban-batch` | 安全页「批量解封」 |
| `POST /panel/api/security/ip/ban`（多地址） | 安全页支持逗号或换行分隔多个地址 |
| `POST /panel/api/assets/convert` | 素材页「远程转换」卡片 |
| `GET /panel/api/epg/{scripts,script,script-delete}` | EPG 页「采集脚本」卡片 |
| `GET /panel/api/epg/match-report` | EPG 页「匹配报告」卡片（原样显示表格） |
| `GET /panel/api/epg/suggest/:gid/:cid` | EPG 页「匹配工具」的建议表 |
| `GET /panel/api/epg/source-channels/:id` | EPG 页「匹配工具」的源频道浏览 |
| `GET /panel/api/epg/tz-preview/:id` | EPG 页「匹配工具」的时区预览 |
| `GET /panel/api/epg/viewer-channels` | EPG 页「公开查看器频道」卡片 |
| `GET /panel/api/epg/translate/{models,probe}` | EPG 页「翻译模型」卡片 |
| `GET /panel/api/link-info` | 安全页「播放链接审计」卡片 |
| `GET /panel/api/subs/status` | 订阅页每个源的状态行 |
| 分组工具栏「多选」 | 分组卡片复选框 + 「批量导出」「批量删除」（语义自定，见 §3.3） |

已确认等价、不算缺口：

| 端点 | 说明 |
| --- | --- |
| `GET /panel/api/logo/list` | 与公开的 `/logo/api/list` 返回**同一份** `list_body`；面板走公开入口即可 |

仍未接线：无。

### 3.3 已定案差异（15 项：9 项已实现或保留，6 项未实现）

下表是**与目标不完全一致、但已定案**的项。可以再做、但排在后面的能力见 §6。

状态词：

- `✅` 本实现已有对应代码，功能可用。与目标的差异只在算法或取值，不在行为。
- `❌` 本实现没有对应代码，且已决定不做。

| 项 | 状态 | 结论与实现位置 |
| --- | --- | --- |
| `<P>` 算法 | ✅ | 目标算法未解，本实现用自研算法：`sha256(域标签 ‖ len(gid) ‖ gid ‖ len(cid) ‖ cid)` 取前 16 位。四条可观测性质已对齐。`src/proxy/media.rs:58` |
| 伪装扩展名轮换顺序 | ✅ | 取自 `[cache.disguise].formats`，按序循环。目标序列未解，不影响功能。`src/http/routes/stream.rs:497`、`src/dash_hls.rs:606` |
| Remux 伪装后缀集合 | ✅ | 轮换 5 种图片扩展名，按 `seq % 5` 取值。目标具体序列未解。`src/remux.rs:33`、`src/remux.rs:256` |
| `out_groups`/`dropped_groups` 顺序 | ✅ | 本实现稳定排序，目标是哈希序。只影响展示顺序。`src/subs.rs:186` |
| 旧数据迁移 `./var/dash` | ❌ | 未实现。只有目标 v0.8.8 有这个行为。本实现直接读 `cache/` 新布局 |
| CENC/ClearKey 解密 | ✅ | 无解密代码，按原字节转发。目标也不解密，行为一致。F-009 |
| 面板语言（i18n） | ❌ | 未实现。目标是真 i18n，本实现只有中文文案。已决定不做。`07` §3.C4 |
| 素材「未被引用」判定 | ✅ | 自定：配置里没有字符串指向 `/assets/<文件名>`。只标不删。`frontend/src/assets.tsx:257` |
| 分组「多选」的批量语义 | ✅ | 自定：批量导出（合成 JSON 数组）与批量删除。目标语义未观测。`frontend/src/groups.tsx:409`、`:428` |
| `traffic_in_bytes` 恒 0 | ✅ | 固定输出 0。复刻目标缺陷，不要"修"。`src/http/routes/panel.rs:134`。S-BUG-01 |
| `[share].share_token` | ❌ | 未实现。实测 `share_token=false` 时 `/api/share/pull` 载荷里仍有 Token（F-013），所以该键不控制载荷。面板文案「同时分享 token」与实际行为不符，语义未知，不猜 |
| 缓存容量上限（3 键） | ❌ | 未实现。目标侧 `segment_cache_max_mb`、`m3u8_max_entries`、`segment_max_entries` 均未观测到生效（S-CACHE-03）。复刻「不生效」才与目标一致；真正的上限由 `[cache.gc]` 与 `[cache.segments]` 提供 |
| `[subtitle].style.*` 样式注入 | ❌ | 未实现。本实现没有面向播放器的字幕出口，注入样式无处生效 |
| 播放侧 `compat_mode`、`srt_ua_keywords` | ❌ | 未实现。目标未测出行为（D19），不猜 |
| DASH 文字轨出口 | ✅ 保留 | 目标夹具里不写 master。本实现输出 `SUBTITLES` 组，是超集，不影响播放 |

> `[cdn]`、`[cache.gc]`、分组 `prefetch` 是本实现新增的配置面。目标 v0.8.2 没有这些键。默认值不改变既有行为。

### 3.4 待补目标证据（仅验证类）

下表是**只缺证据、不缺代码**的项。要补代码的项见 §6。

| ID | 项 | 当前结论 |
| --- | --- | --- |
| V-10 | 面板逐屏视觉与交互 | API 对拍 22/22 差异 0。2026-10-29 完成逐屏对拍（P3-6）。页头骨架、侧栏、13 个分区可达性一致。版式栅格与 4 项功能入口不一致，见 §3.7 与 [`docs/tests/runs/2026-10-29.md`](docs/tests/runs/2026-10-29.md) |
| — | `dash-timeshift/status` 的 `hours` | 本实现统一返回 `hours`。值按频道类型取 `dash_timeshift_hours` 或 `timeshift_hours`。目标的键名未验证（P3-7）。方案见 [`08-open-decisions.md`](docs/design/08-open-decisions.md) §3 |
| — | U-06 其余 5 种台标效果 | 只有 `breathe` 逐字节一致。其余为近似 |

### 3.5 面板外观与安装行为（2026-10-29）

本节记录一次面向目标版式的外观重做，以及一键安装的默认行为调整。

| 项 | 结论 | 位置 |
| --- | --- | --- |
| 配色 | shadcn/ui 风格：中性 zinc 灰阶打底，主色保留青绿 `#0f8f7f`，卡片白底 1 px 细边，页面底色 `#fafafa` | `frontend/src/style.css` |
| 图标 | 自绘 16×16 描边图标集，随文字颜色取色。不引第三方图标库 | `frontend/src/icons.tsx` |
| 页头 | 全部一级分区改为「图标 + 标题 + 副标题 + 右侧操作」的 `.page-head`，标题移出卡片 | `frontend/src/ui.tsx` |
| 分组页版式 | 分组头一行放名称、`[id]`、模式徽章、频道数、操作；频道逐行放状态点、名称、上游与本地地址、徽章 | `frontend/src/groups.tsx` |
| 频道字段 | 弹窗补齐 `logo`、`tvg_id`、`tvg_name`、`resolver_*`、回看、代理与请求头。这些键落在 `Channel.extra`，后端已消费 | `src/playlist.rs:87`、`src/resolver.rs:42` |
| 分组字段 | 弹窗补齐 `format`、`headers`、`proxy`。`access` 与 `mode` 用中文标签 | `src/config/model.rs:876` |
| 分组卡拖动把手 | ✅ 已实现（2026-10-29）。分组头左侧六点把手可拖动重排，落点画虚线框。顺序写回配置。上移/下移按钮保留 | `frontend/src/groups.tsx` |
| 分组 EPG 地址 | 未做。本实现没有读分组级 `epg_url`（EPG 走 `[[epg.sources]]`），加字段会是无出口的开关 | `src/epg.rs` |
| 安装后启动 | `install.sh` 默认启动服务；`--no-start` 关闭。启动失败只告警，不判安装失败 | `install.sh` |

2026-10-30 再做一轮视觉与交互打磨，风格取 shadcn/ui：

| 项 | 结论 | 位置 |
| --- | --- | --- |
| Token 层 | 灰阶改 zinc；圆角 10 px / 控件 8 px；统一焦点环与过渡时长 | `frontend/src/style.css` |
| 控件 | 输入框白底 + 细边框；按钮高 34 px；徽章改 6 px 圆角 | 同上 |
| 表格 | 表头去底色；行 hover 高亮 | 同上 |
| 日志区 | 深色终端底；按级别着色（INFO/WARN/ERROR） | `style.css`、`ops.tsx` |
| 登录页 | 网格背景 + 中心光晕 | `style.css` |
| 页脚 | 自研品牌行。不复制目标文案 | `panel.tsx` |
| 侧栏底部 | 显示版本与运行时长 | `panel.tsx` |
| 指标卡 | 概览与热度页的卡片加图标 | `panel.tsx`、`cache.tsx` |
| 行内代码 | 界面文案里的配置键改用 `code` 标签，不再显示反引号 | 多个页面 |
| 表单排布 | 录制表单改双行紧凑排布；卡片头允许换行 | `schedule.tsx`、`style.css` |
| 缺标题的卡片 | 播放 Token、EPG 状态、缓存目录、聚合设置补标题 | 对应页面 |

### 3.8 小屏适配（2026-10-30）

样式表原来没有任何媒体查询。窄屏下侧栏固定 224 px，内容区被挤压。
现在加两个断点：

| 断点 | 行为 |
| --- | --- |
| ≤1080 px | 侧栏收成 64 px 图标栏。品牌文字、导航标签、侧栏底部信息隐藏。图标带 `title` 提示 |
| ≤720 px | 侧栏变顶部横向导航条，可横向滑动。内容区全宽。页头按钮换行。分组头与频道操作换行。宽表格横向滚动。模态框内边距收窄 |

配套修 3 处会撑破布局的地方：

1. `settings.tsx` 的栅格 `minmax(380px, 1fr)` 在窄屏固定 380 px。
   改为 `minmax(min(380px, 100%), 1fr)`。
2. 网格与弹性子项默认 `min-width: auto`，被长内容撑开。窄屏统一置 0。
3. 设置页「能力」卡的 ffmpeg 徽章带完整版本串，`white-space: nowrap` 撑到 430 px。
   改为徽章只写「已安装」，版本另起一节。

验证：1024、768、390 三档宽度下 `document.scrollWidth` 均等于视口宽。
13 个分区逐屏截图复核。

### 3.6 手工验收测试（2026-10-29）

新增 `docs/tests/`，存放**由项目负责人手工执行**的验收测试。共 6 份文档与 1 份索引。

| 文档 | 覆盖 | 对应进度项 |
| --- | --- | --- |
| `01-install-and-start.md` | 一键安装、服务注册与启动、面板可达、CLI | — |
| `02-panel-acceptance.md` | 13 个分区的元素存在性、顺序与尺寸 | P3-6、V-10 |
| `03-playback-modes.md` | 六种模式端到端播放、DASH 轨道、回看 | — |
| `04-security.md` | 会话上限、两级限流、违规封禁、白名单、登录锁、Token、随机 id | P2-1、P2-2、P2-3 |
| `05-low-spec-acceptance.md` | S4 磁盘回收与 S6 低配 24 小时 | E2、S4、S6 |
| `06-cdn-console.md` | T0 五处配置与命中验证 | E1、T0 |

约束：

- 判据只用「通过 / 不通过 / 未验证」三种取值。
- 记录落在 `docs/tests/runs/<日期>.md`。已有 [`2026-10-29.md`](docs/tests/runs/2026-10-29.md)（P3-6 逐屏对拍）。
- 截图脚本：`scripts/panel-shots.js`。用完不提交图片，只提交结论。
- 文档内所有命令、端点与配置键都按源码核对过。01 与 02 的关键路径在本机独立实例上实跑过。

不覆盖的内容：目标侧的像素比对、浏览器兼容性、真实付费上游。

### 3.7 P3-6 对拍发现的功能缺口（2026-10-29）

逐屏对拍的方法与全部结果见 [`docs/tests/runs/2026-10-29.md`](docs/tests/runs/2026-10-29.md)。
本节只登记**功能面**的缺口。版式与装饰差异见该文档 §6，不在此登记。

| ID | 项 | 现状 | 影响 |
| --- | --- | --- | --- |
| VG-1 | 分组卡折叠 | ✅ 已实现（2026-10-29）。分组头末尾加折叠箭头，点击收起或展开频道列表。状态只在本页内存，不写回配置 | 分组多时只能滚动定位 |
| VG-2 | 台标批量动画与还原 | ✅ 已实现（2026-10-29）。页头加「一键全部动态化」「全部还原静态」，串行调用已有接口；统计卡加拖放上传区 | 批量处理台标要逐个点 |
| VG-3 | 素材页「配置中检测到的视频 URL」 | ✅ 已实现（2026-10-29）。新增 `GET /panel/api/assets/candidates`，扫描 5 个占位跳转键。素材页加卡片，支持逐项与一键转本地 HLS。转换只写 `[assets].map`，不改配置键 | 配置里的 404 跳转等 URL 要手工发现并逐个转换 |
| VG-4 | 频道热度排行维度 | ✅ 已实现（2026-10-29）。在线记录改存「时间、观众、频道」。`/panel/api/stats` 补 `online_viewers`、`active_channels`、`channels[]`（含频道名、在线数、累计请求）。热度页改为三张大数字卡 + 频道排行 | 看不到单个频道的热度 |

VG-3 与 VG-4 带后端改动。`src/stats.rs` 加频道维度计数，`src/metrics.rs` 加按频道统计。

参考截图里的页脚 `© 2026 YanG · GitHub` 与品牌名属目标派生内容。按数据纪律不复制。

### 3.9 面板功能入口开关（2026-10-31）

新增 `[features]` 配置段，控制功能是否可用。

| 开关 | 控制的功能 |
| --- | --- |
| `recording` | 定时录制 |
| `livestream` | 回看管理 |
| `logo` | 台标管理 |
| `playlist` | 订阅聚合与聚合源接口 |
| `assets` | 视频素材与字幕 |
| `users` | 用户与设备 |

节目指南与频道热度不在这里。它们分别由 `[epg].enabled` 与 `[stats].enabled`
控制，避免同一个功能有两套开关。

概览、缓存状态、分组管理、系统设置、安全管控、运行日志没有开关，始终显示。

语义约束：

1. **关闭即不可用**。侧栏不显示该分区，相关接口返回 `403`。
2. **与部署规格无关**。高配与低配行为一致，不做「低配才拦截」的分支。
3. 恢复路径不受拦截。`/panel/api/config`、`/panel/api/login`、`/panel/api/me` 始终可用。
4. 关闭某分区时，若前端停在上面，回退到概览。
5. 配置在 `/panel/api/status` 的 `features` 字段返回。

拦截在中间件里按路径前缀做，见 `src/http/guard.rs` 的 `feature_gate`。

面板位置：系统设置 → 功能开关。

低配与高配的参数差异见 [`docs/design/10-low-vs-high-spec.md`](docs/design/10-low-vs-high-spec.md)。

---

## 4. 质量门

2026-10-27 实测：

| 项 | 结果 |
| --- | --- |
| `cargo fmt --check` | 通过 |
| `cargo test --offline --all-targets` | 349 项通过（257 单元 + 92 契约） |
| `cargo clippy --offline --all-targets` | 0 error，27 条历史 warning |
| 冒烟脚本 | 26 个全部通过。`smoke-record-dash` 需要 ffmpeg，且要求端口 19970/19971 空闲 |
| `pnpm build` | 通过（含 `tsc --noEmit`） |
| 发布校验 | 上一轮双架构产物通过 `verify-release.sh`。本轮未重跑 |
| CI | 只在 `v*` tag 触发。fmt/clippy/test、CLI 冒烟、双架构校验已配置 |

---

## 5. 存储迁移（v2026.6）

**事实源**：`<安装目录>/tv-buddy.db`。表 `settings`（配置 JSON）、`users`、`access_tokens`。

**读取**：启动时从库读配置。库为空时写入默认值。
同目录存在旧 `config.toml` 时拒绝启动，防止用空分组跑起来。

**写入**：面板保存、分组编辑、CLI 都写库。进程内一把写锁串行化「读 → 改 → 写 → 替换快照」。

**迁移**：

| 来源 | 做法 |
| --- | --- |
| 旧 `config.toml` | `tv-buddy import-config <toml> --db <db>` |
| 旧 `<cache>/users.db` | 首次启动只读迁移，`users_migrated` 标记防止重复；旧文件保留 |
| 旧备份包 | 包里是 `config.toml` 时按旧格式解析恢复 |

**兼容性变化**：不再直接读取目标产品的 `config.toml`。升级需先导入一次。

**子命令**：`import-config`、`config get/set/reset`、`passwd`。

---

## 6. 关键设计结论

EPG：

- `out_id = tvg_id || channel.id`。
- 匹配顺序：手工映射 → `tvg_id` → 名称。
- 名称规范化：删除全部空白，转小写。
- 手工映射存 `<cache>/epg/mappings.json`。额外频道存 `<cache>/epg/extra.json`。
- 源正文缓存到 `<cache>/epg/raw/<source_id>.xml`。`regenerate` 只读 raw 缓存。
- `public_viewer_enabled=false` 时，公开 `/epg` 返回 `404`。

DASH：

- 轨道命名：`video_{高}p`、`audio-{语言}`（缺语言时 `audio-{n}`）、`text-{n}`。
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

## 7. 推进顺序

排序原则：**反代与 302 转发 > 必须使用的功能 > 其他功能**。

同优先级内，先做有目标证据的项。没有证据的项，先按 `07` §5 出对比方案，再定实现。

### 6.1 优先级 1：反代与 302 转发（已完成 2026-10-27）

| # | 项 | 状态 | 依据 |
| --- | --- | --- | --- |
| P1-1 | URL 池回退顺序（`url_pool_size > 1`） | ✅ 池大小作候选上限，N = min(地址数, 池大小, 6)。视频固定 `pool[0]`；音频/字幕在 `pool[1..N]` 轮询 | V-01、F-015 |
| P1-2 | DASH 多 CDN 故障切换 | ✅ 池启用时首轮逐址各试一次，失败立即换址；视频记住最近成功的上游地址 300 秒 | F-015 |

两项语义都是自定。目标在 `pool>=2` 时只拉 MPD N 次后零切片、入口 503（F-015 §1.2），
取用方式不可对拍。本实现不复刻该缺陷，也不把它算作已对齐。

不变量：`url_pool_size<=1` 或单 `<BaseURL>` 时走旧路径，抓取序列逐项不变。
MPD 清单层保持单地址：`channel.url` 不支持多值（F-015 §1.3）。

测试：`pool_caps_to_size`、`pool_caps_audio_ring`、`pool_is_clamped_to_panel_max`、
`attempt_plan_legacy_matches_old_loop`、`attempt_plan_pool_round_robin`、
`render_media_caps_entry_urls`、`dash_pool_falls_back_and_remembers_success` 等 13 项。

### 6.2 优先级 2：必须使用的功能

| # | 项 | 现状 | 依据 |
| --- | --- | --- | --- |
| P2-1 | `[auth].whitelist`、`max_sessions_per_ip`、`session_ttl_secs` | ✅ 已实现（2026-10-28）。会话 = `(IP, 频道)`，超限踢掉最久未用的那路；白名单豁免封禁与会话上限。三项只在 `ip_guard_enabled=true` 时生效，默认行为不变。语义自定 | F-010 |
| P2-2 | `[auth]` 的 `l1/l2` 限流、违规计数、升级封禁（13 个键） | ✅ 已实现（2026-10-28）。只计播放入口请求；L2 突发、L1 持续；违规衰减后升级封禁；登录锁按阶梯升级。全部在 `ip_guard_enabled=true` 时生效 | F-010 |
| P2-3 | `random_id_enabled`（播放链接随机化） | ✅ 已实现（2026-10-28）。导入 M3U 时忽略 `tvg-id`，新频道取 `r-<8hex>`。目标开启后已有频道与切片前缀不变，所以只影响新导入的频道 | F-010 §4 |
| P2-4 | `[share].share_token` | ❌ 不做（2026-10-28）。实测 `share_token=false` 时载荷里仍有 Token（F-013），该键不控制 `/api/share/pull` 的内容；面板文案「同时分享 token」与实际行为不符，语义未知 | F-013、S-AUTH-06 |
| P2-5 | `cache.backend=memory`（内存缓存后端） | ✅ 已实现（2026-10-28）。切片正文留在进程内，不写 `seg_bytes`；按 `memory_max_mb` 做 LRU 淘汰；flush 一并清空 | F-015 |
| P2-6 | 缓存容量上限（`segment_cache_max_mb`、`m3u8_max_entries`、`segment_max_entries`） | ❌ 不做（2026-10-28）。目标侧三项上限均未观测到生效（S-CACHE-03），复刻「不生效」才是一致行为。运维上限由 `[cache.gc]` 与 `[cache.segments]` 承担 | S-CACHE-03、`04` §11.5 |

### 6.3 优先级 3：其他功能

| # | 项 | 现状 | 依据 |
| --- | --- | --- | --- |
| P3-1 | DASH 音轨命名对齐 `audio-{语言}` | ✅ 已实现（2026-10-28）。有 `lang` 时用 `audio-{语言}`，缺语言时退回序号；同名加 `-2` 后缀 | F-016 §5.1 |
| P3-2 | DASH 文字轨出口 | ✅ 定案保留（2026-10-28）。目标在夹具里不写 master，本实现输出 `SUBTITLES` 组：这是超集，播放器不用也不受影响 | F-016 §5.1 |
| P3-3 | `[subtitle].style.*` 样式注入 | ❌ 不做（2026-10-28）。本实现没有面向播放器的字幕出口：字幕只在面板里上传/管理，注入样式无处生效 | `05` §4.1 |
| P3-4 | 播放侧 `compat_mode`、`srt_ua_keywords` | ❌ 不做。目标未测出行为（D19），按「不猜」处理 | D19 |
| P3-5 | U-06 其余 5 种台标效果 | ✅ 定案（2026-10-29）。保持近似。`breathe` 已逐字节对齐，其余 5 种不补取证（[`08`](docs/design/08-open-decisions.md) §2） | F-011 |
| P3-6 | 逐屏视觉对拍（验证类） | ✅ 已完成（2026-10-29）。13 个分区加登录页按 1440×1000 截图，与 `reference/img/` 并排比对。页头骨架与侧栏一致；发现 4 项功能缺口（VG-1 至 VG-4）与若干版式差异。见 §3.7 与 `docs/tests/runs/2026-10-29.md` | `07` §5、V-10 |
| P3-7 | `dash-timeshift/status` 的 `hours` 键核对（验证类） | ✅ 定案（2026-10-29）。保持统一键 `hours`（[`08`](docs/design/08-open-decisions.md) §3） | F-016 §5.1 |

### 6.4 外部依赖（需要你参与）

| # | 项 | 说明 |
| --- | --- | --- |
| E1 | T0 CDN 面板配置 | 按 [`docs/tests/06-cdn-console.md`](docs/tests/06-cdn-console.md) 执行 |
| E2 | 真机验收 | 按 [`docs/tests/05-low-spec-acceptance.md`](docs/tests/05-low-spec-acceptance.md) 跑 S4/S6 |

### 6.5 每项完成后的固定动作

1. 运行完整质量门（§4）。
2. 更新本文件与对应 `docs/`。
3. 提交并立即推送 `main` 分支。

### 6.6 后续规划（需要单独设计）

| # | 项 | 说明 |
| --- | --- | --- |
| F-1 | 用户与设备管理 | 调研已完成（2026-10-30），见 [`09-users-and-devices.md`](docs/design/09-users-and-devices.md)。数据模型、API、面板结构、权限语义已按 mpd-hls 摸清。存储建议 SQLite（`rusqlite` bundled，已实测两个 musl 架构可静态构建）。待定案 3 点见该文档 §8 |

---

## 8. 文档入口

| 主题 | 路径 |
| --- | --- |
| 目标规范 | [`docs/spec/08-target-spec.md`](docs/spec/08-target-spec.md) |
| 验证矩阵 | [`docs/spec/07-baseline-and-verification.md`](docs/spec/07-baseline-and-verification.md) |
| 全量覆盖对比与规划 | [`docs/design/07-coverage-and-plan.md`](docs/design/07-coverage-and-plan.md) |
| 未决项对比方案 | [`docs/design/08-open-decisions.md`](docs/design/08-open-decisions.md) |
| F-1 用户与设备调研 | [`docs/design/09-users-and-devices.md`](docs/design/09-users-and-devices.md) |
| 低配与高配参数差异 | [`docs/design/10-low-vs-high-spec.md`](docs/design/10-low-vs-high-spec.md) |
| HTTP 契约 | [`docs/design/03-http-contract.md`](docs/design/03-http-contract.md) |
| 配置与存储 | [`docs/design/04-config-and-storage.md`](docs/design/04-config-and-storage.md) |
| 低配置优化 | [`docs/design/06-low-spec-optimization.md`](docs/design/06-low-spec-optimization.md) |
| 低配置示例 | [`docs/design/config-low-spec.toml`](docs/design/config-low-spec.toml) |
| 架构 | [`docs/design/02-architecture.md`](docs/design/02-architecture.md) |
| 面板字段清单 | [`docs/design/05-frontend.md`](docs/design/05-frontend.md) |
| 手工验收测试 | [`docs/tests/README.md`](docs/tests/README.md) |
| 发现索引 | [`docs/findings/README.md`](docs/findings/README.md) |
| 历史进度 | [`PROGRESS_bak.md`](PROGRESS_bak.md) |

---

## 9. 归档

2026-10-25，本文件按 ASD-STE100 重写。旧内容全部迁入 [`PROGRESS_bak.md`](PROGRESS_bak.md)。

归档含：

- P0 至 P4 的逐项完成记录。
- 每轮实现、对拍、测试与发布细节。
- 旧任务编号与验证命令。
- 2026-10-25 之前的续接快照。

本文件只保留：目标与约束、状态总览、功能缺口、质量门、设计结论、推进顺序、文档入口。
