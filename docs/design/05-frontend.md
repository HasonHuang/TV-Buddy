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

### 2.1 主题与视觉令牌

`style.css` 的 `:root` 定义浅色令牌。`:root[data-theme='dark']` 覆盖同名令牌，得到深色主题。

主题在首屏绘制前定好。每个 HTML 入口的内联脚本读取 `localStorage.tv_buddy_theme`。没有值时跟随系统 `prefers-color-scheme`。侧栏底部与登录页右上角有主题开关，带文字标签。点一次即保存选择，此后不再跟随系统。开关同时更新 `<meta name="theme-color">`。

| 令牌 | 浅色 | 深色 | 用途 |
| --- | --- | --- | --- |
| `--bg` | `#f2f5fa` | `#0a0e16` | 页面底色 |
| `--panel` | `#ffffff` | `#121826` | 卡片、弹窗 |
| `--field` | `#fbfcff` | `#0f1521` | 输入框、下拉、文本域 |
| `--side` | `#ffffff` | `#121826` | 侧栏 |
| `--panel-2` | `#f7f9fe` | `#0e1420` | 悬停、内嵌块 |
| `--line` / `--line-strong` | `#e4e9f3` / `#d3dbea` | `#1f293b` / `#2b3850` | 分隔线、控件边框 |
| `--text` / `--text2` / `--text3` | `#16203a` / `#6a7690` / `#98a3b8` | `#e9eefb` / `#94a1ba` / `#6f7c94` | 正文、次要、提示 |
| `--accent` / `--accent-dark` | `#2f5bff` / `#2349dd` | `#7fa0ff` / `#9db6ff` | 链接、选中、焦点 |
| `--accent-weak` / `--accent-line` | `#edf1ff` / `#ccd7ff` | `#17223c` / `#2e3f63` | 选中底色、描边 |
| `--btn` / `--btn-hover` / `--btn-ink` | `#2f5bff` / `#2349dd` / `#ffffff` | `#7fa0ff` / `#9db6ff` / `#0a0e16` | 主按钮 |
| `--brand-1/2/3` | `#1d3fd8` / `#17a34a` / `#ff3b30` | `#7d95ff` / `#30d158` / `#ff6961` | 台标三色 |

规则：

- 新增颜色先加令牌，两套主题一起给值。组件里不写死色值。
- 主题只由 `data-theme` 决定。CSS 不写 `prefers-color-scheme` 媒体查询。
- 表单控件的底色用 `--field`，不要用 `--panel`。
- 选中导航、链接、焦点环用 `--accent`。主按钮用 `--btn`，不占强调色。
- 深色不用投影分级，改用边框；`--shadow` 与 `--shadow-xs` 置空。
- `color-scheme` 跟随主题，让下拉、勾选、滚动条一起变深。
- 语义色（`--ok`、`--warn`、`--danger`、`--info`）各带 `-weak` 底色。描边用 `color-mix()` 从语义色派生，保证两套主题都成立。

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
| 10 | 订阅源 | `subs` | `/subs/*`、`/playlist*` | ✅ v2026.16 列表加抽屉 |
| 11 | 回看管理 | `livestream` | `/livestream/*` | P1 |
| 12 | 视频素材 | `assets` | `/assets/*` | P2 |
| 13 | 运行日志 | `logs` | `/logs` | M3 |

弹窗体系（对齐目标，共 13 类）：分组编辑、频道编辑/新建、M3U 导入、录制任务、EPG 脚本、源频道浏览器、翻译模型、台标动效、批量解封、后台节流详情、台标选择器、确认框。M0 只实现确认框。

### 4.1 系统设置页的字段级表单

基线：`reference/img/p12-settings.png`（目标同一分区的 10 张卡片）。
一次 `GET /panel/api/config` 取整份配置到草稿，顶部「保存设置」一次 `PUT` 回去
（对应目标的「修改后点击保存生效」）。卡片用两列多栏布局，卡片高度互不拉伸；
字段为「标签 + ? ｜ 控件 ｜ 单位」横向行。页头吸顶，显示未保存项数。

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
| `dashboard` | 实时指标（下行·回源、上行·出网、进程内存、CPU、整盘内存峰值）；快速操作（清空缓存、热重载、刷新状态、导出全部 M3U）；系统信息含 RSS | ✅ 已对齐（2026-10-27）。`/panel/api/status` 已按目标字段补齐（见 `07` §3.B1/B2），页面直接渲染键值；快速操作四项已在。`traffic_in_bytes` 恒 0，与目标一致 |
| `groups` | 分组与频道排序（`▲`/`▼`，组名左侧另有拖拽柄）；工具栏 `M3U 导入`/`JSON 导入`/`JSON 覆盖`/`导出全部`/`多选`；频道行状态点与 `异常`/`常驻`/`全轨道` 徽章 | ✅ 已对齐（2026-10-27）。排序、四个导入导出入口、配置型三枚徽章、绿点与 `异常`（来自 `/panel/api/health` 的 `entries`）都在。`多选` 已自定实现：分组复选框 + 批量导出 + 批量删除（目标语义未观测，登记在 `PROGRESS.md` §3.3）。拖拽柄未做（等价功能由上/下移覆盖）。证据：`reference/img/p02-groups.png`；`07` `D-006` |
| `security` | 白名单、限流 `l1`/`l2`、会话 TTL 与并发、防爆破 | ✅ 「访问控制」卡片读整份配置、改 `[auth]` 子树后 `PUT /panel/api/config`（与设置页同机制）。`[auth]` 全部键已生效（2026-10-28）：白名单、`max_sessions_per_ip`、`session_ttl_secs`、`l1`/`l2` 限流、违规升级与登录锁阶梯都只在「IP 管控」开启时参与判定。界面不再有 `未消费` 标记。另加「播放链接审计」卡片显示 `GET /panel/api/link-info` 的 Token 状态、随机 ID 与别名 |
| `logs` | 级别过滤 | ✅ 已加级别下拉（全部/TRACE/DEBUG/INFO/WARN/ERROR），前端按行过滤；顺带剥掉环形缓冲里的 ANSI 颜色码（原先直接塞 `<pre>` 会显示成控制符） |
| `epg` | 翻译设置（`m08` 翻译模型）；采集节奏 | ✅ 已加五张卡：「采集节奏」= cron 输入 + `POST /panel/api/epg/cron-preview`；「EPG 翻译」= 引擎与目标语言 + 测试 + 立即翻译 + 清空缓存；「翻译模型」= `GET /panel/api/epg/translate/models` + `probe`；「采集脚本」= `/epg/scripts` 列表 + 上传/覆盖 + 删除；「匹配报告」= `GET /panel/api/epg/match-report` 原样显示 `text/plain` 表格；「匹配工具」= 建议、源频道浏览、时区预览；「公开查看器频道」= `GET /panel/api/epg/viewer-channels` |
| `logos` | 动效（`m09`）、转码参数（最大边 / 质量 / 转 WebP） | ✅ 已接：上传前在浏览器里按最长边等比缩放、可选转 WebP（目标同，服务端不转码）；每个台标卡加「动效」→ 6 效果选择 + `POST /panel/api/logo/animate`，生成后显示帧数/尺寸/时长/体积变化；有 `.orig/` 备份的显示「还原」 |
| `schedule` | 「从 EPG 加载节目」 | ✅ 已接：选好分组与频道后「从 EPG 加载节目」→ `GET /panel/api/epg/programmes/{epgKey}`（`epgKey` = `tvg_id` 或频道 id 的小写，与 `epg::channel_targets` 的 `out_id` 同规则）。节目表每条可「填入」（取已含 `record_pad_*` 的 `rec_start`/`rec_end`）或「排期」（`POST /panel/api/schedule/from-epg` 单条提交） |
| `subs` | 源行的「读取分组」 | ✅ 已加：「读取分组」按钮 → `POST /panel/api/subs/groups`，可「加入映射」填 `group_map`。源行状态徽章显示同步中 / 失败 / 频道变少 / 待确认；失败原因悬停可见。「最近同步」列显示最近成功时间。 |

已对齐的部分：`cache` 按目录渲染 `/cache/stats` 的 `entries`（含 `seg_url` / `sub_url`）；
`stats` 显示在线窗口与热度排行；`assets` 见 §4.1；`livestream` 有回看列表与缓冲；
`settings` 见 §4.1。弹窗族本轮未逐项核对。

### 4.3 订阅源页（v2026.16）

设计见 [`11-subscription-sync.md`](11-subscription-sync.md) §9。

布局：

- 顶部 `PageHead`：标题「订阅源」，说明「聚合与托管共同形成一个总清单」。
- 源列表：启用、名称、地址、模式徽章、频道/分组、最近同步、状态、操作。
- 操作：立即同步、编辑、删除。每 5 秒轮询 `/panel/api/subs/status`。
- 编辑用右侧抽屉，分区：基本、模式、分组、同步、高级、预检。
- 全局设置单独一张卡，独立保存。

模式语义：

- `聚合`：渲染订阅时拼接。条目不可管理。
- `托管`：物化进总清单。可管理、可授权、可进 EPG。

删除源时二选一：「保留为手工」或「一并删除」。

分组页与用户页：

- 分组徽章：`手工` / `订阅：<源名>`。
- 频道徽章：`手工` / `订阅：<源名> · 跟随` / `订阅：<源名> · 固定`。
- 编辑上游字段后自动固定。可「恢复跟随」。
- 新同步分组不自动授权。用户页只显示来源徽章。
- 全量配置保存统一走 `putConfig()`。`409` 时提示并重新加载。

## 5. 组件清单

| 组件 | 职责 | 对应参考截图 |
| --- | --- | --- |
| `Sidebar` | 侧栏导航、品牌区、分区搜索、运行状态 | 全部 `p*` |
| `PageHeader` | 标题、副标题、右侧操作 | 全部 `p*` |
| `ThemeToggle` | 明暗主题开关。显示目标主题的文字与图标 | 侧栏底部、登录页 |
| `DataTable` | 分组/频道/任务列表 | `p02`、`p06` |
| `Modal` | 弹窗壳（遮罩、标题、动作区） | `m01`–`m13` |
| `Drawer` | 右侧抽屉：遮罩、标题、滚动正文、可选底栏；Esc 与遮罩关闭；窄屏满宽 | 订阅源编辑 |
| `Toast` | 操作反馈 | — |
| `Badge` | 模式、状态、access 徽章 | `p02`、`p04` |
| `BrandMark` | 台标：`丷` 天线、空心机身、三色条。64 栅格；描边取 `currentColor` | 侧栏、登录页、页脚 |
| `Field` | 表单字段：标签、`tip`、控件与说明 | `m01`、`m02`、`p12` |
| `Select` | 自绘下拉：按钮 + `role=listbox` 菜单；支持键盘与点击外部关闭。全部管理页与登录后页面都使用它，不使用原生 `select` | `p12` 等 |
| `Tip` | 标签后的 `?`。气泡挂在 `body`，fixed 定位 | `p12` 等 |
| `Card` | 卡片壳：标题、说明、右侧操作 | 全部分区 |
| `settings.tsx` | 系统设置页：配置草稿 + 顶部单一保存 + 字段卡片（§4.1） | `p12` |
| `LoginPanel` | 登录页：左侧品牌区 + 右侧表单；≤1120 px 隐藏品牌区 | `p00` |

## 6. 交互约定

| 场景 | 交互 |
| --- | --- |
| 登录 | 成功后 token 存 `localStorage.tv_buddy_token`；失败显示错误文案 |
| 主题 | 侧栏底部、登录页右上角的开关切换明暗。选择存 `localStorage.tv_buddy_theme`；无值时跟随系统 |
| 下拉框 | 自绘 `Select`。点击或 Enter 打开；方向键、Home、End 移动；Enter 选择；Esc 关闭并回到按钮；键入首字跳转。点击外部、页面滚动或窗口改尺寸时关闭 |
| 数字字段 | 设置页的数字输入固定 96 px，右对齐，等宽数字。短选项下拉收窄到 128 px |
| 下拉宽度 | 默认撑满字段。短选项加 `compact`（128 px）；其他固定宽度用 `width` |
| 分区搜索 | 侧栏搜索框按标题过滤分区。`Cmd/Ctrl+K` 聚焦；`Esc` 清空并退出 |
| 401 | 清除 token，回到登录页 |
| 保存配置 | 顶部单一「保存设置」按钮；整份 `PUT /config`；成功后提示"设置已保存"并刷新推荐值。`Ctrl/Cmd+S` 等效 |
| 未保存状态 | 递归比较草稿与基线，显示未保存项数。离开分区、重新读取、关闭页面都会拦截 |
| 分组/频道改动 | 立即调用对应接口；成功后刷新列表 |
| 危险操作 | 先弹**自研确认框**（目标另有 20 处原生 `confirm()`，不复刻） |
| 提示 | 表单标签后缀 `?`，悬浮或聚焦显示作用说明；图标按钮带 `title` 与 `aria-label` |
| 长列表 | 客户端搜索过滤 |

## 7. 分期

| 里程碑 | 前端范围 |
| --- | --- |
| M0 ✅ | 登录、概览、配置全量编辑；4 个页面骨架 |
| M1 | 分组管理（分组卡、频道表、增删改）、订阅地址复制、M3U 导出 |
| M2 | 缓存状态页（条目数、占用、flush）、概览接真实指标 |
| M3 | 频道热度、运行日志 |
| P1/P2 | 安全、录制、EPG、台标、订阅聚合、回看、素材、登录弹窗族 |
| v2026.16 ✅ | 订阅源页重写为列表加抽屉；分组/频道来源徽章、固定与 detach；安全卡令牌与 passthrough 警告；全量保存 revision 冲突提示 |

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
