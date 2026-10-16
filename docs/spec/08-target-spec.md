# 08 · 目标功能规格（iptv-proxy v0.8.2）

> 规范版本 v1.0 ｜ 2026-10-15 ｜ 目标：**IPTV Proxy v0.8.2**（YanG，闭源 Rust，linux-arm64/amd64）
>
> **本文是"待验证的功能规格"（Normative Spec），不是逆向发现。**
> 内容由**公开 README** 与**基线文档**（`HasonHuang/sontv/docs/iptv-proxy/`，实测于 2026-10-02/03）
> 整理而成。每条声明都用 `规格 ID` 标注，供 `07-baseline-and-verification.md` 逐条核验。
> **级别**：本文所有条目默认为 **【基线观测】**（来自基线实测）；逆向未复位前，状态列默认为 `⏳`。
> 核验结果（观测/推断/未知）由逆向产出回填，**不得**在此文件直接改写结论。
>
> **版本说明**：上游**无 tag/release**，`main` 随构建变动。主分析对象为 **v0.8.2**
> （提交 `d302316fb8`，与本文基线一致）；`main` 实为 **v0.8.8**，仅作差异对照。
> 锚点与判定见 [`../findings/artifact.md`](../findings/artifact.md) §5 与 [`07`](07-baseline-and-verification.md) §3.1 `D-001`。
> 本文状态列的 `✅` 只表示**静态存在性**一致；**行为**待动态核验。

来源缩写：`RM`=公开 README ｜ `D-README`/`D-iptv`/`D-fields`/`D-panel`/`D-disg`/`D-res`/`D-mode-*`=基线各文档。

---

## 1. 身份与运行形态

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-ENV-01 | 产品名 IPTV Proxy，作者 YanG，上游 `YanG-1989/rust` 子目录 `IPTV Proxy/`；**源码未公开** | RM | ⏳ |
| S-ENV-02 | 发布形态：预编译二进制 `iptv-proxy-linux-{amd64,arm64,armv7}` + 安装脚本 | RM | ⏳ |
| S-ENV-03 | 安装目录 `/opt/iptv-proxy/`：`iptv-proxy`、`config.toml`、`iptv-proxy.log`、`cache/`、`iptv-proxy.sh` | RM/D-panel | ⏳ |
| S-ENV-04 | 服务常驻：systemd（Alpine 用 OpenRC），开机自启 | RM | ⏳ |
| S-ENV-05 | 默认面板端口 `19899`，默认监听 `[::]`（双栈） | RM/D-panel | ⏳ |
| S-ENV-06 | 启动命令形如 `./iptv-proxy config.toml`（配置以参数传入） | D-panel | ⏳ |
| S-ENV-07 | 依赖语言/工具链为 **Rust** | RM | ✅【观测】`rustc version 1.98.1`（v0.8.2） |
| S-ENV-08 | 分发产物经 **UPX 4.24** 加壳；脱壳后静态、非 PIE、已 strip | 二进制 | ✅【观测】 |

## 2. HTTP 端点（surface）

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-EP-01 | 面板：`GET /panel` | D-panel | ⏳ |
| S-EP-02 | 分组订阅：`GET /{gid}/playlist.m3u`；总订阅：`GET /playlist.m3u` | D-panel | ⏳ |
| S-EP-03 | 频道播放：`GET /{gid}/{cid}.{ext}`（v0.5.1 起路由**不带** `/index.m3u8` 路径段） | D-panel | ⏳ |
| S-EP-04 | 回看：`GET /{gid}/{cid}?playseek=<start>-<end>` | D-panel | ⏳ |
| S-EP-05 | EPG：`/epg.xml`、`/epg.xml.gz`、`/epg`；**EPG 总开关关闭时三者全部 404** | D-panel | ⏳ |
| S-EP-06 | 台标：`GET /logo/<文件名>` | D-panel | ⏳ |
| S-EP-07 | 自助解封：`GET /unban`，**无需登录**，每月限 3 次 | D-panel | ⏳ |
| S-EP-08 | 状态：`/status`（返回 `entry_exts` 等）、`/status/metrics` | D-panel/D-res | ⏳ |
| S-EP-09 | 多 VPS 数据分享端点 `POST /api/share/pull`（同步 token/封禁；基线路径待确认→已确认） | D-panel/二进制 | ⏳路径✅ |
| S-EP-10 | 健康检查：`GET /health`（安装脚本用于就绪探测） | 安装脚本 | ⏳ |
| S-EP-11 | 运维 API：`/api/reload`、`/api/backup`、`/api/restore`、`/whoami` | 二进制 | ⏳ |

## 3. 播放地址与扩展名规则

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-URL-01 | 播放地址扩展名由**后端**计算（`/status.entry_exts`），前端只查表：HLS→`.m3u8`，MPEG-DASH→`.mpd`，**开启切片伪装→`.txt`** | D-panel | ⏳ |
| S-URL-02 | **Passthrough 例外**：播放地址直接是上游原始 URL，不走 `/{gid}/{cid}` 路由 | D-panel | ⏳ |
| S-URL-03 | 订阅地址含分组 ID：`http://host:19899/<gid>/playlist.m3u`；改分组 ID 使旧地址失效 | D-fields | ⏳ |
| S-URL-04 | 开启 Token 后，**导出** M3U 自动附加 token；手工拼地址需自带 token | D-panel | ⏳ |
| S-URL-05 | 回看时间戳 `yyyyMMddHHmmss`，**使用本地时间**（依赖 `timezone_offset_hours`） | D-panel | ⏳ |

## 4. 六种工作模式

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-MODE-01 | 面板提供**六种**模式：`proxy`/`remux`/`dash`/`redirect`/`rewrite`/`passthrough`（公开 README 只写四种） | D-README | ⏳ |
| S-MODE-02 | 模式属于**分组**；同组频道共用 | D-README | ⏳ |
| S-MODE-03 | Proxy：反代 + 切片缓存；内容格式 HLS / MPD；`MPD 裁剪保留秒数` 仅在格式=MPD 时出现 | D-mode-proxy | ⏳ |
| S-MODE-04 | Proxy + 内容格式=MPD 时，频道多出 **ClearKey 密钥** 字段（`kid_hex:key_hex`，每行一对）；行为为"反代穿透本服务" | D-mode-proxy | ⏳ |
| S-MODE-05 | Remux：连续流（FLV / 裸 TS / MMT-TLV）→ HLS；**不解码**，CPU 低；`源容器类型`= 自动识别 / TS / FLV / MMT-TLV；**无**内容格式字段 | D-mode-remux | ⏳ |
| S-MODE-06 | DASH：`.mpd`→HLS 实时转封装，支持 CENC ClearKey 解密；自动 304 增量刷新 / 多 CDN 故障切换 / 防断流备份 / 片段级容错；**无**内容格式字段 | D-mode-dash | ⏳ |
| S-MODE-07 | DASH 频道级 `URL 池`（池大小 2–6，默认 3）：视频固定 `pool[0]`，音频/字幕在 `pool[1..]` 轮询 | D-mode-dash | ⏳ |
| S-MODE-08 | Redirect：客户端 302 直连上游，本服务仅调度；`跳转方式`= 直接跳转（默认）/ 解析最佳画质；`上游缓存时长` 默认 300s（缓存的是解析出的上游地址，非切片） | D-mode-redirect | ⏳ |
| S-MODE-09 | Rewrite：仅取一次清单并把相对地址改绝对；切片客户端直连；**请求头只作用于取清单**（源站要求切片带 Referer 时会失败） | D-mode-rewrite | ⏳ |
| S-MODE-10 | Rewrite + MPD / Redirect + MPD → **直连 CDN**（不转封装）；Proxy + MPD → 反代穿透本服务 | D-mode-rewrite | ⏳ |
| S-MODE-11 | Passthrough：原样导出上游 URL，**不套 Token**；有密钥时按 MPD 形式下发（但**不解密**） | D-mode-passthrough | ⏳ |
| S-MODE-12 | 仅 Proxy / Remux / DASH 支持**本地回看与定时录制**；Rewrite / Redirect / Passthrough 只有**源站回看**（`上游支持回看`+`回看天数`，追加 `playseek`） | D-iptv/D-mode-* | ⏳ |

### 4.1 模式 × 字段显隐矩阵

| 字段 | Proxy | Remux | DASH | Redirect | Rewrite | Passthrough |
| --- | :-: | :-: | :-: | :-: | :-: | :-: |
| 内容格式 | ✅(HLS/MPD) | ✖ | ✖ | ✅ | ✅ | ✖ |
| MPD 裁剪保留秒数 | 格式=MPD 时 | ✖ | ✖ | ✖ | 格式=MPD 时 | ✖ |
| 源容器类型 | ✖ | ✅ | ✖ | ✖ | ✖ | ✖ |
| 跳转方式 / 上游缓存时长 | ✖ | ✖ | ✖ | ✅ | ✖ | ✖ |
| ClearKey 密钥 | 格式=MPD 时 | ✖ | ✅ | ✖ | ✖ | ✅(按 MPD 下发) |
| 预载配置 | ✅ | ✖ | ✖ | ✖ | ✖ | ✖ |
| 网络配置(UA/代理/请求头) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

> 面板仅对 **5 个字段**做模式区分（上游缓存时长、跳转方式、源容器类型、内容格式、MPD 裁剪保留秒数）；其余（含网络配置）各模式通用。来源：D-fields。

## 5. 数据模型与字段

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-MODEL-01 | **分组级**字段：分组 ID、显示名称、工作模式、EPG 地址、访问范围、内容格式、网络配置（分组默认，频道可覆盖） | D-fields/D-panel | ⏳ |
| S-MODEL-02 | **频道级**字段：频道 ID、名称、上游 URL、Logo URL、tvg-id、tvg-name、上游解析（静态/脚本 PY）、源站回看、网络配置 | D-fields/D-panel | ⏳ |
| S-MODEL-03 | 工作模式/访问范围/网络配置**属于分组**；频道级网络配置覆盖分组，自定义请求头**合并**、同名以频道级为准 | D-iptv/D-fields | ⏳ |
| S-MODEL-04 | `tvg-id` 缺省则用频道 ID；`tvg-name` 缺省则用频道名称（EPG 匹配） | D-fields | ⏳ |
| S-MODEL-05 | 上游 URL 支持**脚本解析（PY）**，每次播放前算地址，需 `python3`；有解析缓存时长（须小于签名有效期） | D-fields | ⏳ |

## 6. 订阅来源：M3U 导入 vs 订阅聚合

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-SUB-01 | **M3U 导入**：只接受粘贴内容或上传文件（**不填地址**）；频道落地成分组/频道，获得**代理地址**，支持模式/缓存/录制/Token | D-iptv | ⏳ |
| S-SUB-02 | **订阅聚合**：接受订阅地址；输出**上游原始地址**，**不做反向代理**，不具分组归属，套不上模式/Token/缓存/录制 | D-iptv | ⏳ |
| S-SUB-03 | 输出可区分两类：手动/导入频道 `group-title=<显示名>`；聚合频道 `group-title=<分组ID>`（不同代码路径） | D-iptv | ⏳ |
| S-SUB-04 | M3U 导入 `频道模式`= 自动识别（`.mpd`→DASH，其余→Rewrite）/ Redirect / Rewrite / Proxy / DASH / Passthrough | D-panel | ⏳ |
| S-SUB-05 | 导入`策略`：追加（默认，同名跳过）/ 合并 / 覆盖 | D-panel | ⏳ |
| S-SUB-06 | 导入保留 `tvg-id`/`group-title` 等元数据；自动抓取 UA/Referer/Origin（含 `#EXTVLCOPT`/`#KODIPROP`/`#EXTHTTP`）；ClearKey 自动填入 `license_key` | D-panel | ⏳ |
| S-SUB-07 | 订阅聚合的 include/exclude 按**频道名称**匹配（不区分大小写、子串、多词 OR）；`group:`/`id:`/`/正则/` 仅用于**全局排除** | D-iptv | ⏳ |
| S-SUB-08 | 分组处理：各自保留原名 / 逐个指定 / 全部合并到一个组；**只改输出分组标题，不改代理规则** | D-iptv | ⏳ |

## 7. 认证与安全

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-AUTH-01 | 面板密码在配置中存 **SHA-256**（明文不落盘）；改密码后旧会话立即失效 | RM/D-panel | ⏳ |
| S-AUTH-02 | Token 验证：开启后所有播放地址需带 token；**随机频道 ID**（6 位）替换真实 ID，与 Token 同步轮转 | D-panel | ⏳ |
| S-AUTH-03 | Token 有效期（小时）可配；失效跳转可指向自定义提示页 | D-panel | ⏳ |
| S-AUTH-04 | IP 管控：每 IP 并发会话数、会话超时、频率限制与封禁、白名单；**仅监控播放列表请求** | D-panel | ⏳ |
| S-AUTH-05 | 访问范围：公开 / 隐藏（不进总订阅，独立地址可播）/ 归档（整组停用，EPG+台标照常匹配）/ 免验证（跳过 Token） | D-fields | ⏳ |
| S-AUTH-06 | 多 VPS 数据分享：共享 token 与封禁 IP；可开关"同时分享 token" | D-panel | ⏳ |
| S-AUTH-07 | 自助解封 `/unban` 无需登录，每月限 3 次 | D-panel | ⏳ |
| S-AUTH-08 | `iptv-proxy pass <密码> [账号]` 可直接改密码（无需进面板）；改端口/密码均改 `config.toml` 并重启 | RM/D-panel | ⏳ |

## 8. EPG / 台标 / 录制 / 回看

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-EPG-01 | 多源采集→匹配本机频道→生成统一 `/epg.xml.gz`；EPG 关闭时 `/epg*` 全 404 且导出 M3U 不带 `x-tvg-url` | D-panel | ⏳ |
| S-EPG-02 | 采集节奏：每 4h / 每 6h / 每日 4 点 / 每日 3、5、12 点 / 每 30 分钟 | D-panel | ⏳ |
| S-EPG-03 | 名称匹配开关（tvg-id 不匹配时按频道名再匹配一次）；脚本源需 python3 | D-panel | ⏳ |
| S-EPG-04 | EPG 标题可翻译（Google 免费 / Gemini(Key) / MTranServer），14 种目标语言 | D-panel | ⏳ |
| S-LOGO-01 | 台标本地托管，`tvg-logo` 与 EPG `icon` 指向本地；支持上传转码与动效生成 | D-panel | ⏳ |
| S-REC-01 | 定时录制：Proxy / DASH 频道；**必须 ffmpeg**，否则任务失败；录制合成 MKV（多音轨，中断可播）；存 `cache/recordings/` | D-panel | ⏳ |
| S-CAP-01 | 回看播放地址随「回看格式」变化：国产 `${start}-${end}` / 国外 `${start}-${end}` / Kodi `{utc}-{utcend}` / both | D-panel | ⏳ |
| S-CAP-02 | 回看切片存 `cache/livestream/`；时间偏移可调（录进时间戳与播出时间差） | D-panel/D-mode-proxy | ⏳ |

## 9. 切片伪装（disguise）

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-DIS-01 | 配置段 `[cache.disguise]`：`enabled`/`playlist`/`subtitle`/`master`/`formats`；**出厂默认 `enabled=true` + `playlist=true`** | D-panel/D-disg | ⏳ |
| S-DIS-02 | **不改切片载荷字节**：只改 URL 后缀与 Content-Type；四种后缀同一切片前 32 字节相同，首字节 `0x47` | D-disg | ⏳ |
| S-DIS-03 | 切片为原始 MPEG-TS：每 188 字节 `0x47`，文件大小 188 倍数；无 FLV/图片/fMP4 特征 | D-disg | ⏳ |
| S-DIS-04 | 后缀**轮换稳定且按频道独立**：同一频道重复拉清单序列不变；同一切片哈希在不同频道后缀不同（由 频道ID+切片哈希 推导） | D-disg | ⏳ |
| S-DIS-05 | 后缀**不参与校验**：清单写 `.png`，请求 `.jpg` 得同字节（MD5 相同） | D-disg | ⏳ |
| S-DIS-06 | `Cache-Control` 切换：切片刚写入短暂 `public, max-age=300`，之后 `private, max-age=300` | D-disg | ⏳ |
| S-DIS-07 | 切片不支持 `Range`（忽略，返回 200 全量）；无 `Accept-Ranges`/`ETag`/`Last-Modified` | D-disg | ⏳ |
| S-DIS-08 | 每切片自包含 MPEG-TS（PAT/SDT/PMT + ~10s）；PID 布局非 ffmpeg 默认（PMT `0x100`、视频 `0x102`、音频 `0x101`） | D-disg | ⏳ |
| S-DIS-09 | 每切片第 0 个包为 SDT，描述符载荷 ASCII `lumberjack`（两次）——来源水印，与伪装无关 | D-disg | ⏳ |
| S-DIS-10 | 媒体清单伪装成 `.txt`（`.../video/index.txt`），主清单入口仍为 `.m3u8`；媒体清单无 `#EXT-X-MAP` | D-disg | ⏳ |
| S-DIS-11 | 文件名混淆由独立的 `cache.dash.obfuscate_filenames` 控制，**与伪装无关** | D-panel | ⏳ |

## 10. 缓存

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-CACHE-01 | 后端 `disk`（持久，重启不丢）/ `memory` | D-panel | ⏳ |
| S-CACHE-02 | 缓存目录 `./cache`，结构：`epg/` `logos/` `m3u8/` `recordings/` `seg_bytes/` `seg_url/` `sub_url/`（+ `livestream/`） | D-panel | ⏳ |
| S-CACHE-03 | 条目上限：`m3u8_max_entries`(默认 500)、`segment_max_entries`(默认 2000)；`disk_max_mb`(0=不限) | D-panel | ⏳ |
| S-CACHE-04 | **预载仅 Proxy 生效**：预载开关 / 拉取并发 / 拉取超时 / HLS 预载 / MPD 预载；无人观看仍回源 | D-mode-proxy | ⏳ |
| S-CACHE-05 | 日志 `iptv-proxy.log` 最大 **5MB** | D-panel | ⏳ |

## 11. 配置项（config.toml）规格

```toml
[server]
bind = "[::]:19899"
timezone_offset_hours = 8
log_level = "info"

[panel]
enabled = true
username = "admin"
password_sha256 = "..."
session_ttl = 86400

[cache]
backend = "disk"
disk_path = "./cache"
m3u8_max_entries = 500
segment_max_entries = 2000
disk_max_mb = 0

[auth]
token_enabled = false

[cache.disguise]
enabled = true
playlist = true
subtitle = false
master = false
formats = ["jpg","png","gif","bmp","webp"]
```

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-CFG-01 | 面板改动最终写入 `/opt/iptv-proxy/config.toml`；文件只写最小必要项，其余走默认；保存后回写完整配置 | D-panel | ⏳ |
| S-CFG-02 | 配置有**热重载监视**（可直接改文件生效，前提是热重载未被关闭） | D-panel | ⏳ |
| S-CFG-03 | 下列开关**启动时读取，改后须重启**：DASH 会话预启动、配置文件热重载监视、登录失败计数清理 | D-iptv | ⏳ |
| S-CFG-04 | 顶层键与语义如上表（须由二进制/实测确认键名与默认值） | D-panel | ⏳ |

## 12. 命令行与运维

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-CLI-01 | 安装后脚本软链到 `PATH`，命令 `iptv-proxy`：`update`/`reinstall`/`port <N>`/`pass <密码> [账号]`/`restart`/`log`/`url`/`uninstall` | RM/D-panel | ⏳ |
| S-CLI-02 | `uninstall` 询问是否删 `/opt/iptv-proxy`，选 N 仅停服务保留数据 | RM | ⏳ |

## 13. 可选依赖

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-DEP-01 | `ffmpeg`（可选）：视频素材 MP4→HLS、定时录制合成 MKV；**不装则录制失败** | D-panel | ⏳ |
| S-DEP-02 | `python3`（可选）：EPG 脚本源、频道脚本解析 | D-panel | ⏳ |
| S-DEP-03 | `MTranServer`（可选）：字幕机翻（离线自部署） | D-panel | ⏳ |
| S-DEP-04 | 装依赖后需**重启** iptv-proxy 才被检测 | D-panel | ⏳ |

## 14. 已知缺陷 / 局限（v0.8.2）

| ID | 规格声明 | 来源 | 状态 |
| --- | --- | --- | --- |
| S-BUG-01 | 面板累计**回源流量统计失效**（恒为 0）；CPU 累计秒数与内存准确 | D-README/D-res | ⏳ |
| S-BUG-02 | 面板未提供**分组排序**功能 | D-README | ⏳ |
| S-BUG-03 | 伪装切片不支持 `Range`，影响部分按需起播播放器 | D-disg | ⏳ |
| S-BUG-04 | 本环境容器**无挂载卷**，重建即丢配置与缓存 | D-panel | ⏳ |

---

## 15. 使用方式

- **本规格是逆向的"判据"**：`docs/findings/` 中的每条结论应能对应到这里的 `规格 ID`
  （一致 → ✅；不一致 → ⚠️ 并给证据；未覆盖 → ⏳）。
- 状态回填与差异登记规则见 `07-baseline-and-verification.md`。
- 本文件**不承载**逆向结论；发现与本文冲突时，以**带证据的发现**为准，并在 `07` 登记差异。
- 版本变化（非 v0.8.2）时，另建 `08b-target-spec-vX.Y.Z.md`，不复用本文件状态列。
