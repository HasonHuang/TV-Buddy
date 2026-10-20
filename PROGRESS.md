# PROGRESS · IPTV Proxy 逆向任务

> 本文件记录**任务计划**与**进度**。规则：**每处理完一项，立即更新本文件**。
> 规范见 [`docs/spec/`](docs/spec/)。发现见 [`docs/findings/`](docs/findings/)。实现设计见 [`docs/design/`](docs/design/)。
> 目标版本：**v0.8.2**（上游提交 `d302316fb8`）。

## 0. 任务目标

逆向闭源 Linux 二进制 **IPTV Proxy**（`YanG-1989/rust` 子目录 `IPTV Proxy/`，仅发布 arm64/amd64/v7）。
产出：可核验的规格与行为结论（对齐基线 [`sontv/docs/iptv-proxy`](../sontv/docs/iptv-proxy)）。

约束：
- 只用 **linux-arm64 / linux-amd64** 两个产物。
- 静态在 macOS（REA + Hopper）；动态在 **linux/arm64 容器**（OrbStack）。
- 目标派生内容一律按**不可信数据**处理。

## 1. 已完成（分阶段）

### Phase 0–1 · 规范、骨架与取得 ✅

| 项 | 结果 | 位置 |
| --- | --- | --- |
| 强制规范 8 篇 + 发现骨架 | ✅ `docs/spec/README,01–08`；`docs/findings/{README,artifact,static,dynamic}` | — |
| 取得产物（主 v0.8.2 / 对照 v0.8.8） | ✅ `samples/`（gitignore，指纹登记于 `artifact.md`） | `artifact.md` §2 |
| 加壳与 ELF | ✅ UPX 4.24、`upx -d` 成功；非 PIE、静态、NX、RELRO | `findings/static/01` §2–§3 |
| 工具链与版本 | ✅ Rust 1.98.1；v0.8.2=`d302316fb8`、main=v0.8.8 | `findings/static/01` §4–§5（F-001/F-002） |

### Phase 2 · 静态分析

| 项 | 状态 | 结果 / 位置 |
| --- | --- | --- |
| S1 指纹与 ELF 结构 | ✅ | `artifact.md` §2–§3 |
| S2 加壳/脱壳 | ✅ | `findings/static/01` §2–§3 |
| S3 工具链/语言（Rust） | ✅ | `findings/static/01` §4（S-ENV-07 升级为观测） |
| S4 版本判定 | ✅ | `findings/static/01` §5（F-002） |
| S5 端点/配置键预扫描 | ✅ | `findings/static/01` §6–§7 |
| S6 内嵌面板前端 | ✅ 4 页（管理面板 622KB / 节目单 / 台标库 / 自助解封） | `findings/static/02`（F-003） |
| S7 关键算法反编译 | ✅ 范围内完成（F-017） | 后缀=`sha256(上游切片URL)[:32]`；缓存窗口（F-006）；前缀 `<P>` 仅依赖 `(gid,cid)`、参与校验、**算法未解**（F-017，低影响） |

### Phase 3 · 动态分析（容器 `iptv82`，v0.8.2）

| 项 | 状态 | 结果 |
| --- | --- | --- |
| D1–D4 启动/端点/认证/版本 | ✅ | `[::]:19899`；`/health`=200、`/panel`=200、`/unban`=200、**`/status`=404**、EPG 关闭时 `/epg*` 404；未知频道→302 占位视频；`POST /panel/api/login`→`{"token":uuid}`（`Bearer` 或 `X-Token`）；`/panel/api/status`→`"version":"0.8.2"` |
| D5 配置 schema | ✅ | `/panel/api/config` 全量；见 `findings/dynamic/raw/v0.8.2-config.json` |
| D6 分组/频道 | ✅ | `POST /panel/api/groups`；`…/groups/{gid}/channels`；`PUT …/channels/{cid}` |
| D7 六模式行为 | ✅ | proxy/redirect/rewrite/passthrough 实测；dash/remux 失败路径+看门狗（F-005） |
| D8 切片伪装字节级 | 🔶 | 后缀=`sha256(上游切片绝对URL)[:32]`；前缀仅依赖 `(gid,cid)`、参与校验、扩展名不校验；算法未解（F-017） |
| D9 缓存窗口 | ✅ | 首写 `public,max-age=300` → 命中 `private,max-age=300`；入口 `public,max-age=5`（F-006） |
| D10 EPG | ✅ | `/epg.xml[.gz]`、`/epg` 免登录 200；根 `iptv-proxy`；时间裁剪与 `lang`（F-008） |
| D10b 录制/台标 | ✅ | 台标 `/logo/<file>`（`public,max-age=86400`）；录制 MKV + `ffmpeg -c copy`；`/assets/ffmpeg-status`→`installed:true`（F-014） |
| D11 播放 Token | ✅ | 入口需 `?token=<48hex>`（TTL 24h）；失效→302 `token_failure_url`；切片无 token 也 200（F-007） |
| D13 入口改写/访问范围 | ✅ | access 四档（open 不进总订阅）；`m3u_header_lines`；`PUT /panel/api/config` 全量原子保存（与文件直改有竞态）（F-009） |

### Phase 4 · 报告与回填

| 项 | 状态 | 结果 / 位置 |
| --- | --- | --- |
| R1 差异登记 | ✅ | `07` §3.1 `D-001…D-005` |
| R2 核对矩阵回填 | ✅ | `07` §2.3 动态项覆盖 F-004…F-017（新增 S-DIS-17） |
| R3 阶段性报告 | ✅ | [`findings/report-stage-01.md`](docs/findings/report-stage-01.md) |

### Phase 5 · 实现设计（进行中）

| 项 | 状态 | 结果 / 位置 |
| --- | --- | --- |
| E1 技术选型 | ✅ | [`design/01-tech-selection.md`](docs/design/01-tech-selection.md)（Rust + axum/tokio；Vite+React 面板嵌入；drop-in 兼容；首期 P0） |
| E1b UI 参考截图 | ✅ | [`design/reference/`](docs/design/reference/)（30 张：13 页 + 13 弹窗 + 4 独立页；含功能↔证据映射） |
| E2 架构与模块 | ✅ | [`design/02-architecture.md`](docs/design/02-architecture.md)（分层、模块清单、流水线、状态、缓存/伪装设计、ADR、里程碑） |
| E3 HTTP 契约 | ✅ | [`design/03-http-contract.md`](docs/design/03-http-contract.md)（公开/私有路由表、播放与订阅契约、access 矩阵、错误串、M1 清单） |
| E4 配置与存储 | ✅ | [`design/04-config-and-storage.md`](docs/design/04-config-and-storage.md)（配置写入与热重载矩阵、存储布局、运行时数据、日志、发布布局） |
| E5 前端设计 | ✅ | [`design/05-frontend.md`](docs/design/05-frontend.md)（技术形态、13 分区与弹窗体系、组件清单、交互约定、分期） |
| E6 P0 实现 | 🔄 | **M0 ✅、M1 ✅、M2 ✅、M3 ✅**；**M4 进行中**（热重载/看门狗/运维接口 ✅，对拍与打包待做）（判据见 [`design/02-architecture.md`](docs/design/02-architecture.md) §14） |

## 2. 关键结论

1. 样本：UPX 4.24 加壳；脱壳后 Rust 1.98.1、静态、非 PIE。
2. 版本：主对象 **v0.8.2**（`d302316fb8`）；`main` = **v0.8.8**；运行时 `/panel/api/status` 报 `0.8.2`。
3. 技术栈：`axum 0.7.9` + `tokio 1.52.1` + `hyper 1.9.0` + `reqwest 0.12.28` + `rustls` + `tracing`。
4. 面板前端**完整内嵌**（4 页）；接口前缀 `/panel/api/…`。
5. 伪装默认开启：`[cache.disguise]` enabled/playlist 开、subtitle/master 关、formats=jpg/png/gif/bmp/webp；`[cache.dash].obfuscate_filenames=true`。与基线 S-DIS-01 一致。
6. **切片伪装**：形状 `/{gid}/{cid}/<16hex>_<32hex>.<ext>`；`<32hex>` = `sha256(上游切片绝对 URL)[:32]`；
   `<P>`（16 hex）仅依赖 `(gid,cid)`、参与校验（错→302）、算法未解（F-017）；扩展名不校验。
7. **清单伪装**：主清单 `/{gid}/{cid}.m3u8`（`no-cache`）；媒体清单 `/{gid}/{cid}/video/index.txt`（`text/plain`）。切片忽略 `Range`。
8. 未知频道 → **302** 占位视频 `material.yang-1989.eu.org/v/{404,mistake,Failure,limit,Ban}.mp4`。
9. 面板认证：登录换取 **UUID token**；`Authorization: Bearer` 或 `X-Token` 二选一。
10. **六种模式**（分组级）：`proxy` 反代+伪装切片；`remux`；`redirect`/`passthrough` → 302 直连上游；`rewrite` 清单绝对化；`dash`。
    `redirect` 缓存上游地址 TTL 300s。`rewrite` + MPD 正文损坏（`D-005`）。
11. 看门狗：`dash` 主清单就绪超时 30s；`remux` 预热 `503 Retry-After:2`；连续 5 次失败 → 冷却 3600s。
12. **切片缓存窗口**：首写 `public, max-age=300` → 命中 `private, max-age=300`；入口 `public, max-age=5, s-maxage=5`。
13. **播放 Token**：入口需 `?token=<48hex>`（全局稳定、TTL 24h）；失效 → 302 `token_failure_url`；**切片不受保护**。`random_id`/`ip_guard` 无可测效果。
14. **EPG**：关闭时 `/epg*` 全 404；开启后 `/epg.xml`、`/epg.xml.gz`、`/epg` 免登录 200。
    输出根 `generator-info-name="iptv-proxy"`；按 `keep_days`/`future_days` 裁剪；缺 `lang` 补 `zh`；配置为严格 TOML。
15. **访问范围**（`access`）：`public` 进总订阅；`hidden` 不进但可直连/分组订阅；`archived` 全停（EPG 照常）；`open` 免 token 且不进总订阅。
16. **配置保存**：面板改动经 `PUT /panel/api/config` 全量原子写并规范化；与文件直改有竞态；`m3u_header_lines` 热重载；EPG 开启注入 `x-tvg-url`。

## 3. 当前状态与下一步

- **路线图已收敛**：§5 的 P0/P1/P2 项均已完成或登记为环境受限/仅 088（见各行状态）。
- 已建：分组 `test1`（proxy）、`g2`、`gdash`（dash）、`gremux`（remux）；频道 `c1`/`c2`（HLS 上游）、`v`（MPD）、`r`（TS）。
- **未解项（已定界）**：`<P>` 生成算法（F-017，低影响）；`S-DIS-08/09`（切片 TS 结构/水印）仅部分核验；
  多 CDN 故障切换未观测到（`D-004`）；`epg/api/*` 仅 088 未测。
- **收尾**：主实例配置已还原默认；停 6 个测试实例与容器；`samples/work/` 下运行产物不纳入版本控制。
- 阶段报告：[`docs/findings/report-stage-01.md`](docs/findings/report-stage-01.md)。

## 3b. 下一步（实现阶段）

- 阶段目标：按 [`docs/spec/08`](docs/spec/08-target-spec.md) 重实现 IPTV Proxy v0.8.2 的功能，**drop-in 兼容**。
- 节奏：先写设计（`docs/design/`），再写 P0 代码；P0 完成后与目标实例逐条对拍。
- 已完成：rustup 工具链（`rustc 1.99.1`）+ 两个 musl 目标；**M0 骨架**（配置读写、面板登录、内嵌 4 页前端、冒烟测试）；
  **M1 数据面**（分组/频道 CRUD、订阅导出、总/分组订阅与 `access` 过滤、播放 Token、分组管理页）。
- 已完成：**M4c 打包**——双架构 musl 静态包、`/opt/iptv-proxy` 布局、systemd/OpenRC 单元（见 §3g）。

## 3c. M1 数据面（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 分组 CRUD：`POST/PUT/DELETE /panel/api/groups[/{gid}]`（局部更新、`409`/`404` 错误串） | ✅ | `src/http/routes/groups.rs`；`smoke-m1.sh` |
| 频道 CRUD：`POST/PUT/DELETE /panel/api/groups/{gid}/channels[/{cid}]` | ✅ | 同上 |
| 导出：`GET /panel/api/groups/export`（数组）、`/{gid}/export`（对象） | ✅ | 同上 |
| 订阅：`GET /playlist.m3u`、`/{gid}/playlist.m3u`；`access` 过滤；直通组用上游地址；归档组订阅 `404` | ✅ | `src/playlist.rs`（6 个单元测试） |
| 播放 Token：48 位十六进制、内存、轮换宽限 1 h；`open` 组与直通组不带 Token | ✅ | `src/auth/play_token.rs` |
| 保存语义：改动立即原子重写 `config.toml`（以磁盘为事实源） | ✅ | `AppState::mutate` |
| 面板：侧栏 + 分组管理页（分组卡、频道表、弹窗增删改、复制订阅/播放地址） | ✅ | [`reference/ours/m1-groups.png`](docs/design/reference/ours/m1-groups.png) |
| 验证：`scripts/smoke-m0.sh`（M0 15 项）+ `scripts/smoke-m1.sh`（M1 全绿，debug 与 release）+ 无头浏览器端到端 9 项 | ✅ | 日志见 `PROGRESS` §4 |
| 未做（属 M2/M3）：播放链路 `/{gid}/{cid}.{ext}`、切片伪装与缓存、`redirect`/`rewrite`/`passthrough` 302 | ⏳ | `design/02` §14 |

## 4. 进度日志

| 日期 | 事项 | 结果 |
| --- | --- | --- |
| 2026-10-15 | Phase 0–2：规范骨架、取得产物、加壳与工具链、版本判定 | ✅ `de898fe`/`d2ade72`/`c938fbd`（F-001/F-002） |
| 2026-10-16 | 内嵌面板雕刻 + 接口表面；动态基线（启动/端点/认证/配置 schema/版本） | ✅ `5d94435`（F-003） |
| 2026-10-16 | 建组/建频道；**切片伪装后缀破解**（sha256） | ✅（F-004）`54a5b9f` |
| 2026-10-17 | 动态：六种模式+看门狗（F-005）·缓存窗口与布局（F-006）·播放 Token（F-007）·EPG（F-008）·台标 | ✅ `6311561`/`74ee89c`/`14fbe5e` |
| 2026-10-17 | 路线图 Batch1：**D13** 入口改写/访问范围（F-009）；**D13a+D29** 安全审计/指标面（F-010） | ✅ |
| 2026-10-17 | 基建：容器装 ffmpeg 6.1.1 + python3 3.12；起 4 个独立实例（19901–19904）；派发 4 个子代理（D12/D15/D21/D28、D24/D25/D27、D16/D18/D19/D19b/D20、D14/D17ab/D22） | ✅ 全部回收（F-013…F-016） |
| 2026-10-17 | 盘点二进制功能全集（55k 字符串+面板），产出优先级路线图 | ✅ §5 |
| 2026-10-18 | **D23** 台标库 + 配置段补录（F-011）；**D26** 多 VPS 分享 + 完整路由表（F-012） | ✅ |
| 2026-10-18 | 子代理：D24/D25/D27（F-013）·D16/D18/D19/D19b/D20（F-014）·D12/D15/D21/D28（F-015）·D14/D17ab/D22（F-016） | ✅ |
| 2026-10-18 | 收尾：**S7** 伪装前缀（F-017）；资产端点；一致性回填（spec/07-08 + 两个 README）；新增 `D-005`、`S-BUG-02` | ✅ |
| 2026-10-18 | Phase 4：R3 阶段性报告 | ✅ `findings/report-stage-01.md` |
| 2026-10-19 | 全库文档精简：删中间过程与重复叙述，保留全部编号/数字/状态；空行规范化 | ✅ 正文约 −25%~−30% |
| 2026-10-19 | 实现阶段启动 · **E1 技术选型**（Rust + axum；Vite+React 面板嵌入；drop-in；首期 P0） | ✅ [`design/01-tech-selection.md`](docs/design/01-tech-selection.md) |
| 2026-10-19 | **E1b UI 参考截图**：启动目标主实例造夹具，采集 30 张界面截图并映射功能与证据 | ✅ [`design/reference/`](docs/design/reference/) |
| 2026-10-19 | **E2 架构与模块设计**：分层、模块清单（P0/P1/P2）、请求流水线、共享状态、缓存/伪装设计、后台任务、ADR、里程碑 | ✅ [`design/02-architecture.md`](docs/design/02-architecture.md) |
| 2026-10-19 | 工具链：rustup `1.99.1`（stable）+ `aarch64/x86_64-unknown-linux-musl` 目标；两目标 `cargo check` 通过 | ✅ |
| 2026-10-19 | **E3 HTTP 契约**：公开/私有路由表、播放与订阅契约、`access` 矩阵、错误串对照、已知差异、M1 实施清单 | ✅ [`design/03-http-contract.md`](docs/design/03-http-contract.md)（`b080dec`） |
| 2026-10-19 | **E6 · M0 骨架**：配置模型/严格加载/原子保存、面板登录（UUID 会话）、状态与配置接口、内嵌 4 页前端（Vite+React）、`scripts/smoke-m0.sh` 15 项全绿 | ✅ 代码入仓（`cargo build` 无警告） |
| 2026-10-19 | **E4 配置与存储 + E5 前端设计**：热重载矩阵与存储布局、13 分区与弹窗体系、组件与交互约定 | ✅ [`design/04`](docs/design/04-config-and-storage.md)、[`design/05`](docs/design/05-frontend.md)（`aba1d1e`） |
| 2026-10-19 | **E6 · M1 数据面**：分组/频道 CRUD（局部更新、409/404 错误串）、导出、总/分组订阅与 `access` 过滤、播放 Token；面板分组管理页；`scripts/smoke-m1.sh` 全绿 | ✅ 见 §3c |
| 2026-10-19 | **E6 · M2 播放链路**：入口清单 → 媒体清单 → 伪装切片（`<P>_<S>.<ext>`）、切片索引与字节缓存、缓存窗口（首写 `public` / 命中 `private`）、302 占位、Token 校验；`scripts/smoke-m2.sh` 全绿 | ✅ 见 §3d |
| 2026-10-19 | **E6 · M3 直连模式**：`redirect`（含解析最佳画质、地址 TTL 缓存）、`rewrite`（清单绝对化）、`passthrough`（302 上游，校验 Token）、`remux` 预热 503、`dash` 30 s 就绪门；`scripts/smoke-m3.sh` 全绿 | ✅ 见 §3e |
| 2026-10-19 | **E6 · M4a 运维面**：配置热重载（只监视配置文件）、看门狗（连续 5 次失败 → 冷却 3600 s，重载即解除）、`cache/{stats,flush}`、`stats`、`logs`、日志文件 5 MB 截断；`scripts/smoke-m4.sh` 全绿 | ✅ 见 §3f |
| 2026-10-20 | **M4b 对拍**：同一夹具下 22 项用例差异 0；修正代理分支、缓存窗口、Token 判断顺序、`passthrough` 校验等 11 处契约差异 | ✅ [`reconciliation-report.md`](docs/design/reconciliation-report.md)、F-018 |
| 2026-10-20 | **目录结构**：改用「同名文件 + 目录」；新增 `src/lib.rs`，`main.rs` 只做启动 | ✅ `cargo test` 30 项 |
| 2026-10-20 | **M4c 打包**：Docker `rust:1-alpine` 双架构静态包、`/opt/iptv-proxy` 布局、systemd/OpenRC 单元；`verify-release.sh` 两架构通过（静态链接 + `/health`） | ✅ 见 §3g |

## 5. 功能全集路线图（按优先级 · 2026-10-17）

> 来源：委派子代理对**二进制产物**的全量盘点（脱壳 strings 55k 条 + 雕刻面板 `panel.html` + 配置 schema）。
> 注：盘点证据为内嵌字符串/面板表单，路由前缀（如 `/panel/api`）以实测为准；「状态」为当前验证覆盖。

### P0 · 反代与直连核心

| 域 | 代表能力（二进制证据） | 结论（含路线图项） |
| --- | --- | --- |
| ① 反代 HLS（proxy/rewrite） | `/{gid}/{cid}.m3u8`、【代理】×44、`badge-proxy` | ✅ F-004/F-005；`rewrite`+MPD 损坏（`D-005`） |
| ② 切片伪装 URL | `<16hex>_<32hex>.<ext>`、`invalid segment hash` | ✅ `<S>`=sha256[:32]；`<P>` 依赖/校验已解、算法未解（F-017，低影响）；**S7 ✅** |
| ③ 切片缓存/预取 | `m3u8_max_entries`、`segment_cache_max_mb`、`prefetch.*`、`/cache/flush`、`/cache/stats` | ✅ 容量上限均未生效；flush 保留 `dash/`；memory 后端统计恒 0（F-015，D12） |
| ④ 302 直连 redirect | `redirect_mode`、`redirect_ttl`、`redirect_url_…` | ✅ direct + best（选最高 `BANDWIDTH`）（F-016，D17a） |
| ⑤ passthrough 直链 | `badge-passthrough`；直链不套 token | ✅ D7 |
| ⑥ Token 鉴权 | `token_enabled/ttl/rotate`、`/security/token{,/rotate}`、`/link-info`、登录 429 锁 | ✅ 轮换+1h 宽限+封禁面（F-010，D13a） |
| ⑦ 清单改写/聚合 | `m3u_header_lines`、`x-tvg-url`、`subscription-merge`、`access` | ✅ access/头行/导入/聚合/订阅合并（F-009/F-013，D13） |
| ⑧ MPD/ClearKey | `mpd_keep_seconds`、`license_key`、`#KODIPROP`、`key_manager.rs`(088) | ✅ MPD→HLS；KID 双路检测；不解密；`#KODIPROP` 仅 passthrough（F-016，D14） |
| ⑨ URL 池 | `url_pool_size`、`pool[0..n]`、`/dash/scan-url`、`final_url` | ✅ 契约已验；仅 DASH；`pool≥2` 本环境 503（环境受限）（F-015，D15） |
| ⑩ 解析器画质 | `/resolver/scripts`、`resolver_script/ttl/catchup` | ✅ `python3 <cache>/resolvers/<脚本> <arg>`；TTL/SWR；超时转 `error_url`（F-014，D19b） |

### P1 · 流处理与增值

| 域 | 代表能力 | 结论（含路线图项） |
| --- | --- | --- |
| Remux（TS/FLV/MMT） | `remux_container(auto/ts/flv/mmt)`、`libx264…aac` | ✅ 仅连续流；输出 HLS/5s/TS/伪装；**不用 ffmpeg**（503=预热态）（F-014，D16） |
| DASH 时移 | `/dash-timeshift/:track/…`、`dash_timeshift_hours/max_gb` | ✅ `playlist.txt?playseek=` VOD；窗=已录区间；缺参 400、不重叠 404（F-016） |
| 回看 catchup | `playseek=`、`catchup_days`、`catchup_tpl` | ✅ 只转发 `playseek`；4 模板；`catchup-days` 由 days 或 `timeshift_hours/24` 推导（F-016，D17ab） |
| 定时录制 | `/schedule{,/from-epg,/:idx/stop}`、`recordings/`、【录制】×49 | ✅ MKV + `ffmpeg -c copy`；`{idx}`=u64 下标（F-014，D18） |
| 字幕 | `WebVTT` 转换、`.vtt` 伪装 | ✅ SUBTITLES 组保留；`.vtt`/`text/vtt` 透传（F-014，D19） |
| 推流 livestream | `/livestream/:gid/:cid/start\|stop`、【推流】×35 | ✅ `?start/end`/`?playseek` 回看 + VOD 清单 + `timeshift/<hex>.<ext>`（F-014，D20） |
| 多 CDN 容错 | `consecutive_fails`、`fallback_pct`(088) | ❌ 未观测到故障切换：按 BANDWIDTH 选变体、失败不重试（F-015，`D-004`，D21） |
| EPG 匹配/译制 | `match-report`、`mappings`、`epg_map`、gemini/mtran | ✅ 匹配算法已解；译制未测（F-016，D22） |
| 台标库 | `/logo/{suggest,match-report,apply,animate}`、`to_webp` | ✅ 9 端点全验；转码在浏览器侧；动效 Rust 编码 + `.orig` 备份；改名/删除同步引用（F-011，D23） |

### P2 · 运维与管理面

| 域 | 代表能力 | 结论（含路线图项） |
| --- | --- | --- |
| 组导入导出 | `/groups/{export,import,/m3u}`、`channels/:cid/purge`(088) | ✅ JSON 文本导入；组 id 去非 ASCII；merge 仅新增；聚合输出上游原始地址（F-013，D24） |
| 备份/恢复 | `/panel/api/backup?include=`、`restore` | ✅ 三 scope；restore 另存 `.bak`（F-013，D25） |
| 多 VPS 分享 | `share.{role,master_url,trust_ips}`、`/api/share/pull` | ✅ GET-only；key+trust_ips 双门禁；token/封禁 20s 同步；follower 禁本地轮换（F-012，D26） |
| 边缘校验 | `edge_mode(off/proxy/cloudflare)`、`x-ipp-edge-secret` | ✅ 头名 `X-IPP-Edge-Secret`；带转发头且密钥不符→403；`edge_refresh` 拉 CF 段（F-013，D27） |
| 健康检查 | `health{interval,timeout,concurrency}` | ✅ `GET`+`Range: bytes=0-1`；失败 +1/成功归零；仅展示（F-015，D28） |
| IP 封禁/解封 | `/security/ip/{ban,stats,unban}`、`/unban` 每月 3 次 | ✅ 面已验；`/unban` 限次未验（F-010，D13a） |
| 指标/资源 | `/metrics`、`requests_total`、`mem_rss_mb` | ✅ `/status/metrics` 为假路由；指标仅面板私有面（F-010，D29） |
| 资产/ffmpeg 检测 | `/assets/{convert,list,ffmpeg-status}` | ✅ `ffmpeg-status`→`{"installed":true,"path":"ffmpeg"}`；`assets/list` 列配置项（F-014，D18） |
| EPG 公开 JSON API | `/epg/api/{channels,programmes,search}` | ⏭ 仅 088；已在 `spec/08` §16 登记，不测 |

### 执行批次（已完成）

- Batch 1：D13 → D13a → D15 → D21 → D24 → D25 → D27 → D29。
- Batch 2（装 ffmpeg）：D18 → D16 → D19 → D20 → D19b。
- Batch 3：D14 → D17ab → D22 → **S7**（后缀/缓存窗口完成；`<P>` 输入依赖+校验已定，算法未解 → F-017）。

### spec 缺口登记

盘点新增的 8 项能力（ClearKey/DRM、`epg/api/*`、`dash/scan-url`、`link-info`、每频道代理、旧数据迁移、`subscription-merge`、`l1/l2` 限流）
已录入 [`spec/08` §16](docs/spec/08-target-spec.md)，状态与证据见该表。

## 3d. M2 播放链路（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 入口清单 `GET /{gid}/{cid}.m3u8`（`.mpd`）：`public, max-age=5, s-maxage=5`，指向 `/{gid}/{cid}/video/index.txt` | ✅ | `src/http/routes/stream.rs` |
| 媒体清单 `GET /{gid}/{cid}/video/index.txt`：抓上游、改写切片地址、`text/plain` | ✅ | `src/proxy/media.rs`（8 个单元测试） |
| 切片后缀 `<S>` = `sha256(上游绝对 URL)[:32]`；前缀 `<P>` = `sha256("<gid>:<cid>")[:16]` | ✅ | `smoke-m2.sh` 断言与独立计算一致 |
| 扩展名轮换：`[cache.disguise].formats`，每频道固定起点 | ✅ | `proxy::media::ext_offset` |
| 切片索引与字节缓存：`cache/seg_url/*.json`、`cache/seg_bytes/*`（重启可命中） | ✅ | `src/proxy/segments.rs` |
| 缓存窗口：首写 `public, max-age=300`；命中 `private, max-age=300` | ✅ | `smoke-m2.sh` |
| 忽略 `Range`（返回全量）；按伪装后缀给 `Content-Type` | ✅ | `smoke-m2.sh` |
| 302 占位：未知频道/分组、归档组、错 `<P>`；`404 Segment not found` | ✅ | `smoke-m2.sh` |
| Token：入口校验（`open` 组免校验），切片与媒体清单不校验 | ✅ | `smoke-m2.sh` |
| 主清单上游（含 `#EXT-X-STREAM-INF`）：变体地址绝对化透传 | ✅ | `smoke-m2.sh` |
| 未做（属 M3/M4）：`redirect` 地址 TTL 缓存、`rewrite` 清单改写、`passthrough` 302、`remux` 预热 `503`、`dash` 就绪判定 | ⏳ | `design/02` §14 |

验证：`cargo test` 18 项；`scripts/smoke-m0.sh`/`m1.sh`/`m2.sh` 在 debug 与 release 均全绿。

## 3e. M3 直连模式（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| `redirect`：入口 302 上游；地址按 `redirect_ttl` 缓存（0 → 300 s）；失败不缓存并跳 `error_url` | ✅ | `smoke-m3.sh`（上游请求计数 = 1） |
| `redirect` + `解析最佳画质`：取主清单挑最高 `BANDWIDTH` 变体 | ✅ | `proxy::modes::best_variant`（单元测试 + 冒烟） |
| `passthrough`：302 上游；**校验 Token**（F-018 修正）；不追加 Token | ✅ | `smoke-m3.sh` |
| `rewrite`：入口 200 `text/plain`，相对地址改绝对，切片客户端直连；清单缓存 ≈5 s | ✅ | `smoke-m3.sh` |
| `remux`：恒 `503` + `Retry-After: 2` + `remux warming up`；首次抓上游分类记日志 | ✅ | `smoke-m3.sh` |
| `dash`：30 s 就绪门；失败 `503 timeout waiting for master playlist`；正文透传 `application/dash+xml` | ✅ | `smoke-m3.sh` |
| MPD 正文（proxy 与 dash）原样透传，不按行改写 | ✅ | `stream.rs` 的 `looks_like_mpd` 分支 |
| 未做（属 P1）：`remux`/`dash` 的 `ffmpeg` 转封装、DASH 轨道命名与片段容错 | ⏳ | ADR-04、S-MODE-06 |

验证：`cargo test` 23 项；`smoke-m0.sh`…`smoke-m3.sh` 在 debug 与 release 均全绿。

## 3f. M4a 运维面（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 热重载：只监视配置文件本身（避免日志文件写盘误触发）；解析失败保留旧快照并记 `error` | ✅ | `src/reload.rs`；`smoke-m4.sh` |
| 看门狗：同一频道连续失败 5 次 → 冷却 3600 s；冷却期内不再请求上游；配置重载立即解除（S-MODE-17） | ✅ | `src/watchdog.rs`（单元测试 + 冒烟） |
| 缓存：`GET /panel/api/cache/stats`（按目录计数与字节）、`POST /panel/api/cache/flush`（保留 `dash/`，并丢弃进程内索引与清单缓存） | ✅ | `src/http/routes/admin.rs` |
| 指标：`GET /panel/api/stats`（请求数、分组请求数、清单/切片数、切片命中、上游失败、在线窗口） | ✅ | `src/stats.rs`（单元测试） |
| 日志：`GET /panel/api/logs` 返回环形缓冲（500 行）；同时输出到标准输出与 `<配置目录>/iptv-proxy.log`，超 5 MB 截断 | ✅ | `src/logging.rs`（单元测试） |
| 对拍（M4b）：同一夹具下 22 项用例，差异 0；发现并修正 11 处契约差异 | ✅ | `scripts/compare-p0.sh`、`design/reconciliation-report.md`、F-018 |
| 打包（M4c）：Docker 双架构静态包 + `/opt/iptv-proxy` 布局 + systemd/OpenRC | ✅ | §3g、`design/04` §6 |

验证：`cargo test` 30 项；`smoke-m0.sh`…`smoke-m4.sh` 全绿。

## 3g. M4c 打包（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 静态构建：`aarch64-unknown-linux-musl`、`x86_64-unknown-linux-musl` | ✅ | `scripts/build-release.sh`：Docker `rust:1-alpine`（镜像宿主三元组即 musl），依赖走宿主 `~/.cargo` 缓存 `--offline` |
| 发布布局：`/opt/iptv-proxy`（二进制、配置样例、启停脚本、安装脚本、服务单元） | ✅ | `packaging/`、[`design/04`](docs/design/04-config-and-storage.md) §6 |
| 产物校验：ELF 无 `PT_INTERP`、容器内启动、`/health` = `ok` | ✅ | `scripts/verify-release.sh`：arm64、amd64 均通过 |
| 安装脚本：不覆盖已有配置；按存在性注册 systemd 或 OpenRC | ✅ | alpine:3.20 容器内 `install.sh` + `iptv-proxy.sh start/status/stop` 实测 |
| 产物 | `dist/iptv-proxy-0.1.0-{arm64,amd64}.tar.gz`（2.5 / 2.7 MB，附 `.sha256`） | 不入库（`.gitignore` 已忽略 `/dist/`） |

验证：`scripts/verify-release.sh arm64` 与 `... amd64` 均输出「静态链接 OK」「/health = ok」。

## 3h. 集成测试（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 路由级集成测试：起真实 HTTP 服务 + 伪上游，用 `reqwest`（不跟随跳转）观测契约 | ✅ | `tests/http_contract.rs`（10 项） |
| 覆盖：健康检查与总订阅、媒体清单入口、主清单入口、变体清单与别名、切片首写/命中缓存头、Token 与未知路径顺序、归档组、`passthrough` 校验、未知面板接口 | ✅ | 同上 |

验证：`cargo test` 共 40 项（30 单元 + 10 集成）全绿。

> 说明：本机访问 `github.com` release 资产与 `static.rust-lang.org` 的 TLS 会被中途断开，
> 因此不用 `cross`，改用 Docker Hub 可用的 `rust:1-alpine` 完成双架构构建。

## 3i. P1 · EPG（已完成，首个 P1 功能）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 配置：`[epg]` 全字段 + `[[epg.sources]]`（键面对齐目标） | ✅ | `src/config/model.rs` |
| 采集：拉 XMLTV → 解析 → 时间窗裁剪 → 频道匹配 → 生成 `epg.xml` 与 `epg.xml.gz` | ✅ | `src/epg.rs`（6 项单元测试） |
| 公开面：`/epg.xml`、`/epg.xml.gz`、`/epg`；关闭时三者 `404`（S-EP-05） | ✅ | `src/http/routes/epg.rs` |
| 面板：`/panel/api/epg/status`、`/panel/api/epg/refresh`（Bearer） | ✅ | 同上 |
| 公开 JSON：`/epg/api/{channels,programmes/:channel,search}` | ✅ | `src/http/routes/epg.rs`；集成测试 `epg_api_exposes_channels_programmes_and_search` |
| 浏览页：`/epg` 频道列表 + 节目列表 + 搜索（自研 UI，不复用目标文案） | ✅ | `frontend/src/epg.tsx` |
| 调度：启动即采集；之后按 `0 */N * * *` 或 4 小时 | ✅ | `epg::spawn` |
| 验证 | ✅ | `scripts/smoke-epg.sh` 13 项全绿；`tests/http_contract.rs` EPG 2 项 |
| 未做 | ⏳ | 脚本源、翻译接口、映射/录制联动（P2） |

验证：`cargo test` 共 51 项（36 单元 + 15 集成）；`scripts/smoke-epg.sh` 18 项全绿。

## 3j. 回看参数与兜底（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 回看：入口只把 `playseek` 追加到上游；缓存键带地址标签，回看区间不互相覆盖 | ✅ | `stream.rs::with_playseek`/`url_tag`；集成测试 `playseek_is_forwarded_to_upstream` |
| 兜底：未知单段路径 → `404`（目标 `/status` 行为，S-EP-08） | ✅ | 集成测试 `single_segment_unknown_path_is_404` |

验证：`cargo test` 50 项（36 单元 + 14 集成）全绿。

## 3k. P1 · 客户端地址与分组导入（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| `GET /whoami`、`GET /unban/whoami` → `{"ip":…}`（`X-Forwarded-For` 首项 / `X-Real-IP` / `unknown`） | ✅ | `src/http/routes/public.rs` |
| `POST /panel/api/groups/import`：分组数组导入，重复 id 跳过 | ✅ | `src/http/routes/groups.rs` |
| `POST /panel/api/groups/import/force`：重复 id 覆盖 | ✅ | 同上 |
| 校验：非数组 → `400`；未登录 → `401`；导入后订阅立即可见 | ✅ | 集成测试 `group_import_adds_skips_and_overwrites`、`whoami_reports_forwarded_ip` |

验证：`cargo test` 53 项（36 单元 + 17 集成）全绿。

## 3l. P1 · M3U 导入（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 解析：`#EXTINF` 属性、`#EXTVLCOPT`、`#EXTHTTP`、行内头 | ✅ | `src/m3u.rs`（5 项单元测试） |
| 落库：`append`/`merge`/`replace`；`auto` 按 `.mpd` 选 `dash`/`rewrite` | ✅ | `groups.rs::import_m3u` |
| id 规则：分组去非 ASCII、空则 `g-<6hex>`；频道优先 `tvg-id` | ✅ | `m3u::group_id`/`channel_id` |
| 属性保留：`tvg_*`/`group_title` → `extra`；头 → `channel.headers`（只落库） | ✅ | 同上；转发附加留 P2 |
| 校验：非法 `mode`/`group_mode`、空 `text`、无频道 → `400` | ✅ | 集成测试 `m3u_import_creates_groups_and_channels` |
| 文档 | ✅ | `docs/design/03-http-contract.md` §6.2.1 |

验证：`cargo test` 59 项（41 单元 + 18 集成）全绿。

## 3m. 发布产物重建（P1 批次后）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 修复 `build-release.sh` 无参数时把 `amd64 arm64` 当作单架构的问题 | ✅ | `scripts/build-release.sh` |
| 重建双架构静态包（含 EPG 页面与 P1 接口） | ✅ | `dist/iptv-proxy-0.1.0-{amd64,arm64}.tar.gz`（3.0/2.8 MB） |
| 校验：ELF 无 `PT_INTERP`、能启动、`/health = ok` | ✅ | `scripts/verify-release.sh amd64/arm64` |
| 校验：发布包内 `/epg` 页面引用新资源，`/__assets/assets/epg-*.js` 返回 2866 字节且含页面文案 | ✅ | alpine:3.20 + amd64 包实测 |

> `dist/` 不入库（`.gitignore`）。

## 3n. P1 · 台标库（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 存储：`<cache>/logos/`；白名单扩展名 `png/jpg/jpeg/gif/webp/svg`；文件名去路径与危险字符 | ✅ | `src/logo.rs`（4 项单元测试） |
| 公开面：`/logo` 页面、`/logo/<file>`（`public, max-age=86400`，缺文件/越界名/非法扩展 → 404）、`/logo/api/list` | ✅ | `src/http/routes/logo.rs` |
| 面板：`/panel/api/logo/{list,upload,delete}`；`data:` URL 与纯 base64 都接受；空内容与非法扩展进 `failed` | ✅ | 同上 |
| 前端：台标库页（网格、上传、删除；上传按钮仅在已登录时显示） | ✅ | `frontend/src/logo.tsx` |
| 验证 | ✅ | `scripts/smoke-logo.sh` 15 项；集成测试 `logo_library_upload_serve_and_delete` |

验证：`cargo test` 64 项（45 单元 + 19 集成）。

## 3o. P1 · 备份与恢复（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| `GET /panel/api/backup`：gzip(tar)，`all`/`epg-scripts`/`resolver` 三种作用域，文件名带 UTC 时间戳 | ✅ | `src/backup.rs`、`routes/backup.rs` |
| 包布局：`config.toml`、`logos/`、`epg/scripts/`、`epg/epg_map.json`、`resolvers/`、`manifest.json` | ✅ | 同上 |
| `POST /panel/api/restore`：解包 → 校验配置 → `config.toml.bak` → 原子写 → 恢复台标 → 重载 | ✅ | 同上 |
| 请求体上限 64 MiB；非法内容 → `400` | ✅ | `http.rs` |
| 验证 | ✅ | `scripts/smoke-backup.sh` 10 项；集成测试 `backup_and_restore_roundtrip`；`backup` 3 项单元测试 |

验证：`cargo test` 68 项（48 单元 + 20 集成）。

## 3p. P1 · 频道自定义头转发（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| `channel.headers`（`"名称: 值"`）在取上游清单与切片时附加 | ✅ | `stream.rs::channel_headers` + `fetch`/`fetch_bytes` |
| 覆盖模式：`proxy`、`rewrite`、`dash`、`remux` | ✅ | 同上；302 模式由客户端直连，无法附加 |
| 非法条目（无冒号、空名）忽略，不影响播放 | ✅ | `channel_headers` |
| 验证 | ✅ | 集成测试 `channel_headers_are_sent_upstream`（伪上游回显） |

验证：`cargo test` 69 项（48 单元 + 21 集成）。

## 5. 后续路线（P1 剩余与 P2）

已完成：P0 全部（M0–M4）、EPG、回看、台标库、备份/恢复、分组导入（JSON 与 M3U）、自定义头转发、发布打包。

| 项 | 规模 | 依赖 | 说明 |
| --- | --- | --- | --- |
| 面板其余分区 UI（缓存、日志、EPG、台标、安全、订阅） | 中 | 无 | 接口大多已就绪；前端按既有页签模式补页 |
| 订阅聚合 `[playlist]`（`sources[]`、`merge_ttl_secs`、`merge_position`、`global_exclude[]`） | 中 | 无 | 需新增配置建模、远端拉取与合并、`include`/`exclude` 前缀过滤（F-009、F-013 §2） |
| 安全面补全：`ip/unban-batch`、按 IP 会话上限、自动限流、`GET /unban` 自助解封 | 中 | 无 | 基础面已在 §3q 实现 |
| `remux` / `dash` 转封装 | 大 | `ffmpeg` | 外部程序；P0 起恒 503 预热 |
| 录制与推流 `/panel/api/schedule`、`/livestream/*`、`/timeshift/*` | 大 | `ffmpeg` | 需任务调度与文件管理 |
| EPG 脚本源与翻译接口 | 大 | `python3` | `kind="script"`、`/epg/translate/*` |
| `/logo/{apply,rename,restore,suggest,animate,match-report}` | 中 | 无 | 库内其余 6 个端点；`animate` 需图像处理 |
| 多 VPS 分享 `/api/share/pull` 与 `edge_mode` | 中 | 无 | 见 F-013 §10、F-016 |
| 面板 `/panel/api/{health,assets,schedule,dash,subtitle,resolver}` | 中 | 视项 | 按 F-010/F-011/F-014 补 |

> 每项开工前先补 `docs/design/` 小节与用例，再按 M0–M4 的节奏提交。

## 3q. P1 · 安全面（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| Token：`GET /panel/api/security/token`、`POST …/token/rotate`（旧值 1 小时宽限） | ✅ | `routes/security.rs`；`PlayTokenStore` 复用 |
| IP 统计与封禁：`ip/stats`、`ip/ban`（默认 1 小时、上限 30 天）、`ip/unban`（单个或批量） | ✅ | `src/auth/ip_guard.rs`（4 项单元测试） |
| 执行：非面板路径按转发头记账，封禁期内 `403`；面板路径不拦截 | ✅ | `src/http/guard.rs`（中间件） |
| 容量：最多 10 000 个地址 | ✅ | 同上 |
| 验证 | ✅ | `scripts/smoke-security.sh` 10 项；集成测试 `security_token_rotation_and_ip_ban` |

验证：`cargo test` 74 项（52 单元 + 22 集成）。

## 3r. P1 · 订阅聚合（已完成）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| `[playlist]` 配置：`merge_ttl_secs`、`merge_position`、`global_exclude`、`[[playlist.sources]]` | ✅ | `config/model.rs` |
| 合并：只作用于 `GET /playlist.m3u`；保留上游原始 URL，不改写、不加 Token | ✅ | `playlist::aggregate`、`compose`、`routes/play.rs` |
| 过滤词：`group:`、`id:`、频道名；大小写不敏感；`exclude` 优先 | ✅ | 同上（4 项单元测试） |
| 缓存：键含时间桶，按 `merge_ttl_secs` 生效 | ✅ | `AppState::sources` |
| 失败隔离：单源失败只记日志，本地订阅照常返回 | ✅ | `aggregate_sources` |
| 验证 | ✅ | `scripts/smoke-aggregate.sh` 5 项；集成测试 2 项 |

验证：`cargo test` 81 项（56 单元 + 25 集成）。

## 3s. P1 · 面板分区（安全/日志/备份）

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 安全管控页：Token 展示与轮换、IP 统计表、封禁与解封 | ✅ | `frontend/src/ops.tsx` |
| 运行日志页：`/panel/api/logs?limit=`，5 秒轮询、可改行数 | ✅ | 同上 |
| 备份恢复页：作用域选择、浏览器内下载；上传 tar.gz 恢复并提示文件数 | ✅ | 同上 |
| 侧栏：已实现分区可切换，其余置灰「待实现」 | ✅ | `frontend/src/panel.tsx` |
| 验证 | ✅ | `pnpm build` 通过；实测 `/panel` 引用 `panel-*.js` 且内含 `security/token` |

验证：`cargo test` 81 项不变（前端不参与单元测试）。

## 3t. P1 · 缓存与热度分区

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 缓存状态页：子目录文件数、总占用、内存清单项数、刷新与清空（二次确认） | ✅ | `frontend/src/cache.tsx` |
| 频道热度页：运行时长、请求/切片/命中/上游失败、分组排行 | ✅ | 同上 |
| 侧栏：两个分区移出「待实现」 | ✅ | `frontend/src/panel.tsx` |
| 验证 | ✅ | `pnpm build` 通过；`cargo test` 81 项 |

## 3u. P1 · 节目指南分区

| 项 | 状态 | 证据 |
| --- | --- | --- |
| 状态：开关、源数、频道/节目数、保留窗口、最近成功与错误 | ✅ | `frontend/src/ops.tsx` → `EpgPanelPage` |
| 操作：立即采集（同步）、刷新状态、新窗口打开 `/epg` | ✅ | 同上 |
| 验证 | ✅ | `pnpm build` 通过 |
