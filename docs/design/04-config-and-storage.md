# 04 · 配置与存储设计

> 设计版本 v1.0 ｜ 2026-10-19 ｜ 状态：**初稿**
> 上游：[`03-http-contract.md`](03-http-contract.md) ｜ 判据：[`../spec/08-target-spec.md`](../spec/08-target-spec.md) §11 ｜ 证据：F-006、F-008、F-015、F-013

## 1. 配置文件

| 项 | 设计 | 依据 |
| --- | --- | --- |
| 路径 | 来自启动参数（`iptv-proxy <config.toml>`），默认 `config.toml` | S-ENV-06 |
| 解析 | 严格 TOML：重复键、类型错都报错并退出 | S-EPG-01（F-008 实测） |
| 默认值 | 缺省键用内置默认补齐；默认值对齐目标 v0.8.2 | S-CFG-04 |
| 未建模键 | 存 `extra`（`toml::Table`），保存时原样写回 | 设计决策（避免丢第三方配置） |
| 写回 | 全量规范化 + 原子替换（`config.toml.tmp` → `rename`） | S-CFG-01（F-006） |
| 并发 | 进程内用一把写锁串行化"改内存 + 写盘" | 设计决策 |

### 1.1 段与职责

| 段 | 关键键 | P0 用途 |
| --- | --- | --- |
| `[server]` | `bind`、`not_found_url`、`error_url`、`home_url`、`m3u_header_lines`、`timezone_offset_hours`、`log_level`、`trust_proxy` | 监听、跳转、订阅头行、日志级别 |
| `[panel]` | `enabled`、`username`、`password_sha256`、`session_ttl` | 面板登录 |
| `[cache]` | `backend`、`disk_path`、`m3u8/segment_max_entries`、`disk_max_mb`、`disguise.*` | 缓存与伪装 |
| `[cache.prefetch]` | `enabled`、`concurrency`、`timeout`、`prefetch_limit`、`skip_tail`、`mpd_readahead` | 媒体清单刷新后按并发与超时预取切片；`skip_tail` 跳过尾部。分组/频道可用 `[groups.prefetch]` 覆盖（见 §11） |
| `[cache.segments]` | `global_max`、`per_channel_max`、`retain_count` | 内存切片索引上限；`per_channel_max > 0` 时同时约束磁盘索引条数（`seg_url/*.json`） |
| `[cache.gc]` | `enabled`、`max_disk_mb`、`max_age_secs`、`interval_secs`、`scan_limit`、`disk_budget_ratio`、`reserve_mb`、`min_mb` | 磁盘回收：切片正文只写不删，靠它兜住可用空间。见 §11 |
| `[cache.ttl]` | `sub_url_secs`、`segment_secs`、`refresh_ratio`、`fallback_mpd_secs` | 订阅地址与切片缓存时长 |
| `[cache.dash]` | `mpd_refresh_*`、`*_timeout_secs`、`fetch_concurrency`、`ondemand_idle_secs`、`retry_*`、`watchdog_*` 等 | MPD/切片超时、重试退避、会话空闲回收、`playlist_min_secs` 媒体窗口、`start_time_offset_secs`、`publish_delay_secs`、看门狗 tick/stall/stagger/上限/冷却已接入；`fast_start_secs` 目标无可观测语义，仅保留配置 |
| `[cache.remux]` | `target_duration_secs`、`window_segments`、`publish_delay_segments`、`reconnect_backoff_ms` | Remux 切片时长、窗口与重连退避 |
| `[auth]` | `token_enabled`、`token_ttl_hours`、`token_failure_url`、`whitelist` | 播放 Token |
| `[cdn]` | `enabled`、`auto_tune`、`segment_max_age_secs`、`shared_cache_hits`、`dont_cache_empty`、`empty_status`、`public_base` | 外部 CDN 侧兜底与被动实测开关；`enabled=false`（默认）时响应头与状态码逐字节不变。见 §11 |
| `[health]`/`[stats]` | `enabled`、`interval_secs`、`online_window_secs` | 健康与指标 |
| `[assets]` | `map`（原始 URL → 本地 HLS 入口） | 占位/错误视频转换 |
| `[epg]` | `enabled`、`cron`、`public_viewer_enabled`、`keep_days`、`future_days`、`keep_yesterday`、`match_by_name`、`allow_scripts`、`python_bin`、`script_timeout_secs`、`record_pad_before_secs`、`record_pad_after_secs` | 节目单采集与浏览 |
| `[[epg.sources]]` | `id`、`name`、`kind`、`enabled`、`url`、`script`、`cron`、`timeout_secs`、`tz_shift_mins` | 节目单源 |
| `[epg.translate]` | `enabled`、`url`/`mtran_url`、`token`/`mtran_token`、`source`、`target(s)`、`mtran_batch` | EPG 机翻 |
| `[subtitle.translate]` | `enabled`、`url`、`token`、`source`、`target(s)`、`batch` | 字幕机翻 |
| `[playlist]` | `merge_ttl_secs`、`merge_position`、`global_exclude` | 聚合缓存参数、插入位置与全局排除 |
| `[[playlist.sources]]` | `id`、`name`、`url`、`enabled`、`ttl_secs`、`group_map`、`group_mode`、`merge_group`、`unlisted_groups`、`include`、`exclude` | 源缓存、分组选择/改名、过滤；`ttl_secs=0` 按 3600 秒 |
| `[[groups]]`/`[[groups.channels]]` | 见 `02-architecture.md` §4.1 | 数据面 |

## 2. 热重载矩阵

| 段 | 生效方式 | 依据 |
| --- | --- | --- |
| `[server]`（含 `m3u_header_lines`） | 立即 | S-CFG-02（F-006） |
| `[auth]` | 立即 | S-CFG-02 |
| `[[groups]]`/`[[groups.channels]]` | 立即 | 面板改动的实测行为 |
| `[cache]`（`disk_path`） | 需重启 | 目标同 |
| `[cache.segments]`/`[cache.ttl]` | 立即（索引上限与 TTL 随快照更新） | 本实现 |
| `[cache.gc]` | 立即（下一轮扫描按新参数） | 本实现 |
| `[cache.prefetch]`/`[cache.dash]` | 下一次媒体清单刷新/抓取生效 | 本实现 |
| `[cdn]` | 立即（下一请求按新头值与 `public_base`） | 本实现 |
| `[cache.remux]` | 新会话生效；已有会话保留旧参数 | 本实现 |
| `[epg]` | **需重启**（`enabled` 与源列表在启动时读取） | F-008 |
| `[epg.translate]`/`[subtitle.translate]` | 立即（下次翻译生效） | 设计决策 |
| `[playlist]`/`[[playlist.sources]]` | 立即；源正文按源级 TTL 复用 | F-020 |
| `[panel]` | 立即（新会话按新口令） | 设计决策 |

实现：`notify` 监视配置文件。事件到达后重读；解析失败只记日志、保留旧快照。自身写盘时先登记内容摘要，忽略自己触发的下一次事件（避免写回循环）。

> 竞态：目标存在"文件直改 vs 面板保存"的竞态（面板以内存态回写）。本实现以**文件为唯一事实源**：任何保存都从当前文件重读合并，再写回。这会与目标行为不同（更安全），登记为设计差异。

## 3. 存储布局

```text
<disk_path>/                     （默认 ./cache）
├─ m3u8/          入口与媒体清单缓存
├─ seg_url/       上游切片地址缓存
├─ seg_bytes/     切片正文缓存
├─ sub_url/       订阅源正文缓存
├─ subs/status.json 订阅源最近抓取状态
├─ dash/          DASH 会话（flush 保留）
├─ livestream/    本地回看
├─ recordings/    录制产物 + schedule.json
├─ logos/         台标文件（含 .orig/ 原图备份）
├─ assets/        素材；convert/<hash>/ 存本地 HLS
├─ resolvers/     解析脚本缓存
├─ epg/scripts/   EPG Python 脚本
├─ epg/raw/       每个源的原始 XMLTV（regenerate 用）
├─ epg/epg.xml    生成的 XMLTV；同目录有 `epg.xml.gz`
├─ epg/viewer.json 公开频道表快照（含源信息；重启后可查）
├─ epg/.translate/ EPG 翻译缓存
└─ subtitles/.translate/ 字幕翻译缓存
```

| 项 | 设计 | 依据 |
| --- | --- | --- |
| 目录创建 | 仅启动时创建 | F-006 |
| 缓存键 | 清单：`{gid}_{cid}__{sha256(url)[..8]}`；切片：`sha256(上游绝对 URL)` | 内部实现，非兼容契约 |
| 切片伪装 | `<S>` = 同一 sha256 前 32 hex | S-DIS-04 |
| 切片索引（M2 已实现） | `seg_url/<sha256("{gid}/{cid}")[..16]>.json`：`<S>` → `{url, ext}`；内存 + 磁盘两级 | `src/proxy/segments.rs` |
| 切片正文（M2 已实现） | `seg_bytes/<gid>_<cid>_<S>.<ext>`；命中复用；上限由 `[cache.gc]` 控制（§11） | 同上 |
| DASH 缓存（P4 已实现） | `dash/<gid>/<cid>/<track>/{index.txt,init.<ext>,<序号>_<hash>.<ext>}`；媒体清单按时重取，init/切片命中复用 | `src/dash_hls.rs` |
| DASH 时间窗 | `[groups].mpd_keep_seconds > 0` 时媒体清单只保留末尾累计时长；`0` 保留全部。`proxy` 模式 MPD 同样裁剪时间线并推进编号 | `src/dash_hls.rs`、`src/http/routes/stream.rs` |
| DASH URL 池 | 多个 `<BaseURL>` 时首选地址固定给视频；音频/字幕在 `url_pool_size>1` 时轮询其余地址。目标默认 `url_pool_size=1` 实测音视频都只取首选（F-022） | `src/dash_hls.rs` |
| proxy MPD 反代 | `<BaseURL>`（绝对/相对/缺失）统一改为 `/{gid}/{cid}/mpd/{token}/{Base64URL(base)}/`；切片请求解码还原上游地址并校验令牌 | `src/http/routes/stream.rs` |
| 清单复用（M2 已实现） | 进程内 TTL 5 秒（`TtlCache`） | `src/proxy/mod.rs` |
| 订阅源缓存（U-07 已实现） | `sub_url/<sha256("sub:{id}:{url}")>.json`：`{key,body,stored_at}`；`subs/status.json` 保存 `channels/last_ok/last_try/error`；重启后按源级 TTL 命中 | `src/source_cache.rs`、F-020 |
| flush | 清 `m3u8`/`seg_url`/`seg_bytes`/`sub_url`，**保留 `dash/`**；同时清内存正文缓存 | S-CACHE-06 |
| 容量上限 | 目标上限未生效；本实现用 `[cache.gc]` 兜住切片正文与磁盘索引（§11） | F-015 |
| 索引 | 启动扫描目录重建内存索引 | 设计决策 |

## 4. 运行时数据

| 数据 | 存放 | 生命周期 | 依据 |
| --- | --- | --- | --- |
| 面板会话 | 内存 | 重启失效、TTL `session_ttl` | F-004 |
| 播放 Token | 内存 | 重启失效、TTL `token_ttl_hours`、旧值宽限 ≈1h | S-AUTH-09 |
| IP 封禁 | 内存 | 重启失效、默认 30 天 | S-AUTH-10 |
| 限流窗口 | 内存 | 滑动窗口 | S-GAP-08 |
| 录制任务 | `cache/recordings/schedule.json` | 持久 | F-014 |
| 看门狗状态 | 内存 | 重启重置 | F-005 |

`schedule.json` 形状（F-014）：

```json
{ "version": 1, "tasks": [ { "id": "<uuid>", "name": "…", "group_id": "…", "channel_id": "…",
  "start_time": "2026-10-20T19:00:00", "end_time": "…", "format": "mkv", "status": "pending" } ] }
```

## 5. 日志

| 项 | 设计 | 依据 |
| --- | --- | --- |
| 出口 | 标准输出 + `<安装目录>/iptv-proxy.log` | S-ENV-03 |
| 级别 | `[server].log_level`（`trace`/`debug`/`info`/`warn`/`error`） | S-CACHE-05 |
| 截断 | 文件超过 5 MB 截断 | S-CACHE-05 |
| 格式 | `tracing` 文本，含时间、级别、目标 | 设计决策 |

## 6. 发布布局

```text
/opt/iptv-proxy/
├─ iptv-proxy          静态 musl 二进制
├─ config.toml         首次启动生成默认配置
├─ iptv-proxy.log
├─ cache/
└─ iptv-proxy.sh       管理 CLI（无参数打开菜单）
```

| 项 | 实现 | 依据 |
| --- | --- | --- |
| 目标平台 | `aarch64-unknown-linux-musl`、`x86_64-unknown-linux-musl` | 需求 |
| 交叉编译 | Docker `rust:1-alpine`：镜像宿主三元组本身就是 `*-linux-musl`，直接 `cargo build --release` 得静态二进制 | `scripts/build-release.sh` |
| 依赖来源 | 宿主 `~/.cargo/registry` 与 `~/.cargo/git` 挂进容器，`--offline` 构建 | 容器无外网 |
| 目标目录 | `CARGO_TARGET_DIR=target/pack-<arch>`，不污染本机构建 | — |
| 产物校验 | `scripts/verify-release.sh`：`ldd` 无动态依赖、能启动、`/health` 返回 `ok` | `scripts/verify-release.sh` |
| 服务 | systemd 单元；Alpine 用 OpenRC | S-ENV-04 |

发布包内容（`dist/iptv-proxy-<version>-<arch>.tar.gz`）：

```text
iptv-proxy-<version>-<arch>/
├─ iptv-proxy            静态 musl 二进制
├─ config.toml.example   首次安装复制为 config.toml
├─ iptv-proxy.sh         管理 CLI（菜单、启停、端口、密码、更新、卸载）
├─ install.sh            安装到 /opt/iptv-proxy，注册服务
├─ iptv-proxy.service    systemd 单元
└─ iptv-proxy.openrc     Alpine OpenRC 服务
```

> 说明：本机访问 `github.com` 的 release 资产与 `static.rust-lang.org` 会被中间设备断开 TLS，
> 因此不用 `cross`，改用 Docker Hub 可用的 `rust:1-alpine` 完成双架构静态构建。
| 备份（P2） | `tar.gz` 含 `config.toml` + `cache/logos` + `schedule.json`；恢复时原配置另存 `config.toml.bak` | S-BAK-01/02、F-013 §9 |

## 7. 未决问题

| 问题 | 处理 |
| --- | --- |
| 是否复刻目标的"面板保存"规范化格式 | 用 `toml::to_string_pretty`；不追求与目标逐字节相同 |
| CLI `update` 的下载源 | 用 `IPTV_PROXY_{BIN,ARCHIVE,URL}` 覆盖；本仓库未发布 release 资产 |
| `disk_path` 变更是否允许热重载 | 当前要求重启（与目标一致） |

## 9. `[share]`

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `false` | 启用分享端点与同步任务 |
| `role` | `master` | `master` 汇总从机；`follower` 采用主机 Token |
| `key` | 空 | 分享密钥；对端用 `?key=` 传入 |
| `trust_ips` | `[]` | 允许拉取的源地址；支持精确地址、CIDR 与 `*` |
| `master_url` | 空 | follower 拉取的基地址 |
| `follower_urls` | `[]` | master 汇总的基地址 |
| `poll_secs` | `300` | 同步周期（秒），最小 5 |

## 10. 频道级解析脚本

`[groups.channels]` 支持 `resolver_script`、`resolver_arg` 与 `resolver_ttl`。
脚本位于 `<cache>/resolvers/`，通过 `python3` 执行。
详见 `03-http-contract.md` §6.12。

## 11. 低配置优化段

> 设计：[`06-low-spec-optimization.md`](06-low-spec-optimization.md)。
> 现成样例：[`config-low-spec.toml`](config-low-spec.toml)（1 vCPU / 128 MB / 1 GB + 多 CDN）。

三段都**默认不改变既有行为**：`[cdn].enabled = false`、`[cache.gc].enabled = true`（补洞，见下）、
分组/频道的 `prefetch` 缺省即继承全局。

### 11.1 `[cdn]`

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `false` | 关闭时切片响应头与空体状态码与旧版逐字节一致 |
| `auto_tune` | `true` | 被动实测：累计分组窗口参数与切片体积，供推荐 TTL / 磁盘上限 |
| `segment_max_age_secs` | `0` | 切片 `max-age`；`0` = 用实测推荐值（无数据时 600s） |
| `shared_cache_hits` | `false` | 开：命中路径也标 `public`（给只能「遵循源站头」的 CDN） |
| `dont_cache_empty` | `false` | 开：空体加 `no-store`（CDN 跟随源站头时有效） |
| `empty_status` | `502` | 上游失败改用的状态码；强制 TTL 时必须靠它挡住「200 + 空体」 |
| `public_base` | 无 | 固定对外基址；CDN 不下发 `X-Forwarded-Proto` 时用 |

### 11.2 `[cache.gc]`

切片正文（`<cache>/seg_bytes/`）与磁盘索引（`<cache>/seg_url/*.json`）只写不删，
1 GB 盘会被吃满，所以这里默认开。

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `true` | 总开关；关掉后切片不再自动回收 |
| `max_disk_mb` | `0` | 占用上限；`0` = `clamp(min(工作集, 可用×ratio − reserve), min_mb, total×0.7)` |
| `max_age_secs` | `1800` | 超过即删 |
| `interval_secs` | `300` | 扫描周期（服务内下限 30s） |
| `scan_limit` | `500` | 单轮最多处理的文件数，防 1 vCPU 抖动 |
| `disk_budget_ratio` | `0.8` | 自动上限最多占可用空间的比例 |
| `reserve_mb` | `100` | 给系统、日志与其它缓存目录预留 |
| `min_mb` | `50` | 自动下限 |

淘汰顺序：超龄 → 超量 LRU（按 mtime 最旧优先）→ 可用空间低于 `reserve_mb` 时进入紧急模式
（忽略 `max_age_secs`，删到 `cap / 2`）。不用「按频道整组淘汰」：`segment_path` 的文件名
经过 `slug()`，标识超 48 字符时被替换为 SHA-256，无法反解回 `gid/cid`；而直播切片的 mtime
本身就是热度信号。

`statvfs` 取的是 `f_bavail`（非特权用户真正可写的块数），失败时回退 150 MB。

### 11.3 预取覆盖

`[groups.prefetch]` 与 `[groups.channels.prefetch]` 各三个**可选**字段，缺省即继承上层：

```toml
[groups.prefetch]
enabled = true        # 三态：未设置 = 继承
prefetch_limit = 6    # 别名 `limit` 也接受
concurrency = 2
```

合并顺序与 `proxy_for` 一致：**频道 > 分组 > 全局**（`Config::prefetch_for`）。
面板「分组管理」的编辑弹窗提供同样四个选项：跟随全局 / 只覆盖数量与并发 / 开启 / 关闭。

### 11.4 运行时数据

| 路径 | 内容 |
| --- | --- |
| `<cache>/cache_estimate.json` | 被动实测统计（带 `v` 版本号；不匹配即丢弃）。由 GC tick 节流落盘（≥300s） |
| `GET /panel/api/cache/stats` | 新增只读 `recommend` 块：推荐 TTL、磁盘上限、数据来源（`measured`/`manual`/`fallback`）、覆盖分组数、更新时间 |
| `POST /panel/api/cdn/estimate` | 一次性估算：前 3 个非归档分组各第 1 个频道，抓清单与首个切片长度；全失败仍返回 `200`（`source` 回落 `fallback`），并发请求返回 `409` |

### 11.5 `[cache]` 里的容量键（U-10）

目标 v0.8.2 的四个容量键**都没观测到生效**（F-015 / S-CACHE-03）：`m3u8_max_entries=2`
仍留 6–7 条，`segment_max_entries=2` 仍留 17 条，`disk_max_mb=2` 占 2.79 MB，
`segment_cache_max_mb=1` 占 2.7 MB。本实现按下面的规则收口，避免「配了没反应」。

| 键 | 本实现的用法 |
| --- | --- |
| `disk_max_mb` | **生效**。与 `[cache.gc].max_disk_mb` 同义；同时非 0 时取更小者。默认 `0` = 不限 |
| `memory_max_mb` | **生效**。内存切片索引的字节预算。按每条约 256 B 估算成条目数，再与 `[cache.segments].global_max` 取更小者。默认 `0` = 不限 |
| `segment_cache_max_mb` | 保留键面，**不消费** |
| `m3u8_max_entries`、`segment_max_entries` | 保留键面，**不消费** |

`segment_cache_max_mb` 不消费的原因：它的默认值是 `50`，与用户显式填写的 `50` 无法区分。
一旦按「非 0 即生效」处理，`segment_cache_max_mb = 50` 会把缓存静默压到 50 MB，
低于 §3.3 在 1 GB 盘上算出的工作集（约 140 MB），L1 提前淘汰、回源变多。
限容请用 `[cache.gc].max_disk_mb` 或 `[cache].disk_max_mb`。
`m3u8_max_entries`／`segment_max_entries` 同样保留键面：目标既不生效，本实现也不消费，
限条目数请用 `[cache.segments]` 的两个键。
`memory_max_mb` 的 256 B/条 是估算值，只用作上限，偏保守。
