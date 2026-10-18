# PROGRESS · IPTV Proxy 逆向任务

> 本文件记录**任务计划**与**进度**。规则：**每处理完一项，立即更新本文件**。
> 规范见 [`docs/spec/`](docs/spec/)。发现见 [`docs/findings/`](docs/findings/)。
> 目标版本：**v0.8.2**（上游提交 `d302316fb8`）。

## 0. 任务目标

逆向闭源 Linux 二进制 **IPTV Proxy**（`YanG-1989/rust` 子目录 `IPTV Proxy/`，仅发布 arm64/amd64/v7）。
产出：可核验的规格与行为结论（对齐基线 [`sontv/docs/iptv-proxy`](../sontv/docs/iptv-proxy)）。

约束：
- 只用 **linux-arm64 / linux-amd64** 两个产物。
- 静态在 macOS（REA + Hopper）；动态在 **linux/arm64 容器**（OrbStack）。
- 目标派生内容一律按**不可信数据**处理。

## 1. 已完成（分阶段）

### Phase 0 · 规范与骨架 ✅

| 项 | 结果 | 位置 |
| --- | --- | --- |
| 强制规范 8 篇 | ✅ | `docs/spec/00?…08`（README + 01–08） |
| 发现骨架 | ✅ | `docs/findings/{README,artifact,static,dynamic}` |
| 首次提交 | ✅ `de898fe` | — |

### Phase 1 · 取得与环境 ✅

| 项 | 结果 | 位置 |
| --- | --- | --- |
| 取得产物（主 v0.8.2 / 对照 v0.8.8） | ✅ | `samples/`（gitignore，指纹登记于 `artifact.md`） |
| 加壳识别 + 脱壳 | ✅ UPX 4.24，`upx -d` 成功 | `findings/static/01` |
| ELF 结构 | ✅ 非 PIE、静态、NX、RELRO | `artifact.md` §3 |
| 工具链判定 | ✅ Rust 1.98.1（v0.8.2） | `findings/static/01` §4 |
| **版本判定** | ✅ v0.8.2=`d302316fb8`；main=v0.8.8 | `findings/static/01` §5 |

### Phase 2 · 静态分析

| 项 | 状态 | 结果 / 位置 |
| --- | --- | --- |
| S1 指纹与 ELF 结构 | ✅ | `artifact.md` §2–§3 |
| S2 加壳/脱壳 | ✅ | `findings/static/01` §2–§3 |
| S3 工具链/语言（Rust） | ✅ | `findings/static/01` §4（S-ENV-07 升级为观测） |
| S4 版本判定 | ✅ | `findings/static/01` §5（F-002） |
| S5 端点/配置键预扫描 | ✅ | `findings/static/01` §6–§7 |
| S6 内嵌面板前端 | ✅ 4 页（管理面板 622KB / 节目单 / 台标库 / 自助解封） | `findings/static/02`（F-003） |
| S7 关键算法反编译 | ⏳ | 待做（切片伪装后缀推导、缓存窗口） |

### Phase 3 · 动态分析（容器 `iptv82`，v0.8.2）

| 项 | 状态 | 结果 |
| --- | --- | --- |
| D1 启动与就绪 | ✅ | `[::]:19899`，热重载启动，边缘校验 off，无 ffmpeg |
| D2 端点表面行为 | ✅ | `/health`=200 `ok`；`/panel`=200；`/unban`=200；**`/status`=404**；EPG 关闭时 `/epg*` 全 404；未知 `/{gid}/{cid}`→**302 占位视频** |
| D3 认证机制 | ✅ | `POST /panel/api/login {username,password}`→`{"token":uuid}`；用 `Authorization: Bearer` 或 `X-Token` |
| D4 版本运行时确认 | ✅ | `/panel/api/status` → `"version":"0.8.2"` |
| D5 完整配置 schema | ✅ | `/panel/api/config` 全量（默认值）；见 `findings/dynamic/raw/v0.8.2-config.json` |
| D6 分组/频道与伪装 | ✅ | 分组 `POST /panel/api/groups {id}`；频道 `POST /panel/api/groups/{gid}/channels`；`PUT …/channels/{cid}` |
| D7 六模式行为 | ✅ | proxy/redirect/rewrite/passthrough 实测；dash/remux 失败路径+看门狗；见 F-005 |
| D8 切片伪装字节级 | 🔶 | **后缀已破解**：`sha256(上游切片绝对URL)[:32]`；16-hex 频道前缀未解（已排除多种算法；功能可替代） |
| D9 缓存窗口 | ✅ | 首写 `public,max-age=300` → 命中 `private,max-age=300`（3 次复现）；入口 `public,max-age=5`；见 F-006 |
| D10 EPG（采集/下发/裁剪/lang） | ✅ | `/epg.xml[.gz]`、`/epg` 免登录 200；XMLTV 生成 `iptv-proxy`；时间裁剪与 `lang` 处理；见 F-008 |
| D10b 录制/台标 | 🔶 | 台标 `/logo/<file>`（`cache/logos/`，`public,max-age=86400`）✅；录制 ⏳（需 ffmpeg） |
| D11 播放 Token 与护栏 | ✅ | 入口需 `?token=<48hex>`（全局/TTL 24h）；失效→302 `token_failure_url`；**切片无 token 也 200**；见 F-007 |
| D13 入口清单改写/访问范围 | ✅ | access 四档（open 例外：不进总订阅）；`m3u_header_lines`；`x-tvg-url` 自动注入；配置经 `PUT /panel/api/config` 全量原子保存（规范化重写、与文件直改有竞态）；见 F-009 |

### Phase 4 · 报告与回填

| 项 | 状态 | 结果 / 位置 |
| --- | --- | --- |
| R1 差异登记 | ✅ | `07` §3.1 `D-001`（已解决） |
| R2 核对矩阵回填 | 🔶 进行中 | `07` §2.2 已填静态项 |
| R3 阶段性报告 | ⏳ | 待做 |

## 2. 关键结论（截至目前）

1. 产物经 **UPX 4.24** 加壳；脱壳后为 **Rust 1.98.1** 静态、非 PIE 二进制。
2. 主分析对象 **v0.8.2**（`d302316fb8`）；`main` 实为 **v0.8.8**。运行时 `/panel/api/status` 亦报 `0.8.2`。
3. 技术栈：`axum 0.7.9` + `tokio 1.52.1` + `hyper 1.9.0` + `reqwest 0.12.28` + `rustls` + `tracing`。
4. **面板前端完整内嵌**（4 页）；面板接口前缀 **`/panel/api/…`**。
5. **切片伪装默认开启**：`[cache.dash].obfuscate_filenames=true`；`[cache.disguise]` = enabled/playlist 开、subtitle/master 关、formats=[jpg,png,gif,bmp,webp]。**与基线 S-DIS-01 完全一致**。
6. 未知频道 → **302** 到占位视频 `material.yang-1989.eu.org/v/{404,mistake,Failure,limit,Ban}.mp4`。
7. 面板认证：登录换取 **UUID token**，`Authorization: Bearer` 或 `X-Token` 二选一。
8. **切片伪装（实测）**：切片 URL `/{gid}/{cid}/<16hex>_<32hex>.<ext>`；
   **`<32hex> = sha256(上游切片绝对 URL) 前 32 位**（已复现）；`<16hex>` 每频道固定（推导未解）。
   后缀**不校验**（换任意后缀字节相同）；`Content-Type` 由后缀映射。
9. **六种模式**（面板 `mg-mode`）：`proxy`/`remux`/`redirect`/`rewrite`/`dash`/`passthrough`（README 只写四种）。
   `redirect`、`passthrough` → **302 直连上游**；`rewrite` → 清单**绝对化**、客户端直连；
   `proxy` → 反代+伪装切片。模式属**分组**级。`redirect` 缓存上游地址（TTL 300s）。
10. `dash` 主清单就绪超时 **30s**；`remux` 预热期 `503 Retry-After:2`；看门狗失败重建、**连续 5 次→冷却 3600s**。
11. **切片缓存窗口**：首写 `public, max-age=300` → 命中 `private, max-age=300`（3 次复现）；入口 `public, max-age=5, s-maxage=5`。
12. **主/媒体清单伪装**：master 上游 → 主清单 `/{gid}/{cid}.m3u8`（`no-cache`），媒体清单 `/{gid}/{cid}/video/index.txt`（`text/plain`）。切片**忽略 `Range`**。
13. **播放 Token**（`token_enabled`）：`/playlist.m3u` 入口附 `?token=<48hex>`（全局、稳定、TTL 24h）；无/错 token → 302 `token_failure_url`；**切片不受保护**。`random_id`/`ip_guard` 本次未观测到可测效果。
14. **EPG**：`[epg].enabled` 关→`/epg*` 全 404；开启后 `/epg.xml`、`/epg.xml.gz`、`/epg` 免登录 200。XMLTV 源经 `POST /panel/api/epg/refresh` 采集，输出根 `generator-info-name="iptv-proxy"`；按 `keep_days`/`future_days` 裁剪；缺 `lang` 补 `zh`。配置为严格 TOML（重复键致启动失败）。
15. **分组访问范围**（`access`）：`public` 进总订阅；`hidden` 不进但可直连/分组订阅；`archived` 全停（分组订阅 404、播放 302、EPG 照常）；`open` **免 token 且也不进总订阅**（公开入口）。
16. **配置保存**：面板一切改动经 `PUT /panel/api/config` 全量原子保存并**规范化重写** `config.toml`；文件直改与面板保存有**竞态**；`server.m3u_header_lines` 自定义头行热重载+持久；EPG 开启自动注入 `x-tvg-url`。

## 3. 当前状态与下一步

- 容器 `iptv82` **运行中**（`samples/work/dyn-v0.8.2`）；宿主假源 `python3 -m http.server 9999`
  于 `samples/work/origin`（**注意**：后台进程，重启终端后需重开）。
- 已建：分组 `test1`（proxy）、`g2`、`gdash`（dash）、`gremux`（remux）；频道 `c1`/`c2`（HLS 上游）、`v`（MPD）、`r`（TS）。
- **下一步**：见 **§5 功能全集路线图**（Batch 1 起）。
- 快速遗留：16-hex 频道前缀（低影响，功能可替代）→ 归入 S7 反编译批次。

## 4. 进度日志

| 日期 | 事项 | 结果 |
| --- | --- | --- |
| 2026-10-15 | Phase 0 规范与骨架；首次提交 | ✅ `de898fe` |
| 2026-10-15 | Phase 1 取得产物；加壳/工具链判定 | ✅ `d2ade72`（F-001） |
| 2026-10-15 | 版本判定（v0.8.2 vs v0.8.8）；重登记指纹 | ✅ `c938fbd`（F-002） |
| 2026-10-16 | 内嵌面板雕刻 + 接口表面 | ✅ `5d94435`（F-003） |
| 2026-10-16 | 动态：启动、端点、认证、配置 schema、版本确认 | ✅ 见 `findings/dynamic/` |
| 2026-10-16 | 动态：建组/建频道；**切片伪装后缀破解**（sha256） | ✅（F-004）`54a5b9f` |
| 2026-10-17 | 动态：**六种模式 + 看门狗**；前缀推导仍未解 | ✅（F-005）`6311561` |
| 2026-10-17 | 动态：**缓存窗口（public→private）+ 缓存布局**；主/媒体清单伪装；切片忽略 Range | ✅（F-006）`74ee89c` |
| 2026-10-17 | 动态：**播放 Token 与护栏**（入口需 `?token=`，切片不受保护） | ✅（F-007） |
| 2026-10-17 | 动态：**EPG 采集/下发/裁剪/lang**；严格 TOML 校验 | ✅（F-008）`14fbe5e` |
| 2026-10-17 | 动态：**台标** `/logo/<file>`（`cache/logos/`，公开可读） | ✅（补 F-008） |
| 2026-10-17 | 路线图 Batch1·**D13 入口改写/访问范围**（access 四档含 open 例外、m3u_header_lines、配置原子保存） | ✅（F-009） |
| 2026-10-17 | 路线图 Batch1·**D13a 安全审计 + D29 指标面**（token 轮换+1h 宽限、IP 封禁面、登录 429 锁、指标仅面板私有面、面板 API 全集） | ✅（F-010） |
| 2026-10-17 | 基建：容器装 **ffmpeg 6.1.1 + python3 3.12**；起 **4 个独立实例**（19901–19904，独立配置/缓存，宿主机经容器 IP 直连）；派发 **4 个子代理**并行推进 D12/D15/D21/D28、D24/D25/D27、D16/D18/D19/D19b/D20、D14/D17ab/D22 | 🔄 进行中 |
| 2026-10-18 | 路线图 P1·**D23 台标库**（upload/apply/rename/delete/restore/suggest/animate；浏览器侧转码；`.orig/` 备份；引用同步）+ 配置段补录（panel/logo/subtitle/share/playlist/stats） | ✅（F-011） |
| 2026-10-17 | 委派 agent 盘点**二进制实际功能全集**（55k 字符串+面板），产出优先级路线图 | ✅ §5 |

## 5. 功能全集路线图（按优先级 · 2026-10-17）

> 来源：委派子代理对**二进制产物**的全量盘点（脱壳 strings 55k 条 + 雕刻面板 `panel.html` + 配置 schema）。
> 注：盘点证据为内嵌字符串/面板表单，路由前缀（如 `/panel/api`）以实测为准；「状态」为当前验证覆盖。

### P0 · 反向代理与 302 直连核心（最高优先级）

| 域 | 代表能力（二进制证据） | 状态 | 路线图项 |
| --- | --- | --- | --- |
| ① 反代播放 HLS（proxy/rewrite） | `/{gid}/{cid}/index.m3u8`、【代理】×44、`badge-proxy` | ✅ D7/F-004/F-005 | — |
| ② 切片伪装 URL | `<16hex>_<32hex>.<ext>`、`invalid segment hash` | 🔶 S 已破解；P 未解（低影响） | 随 S7 |
| ③ 切片缓存/预取 | `m3u8_max_entries`、`segment_cache_max_mb`、`prefetch.*`、`/cache/flush`、`/cache/stats` | 🔶 盘面已验；容量/淘汰/GC 未验 | 🔄 **D12**（子代理） |
| ④ 302 直连 redirect | `redirect_mode`(direct/default+best 解析)、`redirect_ttl`、`redirect_url_…` 缓存 | 🔶 direct ✅；best 未触发 | **D17a** |
| ⑤ passthrough 直链透传 | `badge-passthrough`、直链不套 token | ✅ D7 | — |
| ⑥ Token 鉴权 | `token_enabled/ttl/rotate`、`/security/token{,/rotate}`、`/link-info` | ✅ 轮换+1h 宽限+封禁面已验（F-010） | ~~D13a~~ ✅ |
| ⑦ 入口清单改写/聚合 | `m3u_header_lines`、`x-tvg-url`、`subscription-merge`、`access`(public/hidden/archived/免验证) | 🔶 access/头行已验；导入与聚合入口未测 | ~~D13~~ ✅；余项 D24 |
| ⑧ MPD/ClearKey 反代 | `mpd_keep_seconds`、`license_key`、`#KODIPROP clearkey`、`key_manager.rs`(088) | ⏳（需 MPD 样本） | **D14** |
| ⑨ URL 池多上游轮询 | `url_pool_size`、`pool[0..n]`、`/dash/scan-url`、`final_url` | ⏳ | **D15** |
| ⑩ 解析器最佳画质 | `/resolver/scripts`、`resolver_script/ttl/catchup`（Python） | ⏳（容器无 python3） | **D19b** |

### P1 · 中优先级（流处理与增值）

| 域 | 代表能力 | 状态 | 路线图项 |
| --- | --- | --- | --- |
| Remux（TS/FLV/MMT） | `remux_container(auto/ts/flv/mmt)`、`libx264…aac` | 🔶 容器识别已验；成功路径未验 | **D16**（需 ffmpeg） |
| DASH 时移/解密 | `/dash-timeshift/:track/…`、`dash_timeshift_hours/max_gb` | ⏳ | 随 D14 |
| 回看 catchup | `playseek=`、`catchup_days`、`catchup_tpl`、本地/上游双模式 | ⏳ | **D17b** |
| 定时录制 | 【录制】×49、`/schedule{,/from-epg,/:idx/stop}`、`recordings/` | ⏳ | **D18**（需 ffmpeg） |
| 字幕 | `WebVTT` 转换、样式注入、`srt_ua_keywords`、`.vtt` 伪装 | ⏳ | **D19** |
| 推流 livestream | `/livestream/:gid/:cid/start\|stop`、【推流】×35 | ⏳ | **D20** |
| 多 CDN 容错/降级 | `consecutive_fails`、`fallback_pct`(088) | ⏳ | **D21**（假源故障注入即可） |
| EPG 匹配/译制 | `match-report`、`mappings`、`epg_map`、gemini/mtran | ⏳ | **D22** |
| 台标库管理 | `/logo/{suggest,match-report,apply,animate}`、`to_webp` | ✅ 9 端点全验（F-011）：转码在浏览器侧、动效 Rust 编码+.orig 备份、改名/删除同步引用 | ~~D23~~ ✅ |

### P2 · 低优先级（运维与管理面）

| 域 | 代表能力 | 状态 | 路线图项 |
| --- | --- | --- | --- |
| 面板配置/组导入导出 | `/groups/{export,import,/m3u}`、`channels/:cid/purge`(088) | ⏳ | **D24** |
| 备份/恢复 | `/panel/api/backup?include=`、`restore`（tar/flate2） | ⏳ | **D25** |
| 多 VPS 分享 | `share.{role,master_url,trust_ips}`、`x-ipp-master`、`/api/share/pull` | ⏳ | **D26**（需双容器） |
| 边缘校验 | `edge_mode(off/proxy/cloudflare)`、`x-ipp-edge-secret`、观察模式 | ⏳ | **D27** |
| 健康检查 | 【健康】、`health{interval,timeout,concurrency}` | 🔶 出口形已验；探测行为进行中 | 🔄 **D28**（子代理） |
| IP 封禁/解封 | `/security/ip/{ban,stats,unban}`、`/unban` 每月 3 次 | ✅ 面已验（F-010/11 §3）；`/unban` 限次未验 | ~~随 D13a~~ ✅ |
| 指标/资源 | `/metrics`、`requests_total`、`mem_rss_mb`；`/status/metrics` 302 之谜 | ✅ `/status/metrics`＝假路由；指标仅面板私有面（F-010/11 §6） | ~~D29~~ ✅ |
| 资产/ffmpeg 检测 | `/assets/{convert,list,ffmpeg-status}` | ⏳ | 随 D18 |
| EPG 公开 JSON API | `/epg/api/channels\|programmes\|search` | 🔶 仅 088，记录不测 | spec 登记 |

### 执行批次

- **Batch 1（无 ffmpeg 即可）**：D13 入口改写/访问范围 → D13a Token 轮换与 IP 封禁面 → D15 URL 池 → D21 多 CDN 故障注入 → D24 组导入导出 → D25 备份恢复 → D27 边缘模式 → D29 指标。
- **Batch 2（容器装 ffmpeg）**：D18 录制 → D16 remux 成功路径 → D19 字幕 → D20 推流 → D19b 解析器（同时装 python3）。
- **Batch 3（样本/深层静态）**：D14 MPD/ClearKey/DASH 时移 → D17ab catchup 与重定向 best → D22 EPG 匹配算法 → S7 反编译（伪装 `<P>` 生成、热重载边界）。

### spec 缺口登记（盘点新发现，待补入 spec/08）

1. ClearKey/DRM 完整子系统（`license_key` + KODI 注入 + 088 `key_manager.rs`/AES-GCM）
2. `/panel/api/epg/api/*` 公开 JSON API（088）
3. `/panel/api/dash/scan-url`（手动 URL 池扫描）
4. `/panel/api/link-info`（播放链接/token 剩余有效期审计）
5. 每频道独立上游代理（`proxy{kind,addr,user,pass}`）
6. 旧数据迁移（088 清理 `./var/dash`）
7. 订阅合并 `subscription-merge`
8. `l1/l2` 双层限流窗口与缓存层级（hls/m3u8/sub_url/seg_url/seg_bytes）
