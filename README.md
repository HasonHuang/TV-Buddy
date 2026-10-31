# TV Buddy

> 自托管的 IPTV 代理服务。带网页管理面板，单二进制部署，静态 musl 编译。

[![Release](https://img.shields.io/github/v/release/HasonHuang/TV-Buddy?logo=github)](https://github.com/HasonHuang/TV-Buddy/releases/latest)
[![CI](https://github.com/HasonHuang/TV-Buddy/actions/workflows/ci.yml/badge.svg)](https://github.com/HasonHuang/TV-Buddy/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-linux--amd64%20%7C%20linux--arm64-lightgrey.svg)](#支持平台)
[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org/)

TV Buddy 把上游 IPTV 直播源转成你自己的订阅地址。它改写、缓存、再分发 HLS/DASH 流，并提供工程单、录制、回看、台标库、安全管控与备份恢复。

服务只用一个静态链接的二进制运行。前端资源内嵌在二进制中，不需要 Node 或数据库。默认监听 `19899` 端口。

- **行为兼容**：配置键面、面板接口与播放链路对齐闭源 IPTV Proxy v0.8.2。可直接替换运行。
- **低配置可跑**：1 vCPU / 128 MB 内存 / 1 GB 磁盘即可运行，配合前置 CDN 承载多观众。见 [低配置部署](#低配置部署)。

---

## 目录

- [功能](#功能)
- [支持平台](#支持平台)
- [快速开始](#快速开始)
- [首次使用](#首次使用)
- [配置](#配置)
- [低配置部署](#低配置部署)
- [命令行管理](#命令行管理)
- [从源码构建](#从源码构建)
- [测试](#测试)
- [接口](#接口)
- [文档](#文档)
- [已知限制](#已知限制)
- [贡献](#贡献)
- [免责声明](#免责声明)
- [许可证](#许可证)

---

## 功能

### 直播分发

| 能力 | 说明 |
| --- | --- |
| 六种分组模式 | `proxy`、`redirect`、`rewrite`、`passthrough`、`remux`、`dash` |
| 协议 | HLS（`.m3u8`）、DASH（`.mpd`）、MPEG-TS |
| 切片缓存 | 磁盘与内存索引可限，按频道设上限 |
| 切片伪装 | 切片路径改名，不暴露上游地址 |
| 订阅聚合 | 合并多个远端订阅源，支持过滤与位置 |
| 预取 | 开播前预取首批切片，减少首帧等待 |
| CDN 兜底 | 响应头加缓存策略，空响应不落 CDN |

### 节目单（EPG）

- 采集：XML 源与脚本源。
- 匹配：按频道名自动匹配，可手工映射。
- 翻译：接 MTranServer，批量翻译标题与描述。
- 输出：`/epg.xml`、`/epg.xml.gz` 与公开查节目页。
- 订阅注入：开启后自动写入 `x-tvg-url`。

### 录制与回看

- 定时录制：按时间窗自动录制，产出入库。
- HLS 与 DASH：DASH 录制用 ffmpeg `-c copy` 并轨，产出 MKV。
- 本地回看：把录制产物转成可播放的清单。
- 时移：按 `playseek` 区间回放已录制的切片。

### 面板与运维

- 12 个管理分区，另有独立台标库与解封页面。
- 台标库：上传、重命名、按名称相似度推荐、批量应用。
- 素材：视频与字幕上传，ffmpeg 状态检测。
- 安全：播放 Token 查看与轮换、IP 统计、封禁解封。
- 备份：一键导出 gzip(tar)，一键回滚。
- 多 VPS：通过分享密钥同步 Token 与封禁表。
- 观测：运行指标、运行日志、缓存占用。
- 交互式 CLI：启停、改端口、改账号密码、更新、重装、卸载。

---

## 支持平台

| 平台 | 架构 | 产物 |
| --- | --- | --- |
| Linux | amd64（x86_64） | 静态 musl 二进制 |
| Linux | arm64（aarch64） | 静态 musl 二进制 |

运行服务只需一个静态二进制。以下能力为可选，用到时才需要：

| 依赖 | 用于 |
| --- | --- |
| `ffmpeg` | DASH 录制、素材转换 |
| systemd 或 OpenRC | 开机自启（安装脚本自动选择） |

> macOS 与 Windows 不在支持范围。开发机上可以跑单元测试与契约测试。

---

## 快速开始

### 一键安装（推荐）

```bash
curl -fsSL https://raw.githubusercontent.com/HasonHuang/TV-Buddy/main/install.sh | sh
```

脚本按如下步骤工作：

1. 读取 `uname -m`，选 `amd64` 或 `arm64`。
2. 从最新 Release 下载 `tv-buddy-<version>-<arch>.tar.gz`。
3. 校验同名 `.sha256` 文件。
4. 解包到 `/opt/tv-buddy`。
5. 注册 systemd 或 OpenRC 服务。
6. 启动服务。

装完就能打开面板。加 `--no-start` 可只装不启动。

指定版本或仓库：

```bash
sudo sh install.sh --version v2026.0.0
sudo sh install.sh --version v2026.0.0 --repo HasonHuang/TV-Buddy
```

> 预发布 tag（带 `-beta`、`-rc` 等后缀）不会被 GitHub 的 `/releases/latest` 选中。仓库里只有预发布版本时，脚本自动退回最新一条 Release，并在下载前打印提示。

### 手动安装

```bash
# 1) 下载并解包
tar -xzf tv-buddy-<version>-arm64.tar.gz
cd tv-buddy-<version>-arm64

# 2) 安装（包内自带 install.sh，不再下载）
sudo ./install.sh
```

### 更多安装方式

| 场景 | 命令 |
| --- | --- |
| 从仓库本地构建包安装 | `sudo ./install.sh --local` |
| 指定归档 | `sudo ./install.sh --archive dist/tv-buddy-<version>-arm64.tar.gz` |
| 指定 URL | `sudo ./install.sh --archive https://example.com/pkg.tar.gz` |
| 只装文件，不注册服务 | `./install.sh --prefix "$HOME/tv-buddy" --no-service` |
| 自定义目录并启动 | `sudo ./install.sh --prefix /srv/tv-buddy` |
| 只装不启动 | `sudo ./install.sh --no-start` |

`install.sh` 选项：

| 选项 | 说明 |
| --- | --- |
| `--prefix <目录>` | 安装目录。默认 `/opt/tv-buddy` |
| `--archive <路径或 URL>` | 指定 tar.gz 安装包 |
| `--version <tag>` | 从 Release 装指定版本。默认最新 |
| `--repo <owner/name>` | Release 所在仓库。默认内置 |
| `--local` | 不从 Release 下载，改用本地 `dist/` 中的包 |
| `--no-service` | 不注册 systemd/OpenRC 服务 |
| `--no-link` | 不创建命令软链 |
| `--no-start` | 安装后不启动服务 |
| `--start` | 安装后启动服务（默认行为，保留以兼容旧用法） |
| `--low-spec` | 写入低配参数：省内存、省 CPU |
| `-h`、`--help` | 显示帮助 |

### 安装内容

```text
<prefix>/
  tv-buddy            # 服务二进制
  tv-buddy.db         # 配置 + 用户 + 令牌（首次启动生成，权限 600）
  cache/              # 切片与录制缓存
  tv-buddy.sh         # 管理脚本
/usr/local/bin/tv-buddy     # 命令软链，可用 --no-link 跳过
```

服务单元按系统自动选择 systemd 或 OpenRC。

---

## 首次使用

安装完成后执行：

```bash
sudo tv-buddy url     # 打印面板地址与账号
sudo tv-buddy start   # 启动服务（未加 --start 时）
```

打开面板：

```text
http://<host>:19899/panel
```

首次启动生成账号 `admin`，口令随机，写在 `<cache>/admin_initial_credentials.txt`（权限 600）。
也可以用环境变量 `TV_BUDDY_INITIAL_PASSWORD` 指定。

> **先改口令。** 执行 `sudo tv-buddy pass '新口令' admin`。改后旧会话立即失效。

接下来在面板里加分组与频道，配置只在面板里改。

订阅地址形如：

```text
http://<host>:19899/playlist.m3u          # 全部公开分组
http://<host>:19899/<gid>/playlist.m3u   # 单个分组
```

---

## 配置

配置存在 `<prefix>/tv-buddy.db`。面板「系统设置」是唯一入口。
命令行也能改：`tv-buddy config get <键> `、`tv-buddy config set <键> <值>`。

旧版本的 `config.toml` 用一次导入即可：

```bash
sudo tv-buddy import-config <prefix>/config.toml --db <prefix>/tv-buddy.db
```

下面的 TOML 片段是**键名参考**，也是 `import-config` 接受的格式。

```toml
[server]
bind = "[::]:19899"
log_level = "info"

[auth]
token_enabled = false

[cache]
disk_path = "./cache"

[[groups]]
id = "demo"
name = "示例分组"
mode = "proxy"
access = "public"

[[groups.channels]]
id = "c1"
name = "示例频道"
url = "http://example.com/live.m3u8"
```

常用可选项：

| 段 | 键 | 说明 |
| --- | --- | --- |
| `[cache.segments]` | `per_channel_max` | 每频道切片上限 |
| `[cache.prefetch]` | `enabled`、`concurrency` | 预取开关与并发 |
| `[cache.gc]` | `max_disk_mb` | 磁盘回收上限。`0` 按可用空间自动 |
| `[cdn]` | `enabled`、`auto_tune` | CDN 兜底与推荐值实测 |
| `[subtitle.translate]` | `enabled`、`url` | 字幕机翻 |
| `[epg.translate]` | `enabled`、`mtran_url` | 节目单翻译 |
| `[[groups]]` | `mode`、`format`、`mpd_keep_seconds` | 分组行为 |

生效方式：面板保存立即生效；`tv-buddy config set` 之后调用 `POST /panel/api/reload` 或重启。
`[epg]` 与 `[cache].disk_path` 需要重启。

### 翻译服务

内置客户端支持 MTranServer v5 与旧批量接口。先请求 `/translate/batch`，失败回退 `/translate`。地址可写根地址、`/translate` 或 `/translate/batch`。

```toml
[subtitle.translate]
enabled = true
url = "https://translate.ipacel.cc/translate"
target = "zh-cn"
source = "auto"
# token = "可选。以 Authorization: Bearer 发送"
```

`https://translate.ipacel.cc/translate` 是社区试用服务。可用性、速度与额度不保证。生产环境建议自建 MTranServer。

联调：

```bash
scripts/smoke-translate-live.sh

# 指定其他服务
TRANSLATE_URL=http://127.0.0.1:8989/translate scripts/smoke-translate-live.sh

# 需要 Token 时
TRANSLATE_URL=http://127.0.0.1:8989 TRANSLATE_TOKEN=secret scripts/smoke-translate-live.sh
```

---

## 低配置部署

TV Buddy 可在 **1 vCPU / 128 MB 内存 / 1 GB 磁盘** 的实例上运行。前提是前面挂 CDN，把播放流量挡在边缘。

装的时候加 `--low-spec`：`sudo ./install.sh --low-spec`。
样例：[`docs/design/config-low-spec.toml`](docs/design/config-low-spec.toml)（可用 `import-config` 导入）。原理与公式：[`docs/design/06-low-spec-optimization.md`](docs/design/06-low-spec-optimization.md)。

### 两个硬前提

缺一条就会在 1 GB 磁盘上出问题。

| # | 前提 | 说明 |
| --- | --- | --- |
| 1 | `[cache.gc].enabled = true` | 切片正文只写不删，全靠它回收。关掉后磁盘会被吃满 |
| 2 | CDN 侧强制 TTL + 忽略源站头 + 忽略 Cookie | 四家 CDN 的规则内容相同。见下文 |

### 服务端配置

```toml
[server]
bind = "[::]:19899"
log_level = "info"

[panel]
enabled = true
# 上线前必须改口令。password_sha256 是口令的 SHA-256 十六进制。

[auth]
token_enabled = true
ip_guard_enabled = true

[cache]
disk_path = "./cache"

# 每个频道的切片索引压到 200 条。0 = 只用内置默认 4096
[cache.segments]
per_channel_max = 200

# 1 vCPU：降低开播瞬间的 CPU 抢占
[cache.prefetch]
enabled = true
concurrency = 2

# 磁盘回收。上限自动，按「可用空间 × 0.8 − 100 MB」与实测工作集取小
[cache.gc]
enabled = true
max_disk_mb = 0          # 0 = 自动。1 GB 盘通常落在 100–150 MB
max_age_secs = 1800
interval_secs = 300
scan_limit = 500         # 单轮最多处理 500 个文件，防抖动
disk_budget_ratio = 0.8
reserve_mb = 100
min_mb = 50

# CDN 兜底。CDN 能忽略源站头时保持默认即可
[cdn]
enabled = true
auto_tune = true           # 被动实测推荐值
segment_max_age_secs = 0   # 0 = 用实测推荐值
empty_status = 502         # 强制 TTL 下挡住「200 + 空体」
# public_base = "https://cdn.example"   # CDN 不下发 XFP 时填这里
```

### CDN 控制台设置

五个设置项，四个域名各做一遍。面板支持「应用到全部域名」时做一次。

| # | 设置项 | 设成 | 原因 |
| --- | --- | --- | --- |
| 1 | 缓存过期时间 | 媒体路径用推荐 TTL | 覆盖窗口加两次重试余量 |
| 2 | 是否遵循源站缓存头 | 关闭（强制 TTL） | 各家能力不一致，强制 TTL 最容易移植 |
| 3 | Set-Cookie | 媒体路径关闭注入 | 每个响应带新 Cookie 时 CDN 跳过缓存 |
| 4 | 图片优化 / 自动 WebP / 压缩改写 | 全部关闭 | 切片是 TS 字节，转码后会「命中但播不动」 |
| 5 | 回源请求头 | 加 `X-Forwarded-Proto: https` | 否则清单一直给 `http://` |

另外确认：**不缓存 3xx**（`redirect` 模式会 302 到带时效的上游地址）、**不缓存非 200**。

缓存规则用两条，顺序不能颠倒：

```text
规则 A：不缓存（放最前）
  /panel*  /epg*  /subs/*  /api/*  /unban*  /share*  /health  /metrics
  /playlist.m3u  /*/playlist.m3u  /*/*.m3u8  /*/*.mpd
  /*/*/index.m3u8  /*/*/index.txt  /*/*/video/*  /*/*/mpd/*

规则 B：缓存（放其后）
  /*/*/* 中的其余路径（伪装切片、remux seg_*、timeshift/*、DASH 轨道切片）
```

> **不要按扩展名写规则。** 伪装扩展名在 jpg/png/gif/bmp/webp 之间轮换。控制台只支持扩展名时，改用路径前缀 `/{gid}/{cid}/`，逐分组配置。

### 推荐值从哪来

TTL 与磁盘上限不能写死。片源窗口在 48–100 秒，单片 1.1–9 MB，固定值要么提前过期，要么长期多占边缘存储。

服务默认被动实测，不向源站额外发请求。首次安装还没有播放数据时用回退值：**TTL 600s / 磁盘 150 MB**。

拿数字的两个入口：

| 入口 | 位置 |
| --- | --- |
| 推荐 TTL（只读，带复制按钮） | 面板 → 缓存状态页顶部 |
| 「立即估算」 | 面板 → 系统设置页 →「CDN 缓存」卡片 |

「立即估算」探测最多 3 个分组、每组 1 个频道，只跑一次。并发请求返回 409。

也可以直接调接口：

```bash
curl -X POST http://127.0.0.1:19899/panel/api/cdn/estimate
```

### 验收

```bash
# 1) 清单必须不缓存
curl -s -o /dev/null -D - "https://<域名>/<分组>/<变体>.m3u8" | grep -iE 'x-cache|cache-control'

# 2) 切片必须缓存。第二次应为 HIT 且带 Age
SEG="https://<域名>/<分组>/<变体>/<从清单复制的切片路径>"
for i in 1 2; do curl -s -o /dev/null -D - "$SEG" | grep -iE '^(x-cache|age|cache-control|content-length)'; done

# 3) 字节未被改写
curl -s "$SEG" | shasum -a 256

# 4) 面板与 API 不被缓存
curl -s -o /dev/null -D - "https://<域名>/panel/api/status" | grep -iE 'x-cache|cache-control'
```

期望：清单与面板返回 `BYPASS`。切片第二次返回 `HIT` 并带 `Age`。两次哈希一致。

### 排障

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 切片仍 `BYPASS` | 规则未生效、未刷新缓存、Cookie 未关 | 查设置项 1 与 3 |
| `MISS` 不转 `HIT` | 缓存键含 Cookie 或请求头，或 TTL 太短 | 缓存键去掉 Cookie，TTL 用推荐值 |
| `HIT` 但花屏或卡住 | 图片优化改写了字节 | 关闭图片处理，比对哈希 |
| 清单被缓存 | 规则顺序颠倒 | 把规则 A 调到最前 |
| 换台 404 | 302 被缓存 | 关闭 3xx 缓存 |
| 清单仍给 `http://` | 未下发 XFP | 加回源请求头，或配 `[cdn].public_base` |

---

## 命令行管理

安装后可用 `tv-buddy` 管理服务。命令由 `<prefix>/tv-buddy.sh` 提供。

```bash
sudo tv-buddy            # 交互菜单
sudo tv-buddy status
sudo tv-buddy port 8080
sudo tv-buddy pass 'new-password' admin
sudo tv-buddy log 200
```

| 命令 | 作用 |
| --- | --- |
| `tv-buddy` / `menu` | 打开交互菜单 |
| `start` / `stop` / `restart` | 启停服务 |
| `status` | 查看状态 |
| `log [行数]` | 查看日志。终端下持续跟随 |
| `url` | 打印面板地址与账号 |
| `port <N>` | 修改面板端口并重启 |
| `pass [密码] [账号]` | 修改口令，可同时改账号。无参数进入交互 |
| `update` | 更换二进制，保留数据库 |
| `reinstall` | 备份数据库、重置配置后重装（用户保留） |
| `install [参数]` | 在发布包目录调用 `install.sh` |
| `uninstall` | 移除服务。询问是否删除数据 |

`update` 的更新源按优先级读取：

```bash
TV_BUDDY_BIN=/path/to/tv-buddy tv-buddy update
TV_BUDDY_ARCHIVE=/path/to/pkg.tar.gz tv-buddy update
TV_BUDDY_URL=https://example.com/tv-buddy-{arch}.tar.gz tv-buddy update
```

三项都没有时，先查 `<prefix>/dist` 与 `<prefix>/../dist`，再回退到 GitHub Release：

```bash
sudo tv-buddy update                                 # 最新 Release
sudo TV_BUDDY_VERSION=v2026.0.0 tv-buddy update    # 固定版本
```

下载的 Release 包会校验同名 `.sha256`。校验不通过时不替换二进制。

无人值守安装设 `TV_BUDDY_YES=1`，脚本直接重装或删除数据，不再询问。

---

## 从源码构建

### 环境要求

| 工具 | 版本 |
| --- | --- |
| Rust | stable（见 `rust-toolchain.toml`） |
| Node.js | 18 或更高（只构建前端时需要） |
| Docker | 只交叉编译发布包时需要 |

### 本地构建

```bash
# 1) 前端（Vite + React + TS → frontend/dist）
scripts/build-frontend.sh

# 2) 后端（Rust）
cargo build --release

# 3) 运行
target/release/tv-buddy ./tv-buddy.db
```

### 交叉编译静态二进制

```bash
rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

### 构建发布包

```bash
scripts/build-release.sh            # amd64 与 arm64 两个静态 musl 包
scripts/verify-release.sh arm64     # 校验：静态链接 + 启动 + /health
```

产物在 `dist/tv-buddy-<version>-<arch>.tar.gz`，含二进制、启停脚本、安装脚本与服务单元。CI 产出的资产名与本地一致。

构建用 Docker `rust:1-alpine`。镜像宿主三元组即 `*-linux-musl`，直接产出静态二进制。容器无外网，依赖来自宿主 `~/.cargo` 缓存。

### 发布

GitHub Actions 只在推送 `v*` tag 时执行。流水线三阶段：前端构建 → 格式/静态检查/测试 → 交叉编译。通过后自动创建 Release 并上传 `tv-buddy-<version>-<arch>.tar.gz` 与 `.sha256`。

```bash
git tag v2026.4.0
git push origin v2026.4.0
```

---

## 测试

```bash
cargo test                 # 单元测试与契约测试
cargo clippy -- -D warnings
cargo fmt --check
```

端到端冒烟脚本按功能拆分，每个脚本自起真实服务与伪上游：

```bash
scripts/smoke-m0.sh   # 健康检查、登录、状态、配置读写、鉴权、兜底
scripts/smoke-m1.sh   # 分组/频道 CRUD、导出、订阅与 access 过滤、播放 Token
scripts/smoke-m2.sh   # 播放链路、切片伪装、缓存窗口、302 占位
scripts/smoke-m3.sh   # 直连模式：redirect/rewrite/passthrough、remux/dash 就绪语义
scripts/smoke-m4.sh   # 热重载、看门狗冷却、缓存状态与清空、指标与日志
scripts/smoke-cli.sh  # CLI：端口、密码、启停、更新、重装、卸载保留数据
```

其余脚本覆盖 EPG、台标、备份、安全、订阅聚合、素材、分享、面板、解析脚本、Remux、录制、回看、DASH 与翻译。完整列表：

```bash
ls scripts/smoke-*.sh
```

部分脚本需要 `ffmpeg`（Remux、录制、回看）或外网（真实翻译服务）。

`scripts/compare-p0.sh` 在相同夹具下与本实现对比目标 v0.8.2，需要容器 `iptv82`。

---

## 接口

全部路由见 [`docs/design/03-http-contract.md`](docs/design/03-http-contract.md)。常用入口：

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/health` | 健康检查，返回 `ok` |
| `GET` | `/playlist.m3u` | 总订阅，只含 `access = "public"` 的组 |
| `GET` | `/{gid}/playlist.m3u` | 分组订阅 |
| `GET` | `/{gid}/{cid}.m3u8`（或 `.mpd`） | 入口清单 |
| `GET` | `/{gid}/{cid}/video/index.txt` | 媒体清单 |
| `GET` | `/epg.xml[.gz]` | 节目单 |
| `POST` | `/panel/api/login` | 面板登录 |
| `GET` | `/panel/api/health` | 本机健康摘要 |

<details>
<summary>展开全部面板与播放接口</summary>

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/{gid}/{cid}/<P>_<S>.<ext>` | 伪装切片（忽略 `Range`） |
| `GET`/`PUT` | `/panel/api/config` | 读取 / 全量保存配置 |
| `GET` | `/panel/api/groups/export` | 导出全部分组 |
| `GET` | `/panel/api/groups/{gid}/export` | 导出单个分组 |
| `POST` | `/panel/api/groups` | 新建分组 |
| `PUT`/`DELETE` | `/panel/api/groups/{gid}` | 局部更新 / 删除分组 |
| `POST` | `/panel/api/groups/{gid}/channels` | 新建频道 |
| `PUT`/`DELETE` | `/panel/api/groups/{gid}/channels/{cid}` | 局部更新 / 删除频道 |
| `POST` | `/panel/api/groups/import[/m3u]` | 批量导入分组（JSON / M3U） |
| `POST` | `/panel/api/reload` | 立即重读配置 |
| `GET` | `/panel/api/cache/stats` | 缓存条目与占用 |
| `POST` | `/panel/api/cache/flush` | 清空缓存（保留 `dash/`） |
| `POST` | `/panel/api/cdn/estimate` | 立即估算推荐 TTL 与磁盘上限 |
| `GET` | `/panel/api/stats` | 运行指标 |
| `GET` | `/panel/api/logs` | 最近运行日志 |
| `GET` | `/panel/api/security/token` | 当前播放 Token 与宽限旧值 |
| `POST` | `/panel/api/security/token/rotate` | 轮换 Token |
| `GET` | `/panel/api/security/ip/stats` | IP 请求统计与封禁状态 |
| `POST` | `/panel/api/security/ip/{ban,unban}` | 封禁 / 解封地址 |
| `GET` | `/panel/api/backup?include=` | 导出 gzip(tar) 备份 |
| `POST` | `/panel/api/restore` | 恢复备份并重载 |
| `GET` | `/logo/<file>` | 台标图片 |
| `GET` | `/logo/api/list` | 台标列表 |
| `POST` | `/panel/api/logo/{upload,delete,rename,apply}` | 台标管理 |
| `GET` | `/panel/api/logo/match-report` | 缺台标、坏引用与未使用 |
| `POST` | `/panel/api/logo/suggest` | 按名称相似度推荐台标 |
| `GET` | `/panel/api/assets/list` | 视频素材列表 |
| `POST` | `/panel/api/assets/{upload,remove}` | 上传 / 删除视频素材 |
| `GET` | `/panel/api/assets/ffmpeg-status` | ffmpeg 检测 |
| `GET`/`POST` | `/panel/api/subtitle/*` | 字幕列表、上传、删除、下载 |
| `GET`/`POST` | `/panel/api/resolver/{scripts,script,remove}` | 解析脚本管理 |
| `GET` | `/api/share/pull?key=` | 多 VPS 同步载荷 |
| `GET` | `/panel/api/dash/tracks/:gid/:cid` | DASH 轨道列表 |
| `POST` | `/panel/api/dash/scan-url` | 解析任意 MPD |
| `GET` | `/epg/api/{channels,programmes,search}` | 节目单 JSON |
| `GET` | `/panel/api/epg/{status,mappings,match-report}` | EPG 状态、映射与匹配报告 |
| `POST` | `/panel/api/epg/{refresh,mapping}` | 立即采集、设置或清除映射 |

</details>

---

## 目录结构

```text
src/
  main.rs          启动入口。只做参数、日志、状态、监听
  lib.rs           库根：模块声明
  state.rs         共享状态（ArcSwap 配置快照 + 改动串行化 + 缓存目录）
  config.rs        配置入口；config/store.rs 为 SQLite 存储；config/model.rs 为键面模型
  playlist.rs      M3U 订阅渲染
  proxy.rs         清单改写、切片索引与字节缓存；proxy/{media,modes,segments}.rs
  auth.rs          面板会话、播放 Token；auth/{play_token,session}.rs
  http.rs          路由、提取器、错误映射、内嵌静态资源
frontend/          Vite + React + TS 面板（12 个管理分区 + 独立台标/解封页）
scripts/           构建、发布与冒烟测试脚本
packaging/         服务单元与管理脚本
docs/              规格、逆向发现、设计与对拍报告
tests/             路由级集成测试（起真实服务 + 伪上游）
```

---

## 文档

| 文档 | 内容 |
| --- | --- |
| [`docs/spec/`](docs/spec/) | 目标规格（`S-*`）与验证基线 |
| [`docs/findings/`](docs/findings/) | 逆向证据（`F-*`、`D-*`） |
| [`docs/design/`](docs/design/) | 实现方案：选型、架构、HTTP 契约、低配优化 |
| [`docs/design/03-http-contract.md`](docs/design/03-http-contract.md) | 路由表与接口契约 |
| [`docs/design/reconciliation-report.md`](docs/design/reconciliation-report.md) | 与目标 v0.8.2 的 P0 对拍报告 |
| [`PROGRESS.md`](PROGRESS.md) | 进度与缺口。唯一进度权威 |

---

## 已知限制

- **平台**：只支持 Linux amd64 与 arm64。
- **Remux**：纯 Rust 切片。冷启动时在首批切片产出前返回 `503 remux warming up`，不是恒 503。
- **DASH**：已把 MPD 转成 HLS。需要 `SegmentTimeline`，或 `SegmentTemplate@duration` 加总时长。CENC/ClearKey 不解密，按原字节代理。
- **缓存**：无容量上限校验。`cache.backend=memory` 只回显，实际仍落盘。
- **EPG**：录制前后缓冲与部分私有端点未实现。
- **录制**：DASH 频道要求直播 MPD 且系统有 ffmpeg。静态 MPD 报「录制区间内无可用切片」。
- **未实现**：`l1`/`l2` 限流、每 IP 会话上限、`random_id_enabled` 的随机 ID 行为、`compat_mode`、`srt_ua_keywords`、`socks5`。部分只保留配置键。
- **兼容差异**：以 [`docs/design/reconciliation-report.md`](docs/design/reconciliation-report.md) 为准。22 项对拍用例差异为 0。

---

## 贡献

欢迎提 issue 与 pull request。

提交前请确认：

1. `cargo fmt --check` 与 `cargo clippy -- -D warnings` 通过。
2. `cargo test` 通过。
3. 改动涉及播放链路时，跑对应 `scripts/smoke-*.sh`。
4. 改行为时同步更新 `docs/design/` 与 `PROGRESS.md`。

写作约定：简体中文，按 ASD-STE100 简化技术写作。短句、主动语态、一般现在时、一句一义。

---

## 免责声明

本项目只用于学习、研究与个人合法的自托管场景。

使用者必须自行确认：所接入的直播源已获授权，使用方式符合当地法律与内容服务商的条款。作者不对任何滥用行为负责。

目标程序派生的内容按不可信数据处理。本项目不复制目标的源码、前端资源与面板文案。

---

## 许可证

[MIT](LICENSE) © 2026 Hason
