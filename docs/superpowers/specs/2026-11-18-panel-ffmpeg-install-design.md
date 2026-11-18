# 面板安装 ffmpeg：设计

日期：2026-11-18

## 目的

现在 ffmpeg 只能由用户登录服务器安装。Docker 容器的进程是非 root（UID 10001），运行期不能装包。用户要在面板里看到「未安装」，却不能处理。

本特性让管理员在 Web 面板安装、删除与重新安装 ffmpeg。安装的二进制放在数据目录，不需要 root。

## 目标

- 管理员在面板安装 ffmpeg。支持 Linux amd64、Linux arm64、macOS amd64、macOS arm64。
- 安装后，服务优先用面板安装的版本。没有时回退系统 `PATH`。
- 状态接口报告当前来源、版本、路径与安装进度。
- 删除只删面板安装的版本，不碰系统 ffmpeg。

## 非目标

- 不调用系统包管理器（apt/apk/dnf/brew）。
- 不支持 Windows。
- 不安装 ffprobe。ffprobe 继续走系统 `PATH`。
- 不自动跟随上游升级。升级 ffmpeg 版本需要随 TV Buddy 发版更新固定版本与哈希。
- 不支持在面板上传本地二进制。
- 不做安装包的断点续传。

## 反目标

- 不能覆盖或删除系统安装的 ffmpeg。
- 不能在哈希校验失败时启用下载结果。
- 不能把用户输入拼进 shell 命令。
- 不能在 HTTP 请求内同步下载几十 MB 文件。
- 不能把二进制放进缓存目录，避免缓存回收误删。

## 约束

- Docker 运行时是非 root，只有 `/data` 可写。
- systemd 单元有 `ProtectSystem=strict`，只有 `/opt/tv-buddy` 可写。
- 单二进制部署。已有依赖 `reqwest`（rustls）、`flate2`、`sha2`，不新增依赖。
- 低配目标磁盘 1 GB。安装包约 20–30 MB。
- 下载的二进制是 GPLv3 构建。仅在运行时下载与调用，不再分发。界面与文档要标注来源与许可。

## 已考虑的方案

1. **面板管理独立静态二进制（选定）**：免 root，Docker 与 systemd 都可用，macOS 可覆盖。
2. **调用系统包管理器**：只覆盖 root 原生部署。Docker 不可用。要放宽 systemd 沙箱。放弃。
3. **混合方案**：覆盖最全，但实现、测试与文档成本最高。放弃。

## 设计

### 目录与解析顺序

- 数据目录取数据库文件所在目录（`state.db_path.parent()`）。Docker 是 `/data`，systemd 是 `/opt/tv-buddy`，macOS 是启动目录。
- 面板安装的二进制：`<数据目录>/tools/ffmpeg`。权限 `0755`。
- 临时文件：`<数据目录>/tools/.ffmpeg.part`。安装成功后再原子改名。
- 解析顺序：
  1. 面板安装的 `tools/ffmpeg` 存在 → 用它。文件存在但不可执行时也不回退。
  2. 不存在时用 `PATH` 里的 `ffmpeg`。
- `ffprobe` 用同一规则：`tools/ffprobe` → `PATH`。本特性不写 `tools/ffprobe`。
- 解析只检查文件是否存在。功能探测由状态接口做。
- 面板版本存在但损坏时不静默回退。状态页报告 `installed=false`、`source=managed`，管理员可重新安装或删除。

### 代码结构

- 新增 `src/tools.rs`，路径函数显式接收数据目录，避免全局状态：
  - `data_dir(db_path: &Path) -> PathBuf`：取数据库文件的父目录。
  - `dir(data_dir: &Path) -> PathBuf`：`<数据目录>/tools`。
  - `managed_path(data_dir: &Path, name: &str) -> Option<PathBuf>`：存在且可执行才返回。
  - `ffmpeg(data_dir: &Path) -> PathBuf`：`managed_path(data_dir, "ffmpeg")`，否则 `"ffmpeg"`。
  - `ffprobe(data_dir: &Path) -> PathBuf`：同理。
- `AppState` 提供 `data_dir()`、`tools_dir()`、`ffmpeg()`、`ffprobe()` 包装。
- 安装任务与状态放在 `src/tools/install.rs`。
- 替换所有硬编码调用：
  - `src/http/routes/media.rs` 3 处（`ffmpeg_probe`、`ffmpeg_status`、`run_ffmpeg`）
  - `src/schedule.rs` 3 处
  - `src/logo_anim.rs` 2 处（ffmpeg 与 ffprobe）
- 备份不包含 `tools/`。现有备份只收集配置、台标与脚本，无需改动。

### 安装源

- 默认仓库：`https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1`。
- Linux 构建来自 johnvansickle（ffmpeg 7.0.2），可在 Alpine/musl 运行。macOS 构建版本以发布资产为准（实测 6.0）。许可为 GPLv3。
- 代码内固定每个平台的文件名、压缩大小与 SHA-256：

| 平台 | 文件 | 压缩字节 | SHA-256（.gz） |
| --- | --- | --- | --- |
| Linux amd64 | `ffmpeg-linux-x64.gz` | 29354986 | `bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa` |
| Linux arm64 | `ffmpeg-linux-arm64.gz` | 25568691 | `754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0` |
| macOS amd64 | `ffmpeg-darwin-x64.gz` | 25296431 | `929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106` |
| macOS arm64 | `ffmpeg-darwin-arm64.gz` | 19246198 | `8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa` |

- 镜像：环境变量 `TV_BUDDY_FFMPEG_BASE_URL` 覆盖默认前缀。镜像必须提供同名文件。
- 平台识别：`std::env::consts::OS` 与 `std::env::consts::ARCH`。其他组合返回「不支持此平台」。
- 不改配置文件，不新增配置项。

### 安装任务

- `AppState` 增加 `ffmpeg_install: Arc<ToolInstall>`。
- 状态机：`idle` → `downloading` → `verifying` → `installing` → `done` / `failed`。
- 状态字段：`phase`、`downloaded`、`total`（取 `Content-Length`，可能为空）、`error`、`started_at`、`finished_at`。
- 同一时间只允许一个任务。运行中再次安装或删除返回 `409`。
- 流程：
  1. 检查平台与 URL（只接受 `https`）。
  2. 用现有 `reqwest::Client` 流式下载。每块写入临时文件，同时更新 SHA-256 与进度。
  3. 下载完成后比对压缩字节数与固定 SHA-256。不一致就失败并删除临时文件。
  4. 用 `flate2` 解压到 `tools/.ffmpeg.unpack`。
  5. 设 `0755`，运行 `-version`。退出码非 0 或输出不含 `ffmpeg version` 就失败。
  6. 原子改名为 `tools/ffmpeg`。
  7. 任一步失败：删除临时文件，保留旧的 `tools/ffmpeg`，状态置 `failed`。
- 下载上限 200 MB。超过就中止。
- 日志写入现有运行日志，包含阶段与错误。

### 接口

- `GET /panel/api/assets/ffmpeg-status`（现有，扩展）：
  - 保留 `installed`、`version`、`remux_ready`、`note`。
  - 新增 `source`：`managed` / `system` / `null`。以 `tools/ffmpeg` 是否存在为准。
  - 新增 `path`：实际执行路径或 `null`。
  - 新增 `managed_installed`：`tools/ffmpeg` 是否存在且可执行。
  - 新增 `system_installed`：`PATH` 里的 ffmpeg 是否可用。
  - 新增 `job`：当前或最近一次安装任务的状态；没有任务时为 `null`。
- `POST /panel/api/tools/ffmpeg/install`：仅管理员。启动任务，立即返回 `{ok, job}`。不支持平台返回 `400`；任务运行中返回 `409`。
- `POST /panel/api/tools/ffmpeg/remove`：仅管理员。删除 `tools/ffmpeg` 并清理临时文件。任务运行中返回 `409`。系统版本不受影响。
- 管理员判定用现有 `PanelUser`，要求 `is_admin`。

### 界面

- 抽一个 `FfmpegCard` 组件，供「系统状态」和「视频素材」页共用。
- 显示：已安装/未安装、版本、来源（面板安装/系统）、路径。
- 按钮：安装、重新安装、删除。删除前用现有 `Confirm`。
- 只有管理员看到写操作按钮。普通用户只看状态。
- 任务运行中显示阶段与已下载/总字节，按钮禁用。前端每秒轮询状态，结束后停止。
- 未安装时说明影响：DASH 录制转 MKV、素材转换不可用；remux 不受影响。
- 安装失败时显示服务端错误文本，并给出「重试」。

### 安全

- 下载地址只允许 `https`（默认或镜像）。回环地址允许 `http`，仅用于测试。重定向后的最终地址也必须满足同一规则。
- 只使用代码内固定的 SHA-256。不下载远端校验文件。
- 不使用 shell。`Command::new(path)` 直接执行。
- 文件名不由用户输入决定。数据目录来自数据库路径。
- 管理员会话才可写。普通用户与未登录请求被拒。
- 安装包来源与 GPLv3 许可在界面与 README 中标注。

### 测试

- 单元：
  - 平台映射：四个受支持组合与一个不支持组合。
  - 解析顺序：临时数据目录有/无 `tools/ffmpeg` 时返回正确路径。
  - 哈希校验：正确哈希通过，错误哈希失败且不保留临时文件。
  - 删除：只删 `tools/ffmpeg`；系统路径不受影响。
  - 删除时清理 `.ffmpeg.part` 与 `.ffmpeg.unpack`。
- 集成：
  - 本地 HTTP 服务提供测试用压缩二进制与固定哈希，注入安装源，走完整任务状态机。
  - 任务运行中重复安装返回 `409`。
  - 非管理员返回 `403`。
- 手工：
  - Docker（UID 10001）安装后，素材转换与 DASH 录制转 MKV 可用。
  - systemd 下安装成功，`ProtectSystem=strict` 不影响。
  - 删除面板版本后回退系统版本。
  - 断网或哈希错误时任务失败，旧版本仍可用。

### 文档

- README「可选依赖」补充：可在面板安装 ffmpeg。
- README 接口表补充两个写接口。
- 说明 GPLv3、来源仓库与镜像环境变量。

## 留给实现者

- 组件拆分与样式复用方式。
- 轮询间隔与文案。
- 是否在状态卡显示 `ffprobe` 一行（本特性不安装）。
- 安装任务的日志级别。
