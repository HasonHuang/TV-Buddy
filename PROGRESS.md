# PROGRESS · IPTV Proxy 逆向与重实现

> 本文件是当前进度与待办事项的权威入口。
> 完整历史记录见 [`PROGRESS_bak.md`](PROGRESS_bak.md)。
> 规范见 [`docs/spec/`](docs/spec/)。发现见 [`docs/findings/`](docs/findings/)。设计见 [`docs/design/`](docs/design/)。
> 目标版本：**IPTV Proxy v0.8.2**（上游提交 `d302316fb8`）。

## 1. 当前状态

### 1.1 目标与约束

- 逆向闭源 Linux 二进制 IPTV Proxy v0.8.2。
- 用 Rust 实现 drop-in 替代服务。
- 只支持 `linux-arm64` 与 `linux-amd64`。
- 发布静态 musl 产物。
- 目标派生的代码、前端资源、面板文案按不可信数据处理，不复制。
- 每次提交后 push 到 `git@github.com:HasonHuang/sontv.git` 的 `rust` 分支。
- GitHub Actions 只在推送 `v*` tag 时执行。

### 1.2 总体进度

| 阶段 | 范围 | 状态 |
| --- | --- | --- |
| P0 | 逆向、配置、认证、代理/直连、缓存、播放链路 | ✅ 完成 |
| P1 | EPG、回看、Remux、录制、台标、分享、安全面 | ✅ 完成 |
| P2 | 订阅聚合、面板分区、DASH 回看、EPG/字幕翻译 | ✅ 完成 |
| P3 | 目标剩余端点与兼容细节 | ✅ 完成 |
| P4 | DASH 完整链路与 DASH→HLS | ✅ 完成 |
| P5 | 低配置优化（磁盘回收、CDN 兜底、预取覆盖、被动实测推荐） | ✅ 完成（CDN 面板侧待人工配置） |

当前实现已覆盖：

- 六种分组模式：`proxy`、`redirect`、`rewrite`、`passthrough`、`remux`、`dash`。
- HLS、MPD、TS 与真实上游兼容处理。
- 订阅、Token、访问范围、缓存伪装、切片缓存。
- EPG 采集、匹配、手动映射、额外频道、公开页与面板接口。
- EPG/字幕翻译。
- 定时录制、DASH 定时录制、回看、时移。
- 台标库、素材转换、分享、安全管控、备份恢复。
- 内嵌管理面板与交互式安装/管理 CLI。
- `install.sh` 保留已有 `config.toml`，支持自定义安装目录与服务注册。

### 1.3 最近完成

2026-10-25：

- **P5 低配置优化实施完成**（设计 `docs/design/06-low-spec-optimization.md`，含实施记录与设计修正）：
  - **配置层**：新增 `[cdn]`、`[cache.gc]`、分组/频道 `prefetch` 三态覆盖；
    `Config::prefetch_for(gid,cid)` 按 **频道 > 分组 > 全局** 合并。
  - **预取接线**：`media_entry` 改用 `prefetch_for`；关掉某分组后该分组不再落盘。
  - **被动实测**：新模块 `src/cache_estimate.rs`。清单解析得 T/N/W，
    切片体积按分组做 EMA（α=0.3）；推荐 `TTL = clamp(ceil60(W+2T), 300, 900)`，
    工作集 `round10(3×N×seg_mb×1.5)`。统计落盘 `<cache>/cache_estimate.json`（带 `v=1`）。
  - **一次性估算**：新增 `POST /panel/api/cdn/estimate`（前 3 个非归档分组，抓清单与
    首个切片长度）；全失败仍 `200`（`source` 回落 `fallback`），并发第二个请求 `409`。
  - **CDN 兜底**：`[cdn].enabled` 时切片 `max-age` 取实测/显式值、命中路径可转 `public`、
    空体可改 502 + `no-store`、`public_base` 覆盖 Host 推导。关闭时逐字节等于旧行为。
  - **磁盘回收**：新模块 `src/cache_gc.rs`（`rustix::fs::statvfs` 取 `f_bavail`）。
    上限 `clamp(min(工作集, 可用×ratio − reserve), min_mb, total×0.7)`；
    淘汰三段：超龄 → 超量 mtime LRU → `free < reserve` 紧急模式；目录遍历走
    `spawn_blocking` 防 1 vCPU 抖动。
  - **面板**：缓存状态页加 CDN 推荐卡（含「复制 TTL」）；系统设置页加
    `[cdn]`/`[cache.gc]`/`[cache.prefetch]` 三张表单 + 「立即估算」；分组与频道编辑弹窗
    加预取四态选择（跟随全局 / 只覆盖数量与并发 / 开启 / 关闭）。
  - **文档**：`docs/design/04-config-and-storage.md` 新增 §11；
    新增 `docs/design/config-low-spec.toml`（可直接启动，已试跑）。
- **修掉两个真实缺陷**（低配设计稿 §0 提到、实测确认）：
  - 切片正文 `<cache>/seg_bytes/` 只写不删 → 新增磁盘 GC 兜底。
  - 磁盘索引 `seg_url/*.json` 随清单刷新无限增长 → `save_index` 加上限；
    `[cache.segments].per_channel_max = 0`（默认）时用内置 `DEFAULT_INDEX_MAX = 4096`。
    截断按**登记时间**而非 suffix：suffix 是 `sha256(上游 URL)` 前缀，不是递增序号。
- **U-10 `[cache]` 容量键收口**：目标这四个容量键**都没观测到生效**（F-015 / S-CACHE-03）。
  - `disk_max_mb` 接进 GC：与 `[cache.gc].max_disk_mb` 同义，同时非 0 时取更小者。
  - `memory_max_mb` 接进内存索引：按每条约 256 B 估算成条目预算，与
    `[cache.segments].global_max` 取更小者。
  - `segment_cache_max_mb`、`m3u8_max_entries`、`segment_max_entries` 保留键面、**不消费**：
    默认值非 0（`50`/`500`/`2000`），与用户显式填写的同值无法区分，生效会静默压容。
  - 两个生效键默认 `0` = 不限，所以默认配置的行为与序列化都不变。
  - 文档：`04-config-and-storage.md` §11.5、`06` §3.4。
- **系统设置页字段级表单**（V-10 收尾的一部分）：
  - 新 `frontend/src/settings.tsx`：一次 `GET` 取整份配置到草稿，顶部「保存设置」一次 `PUT` 回去
    （对齐目标的「修改后点击保存生效」）。
  - 九张卡片：服务器、面板认证、切片伪装、监控、缓存配置、预载配置、磁盘回收、CDN 兜底、
    切片缓存；未覆盖的段保留最下面的「其余字段（JSON）」兜底编辑。
  - 新密码在浏览器里算 SHA-256（`frontend/src/sha256.ts`；不用 `crypto.subtle`，面板常走 http）。
    实现与 Node `crypto` 对拍 0–2000 字节（含填充边界与非 ASCII）全等。
  - 新冒烟 `scripts/smoke-settings.sh` 六组断言：配置 `GET→PUT→GET` 逐字节等价、字段路径存在性、
    U-10 被服务端消费（改 `disk_max_mb` 后推荐值转 `manual`）、口令哈希契约、前端资源已内嵌。
  - 基线：`docs/design/reference/img/p12-settings.png`；字段清单见 `05` §4.1。
- **字幕设置卡片 + `[subtitle]` 建模**：
  - `[subtitle]` 原先没建模，整段落在 `extra` 里：新配置读不到默认值，面板保存还会把默认值写坏。
    现在按目标形状建成类型化段（`SubtitleCfg` / `SubtitleStyleCfg` / `SubtitleTranslateCfg`），
    默认值取自目标抓包（含 `style.position = 90`、`font_size = "80%"`、`timeout_ms = 1500`）。
  - 两处消费点（`media.rs::subtitle_translate`、`epg.rs::translate_credentials`）改为读类型化字段，
    删掉 `extra.get("subtitle")` 的取值路径。
  - 面板新增「字幕设置」卡片：样式开关 / 字号 / 颜色 / 字重 / 位置滑杆、机翻开关、目标语言多选
    （首个选中项即主目标语言）、双语与译文上下、译轴默认与超时、MTranServer 与 API Key，
    外带「测试」（`POST /panel/api/subtitle/mtran/test`）与「清理译文缓存」。
  - 行为变化：`translation_first` 缺省由我们旧的非目标默认 `false` 改为目标默认 `true`（译文在上），
    两个契约测试的期望随之更新。
- **分享卡片与素材页补强**：
  - 系统设置页新增「分享（多 VPS）」卡片：`enabled`、`role`、`key`、`trust_ips`、`master_url`、
    `follower_urls`、`poll_secs`；`share_token` 是目标键，本实现未消费，卡片上标明仅回显。
    `[share]` 后端本来就在跑（`src/share.rs` 主从拉取 + `GET /api/share/pull`），所以不是死配置。
  - 视频素材页加「复制外链」（`location.origin + /assets/<文件名>`）与「未被引用」标记 + 过滤开关。
  - 弃用判定是本实现自定：配置里没有任何字符串指向 `/assets/<文件名>` 即为未被引用；只标不删。
- **全量覆盖对比与规划**（新 `docs/design/07-coverage-and-plan.md`）：
  - 口径：`docs/spec/08-target-spec.md` 的 160 条 `S-*` 逐域对比「本实现做没做」；
    特别区分「目标侧未验」（`🔶`/`⏳`）与「本实现缺口」，不把前者当欠账。
  - 结论：端点面（130+ 路由）、六模式、订阅、认证、EPG/台标/录制/回看后端、伪装、缓存、
    配置键面、CLI、依赖检测均已实现；缺口集中在**面板 UI**。
  - 缺口分四组：A 面板接线（8 项，端点已存在）、B 需新后端（概览实时指标）、
    C 冲突与文档债（分组排序冲突、i18n 决策）、D 已登记不实现。
  - 规划：P2 文档债 → P1 面板接线 → P3 指标 → P4 深化；难验证项各自给了对比方案。
- **P1 面板接线（进行中，A1/A2/A3/A4/A6/A7/A8/A9/A10 已完；只剩 A5）**：
  - 设置页「DASH 调优」卡片（三组折叠，覆盖 `[cache.dash]` 全部键，组名与说明沿用 `p12` 文案）；
    概览页快速操作（刷新状态 / 清空缓存 / 热重载配置 / 导出全部 M3U）。
  - 分组管理：分组卡与频道行各加 `▲`/`▼`，机制与目标一致（前端改数组顺序 →
    `PUT /panel/api/config` 全量保存，**无新端点**；订阅顺序即数组顺序）；工具栏补
    `M3U 导入` / `JSON 导入` / `JSON 覆盖` / `导出全部`；频道行补配置型徽章
    （`常驻`=`livestream`、`全轨道`=DASH 且 `selected_tracks` 空、`tvg-id`/`Logo`）。
  - 新增 `scripts/smoke-groups.sh`（41 条）：排序经 `PUT /config` 往返后订阅顺序随之改变、
    三种导入语义与错误码、徽章字段可读、前端入口已内嵌。
  - 运行日志加级别过滤（全部/TRACE…ERROR）；顺手剥掉环形缓冲行里的 ANSI 颜色码
    （原先直接塞进 `<pre>` 会显示成控制符）。
  - 订阅聚合：源行加「读取分组」（`POST /panel/api/subs/groups`），结果表可「加入映射」。
  - 录制页接「从 EPG 加载节目」：按频道的 EPG 键拉节目单，逐条「填入」（用已含
    `record_pad_*` 的 `rec_start`/`rec_end`）或「排期」（`/schedule/from-epg` 单条）。
  - 台标库：上传前浏览器侧转码（最长边等比缩放 + 可选转 WebP，目标同）；
    每张台标加「动效」（6 效果 → `/logo/animate`，显示帧数/尺寸/时长/体积变化）
    与「还原」（`.orig/` 备份回滚）。
  - 安全管控：新增「访问控制」卡片（白名单 / 会话与 Token / 限流与会话 / 登录防爆破）。
    `[auth]` 的 13 个原本落在 `extra` 里的目标键（`l1_*`、`l2_*`、`violation_*`、
    `ban_*`、`auth_fail_ban_*`）已建成类型化字段，默认值与目标 `config.toml` 一致；
    本实现不消费的键在界面与模型注释里都标 `未消费`。
  - 冒烟补强：`smoke-security.sh` 增 10 条（键面 25 个、未消费键默认值、白名单写入与
    回读、前端 chunk needle）；`smoke-settings.sh` 增 23 条 `auth.*` 路径断言。
  - 冒烟补强：`smoke-logo.sh` 增 15 条（动效 10 帧/2220 ms、`quality` 必须是字符串、
    还原后体积回原值、前端 chunk 含 animate/restore/toBlob/webp）；
    `smoke-settings.sh` 增 `/subs/groups` 与级别词 needle；`smoke-m4.sh` 增
    「每行都能判出级别」（复刻前端的 ANSI 剥离 + 级别抽取）；`smoke-schedule.sh` 增
    节目单形状与 `from-epg` 的逐条成功/失败语义。
  - 三条过期断言修正（都是断言落后于实现，不是实现回归）：
    `smoke-m1.sh` 的 `x-tvg-url` 首行与 Token 取行方式；`smoke-m4.sh` 的冷却日志文案
    （实际是「DASH 连续失败，进入冷却」+ `cooldown_secs=3600` 字段）。
- **质量门**：`cargo fmt --check`；285 项测试通过（203 单元 + 82 契约）；
  clippy 0 error（27 条历史 warning，新增文件 0 条）；`pnpm build`；
  冒烟 `smoke-m2`/`smoke-panel`/`smoke-cli`/`smoke-backup`/`smoke-assets`/`smoke-share` 通过。

2026-10-24：

- **V-07 切片 TS 结构对齐**：
  - Proxy 模式切片与上游逐字节相同。
  - Remux 切片改为上游 TS 连续字节切片，不改 PID、不插 SDT、不加水印。
  - 首片从源流第一个 TS 包开始；后续片复制关键帧前的最近 PAT/PMT。
  - Remux 切片路径从 `rseg_` 改为目标的 `seg_<12 位序号>.<ext>`。
  - 首两片与目标 SHA-256 完全一致。
  - 证据：`docs/findings/dynamic/09-recording-livestream-remux-subtitle.md` §1.5、`raw/v0.8.2-ts-structure.txt`。
- **V-05 每频道代理按目标对齐**：
  - 频道代理覆盖分组代理；频道代理失败不回退分组或直连。
  - 分组头与频道头合并，频道同名头覆盖；支持 `{name,value}` 与旧字符串。
  - `user_agent` 频道覆盖分组；分组显式 `User-Agent` 头优先。
  - 代理字段改为目标 `kind/addr/username/password`；`addr` 可省略协议。
  - 支持 `http`/`https`/`socks4`/`socks5`/`socks5h`，含 SOCKS5 用户名密码认证。
  - 默认上游 UA 对齐目标 Chrome/124。
  - 证据：`docs/findings/dynamic/02-modes-and-watchdog.md` §2.1。
- **V-06 自助解封按目标对齐**：
  - `/unban/whoami` 返回 `banned/enabled/ip/max_per_month/remaining/used`；IP 管控关闭时只返回 `enabled/ip`。
  - `POST /unban` 固定 `200` + `{"msg","ok"}`；只有成功解封消耗配额。
  - 配额耗尽不再返回 `429`；未封禁、管控关闭都返回 `ok:false`。
  - 移除目标未注册的 `/whoami`。
  - `X-Forwarded-For` 改为取末项，与目标代理追加语义一致。
  - 证据：`docs/findings/dynamic/11-security-and-metrics.md` §3.1。
- **U-06 台标动效改为动画 WebP**：
  - 输出固定 10 帧：前 9 帧 80 ms，末帧 1500 ms，`total_ms=2220`。
  - 生成 `VP8X`/`ANIM`/`ANMF` 容器，帧载荷为无损 `VP8L`。
  - PNG 与 WebP 改为纯 Rust 解码；其他格式保留 `ffmpeg` 回退。
  - 第 0 帧整幅、后续帧脏矩形；`x/y` 取偶数；无变化帧用 `2x2`。
  - `dirty_ratio` 按矩形面积整数百分比计算。
  - `breathe` 输出与目标逐字节一致；其他效果对齐透明像素、级别量化和移动路径。
  - `logo/list` 的 `animated` 改为“`.webp` 扩展名 + 动画内容”；已动画源图拒绝再次生成。
  - 证据：更新 `docs/findings/dynamic/12-logo-library.md`。
- **V-03 公开 EPG API 完成**：
  - `/epg/api/channels`、`/programmes/:channel`、`/search` 按目标 v0.8.2 抓包对齐。
  - 频道表含 `icon/id/kind/last_stop/name/programme_count/source_id/source_name`；生成时写 `viewer.json`。
  - 节目与搜索使用 Unix 秒；搜索只匹配频道名与节目标题、按开始时间排序、最多 200 条。
  - 输出 ID 统一小写；关闭态返回目标 JSON 错误；加入全局宽松 CORS。
  - 证据：F-023、`docs/findings/dynamic/20-public-epg-api.md`、`raw/v0.8.2-public-epg-api.txt`。
- **U-05 `[cache.dash]` 剩余参数完成**：
  - `publish_delay_secs>0` 按 `ceil(delay/切片时长)` 压后直播清单末端；`=0` 使用 `fetch_extra_segments`。
  - 看门狗接入 `watchdog_tick_secs`、`watchdog_stall_secs`、`watchdog_rebuild_stagger_ms`，并复用重建上限与冷却。
  - `fast_start_secs` 在三类目标场景中无可观测差异，保留配置字段，不改变输出。
  - 证据：F-021、`docs/findings/dynamic/18-dash-window-knobs.md`。
- **V-01 / V-02 DASH BaseURL 完成**：
  - 目标 `dash` 默认只取第一个 `<BaseURL>`，音视频一致。
  - `proxy`+MPD 把绝对/相对/缺失 `<BaseURL>` 全部改写为本地令牌路径，不再直连绝对地址。
  - 证据：F-022、`docs/findings/dynamic/19-dash-multi-baseurl-proxy.md`。

2026-10-23：

- **V-09 订阅聚合字段对齐**：
  - 实现 `ttl_secs`、`group_map`、`group_mode`、`merge_group`、`unlisted_groups`。
  - `ttl_secs=0` 按 3600 秒处理。
  - `group_map` 是选择集，不是“未列出保留全部”。
  - 未分组名为 `(无分组)`。
  - `group:` 同时匹配原分组与映射后分组。
  - 聚合按 URL 跨源去重。
  - `/subs/{status,groups,preview}` 与 `/panel/api/subs/*` 已对齐。
  - 证据：F-020、`docs/findings/dynamic/17-subscription-merge.md`。
- 提交：`66bacd9 feat(playlist): align subscription merge semantics with target`。
- 修复 `scripts/smoke-aggregate.sh` 的新分组语义。

2026-10-22 及以前：

- U-01 至 U-04 完成：`x-tvg-url`、EPG 端点、EPG 配置语义、`proxy`+MPD 裁剪。
- V-11 真实 HLS/M3U 验证完成：超时、半包、404 回退、302 错误页、重试、属性白名单、缓存头、扩展名保留。
- DASH 媒体窗口 `playlist_min_secs`、`start_time_offset_secs`、看门狗阈值与冷却已接入。
- 台标扩展名白名单与目标一致。
- 订阅聚合字段对拍完成。

### 1.4 质量门（2026-10-25）

| 项 | 结果 |
| --- | --- |
| `cargo fmt --check` | ✅ 通过 |
| `cargo test --offline --all-targets` | ✅ 285 项通过（203 单元 + 82 契约） |
| `cargo clippy --offline --all-targets` | ✅ 无 error；27 条历史 warning（本轮新增文件 0 条） |
| 冒烟脚本 | ✅ 低配置轮次重跑 6 个：`m2`、`panel`、`cli`、`backup`、`assets`、`share`；字段级表单后重跑 `m2`、`panel`、`settings`（新）；其余 8 个上次通过 |
| `pnpm build` | ✅ 通过（含 `tsc --noEmit`） |
| 发布校验 | ⏳ 本轮未重跑（上次 2026-10-24 双架构产物通过 `verify-release.sh`） |
| CI | ✅ 仅 `v*` tag 触发；fmt/clippy/test、CLI 冒烟与双架构校验已配置 |

### 1.5 当前工作区

- 代码基线：低配置优化已提交并推送 `rust` 分支，见 §1.7。
- `PROGRESS_bak.md` 保存 2026-10-23 之前的完整 1169 行记录。
- `dist/` 与 `samples/` 不入库。
- 目标测试容器、源站与本实现的临时实例应按需清理。

### 1.6 续接快照（2026-10-24 本轮结束）

本轮已完成并推送：

- `5f9d85c feat(panel)`：面板改为目标的 13 个一级分区；系统设置合并配置/状态/备份；台标与回看独立；订阅源规则/预检；EPG 手工映射/额外频道。
- `7859b52 feat(subs)`：订阅源正文落盘 `<cache>/sub_url/*.json`，状态落盘 `<cache>/subs/status.json`；重启后按源级 TTL 命中；`cache/flush` 清正文。
- `b101e23 fix(dash)`：proxy MPD 错令牌 → `302 error_url`；已有 `minimumUpdatePeriod` → `PT1S`；已有 `publishTime` → 当前 UTC；响应头改为 `application/dash+xml; charset=utf-8` 与 `max-age=1, s-maxage=1`。
- V-08 收口：目标 `<P>` 新增 5 个样本（`x1/c1`、`x1/c2`、`x2/c1`、`x_3/c-4`、`ts1/c1`）；约 2005 种输入模板与主流哈希族均不匹配；判定为低影响，停止黑盒穷举。
- 质量门：`cargo fmt --check`；237 项测试（168 单元 + 69 集成）；`pnpm build`；`smoke-panel.sh`、`smoke-logo.sh` 通过。
- 目标清理：已删除测试分组 `x1`、`x2`、`x_3`、`gprobe`；宿主 `:19994` 源站已停止；目标容器 `rea82v09` 保留。

遇到的问题与阻塞：

1. V-08 的 `<P>` 精确算法仍未解。目标为 stripped Rust；Hopper 不可用；环境无 `rizin`/`ghidra`；cargo-auditable 依赖表为空。功能影响低，保持自洽实现。
2. V-10 只完成 API 与分区级对齐。未做逐屏视觉/交互对拍；缺目标操作录像；系统设置仍是原始配置编辑，不是字段级表单。
3. 视频素材的 URL 转换/弃用资源列表、多 VPS 分享子页缺后端端点，面板未实现。
4. U-06 只有 `breathe` 效果逐字节一致；其余效果与缩放仍为近似。
5. `match-report` 目标 raw 抓包为 `text/plain` 表格，面板 JS 期望 JSON；冲突未解。
6. 扩展名轮换未覆盖 `.ts` 与无扩展名；Remux 后缀选择与目标不同（目标观测 `.gif/.png`，本实现轮换 5 种）。
7. `out_groups`/`dropped_groups` 顺序为目标哈希序，本实现为稳定排序；低影响。
8. V-01 URL 池回退顺序未验；目标 `url_pool_size>1` 多 BaseURL 曾失败并产生零切片，无法继续对拍。
9. MPD 本地令牌算法功能兼容，但不保证与目标逐字节相同。

本轮新增结论：

- 目标面板一级分区顺序：`dashboard, cache, groups, settings, security, stats, schedule, epg, logos, subs, livestream, assets, logs`。
- 目标入口扩展名由分组 `format` 决定：`hls` → `.m3u8`，`mpd` → `.mpd`，与 `mode` 无关。探测 DASH proxy 时必须先把分组 `format` 设为 `mpd`。
- 目标新建频道用 `POST /panel/api/groups/{gid}/channels`；`PUT .../channels/{cid}` 只更新已有频道。
- 目标 proxy MPD 的 `publishTime` 每次请求更新为当前 UTC；上游缺该属性时不插入。
- 目标 proxy MPD 错令牌跳 `server.error_url`，不是 `not_found_url`。

下一步任务（按优先级）：

1. V-10 收尾：补系统设置字段级表单、素材 URL 转换/弃用资源、分享子页；能拿到目标录像后做逐屏对拍。
2. 扩展名轮换：补 `.ts` 与无扩展名的目标证据；决定 Remux 后缀是否改为目标观测集合。
3. `match-report`：先确认面板调用方与目标 raw 形状，再决定保留 `text/plain` 还是兼容 JSON。
4. V-01：若目标 URL 池可修复，补 `url_pool_size>1` 的首选与回退顺序；否则保持登记。
5. U-06 可选：逐像素对齐其余 5 种台标效果与缩放采样。
6. 每次改完运行完整质量门，提交并立即推送 `rust` 分支。

当前工作区：除未跟踪的 `.opencode/` 外无未提交改动；`frontend/dist` 与 `samples/` 已重建但不入库。

### 1.7 续接快照（2026-10-25 本轮结束）

本轮做的事：把低配置优化全部落地，并修掉设计稿登记的两个真实缺陷。
设计见 `docs/design/06-low-spec-optimization.md`，验收状态见其 §10。
`06` 已按 STE100 精简：实测段的真实域名与路径改为占位符，任务清单内容并入 `06`。
本轮低配置改动已合并为**单个提交**。带真实域名的旧提交不再可达。

该提交含三部分：

- 后端：`src/config/model.rs`、`src/http/routes/{stream,admin,cdn}.rs`、`src/http.rs`、
  `src/proxy/{media,segments}.rs`、`src/state.rs`、`src/lib.rs`、`src/main.rs`、`Cargo.{toml,lock}`；
  新文件 `src/cache_estimate.rs`、`src/cache_gc.rs`、`src/http/routes/cdn.rs`
- 面板：前端新 `frontend/src/lowcfg.tsx`，改 `panel.tsx`、`cache.tsx`、`groups.tsx`、`style.css`
- 文档：`04-config-and-storage.md` §11、`06`（精简与脱敏）、`docs/design/config-low-spec.toml`、
  `packaging/config.toml.example` 注释、本文件

未做 / 待办：

1. **T0：CDN 面板侧配置**——`06` §2 的五处改动与规则 A/B 需要在四家 CDN 的控制台手工完成
   （强制 TTL、关闭遵循源站头、关 Set-Cookie 注入、关图片改写、下发 `X-Forwarded-Proto: https`，
   然后 Purge All）。验收命令见 `06` §2.4。这是唯一还没做的一环。
2. **真机验收**：`06` §10 的 S4（造 200 MB → 一个周期回落到上限；占满磁盘走紧急清理）
   与 S6（64 MB 实例 24h 无 OOM、`seg_bytes` 不涨、tick 不抖动）需要目标 VPS，
   本地只覆盖了公式与淘汰逻辑的单测。
3. **`[cache]` 容量键已收口**（U-10）：`disk_max_mb` 接进 GC（与 `[cache.gc].max_disk_mb`
   取更小者），`memory_max_mb` 接进内存索引条目预算；`segment_cache_max_mb`、
   `m3u8_max_entries`、`segment_max_entries` 保留键面但不消费（默认值非 0，与显式填写的
   同值无法区分）。`[cache.segments]` 的 `global_max`/`per_channel_max` 仍默认 `0`（不限），
   低配样例里已显式设 `per_channel_max = 200`。见 §2.1、`04` §11.5。
4. 上一轮遗留的 V-08/V-10 与其余登记差异（见 §2.2、§2.3）状态未变。

下一轮建议顺序：

1. 在 CDN 面板完成 T0，然后核对 `06` §2.4 的四条命令。
2. 在目标 VPS 上跑 S4/S6 真机验收。
3. 再考虑上一轮的 V-10 面板收尾与零散登记差异。

### 1.8 历史脱敏（2026-10-25）

`docs/findings/dynamic/16-real-upstream-hls.md`（F-019）与 `PROGRESS_bak.md` 里写着实测上游的
域名与品牌名，从 `1b54f54` 起就在库里。处理分两步：

1. **当前版本**：新增提交把域名改为 `<实测域名>`、品牌名改为「源站」。路径形状、字节数、
   缓存头等观测结论都保留。
2. **历史**：用 `git filter-branch --tree-filter` 重写 `rust` 上 `1b54f54` 之后的 20 个提交
   （含本轮 4 个），在每个提交的工作树上做同样替换。`main` 与 7 个 release tag 不含该文件，
   未动。`git log -S` 对两个域名与品牌名在新线上都返回空。

重写把短哈希全部改号，`PROGRESS.md` 里引用的四个已同步：`50ec323`→`b101e23`、
`56066c9`→`7859b52`、`96d5f7c`→`5f9d85c`、`6911fa8`→`66bacd9`。

发布：重写后的线先以 `rust-clean` 分支推送（`109fe03`）；远端 `rust` 仍是旧线 `dbfd483`。
把 `rust` 切到干净线需要一次强制推送，**待授权**。

残留：GitHub 上按 SHA 仍可能取到已推送过的旧对象，彻底清除需走 GitHub Support。
本地 `refs/original/`、reflog 与 `backup/rust-before-squash-20261025` 也留有旧对象。

## 2. 待处理任务

### 2.1 代码缺口

| ID | 优先级 | 任务 | 当前状态 | 下一步 |
| --- | --- | --- | --- | --- |
| U-06 | P1 | 台标动效与解码 | ✅ 完成：动画 WebP、10 帧、脏矩形、PNG/WebP 纯 Rust 解码；其他格式回退 `ffmpeg` | 可选：逐像素对齐其余 5 种效果与缩放采样 |
| U-07 | P2 | 订阅源缓存跨重启 | ✅ 完成：正文落盘 `<cache>/sub_url/`，状态 `<cache>/subs/status.json`；重启后按源级 TTL 命中 | 可补目标缓存文件名格式对拍（不影响行为） |
| U-08 | P1 | 磁盘回收与索引上限 | ✅ 完成：新增 `[cache.gc]` 与 `src/cache_gc.rs`；`save_index` 加上限 | 真机跑 S4/S6（见 §1.7） |
| U-09 | P1 | CDN 兜底与被动实测推荐 | ✅ 完成：`[cdn]`、`src/cache_estimate.rs`、`POST /panel/api/cdn/estimate` | T0 的 CDN 面板侧配置待人工完成 |
| U-10 | P2 | `[cache]` 的容量键 | ✅ 完成：`disk_max_mb` 接入 GC（与 `[cache.gc].max_disk_mb` 取更小者）；`memory_max_mb` 接入内存索引条目预算；其余三个键登记为兼容保留（目标同键亦不生效） | 无 |

### 2.2 待补目标证据

| ID | 项 | 当前结论 | 所需证据 |
| --- | --- | --- | --- |
| V-08 | 切片前缀 `<P>` 算法 | 自研 `sha256("<gid>:<cid>")[:16]`；目标算法未解；新增 5 个样本与约 2005 种模板排除 | 低影响；客户端只读清单，保持自洽方案 |
| V-10 | 面板与录制/EPG 交互 | API 对拍 22/22 差异 0；13 个一级分区已对齐；系统设置页字段级表单覆盖服务器 / 面板认证 / 切片伪装 / 监控 / 缓存配置 / 预载配置 / 磁盘回收 / CDN 兜底 / 切片缓存 / 字幕设置 / 分享；素材页补复制外链与未被引用标记（见 `05` §4.1） | DASH 调优卡片与语言（待决策）；逐屏对拍按 `05` §4.1 做字段清单对拍 |

已完成：

- V-06 `/unban` 配额与 `/unban/whoami` ✅
- V-03 公开 `/epg/api/*` 形状 ✅
- U-05 `[cache.dash]` 发布延迟与看门狗参数 ✅
- V-01 DASH 多 `<BaseURL>` 请求分布 ✅（回退顺序未验）
- V-02 `proxy`+MPD 绝对 `<BaseURL>` 本地反代 ✅
- V-04 `/subs/{groups,preview}` 字段 ✅
- V-09 `[playlist]` / `[share]` 目标语义 ✅
- V-11 真实 HLS/M3U 验证 ✅
- U-07 订阅源缓存跨重启 ✅
- DASH 本地反代错误令牌 → 302 `error_url`；`minimumUpdatePeriod` → `PT1S`；`publishTime` → 当前 UTC；MPD 头对齐 ✅
- 低配置优化：`[cdn]`、`[cache.gc]`、分组/频道预取覆盖、被动实测推荐、`POST /panel/api/cdn/estimate`、面板推荐卡与三张表单 ✅

### 2.3 已登记差异与不实现项

| 项 | 结论 | 依据 |
| --- | --- | --- |
| 切片前缀 `<P>` 算法 | 未解；仅依赖 `(gid,cid)`，低影响 | F-017 |
| 缓存容量上限 | 不实现；目标配置存在但未生效 | F-015 |
| 旧数据迁移 `./var/dash` | 不实现；仅 v0.8.8 行为 | F-015 |
| DASH 多 CDN 故障切换 | 未复刻；只做顺序回退 | F-015 |
| CENC/ClearKey 解密 | 不实现；按原字节代理 | F-009 |
| `l1/l2` 限流与每 IP 会话上限 | 不实现；触发条件未复现 | F-010 |
| `random_id_enabled` | 仅回显；行为未观测 | F-018 |
| `cache.backend=memory` | 仅回显；实际仍落盘 | F-015 |
| 播放侧 `compat_mode` 与 `srt_ua_keywords` | 不实现；目标未测出行为 | D19 |
| `match-report` 形状 | 保留目标 raw 抓包的 `text/plain` 表格；面板 JS 期望 JSON，冲突未解 | F-013 |
| 扩展名轮换算法 | 未完全破解；只保证取自 `[cache.disguise].formats` | F-018、F-019 |
| `out_groups`/`dropped_groups` 顺序 | 目标为哈希序；本实现稳定排序 | F-020 |
| `[cdn]` / `[cache.gc]` / 分组 `prefetch` | 目标 v0.8.2 无此配置面；本实现新增，默认全部不改变既有行为 | `06` §6、§7 |
| 磁盘索引内置上限 4096 | 目标未观测；本实现为防 `seg_url/*.json` 随清单刷新无限增长而设，`per_channel_max > 0` 时按配置 | `06` §6.2 |
| 磁盘回收按 mtime LRU | 不按频道整组淘汰；`segment_path` 的文件名经 `slug()` 后无法反解回 `gid/cid` | `06` §6.2 |
| `[cache].disk_max_mb`、`memory_max_mb` | **生效**：目标不消费这两个键（F-015），本实现分别接进 GC 与内存索引；默认 `0` = 不限，行为与序列化都不变 | U-10、`04` §11.5 |
| `segment_cache_max_mb`、`m3u8_max_entries`、`segment_max_entries` | **不消费**：目标同样未观测到生效；默认值非 0（`50`/`500`/`2000`），与显式填写的同值无法区分，生效会静默压容 | F-015、S-CACHE-03、`04` §11.5 |
| 内存索引条目按 256 B/条估算 | 估算值，只作上限、偏保守；目标给的是字节预算，本实现按条目计数 | U-10、`04` §11.5 |
| `[subtitle]` 段由 `extra` 改为类型化 | 原先未建模，新配置读不到默认值；现按目标形状建模，键面与默认值一致 | 目标 `v0.8.2-config.json` |
| `[subtitle.translate].translation_first` 缺省 | 由 `false` 改为 `true`，与目标默认一致；两个契约测试期望已更新 | 同上 |
| `[subtitle].style.*` 样式注入 | 保留键面、**不实现**：目标把样式注入 VTT 的行为未观测；面板可编辑，运行时不生效 | `05` §4.1 |
| 面板语言（i18n） | **已定：不做**。目标 `p12` 有语言选择器且会切换界面语言；我们只有中文文案，只放切不动的选择器是「配了没反应」的反模式。将来要做需先抽全部文案 | `07` §3.C4、`05` §9 |
| `[share].share_token` | 目标键、**本实现未消费**（仅回显）：未找到消费点，做成开关会误导，故在卡片上标明 | 无消费点、目标 `v0.8.2-config.json` |
| 素材「未被引用」判定 | 本实现自定：配置里没有字符串指向 `/assets/<文件名>`；目标未观测，只标不删 | `05` §4.1 |
| `08` S-BUG-02「面板无分组排序」 | **结论错误，已更正**：`p02-groups.png` 显示分组卡与频道行都有 `▲`/`▼`，组名左侧另有拖拽柄；工具栏含 JSON 导入/覆盖、导出全部、多选 | `reference/img/p02-groups.png`、`07` `D-006` |

### 2.4 面板缺口（C1a 字段清单对拍结果）

> 2026-10-25 补：全量覆盖对比（`docs/spec/08` 的 160 条 `S-*` vs 本实现）与分期规划已成文，见 [`docs/design/07-coverage-and-plan.md`](docs/design/07-coverage-and-plan.md)。本节只留面板侧缺口；该文档另有后端能力缺口（概览指标）与冲突登记。

基线 `docs/design/reference/`（目标 30 张截图 + 关键元素清单）与 `05` §4.2。逐项证据见该节。
按「能不能直接做」分组：

**能直接做（端点或后端能力已存在）** —— 进度见括号


1. `schedule`：「从 EPG 加载节目」——端点 `/panel/api/schedule/from-epg` 已在，面板未接。
2. `logos`：动效生成（`m09`：6 种效果、质量、生成）与转码参数（最大边 / 质量 / 转 WebP）——
   后端 U-06 的动效与转码已实现。
3. `security`：白名单、限流 `l1`/`l2`、会话 TTL 与并发、防爆破——配置键都在 `[auth]`，只差展示与编辑。
4. `logs`：级别过滤——日志接口已按级别返回。
5. **`groups`：分组与频道排序、工具栏与频道徽章**（复核 `p02-groups.png` 后确认是真缺口）——目标有 `▲`/`▼` 与拖拽柄、`JSON 导入`/`JSON 覆盖`/`导出全部`/`多选`、频道行的状态点与 `异常`/`常驻` 徽章。**同时更正一处规格错误**：`08` 的 `S-BUG-02` 原记「面板无排序功能」，与本截图矛盾；已按证据更正该条并登记`docs/spec/07` `D-006`。见 `07` §3.A9–A11。

**需要新后端能力**

6. `dashboard` 的实时指标（下行·回源、上行·出网、进程内存、CPU、整盘内存峰值）与快速操作
   （清空缓存、热重载、刷新状态、导出全部 M3U）。指标采集是新工作量；快速操作里有三个端点已在
   （`/cache/flush`、`/reload`、导出 M3U）。

**待核对**

7. `subs` 源行的「读取分组」；`epg` 的翻译设置（`m08` 翻译模型）与采集节奏；弹窗族逐项核对。

## 3. 建议推进顺序

1. **T0 + 真机验收**：在 CDN 面板按 `06` §2 配五处，跑 §2.4 四条命令；在目标 VPS 上跑 S4/S6。
2. **V-10 收尾**：DASH 调优卡片与语言（待决策）；逐屏对拍按 `05` §4.1 做字段清单对拍。
3. 扩展名轮换与 `match-report` 形状：先补证据，再决定是否改实现。
4. V-01 URL 池回退：目标可复现时再补；否则保持登记。
5. 可选：逐像素对齐 U-06 其余效果与缩放采样。
6. 每项完成后运行完整质量门，更新本文件与对应 `docs/`，提交并立即 push `rust` 分支。

## 4. 关键设计结论

- `[epg]` 的 `out_id = tvg_id || channel.id`。
- EPG 匹配顺序：手工映射 → `tvg_id` → 名称。
- 名称规范化：删除全部空白并转小写。
- 手工映射保存到 `<cache>/epg/mappings.json`；额外频道保存到 `<cache>/epg/extra.json`。
- EPG 源正文缓存到 `<cache>/epg/raw/<source_id>.xml`；`regenerate` 只读 raw 缓存。
- `public_viewer_enabled=false` 时公开 `/epg` 返回 404。
- `allow_scripts=false` 时拒绝脚本源。
- DASH 轨道命名：`video_{高}p`、`audio-{n}`、`text-{n}`。
- DASH 媒体清单路径：`/{gid}/{cid}/{track}/index.txt`。
- DASH 媒体窗口取 `[cache.dash].playlist_min_secs`；分组 `mpd_keep_seconds > 0` 优先。
- 直播 DASH 清单按 `start_time_offset_secs` 输出 `#EXT-X-START:TIME-OFFSET=-…`。
- CENC/ClearKey 不解密，按原字节代理。
- 台标扩展名白名单：`png/jpg/jpeg/webp/bmp/avif`。
- 订阅源 `group_map` 是选择集；`group_mode` 未知值按 `keep`。
- 订阅源正文落盘到 `<cache>/sub_url/`，状态在 `<cache>/subs/status.json`；重启后仍按源级 TTL 命中。
- `map` 模式空映射值保留原名；`merge` 模式仅在 `merge_group` 非空时统一分组。
- 聚合按 URL 去重，范围覆盖同源与跨源。
- 源 `ttl_secs=0` 按 3600 秒；源级 TTL 优先于 `merge_ttl_secs`。
- `[cdn].enabled=false`（默认）时，切片响应头、空体状态码与 `self_base` 推导逐字节等于旧行为。
- 推荐 TTL = `clamp(ceil60(W + 2T), 300, 900)`，取各分组最大值；无实测用回退 600s。
- 磁盘上限 = `clamp(min(工作集, 可用空间×0.8 − 100MB), 50MB, 总容量×0.7)`；工作集 = `round10(3 × N × seg_mb × 1.5)`。
- `statvfs` 取 `f_bavail`（非特权用户可写块数）与 `f_frsize`；失败回退 150MB。
- 工作集按**十进制 MB**（10⁶）计算，与设计稿 §3.3 的算例一致。
- `[cache].disk_max_mb` 与 `[cache.gc].max_disk_mb` 同义：同时非 0 时取更小者；`0` = 不限。
- `[cache].memory_max_mb` 是内存索引的字节预算，按每条约 256 B 估算成条目数，
  再与 `[cache.segments].global_max` 取更小者。
- `segment_cache_max_mb`／`m3u8_max_entries`／`segment_max_entries` 保留键面但不消费：
  默认值非 0，与用户显式填写的同值无法区分。
- 预取合并顺序与 `proxy_for` 同：**频道 > 分组 > 全局**；覆盖字段是三态 `Option`，未设置不输出。
- 磁盘切片索引按**登记时间**截断（suffix 是哈希，不能当序号）；`per_channel_max=0` 时用内置 4096。
- GC 淘汰顺序：超龄（`max_age_secs`）→ 超量 mtime LRU → `free < reserve_mb` 时紧急（忽略龄，删到 cap/2）。
- 估算统计落盘 `<cache>/cache_estimate.json`，带 `v` 版本号，不匹配即丢弃；落盘由 GC tick 节流（≥300s）。

## 5. 主要文档入口

| 主题 | 路径 |
| --- | --- |
| 目标规范 | [`docs/spec/08-target-spec.md`](docs/spec/08-target-spec.md) |
| 验证矩阵 | [`docs/spec/07-baseline-and-verification.md`](docs/spec/07-baseline-and-verification.md) |
| HTTP 契约 | [`docs/design/03-http-contract.md`](docs/design/03-http-contract.md) |
| 配置与存储 | [`docs/design/04-config-and-storage.md`](docs/design/04-config-and-storage.md) |
| 低配置优化（设计） | [`docs/design/06-low-spec-optimization.md`](docs/design/06-low-spec-optimization.md) |
| 低配置示例配置 | [`docs/design/config-low-spec.toml`](docs/design/config-low-spec.toml) |
| 架构 | [`docs/design/02-architecture.md`](docs/design/02-architecture.md) |
| 发现索引 | [`docs/findings/README.md`](docs/findings/README.md) |
| 订阅聚合证据 | [`docs/findings/dynamic/17-subscription-merge.md`](docs/findings/dynamic/17-subscription-merge.md) |
| 真实 HLS 验证 | [`docs/findings/dynamic/16-real-upstream-hls.md`](docs/findings/dynamic/16-real-upstream-hls.md) |
| 历史进度 | [`PROGRESS_bak.md`](PROGRESS_bak.md) |

## 6. 历史归档

原 `PROGRESS.md` 的完整内容已迁移到 [`PROGRESS_bak.md`](PROGRESS_bak.md)。

归档包含：

- P0 至 P4 的阶段记录。
- 每轮实现、对拍、测试与发布细节。
- 旧的任务编号与验证命令。
- 2026-10-23 之前的逐项完成记录。

本文件只保留当前状态、残余缺口、证据需求与推进顺序。
