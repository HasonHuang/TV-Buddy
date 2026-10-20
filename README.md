# IPTV Proxy（兼容重实现）

按逆向文档重实现 IPTV Proxy。目标：与 **v0.8.2**（linux-arm64 / linux-amd64）**drop-in 兼容**。

| 文档 | 内容 |
| --- | --- |
| [`docs/spec/`](docs/spec/) | 目标规格（`S-*`）与验证基线 |
| [`docs/findings/`](docs/findings/) | 逆向证据（`F-*`、`D-*`） |
| [`docs/design/`](docs/design/) | 本实现的方案（选型、架构、UI 参考） |
| [`PROGRESS.md`](PROGRESS.md) | 唯一进度权威 |

## 构建

```bash
# 1) 前端（Vite + React + TS → frontend/dist）
scripts/build-frontend.sh

# 2) 后端（Rust）
cargo build --release

# 交叉编译（静态 musl）
rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

## 运行

```bash
target/release/iptv-proxy config.toml
```

默认监听 `[::]:19899`。面板入口 `http://<host>:19899/panel`，默认口令 `admin` / `admin`。

## 测试

```bash
scripts/smoke-m0.sh   # 健康检查、登录、状态、配置读写、鉴权、兜底
scripts/smoke-m1.sh   # 分组/频道 CRUD、导出、订阅与 access 过滤、播放 Token
scripts/smoke-m2.sh   # 播放链路、切片伪装、缓存窗口、302 占位（含伪上游）
scripts/smoke-m3.sh   # 直连模式：redirect/rewrite/passthrough、remux/dash 就绪语义
scripts/smoke-m4.sh   # 热重载、看门狗冷却、缓存状态与清空、指标与日志
scripts/smoke-epg.sh  # EPG：采集、/epg.xml[.gz]、/epg 页面与 /epg/api/*、状态与刷新
scripts/smoke-logo.sh # 台标库：上传、公开取图与列表、删除
scripts/smoke-backup.sh # 备份与恢复：导出 gzip、改动后回滚、拒绝非法内容
scripts/smoke-security.sh # 安全面：Token 查看/轮换、IP 统计、封禁与解封
scripts/smoke-aggregate.sh # 订阅聚合：远端源合并、过滤与位置
scripts/smoke-assets.sh # 素材：ffmpeg 状态、视频/字幕上传与删除、解析脚本\nscripts/smoke-share.sh  # 多 VPS：门禁、Token 同步、封禁汇总
scripts/compare-p0.sh # 对拍：同一夹具下对比目标 v0.8.2（需容器 iptv82）
tests/             # 路由级集成测试（起真实服务 + 伪上游）
```

## 接口（已实现）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/health` | `200 text/plain` `ok` |
| `GET` | `/playlist.m3u` | 总订阅（只含 `access = "public"` 的组；可聚合 `[playlist].sources`） |
| `GET` | `/{gid}/playlist.m3u` | 分组订阅（归档组 `404`） |
| `GET` | `/{gid}/{cid}.m3u8`（`.mpd`） | 入口清单（`public, max-age=5, s-maxage=5`） |
| `GET` | `/{gid}/{cid}/video/index.txt` | 媒体清单（`text/plain`，切片地址已伪装） |
| `GET` | `/{gid}/{cid}/<P>_<S>.<ext>` | 伪装切片（忽略 `Range`） |
| `POST` | `/panel/api/login` | 登录换取会话 token |
| `GET`/`PUT` | `/panel/api/config` | 读取 / 全量保存配置 |
| `GET` | `/panel/api/groups/export` | 导出全部分组（数组） |
| `GET` | `/panel/api/groups/{gid}/export` | 导出单个分组 |
| `POST` | `/panel/api/groups` | 新建分组 |
| `PUT`/`DELETE` | `/panel/api/groups/{gid}` | 局部更新 / 删除分组 |
| `POST` | `/panel/api/groups/{gid}/channels` | 新建频道 |
| `PUT`/`DELETE` | `/panel/api/groups/{gid}/channels/{cid}` | 局部更新 / 删除频道 |
| `POST` | `/panel/api/reload` | 立即重读配置 |
| `GET` | `/panel/api/cache/stats` | 缓存目录条目与占用 |
| `POST` | `/panel/api/cache/flush` | 清空缓存（保留 `dash/`） |
| `GET` | `/panel/api/stats` | 运行指标 |
| `GET` | `/panel/api/logs` | 最近运行日志 |
| `GET` | `/panel/api/security/token` | 当前播放 Token 与宽限旧值 |
| `POST` | `/panel/api/security/token/rotate` | 轮换 Token |
| `GET` | `/panel/api/security/ip/stats` | IP 请求统计与封禁状态 |
| `POST` | `/panel/api/security/ip/{ban,unban}` | 封禁 / 解封地址 |
| `GET` | `/panel/api/backup?include=` | 导出 gzip(tar) 备份 |
| `POST` | `/panel/api/restore` | 恢复备份并重载 |
| `GET` | `/logo/<file>` | 台标（`public, max-age=86400`） |
| `GET` | `/logo/api/list` | 台标列表 |
| `POST` | `/panel/api/logo/{upload,delete,rename,apply}` | 上传 / 删除 / 重命名 / 应用到频道 |
| `GET` | `/panel/api/logo/match-report` | 缺台标、坏引用与未使用 |
| `POST` | `/panel/api/logo/suggest` | 按名称相似度推荐台标 |
| `GET` | `/panel/api/assets/list` | 视频素材列表 |
| `POST` | `/panel/api/assets/{upload,remove}` | 上传 / 删除视频素材 |
| `GET` | `/panel/api/assets/ffmpeg-status` | ffmpeg 检测 |
| `GET`/`POST` | `/panel/api/subtitle/*` | 字幕列表、上传、删除、下载 |
| `GET`/`POST` | `/panel/api/resolver/{scripts,script,remove}` | 解析脚本管理 |
| `GET` | `/api/share/pull?key=` | 多 VPS 同步载荷 |
| `GET` | `/epg.xml[.gz]` | 节目单 |
| `GET` | `/epg/api/{channels,programmes,search}` | 节目单 JSON |
| `POST` | `/panel/api/groups/import[/m3u]` | 批量导入分组（JSON / M3U） |

## 目录

```text
src/
  main.rs          启动入口（只做参数、日志、状态、监听）
  lib.rs           库根：模块声明
  state.rs         共享状态（ArcSwap 配置快照 + 改动串行化 + 缓存目录）
  config.rs        配置模型入口；config/model.rs 为键面模型
  playlist.rs      M3U 订阅渲染
  proxy.rs         清单改写、切片索引与字节缓存；proxy/{media,modes,segments}.rs
  auth.rs          面板会话、播放 Token；auth/{play_token,session}.rs
  http.rs          路由、提取器、错误映射、内嵌静态资源；http/routes.rs 与 http/routes/*.rs
frontend/          Vite + React + TS 面板（4 页）
scripts/           构建与冒烟测试脚本
```

## 发布与打包

```bash
scripts/build-release.sh            # 构建 amd64 与 arm64 两个静态 musl 包
scripts/verify-release.sh arm64     # 校验：静态链接 + 启动 + /health
```

产物在 `dist/iptv-proxy-<version>-<arch>.tar.gz`，含二进制、配置样例、启停脚本、安装脚本与 systemd/OpenRC 单元。

- 构建用 Docker `rust:1-alpine`：镜像宿主三元组即 `*-linux-musl`，直接产出静态二进制。
- 容器无外网，依赖来自宿主 `~/.cargo` 缓存（`--offline`）。
- 安装：解包后 `sudo ./install.sh`，默认装到 `/opt/iptv-proxy`。

## 运维

- 配置热重载：改 `config.toml` 即生效（`[epg]`、`[cache].disk_path` 需重启，与目标一致）。
- 日志：标准输出与 `<配置目录>/iptv-proxy.log`；超过 5 MB 截断。`[server].log_level` 控制级别。
- 看门狗：同一频道连续失败 5 次后冷却 3600 秒；改配置立即解除。

## 已知限制（M4）

- `remux` 恒 `503 remux warming up`，`dash` 只做 30 s 就绪门与正文透传：转封装需 `ffmpeg`，属 P1。
- `proxy` 链路面向 HLS：媒体清单按行改写；MPD 正文原样透传。
- 缓存无容量上限（与目标一致，见 F-015）。
- 运维接口的响应形状为本实现自定；目标只登记存在性（见 `design/03` §6.4）。
- 面板只实现概览与分组管理；其余分区显示为「待实现」。
- EPG：只实现 `kind = "xml"` 源；脚本源、翻译接口、映射与录制联动未实现（`design/03` §6.5）。
- 与目标的差异以 `docs/design/reconciliation-report.md` 为准（22 项用例差异 0）。
- `/epg`、`/logo`、`/unban` 仍是骨架页；EPG 未接入，订阅不输出 `x-tvg-url`。
