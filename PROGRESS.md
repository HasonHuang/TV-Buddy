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
| D7 六模式行为 | ⏳ | 待做 |
| D8 切片伪装字节级 | 🔶 | **后缀已破解**：`sha256(上游切片绝对URL)[:32]`；16-hex 频道前缀未解 |
| D9 缓存窗口 | 🔶 | 响应头已测（入口 `public,max-age=5`；切片 `private,max-age=300`）；时序未测 |
| D10 录制/EPG/台标 | ⏳ | 待做（需 ffmpeg） |

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

## 3. 当前状态与下一步

- 容器 `iptv82` **运行中**（`samples/work/dyn-v0.8.2`）；宿主假源 `python3 -m http.server 9999`
  于 `samples/work/origin`（**注意**：后台进程，重启终端后需重开）。
- 已建：分组 `test1`（`mode=proxy`，伪装开）+ 频道 `c1`/`c2`。
- **下一步**：
  1. 破解 16-hex 频道前缀（换更多 `(gid,cid)` 样例 / 反编译）。
  2. D7 六种模式行为实测。
  3. D9 缓存窗口时序采样。
  4. S7 关键函数反编译（Hopper/REA）定位前缀哈希与伪装实现。
  5. D10 录制/EPG/台标（需装 ffmpeg）。

## 4. 进度日志

| 日期 | 事项 | 结果 |
| --- | --- | --- |
| 2026-10-15 | Phase 0 规范与骨架；首次提交 | ✅ `de898fe` |
| 2026-10-15 | Phase 1 取得产物；加壳/工具链判定 | ✅ `d2ade72`（F-001） |
| 2026-10-15 | 版本判定（v0.8.2 vs v0.8.8）；重登记指纹 | ✅ `c938fbd`（F-002） |
| 2026-10-16 | 内嵌面板雕刻 + 接口表面 | ✅ `5d94435`（F-003） |
| 2026-10-16 | 动态：启动、端点、认证、配置 schema、版本确认 | ✅ 见 `findings/dynamic/` |
| 2026-10-16 | 动态：建组/建频道；**切片伪装后缀破解**（sha256） | ✅（F-004） |
