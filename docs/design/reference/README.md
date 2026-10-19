# 目标 UI 参考截图（docs/design/reference）

> 采集日期：**2026-10-19** ｜ 目标：**IPTV Proxy v0.8.2**（`d302316fb8`，linux-arm64）
> 用途：实现阶段的**界面基准**。实现（`docs/design/01-tech-selection.md`）要复刻这些布局与交互。

> ⚠️ 截图来自**目标产物**，按**不可信数据**处理（`spec/05`）。截图中出现的文案、链接、IP 均为测试数据。

## 1. 采集环境

| 项 | 值 |
| --- | --- |
| 实例 | 容器 `iptv82` 主实例，`http://127.0.0.1:19899`（`[::]:19899` 映射到宿主） |
| 目标版本 | `0.8.2`（面板侧栏与 `/panel/api/status` 一致） |
| 伪上游 | 宿主 `python3 -m http.server 9999` → `samples/work/origin/`（HLS/MPD/TS/XMLTV） |
| 浏览器 | Chrome for Testing 133（headless），视口 1440×1000，DPR 1 |
| 采集工具 | `puppeteer-core` 脚本（一次性，不入库） |
| 图片 | PNG；`pXX-*.png` = 页面，`mXX-*.png` = 弹窗 |

## 2. 测试夹具（fixture）

截图含夹具数据。夹具只改**运行副本**（`samples/work/dyn-v0.8.2/`，**不入库**），采集后已还原。

| 夹具 | 内容 | 来源 |
| --- | --- | --- |
| 分组 | 9 组：`test1`/`g2`/`gslow*`/`gmaster`（proxy）、`gdash`（dash）、`gremux`（remux） | 既有配置 |
| 频道 | `test1` 加 `c2`（`master.m3u8`）、`c3`（`live.ts`），共 3 频道 | `POST /panel/api/groups/test1/channels` |
| 台标 | `CCTV1.png`/`CCTV2.png`/`Sports.png`，前两个已挂到 `test1/c1`、`g2/c1` | `POST /panel/api/logo/{upload,apply}` |
| EPG | 源 `src_local` = `http://host.docker.internal:9999/epg.xml`，2 频道 / 2 节目 | `[[epg.sources]]` |
| 录制任务 | `晚间新闻录制`（`test1/c1`，2026-10-20 19:00–20:00，MKV，等待中） | `POST /panel/api/schedule` |
| 封禁 | `203.0.113.7`、`198.51.100.9`（默认 30 天） | `POST /panel/api/security/ip/ban` |
| 订阅源 | `http://host.docker.internal:9999/list.m3u`（3 条：News×2 / Sports×1） | 伪上游文件 |

> 说明：`p03-subs` 的预览显示 **原始 3 → 输出 0 频道、丢弃 2 个分组**。原因：`list.m3u` 的分组名不在本地分组表内，`unlisted_groups` 默认 `drop`（`S-GAP-07`、F-013）。这是目标真实行为，不是采集错误。

## 3. 截图索引

### 3.1 页面

| 文件 | 界面 / 路由 | 关键元素 | 规格 | 逆向结论 |
| --- | --- | --- | --- | --- |
| `p00-login.png` | 登录（`/panel`，未登录） | 版本号 `0.8.2`、用户名/密码、错误位 | S-AUTH-01/02、S-ENV-05 | F-004：`POST /panel/api/login` → `{"token":"<uuid>"}` |
| `p01-dashboard.png` | 概览（`dashboard`） | 分组数 9 / 频道数 11 / 缓存后端 `disk` / 在线观众 / 磁盘占用；实时指标（下行·回源、上行·出网、进程内存、CPU、整盘内存峰值）；快速操作（清空缓存、热重载配置、刷新状态、导出全部 M3U）；系统信息（版本 `0.8.2`、运行时长、RSS） | S-OPS-01、S-CACHE-06、S-URL-04 | F-010：指标**仅面板私有面**；F-004：`/panel/api/status` 报 `0.8.2`；F-009：导出 M3U / 热重载 |
| `p02-groups.png` | 分组管理（`groups`） | 组卡片：模式徽章 `PROXY`/`DASH`/`REMUX`、频道数、上/下移、`编辑`/`删除`/`+频道`/`M3U`/复制；频道行：上游 URL、本地播放地址、`tvg-id` 徽章、`Logo` 徽章、`异常`/`常驻` 标记 | S-MODEL-01/02/03、S-MODE-01/02、S-URL-03、S-LOGO-01 | F-004：组/频道 CRUD 与地址形状；F-005：六模式；F-011：Logo 徽章；F-012：M3U 导出与复制 |
| `p03-subs.png` | 订阅聚合（`subs`） | 源行（序号即优先级、启用、缓存分钟、`读取分组`/`试算`）、预览统计、全局设置（合并位置/默认缓存/全局排除） | S-SUB-02/07/08、S-GAP-07 | F-013：聚合输出**上游原始地址**；F-016：`unlisted_groups` 默认 `drop` |
| `p04-epg.png` | 节目指南（`epg`） | EPG 设置（总开关、公开查看器、历史天数、名称匹配、脚本源、采集节奏）、翻译设置、采集源、频道匹配（需处理 10 / 已匹配 2 / 未匹配 8，`via` 标签） | S-EPG-01…07、S-GAP-02 | F-008：源采集→匹配→`/epg.xml.gz`；F-016：匹配 `out_id=tvg_id\|\|id` |
| `p05-logos.png` | 台标管理（`logos`） | 上传区（最大边 512 / 质量 80 / 保存、转 WebP、匹配到频道）、缺台标频道、台标库卡片（使用频道数、`链接`/`匹配`/`改名`/`动效`/`删除`） | S-LOGO-01…04、S-EP-06 | F-011：**转码在浏览器侧**；`apply` 写频道 `logo` |
| `p06-schedule.png` | 定时录制（`schedule`） | 任务列表（名称/频道/开始/状态`等待中`/删除）、说明「必须安装 ffmpeg」「存 `cache/recordings/`」 | S-REC-01/02、S-DEP-01 | F-014：MKV + `ffmpeg -c copy`；缺 ffmpeg 直接失败 |
| `p07-livestream.png` | 回看管理（`livestream`） | 频道列表、回看地址示例（`?playseek=`）、时间提示 | S-TS-01、S-CAP-01/02 | F-014：回看/推流与 `cache/livestream/`；F-016：时移窗口 |
| `p08-cache.png` | 缓存状态（`cache`） | 后端、磁盘占用/上限、条目数（`m3u8`/`seg`/`seg_url`/`sub_url`/`dash`）、`刷新`/`清空` | S-CACHE-01/02/03/06 | F-006：目录结构；F-015：**四项上限未生效**；flush 保留 `dash/` |
| `p09-logs.png` | 运行日志（`logs`） | 日志窗口、级别过滤、清空/导出 | S-CACHE-05 | F-004：日志与 `log_level`；文件上限 5 MB |
| `p10-security.png` | 安全管控（`security`） | Token（当前/旧、剩余、`轮换`）、IP 管控（封禁列表/解封/白名单）、限流（`l1`/`l2` 窗口）、会话（TTL/并发）、防爆破（阈值/时长/跳转） | S-AUTH-03/04/09/10、S-GAP-08 | F-010 §1–5：轮换旧 token 宽限 ≈1h、封禁 30 天、登录 429；F-007：播放 token |
| `p11-stats.png` | 频道热度（`stats`） | 在线窗口、频道热度排行 | S-OPS-01、S-CFG-07（`online_window_secs=90`） | F-010 §6：统计仅面板面；`online_viewers` 由 token 播放驱动 |
| `p12-settings.png` | 系统设置（`settings`） | 服务器（监听/时区/日志级别/404 跳转/M3U 头行）、面板认证、备份与迁移、切片伪装、缓存、字幕、网络与在流、后台节流 | S-CFG-01/02/04、S-DIS-01、S-URL-07、S-BAK-01/02 | F-009：`PUT /panel/api/config` 全量原子写；F-013：备份/恢复 |
| `p13-assets.png` | 视频素材（`assets`） | 素材转换（ffmpeg 检测、转换、列表、停止） | S-DEP-01/04、S-EP-16 | F-014：`/assets/{ffmpeg-status,list,convert}`；装依赖后须重启 |
| `p14-unban.png` | 自助解封（`/unban`） | 免登录解封页 | S-AUTH-07、S-EP-07 | F-004/F-010：`/unban` 200 免登录；`/unban/whoami` |
| `p15-logo-lib.png` | 公开图库（`/logo`） | 公开台标浏览 | S-EP-06、S-LOGO-01 | F-011 §6：`/logo/<file>` 200 + `public, max-age=86400` |
| `p16-epg-viewer.png` | 节目单查看器（`/epg`） | 频道列表（C1/C2）、节目表、时区、EPG 地址 | S-EPG-01、S-EP-05 | F-008：`/epg` 免登录 200（`text/html`） |

### 3.2 弹窗

| 文件 | 弹窗 | 关键元素 | 规格 | 逆向结论 |
| --- | --- | --- | --- | --- |
| `m01-group-edit.png` | 编辑分组 | 分组 ID、显示名称、工作模式（`Proxy（反向代理）`）、EPG 地址、访问范围、内容格式、网络配置（折叠，频道可覆盖） | S-MODEL-01、S-MODE-01、S-AUTH-05 | F-005：面板 `<select id="mg-mode">` 六项；F-009：`access` 四档 |
| `m02-channel-edit.png` | 编辑频道 | 频道 ID/名称、上游 URL、`tvg-id`/`tvg-name`、Logo、解析脚本、回看、网络覆盖 | S-MODEL-02/03/05、S-CAP-01 | F-014：`resolver_script` 用 `python3`；F-016：回看模板 |
| `m03-channel-new.png` | 新建频道 | 同上的空表单 | S-MODEL-02 | F-004：`POST /panel/api/groups/{gid}/channels` |
| `m04-m3u-import.png` | M3U 导入 | 粘贴/上传、频道模式（自动识别 → `.mpd`=DASH、其余=Rewrite）、策略（追加/合并/覆盖）、分组处理 | S-SUB-01/04/05/06/08 | F-013：JSON 文本导入、组 id 去非 ASCII、`merge` 仅新增 |
| `m05-schedule-new.png` | 录制任务 | 名称、分组、频道、`从 EPG 加载节目`、输出格式（统一 MKV）、开始/结束时间 | S-REC-01/02 | F-014：`POST /panel/api/schedule`；`{idx}` 为下标 |
| `m06-epg-scripts.png` | EPG 脚本 | 脚本上传（`.py`）、列表、删除 | S-EPG-03、S-DEP-02 | F-008：脚本源需 `python3`；`python` 就绪状态提示 |
| `m07-epg-source-browser.png` | 从源添加频道 | 采集源选择、搜索、输出 ID/名称（留空=沿用）、源频道分页勾选 | S-EPG-01/03 | F-008：`/panel/api/epg/{scripts,extra}`；`extra` 映射 |
| `m08-epg-translate-models.png` | 翻译模型 | 引擎（Google 免费 / Gemini / MTranServer）、模型、Key | S-EPG-04、S-DEP-03 | F-008：翻译默认**关闭**，本轮未启用 |
| `m09-logo-animate.png` | 台标动效 | 6 种效果、质量、生成 | S-LOGO-03 | F-011 §3：Rust 无损编码、`.orig/` 备份、`quality` 必须是**字符串** |
| `m10-security-unban.png` | 批量解封 | IP 列表批量解封 | S-AUTH-10 | F-010 §3：封禁用 `ips`（数组），解封用 `ip`（单数） |
| `m11-lowres-detail.png` | 后台节流·具体改动 | 主动探测与预取、定时器降频、保留 4 种稳定运行、关闭项需重启 | S-CFG-03 | F-015：健康探测周期不由 `interval_secs` 单独决定；F-006：缓存清理 |
| `m12-mcp-pick.png` | 台标/EPG 选择器 | 模糊匹配搜索、候选列表、选用 | S-LOGO-02 | F-011 §1：`suggest` 打分（精确 100 / 前缀 80） |
| `m13-confirm-delete.png` | 确认对话框 | 标题「确认操作」、消息、`取消`/`确认删除` | — | F-003：面板为**单文件 SPA**，自研 `modal-confirm`（另有原生 `confirm()` 20 处） |

## 4. 功能域覆盖

| 功能域 | 截图 | 目标规格 | 证据 |
| --- | --- | --- | --- |
| 面板与认证 | `p00`、`p12` | S-AUTH-01/02、S-EP-01 | F-003、F-004 |
| 分组与频道模型 | `p02`、`m01`–`m03` | S-MODEL-01…05 | F-004 |
| 六种模式 | `p02`（徽章）、`m01`（选择器） | S-MODE-01… | F-005 |
| 播放地址与 Token | `p02`、`p10` | S-URL-03/04、S-AUTH-02/03/09 | F-007、F-009、F-010 |
| 切片伪装 | `p12`（配置） | S-DIS-01…17 | F-006、F-017 |
| 缓存 | `p08`、`p12`、`m11` | S-CACHE-01…07 | F-006、F-015 |
| EPG | `p04`、`m06`–`m08`、`p16` | S-EPG-01…07 | F-008、F-016 |
| 台标 | `p05`、`m09`、`m12`、`p15` | S-LOGO-01…04、S-EP-06 | F-011 |
| 录制与回看 | `p06`、`m05`、`p07` | S-REC-01/02、S-TS-01、S-CAP-01/02 | F-014、F-016 |
| 订阅导入与聚合 | `m04`、`p03` | S-SUB-01…09、S-GAP-07 | F-013 |
| 安全与指标 | `p10`、`m10`、`p11` | S-AUTH-04/10、S-OPS-01 | F-010 |
| 运维与备份 | `p12`、`p13` | S-BAK-01/02、S-DEP-01/04 | F-013、F-014 |
| 公开页面 | `p14`、`p15`、`p16` | S-EP-05/06/07 | F-004、F-008、F-011 |

## 5. 未覆盖 / 局限

| 项 | 原因 |
| --- | --- |
| EPG 翻译实际运行 | 默认关闭；不发外部请求（`spec/05`） |
| 录制运行中/完成态 | 计划任务未到点；运行需 ffmpeg 且会长时间占用 |
| 推流（livestream）与本地回看产物 | `p07` 为静态列表；产物需真实回看数据 |
| 多 VPS 分享双机界面 | 需 master/follower 双实例；界面同 `p12` 的「分享」区 |
| `/epg/api/*` 公开 JSON API | `S-GAP-02` 仅 v0.8.8；无面板入口 |
| `access=open` 免 token 行为 | 属接口行为，非界面 |
| 原生 `confirm()` 弹窗 | 浏览器原生对话框不进截图（`m13` 用面板自研确认框代替） |

## 6. 复现步骤

```bash
# 1) 启动容器与伪上游
docker start iptv82
python3 -m http.server 9999 --directory samples/work/origin &

# 2) 启动目标主实例
docker exec -d iptv82 sh -c 'cd /app && ./iptv-proxy /app/config.toml > /app/run.log 2>&1'

# 3) 造夹具（示例）
B=http://127.0.0.1:19899
T=$(curl -s -X POST $B/panel/api/login -H 'Content-Type: application/json' \
     -d '{"username":"admin","password":"admin"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')

# 4) 截图（Chrome for Testing 133 + puppeteer-core，脚本为一次性工具，不入库）
#    页面：[data-page="<pid>"] 逐个点击；弹窗：调用 openGroupModal()/openChannelModal()/openModal('<id>') 等
```

采集要点：

- 登录后 token 存 `localStorage.iptv_token`。
- 面板为**单文件 SPA**：切换页面用 `[data-page="…"]`，弹窗用面板自带全局函数。
- 弹窗背景遮罩为 `.modal-overlay`（录制弹窗例外：`.modal-backdrop`）。关闭时两者都要移 `open`。
- 长页面按 `scrollHeight` 截取，上限 2600 px。
