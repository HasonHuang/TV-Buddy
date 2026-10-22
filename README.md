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

## 一键安装（Linux）

`install.sh` 只支持 Linux **amd64/arm64**。它支持三种来源：已解包的发布目录、仓库 `dist/`、指定归档（本地路径或 URL）。

### 从发布包安装（推荐）

```bash
tar -xzf iptv-proxy-0.1.0-arm64.tar.gz
cd iptv-proxy-0.1.0-arm64
sudo ./install.sh
```

### 从仓库安装

先构建发布包，再执行根目录脚本：

```bash
scripts/build-frontend.sh
scripts/build-release.sh              # 或只构建当前架构：scripts/build-release.sh arm64
sudo ./install.sh                     # 自动使用 dist/ 中最新的同架构包
```

### 指定归档或 URL

```bash
sudo ./install.sh --archive dist/iptv-proxy-0.1.0-arm64.tar.gz
sudo ./install.sh --archive https://example.com/iptv-proxy-0.1.0-arm64.tar.gz
```

归档旁有同名 `.sha256` 文件时，脚本自动校验。

### 选项

| 选项 | 说明 |
| --- | --- |
| `--prefix <目录>` | 安装目录；默认 `/opt/iptv-proxy` |
| `--archive <路径或 URL>` | 指定 tar.gz 安装包 |
| `--no-service` | 不注册 systemd/OpenRC 服务 |
| `--no-link` | 不创建 `iptv-proxy` 命令软链 |
| `--start` | 安装后立即启动服务 |
| `-h`、`--help` | 显示帮助 |

### 安装内容

- 二进制：`<prefix>/iptv-proxy`
- 配置：`<prefix>/config.toml`；已有配置不覆盖
- 缓存：`<prefix>/cache/`
- 管理脚本：`<prefix>/iptv-proxy.sh`
- 命令软链：`/usr/local/bin/iptv-proxy`；可用 `--no-link` 跳过
- 服务单元：systemd 或 OpenRC；按系统自动选择

示例：

```bash
# 自定义目录并立即启动
sudo ./install.sh --prefix /srv/iptv --start

# 容器或普通用户：只装文件，不注册服务
./install.sh --prefix "$HOME/iptv" --no-service
```

## 命令行管理

安装后可用 `iptv-proxy` 管理服务。命令由 `<prefix>/iptv-proxy.sh` 提供。

| 命令 | 作用 |
| --- | --- |
| `iptv-proxy` / `iptv-proxy menu` | 打开交互菜单 |
| `iptv-proxy start` | 启动服务 |
| `iptv-proxy stop` | 停止服务 |
| `iptv-proxy restart` | 重启服务 |
| `iptv-proxy status` | 查看状态 |
| `iptv-proxy log [行数]` | 查看日志；终端下持续跟随 |
| `iptv-proxy url` | 打印面板地址与账号 |
| `iptv-proxy port <N>` | 修改面板端口并重启 |
| `iptv-proxy pass [密码] [账号]` | 修改密码，可同时改账号；无参数进入交互 |
| `iptv-proxy update` | 更换二进制，保留配置 |
| `iptv-proxy reinstall` | 备份配置后重装 |
| `iptv-proxy install [参数]` | 在发布包目录调用 `install.sh` 安装或修复 |
| `iptv-proxy uninstall` | 移除服务；询问是否删除数据 |

无参数或 `menu` 打开交互菜单。菜单含更新、面板地址、启停、状态、日志、改端口、改账号密码、重装与卸载。

`port` 与 `pass` 直接改 `config.toml`，随后重启服务。旧登录会话失效。

`update` 需要更新源。按优先级读取：

```bash
# 本地二进制
IPTV_PROXY_BIN=/path/to/iptv-proxy iptv-proxy update

# 本地 tar.gz
IPTV_PROXY_ARCHIVE=/path/to/iptv-proxy.tar.gz iptv-proxy update

# 远程 tar.gz 或二进制；{arch} 替换为 amd64/arm64
IPTV_PROXY_URL=https://example.com/iptv-proxy-{arch}.tar.gz iptv-proxy update
```

未设置更新源时，脚本检查 `<prefix>/dist` 与 `<prefix>/../dist` 下的最新同架构包。

`uninstall` 选择 `N` 只停服务并保留 `<prefix>`。配置与缓存不会删除。
无人值守时设 `IPTV_PROXY_YES=1`，直接重装或删除数据。

示例：

```bash
sudo iptv-proxy                 # 打开菜单
sudo iptv-proxy port 8080
sudo iptv-proxy pass 'new-password' admin
sudo iptv-proxy log 200
```

## 翻译服务（MTranServer）

支持 MTranServer v5 与旧批量接口。客户端优先请求 `/translate/batch`，失败时回退 `/translate`。地址可写根地址、完整 `/translate` 或 `/translate/batch`。

字幕翻译：

```toml
[subtitle.translate]
enabled = true
url = "https://translate.ipacel.cc/translate"
target = "zh-cn"
source = "auto"
# token = "可选；以 Authorization: Bearer 发送"
```

EPG 翻译：

```toml
[epg.translate]
enabled = true
engine = "mtran"
mtran_url = "https://translate.ipacel.cc"
target = "zh-cn"
mtran_batch = 50
# mtran_token = "可选"
```

`engine = "mtran"` 时才读取 `mtran_url`。也可以直接写 `[epg.translate].url`。

`https://translate.ipacel.cc/translate` 是社区试用服务。可用性、速度和额度不保证。生产环境建议自建 MTranServer。

联调：

```bash
scripts/smoke-translate-live.sh

# 指定其他服务
TRANSLATE_URL=http://127.0.0.1:8989/translate scripts/smoke-translate-live.sh

# 需要 Token 时
TRANSLATE_URL=http://127.0.0.1:8989 TRANSLATE_TOKEN=secret scripts/smoke-translate-live.sh
```

脚本检查 `/subtitle/mtran/test`、EPG 采集与缓存、EPG 探测和字幕翻译。

## 测试

```bash
scripts/smoke-m0.sh   # 健康检查、登录、状态、配置读写、鉴权、兜底
scripts/smoke-m1.sh   # 分组/频道 CRUD、导出、订阅与 access 过滤、播放 Token
scripts/smoke-m2.sh   # 播放链路、切片伪装、缓存窗口、302 占位（含伪上游）
scripts/smoke-m3.sh   # 直连模式：redirect/rewrite/passthrough、remux/dash 就绪语义
scripts/smoke-m4.sh   # 热重载、看门狗冷却、缓存状态与清空、指标与日志
scripts/smoke-cli.sh  # CLI：端口、密码、启停、更新、重装、卸载保留数据
scripts/smoke-translate-live.sh # 真实 MTranServer 联调（需外网）
scripts/smoke-epg.sh  # EPG：采集、/epg.xml[.gz]、/epg 页面与 /epg/api/*、状态与刷新
scripts/smoke-logo.sh # 台标库：上传、公开取图与列表、删除
scripts/smoke-backup.sh # 备份与恢复：导出 gzip、改动后回滚、拒绝非法内容
scripts/smoke-security.sh # 安全面：Token 查看/轮换、IP 统计、封禁与解封
scripts/smoke-aggregate.sh # 订阅聚合：远端源合并、过滤与位置
scripts/smoke-assets.sh # 素材：ffmpeg 状态、视频/字幕上传与删除、解析脚本
scripts/smoke-share.sh  # 多 VPS：门禁、Token 同步、封禁汇总
scripts/smoke-panel.sh  # 面板：健康摘要、DASH 探测、回看状态
scripts/smoke-resolver.sh # 解析脚本：URL 覆盖、TTL 缓存、失败转 error_url
scripts/smoke-remux.sh    # Remux：预热、清单、伪装切片（需 ffmpeg 生成测试流）
scripts/smoke-schedule.sh # 录制任务 CRUD、直播开关
scripts/smoke-timeshift.sh # 回看：VOD 清单、伪装切片（需 ffmpeg）
scripts/smoke-record.sh   # 录制：调度到期、合并切片、下载产物（需 ffmpeg）
scripts/smoke-record-dash.sh # DASH 录制：动态 MPD、轨道并轨、下载产物（需 ffmpeg）
scripts/smoke-dash-timeshift.sh # DASH 回看：ffmpeg 转 HLS 后缓冲（缺 DASH 解复用器时跳过）
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
| `GET` | `/panel/api/health` | 本机健康摘要 |
| `GET` | `/panel/api/dash/tracks/:gid/:cid` | DASH 轨道列表 |
| `POST` | `/panel/api/dash/scan-url` | 解析任意 MPD |
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
frontend/          Vite + React + TS 面板（12 个管理分区 + 独立台标/解封页）
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
- 安装：解包后 `sudo ./install.sh`，默认装到 `/opt/iptv-proxy`。详见上文「一键安装」。

## 运维

- 配置热重载：改 `config.toml` 即生效（`[epg]`、`[cache].disk_path` 需重启，与目标一致）。
- 日志：标准输出与 `<配置目录>/iptv-proxy.log`；超过 5 MB 截断。`[server].log_level` 控制级别。
- 看门狗：同一频道连续失败 5 次后冷却 3600 秒；改配置立即解除。

## 已知限制

- `remux` 为纯 Rust 切片；冷启动在首批切片产出前返回 `503 remux warming up`，不是恒 503。
- `dash` 已把 MPD 转成 HLS；需要 `SegmentTimeline`，或 `SegmentTemplate@duration` 加总时长。CENC/ClearKey 不做解密，当前按原字节代理。
- `proxy` 模式面向 HLS：媒体清单按行改写；MPD 的相对 `<BaseURL>` 改写到本服务并代理 init/切片，绝对地址仍直连上游。
- `mpd_keep_seconds` 已用于 DASH 媒体清单的时间窗；`url_pool_size` 已用于多个 `<BaseURL>` 的音视频分流与故障回退。目标侧正常取用方式仍未观测。
- 缓存无容量上限（与目标一致，见 F-015）；`cache.backend=memory` 只回显，实际仍落盘。
- 运维接口的响应形状为本实现自定；目标只登记存在性（见 `design/03` §6.4）。
- 面板有 12 个管理分区，另有独立 `/logo` 与 `/unban` 页面；目标前端代码与文案不复制。
- EPG：支持 `kind = "xml"` 与 `kind = "script"`、EPG 翻译；EPG 手工映射、订阅 `x-tvg-url` 与录制联动未实现（`design/03` §6.5）。
- 定时录制：HLS 频道按切片合并；DASH 频道要求直播 MPD 且系统有 ffmpeg（支持 `-c copy`），产出 MKV。静态 MPD 报「录制区间内无可用切片」。
- `l1/l2` 限流、每 IP 会话上限、`random_id_enabled` 的随机 ID 行为、`compat_mode`、`srt_ua_keywords`、`socks5` 未实现或仅保留配置键。
- 与目标的差异以 `docs/design/reconciliation-report.md` 为准（22 项用例差异 0）。
