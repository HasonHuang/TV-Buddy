# 05 · 前端设计

> 设计版本 v1.0 ｜ 2026-10-19 ｜ 状态：**初稿**
> 上游：[`03-http-contract.md`](03-http-contract.md)、[`reference/README.md`](reference/README.md) ｜ 证据：F-003、F-004、F-012 §3

## 1. 目标与纪律

| 目标 | 说明 |
| --- | --- |
| 布局与交互对齐 | 逐屏对照 [`reference/img/`](reference/) 的目标面板截图 |
| 实现自研 | **不复制**目标的 HTML/CSS/JS 与图片资源（`spec/05`） |
| 契约一致 | 只调 `/panel/api/*`，端点与字段名不变（`03` §6） |
| 单文件发布 | 产物经 `rust-embed` 嵌入二进制（F-003） |

## 2. 技术形态

| 项 | 选择 | 理由 |
| --- | --- | --- |
| 构建 | Vite 6 | 多页入口简单，构建快 |
| 框架 | React 18 + TypeScript | 与 E1 决策一致 |
| 样式 | 自研 CSS + CSS 变量（暗色） | 不引入重型 UI 库；便于逐屏对齐 |
| 状态 | 组件内 `useState` + 少量模块级缓存 | 面板状态量小 |
| 请求 | 自研 `api()` 封装 | 统一带 `Bearer`、统一错误体 |
| 路由 | 单页内分区切换（无前端路由库） | 与目标一致：一个 HTML、侧栏切页 |

## 3. 页面与产物

| 入口 | 路由 | 页面 | 状态 |
| --- | --- | --- | --- |
| `panel.html` | `/panel` | 管理面板（侧栏 + 分区） | M0 骨架 ✅ |
| `epg.html` | `/epg` | 节目单查看器 | P1 |
| `logo.html` | `/logo` | 公开台标库 | P2 |
| `unban.html` | `/unban` | 自助解封 | P1 |

资源前缀 `/__assets/*`（S-EP-16）。构建产物：4 个 HTML + JS/CSS。

## 4. 管理面板信息架构

目标面板有 **13 个分区 + 14 个弹窗**（`reference/README.md` §3）。分区与后端依赖：

| # | 分区 | 目录键 | 主要接口 | 阶段 |
| --- | --- | --- | --- | --- |
| 1 | 概览 | `dashboard` | `/status`、`/stats`、`/cache/stats` | M0 部分 ✅ |
| 2 | 缓存状态 | `cache` | `/cache/stats`、`/cache/flush` | M2 |
| 3 | 分组管理 | `groups` | `/groups*`、导出、M3U | **M1** |
| 4 | 系统设置 | `settings` | `/config`（全量保存） | ✅ 字段级表单见 §4.1（字幕/DASH/语言待补） |
| 5 | 安全管控 | `security` | `/security/*` | P1 |
| 6 | 频道热度 | `stats` | `/stats` | M3 |
| 7 | 定时录制 | `schedule` | `/schedule*` | P1 |
| 8 | 节目指南 | `epg` | `/epg/*` | P1 |
| 9 | 台标管理 | `logos` | `/logo/*` | P2 |
| 10 | 订阅聚合 | `subs` | `/subs/*` | P2 |
| 11 | 回看管理 | `livestream` | `/livestream/*` | P1 |
| 12 | 视频素材 | `assets` | `/assets/*` | P2 |
| 13 | 运行日志 | `logs` | `/logs` | M3 |

弹窗体系（对齐目标，共 13 类）：分组编辑、频道编辑/新建、M3U 导入、录制任务、EPG 脚本、源频道浏览器、翻译模型、台标动效、批量解封、后台节流详情、台标选择器、确认框。M0 只实现确认框。

### 4.1 系统设置页的字段级表单

基线：`reference/img/p12-settings.png`（目标同一分区的 10 张卡片）。
一次 `GET /panel/api/config` 取整份配置到草稿，顶部「保存设置」一次 `PUT` 回去
（对应目标的「修改后点击保存生效」）。

| 卡片 | 配置段 | 字段 | 状态 |
| --- | --- | --- | --- |
| 服务器 | `[server]` | `bind`、`timezone_offset_hours`、`catchup_tpl`、`log_level`、`home_url`、`not_found_url`、`error_url`、`m3u_header_lines` | ✅ |
| 面板认证 | `[panel]` | `username`；新密码（浏览器算 `password_sha256`） | ✅ |
| 切片伪装 | `[cache.disguise]` | `enabled`、`subtitle`、`playlist`、`master`、`formats` | ✅ |
| 监控 | `[health]`、`[stats]` | `enabled`、`interval_secs`、`timeout_secs`、`concurrency`；`enabled`、`online_window_secs` | ✅ |
| 缓存配置 | `[cache]` | `backend`、`disk_path`、`disk_max_mb`、`memory_max_mb`；清空本地缓存 | ✅ |
| 预载配置 | `[cache.prefetch]` | `enabled`、`concurrency`、`timeout`、`prefetch_limit`、`mpd_readahead`、`skip_tail` | ✅ |
| 磁盘回收 | `[cache.gc]` | §11.2 的八个键；立即估算 | ✅ |
| CDN 兜底 | `[cdn]` | §11.1 的键 | ✅ |
| 切片缓存 | `[cache.segments]` | `global_max`、`per_channel_max`、`retain_count`（本实现扩展） | ✅ |
| 字幕设置 | `[subtitle]` | `style.*`（开关 / 字号 / 颜色 / 字重 / 位置滑杆）、`translate.*`（机翻、目标语言多选、双语与译文上下、译轴、超时、MTranServer、API Key）+ 测试与清理译文缓存 | ✅（样式注入未实现，兼容保留） |
| DASH 调优 | `[cache.dash]` | 播放体验 / 网络性能 / 特殊场景三组 | ⏳ |
| 备份与迁移 | — | 复用 `BackupPage` 卡片 | ✅ |
| 语言 | — | 界面语言（目标为「跟随浏览器」） | ⏳ 待决策 |
| 分享（多 VPS） | `[share]` | `enabled`、`role`、`key`、`trust_ips`、`master_url`、`follower_urls`、`share_token`（仅回显）、`poll_secs` | ✅ |
| 其余字段（JSON） | — | 未做成表单的段 | 兜底 |

- 保存写回**整份配置**，所以「其余字段（JSON）」里的改动也会一起生效。该卡片剩下 DASH 调优、
  在流等段（见上表的 ⏳ 行）。
- 视频素材页（`p13`）另加两项行内操作：**复制外链**（`location.origin + /assets/<文件名>`，即公网
  `GET /assets/*`）与**未被引用**标记。判定标准为本实现自定：配置里没有任何字符串指向
  `/assets/<文件名>`；目标未观测，故只标不删。
- 契约由 `scripts/smoke-settings.sh` 守住：`GET → PUT → GET` 必须逐字节等价（不丢未知键、
  不改类型），字段路径必须存在，改口令后新口令可登录。
- 新密码在浏览器里算 SHA-256（`frontend/src/sha256.ts`）。不用 `crypto.subtle`：
  面板常走 `http://<内网 IP>`，非安全上下文里它不可用。

### 4.2 逐屏字段清单对拍（C1a）

基线：`reference/README.md` §3.1 / §3.2 的「关键元素」列与对应截图。判据是**元素与信息层级**，
不做像素比对（缺目标操作录像）。已对齐的分区不列；下表每一行都有可复现的证据。

| 分区 | 目标有的元素 | 本实现现状（证据） |
| --- | --- | --- |
| `dashboard` | 实时指标（下行·回源、上行·出网、进程内存、CPU、整盘内存峰值）；快速操作（清空缓存、热重载、刷新状态、导出全部 M3U）；系统信息含 RSS | `/panel/api/status` 只返回 name / version / uptime_secs / bind / log_level / groups / channels / sessions / cache_* / entry_exts / hls / mpd，页面直接渲染这些键值 → **无指标、无快速操作** |
| `groups` | 分组与频道排序（`▲`/`▼`，组名左侧另有拖拽柄）；工具栏 `JSON 导入`/`JSON 覆盖`/`导出全部`/`多选`；频道行状态点与 `异常`/`常驻`/`全轨道` 徽章 | ❌ **缺**。证据：`reference/img/p02-groups.png`（2026-10-25 复核）；`groups.tsx` 无排序与工具栏相关代码。`08` S-BUG-02 的相反结论已更正（`07` `D-006`）。见 `07` §3.A9–A11 |
| `security` | 白名单、限流 `l1`/`l2`、会话 TTL 与并发、防爆破 | `SecurityPage`（`ops.tsx`）只有播放 Token（含宽限旧值、轮换）与 IP 统计 / 封禁 → **缺四组** |
| `logs` | 级别过滤 | `LogsPage`（`ops.tsx`）无 level 过滤 → **缺** |
| `epg` | 翻译设置（`m08` 翻译模型）；采集节奏 | `EpgPanelPage`（`ops.tsx`）有手工映射、额外频道、历史天数 → **缺翻译与节奏** |
| `logos` | 动效（`m09`）、转码参数（最大边 / 质量 / 转 WebP） | `logo.tsx` 有列表、上传、改名、匹配报告（`/logo/match-report`）→ **缺动效与转码参数**（后端动效已实现，面板未接） |
| `schedule` | 「从 EPG 加载节目」 | `schedule.tsx` 无相关调用，而端点 `/panel/api/schedule/from-epg` 已存在 → **面板未接** |
| `subs` | 源行的「读取分组」 | 有源规则、全局排除、预检结果；`/subs/groups` 端点已在 → **需核对 UI** |

已对齐的部分：`cache` 按目录渲染 `/cache/stats` 的 `entries`（含 `seg_url` / `sub_url`）；
`stats` 显示在线窗口与热度排行；`assets` 见 §4.1；`livestream` 有回看列表与缓冲；
`settings` 见 §4.1。弹窗族本轮未逐项核对。

## 5. 组件清单

| 组件 | 职责 | 对应参考截图 |
| --- | --- | --- |
| `Sidebar` | 侧栏导航、品牌区、运行状态 | 全部 `p*` |
| `PageHeader` | 标题、副标题、右侧操作 | 全部 `p*` |
| `Card` | 分区容器 | 全部 |
| `DataTable` | 分组/频道/任务列表 | `p02`、`p06` |
| `Modal` | 弹窗壳（遮罩、标题、动作区） | `m01`–`m13` |
| `Toast` | 操作反馈 | — |
| `Badge` | 模式、状态、access 徽章 | `p02`、`p04` |
| `Field` | 表单控件（输入、下拉、开关、多选、多行） | `m01`、`m02`、`p12` |
| `settings.tsx` | 系统设置页：配置草稿 + 顶部单一保存 + 字段卡片（§4.1） | `p12` |
| `LoginPanel` | 登录卡 | `p00` |

## 6. 交互约定

| 场景 | 交互 |
| --- | --- |
| 登录 | 成功后 token 存 `localStorage.iptv_token`；失败显示错误文案 |
| 401 | 清除 token，回到登录卡 |
| 保存配置 | 顶部单一「保存设置」按钮；整份 `PUT /config`；成功后提示"设置已保存"并刷新推荐值 |
| 分组/频道改动 | 立即调用对应接口；成功后刷新列表 |
| 危险操作 | 先弹**自研确认框**（目标另有 20 处原生 `confirm()`，不复刻） |
| 长列表 | 客户端搜索过滤 |

## 7. 分期

| 里程碑 | 前端范围 |
| --- | --- |
| M0 ✅ | 登录、概览、配置全量编辑；4 个页面骨架 |
| M1 | 分组管理（分组卡、频道表、增删改）、订阅地址复制、M3U 导出 |
| M2 | 缓存状态页（条目数、占用、flush）、概览接真实指标 |
| M3 | 频道热度、运行日志 |
| P1/P2 | 安全、录制、EPG、台标、订阅聚合、回看、素材、登录弹窗族 |

## 8. 验证方法

1. 逐屏对照：用同一视口（1440×1000）截我们自己的页面，与 `reference/img/` 并排比对布局与信息层级。
2. 交互对齐：核对分区顺序、按钮位置、弹窗动作按钮文案（"取消"/"确认删除"）。
3. 契约核对：面板用到的每个端点必须在 `03` §6 登记。

## 9. 未决问题

| 问题 | 处理 |
| --- | --- |
| 是否需要键位快捷操作（目标含 keybind 提示） | 先不做；按需追加 |
| 移动端适配 | 目标无移动布局；我们只做 ≥1280 px |
| 是否引入 i18n | **已定：不做**。目标是真 i18n，我们只有中文；只放一个切不动的选择器是「配了没反应」的反模式。已登记为差异（`07` §3.C4）；将来要做需先抽全部文案 |
