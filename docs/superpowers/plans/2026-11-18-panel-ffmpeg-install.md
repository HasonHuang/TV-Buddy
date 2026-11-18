# 面板安装 ffmpeg 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 管理员在 Web 面板安装、删除与重新安装 ffmpeg。安装的二进制放在数据目录，不需要 root。

**Architecture:** 新增 `src/tools.rs` 统一解析 ffmpeg 路径，优先 `<数据库目录>/tools/ffmpeg`，其次系统 `PATH`。新增 `src/tools/install.rs` 下载固定版本的静态构建，校验 SHA-256 后原子启用。安装是单实例异步任务，前端轮询状态。

**Tech Stack:** Rust（axum、reqwest、flate2、sha2、hex）、React + TypeScript、Vite。

**Spec:** `docs/superpowers/specs/2026-11-18-panel-ffmpeg-install-design.md`

## Global Constraints

- 支持平台：`linux/x86_64`、`linux/aarch64`、`macos/x86_64`、`macos/aarch64`。其他组合返回 `400`。
- 安装源固定为 `eugeneware/ffmpeg-static` 的 `b6.1.1`。默认前缀：`https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1`。
- 固定 SHA-256（压缩文件）：
  - `ffmpeg-linux-x64.gz`，29354986 字节，`bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa`
  - `ffmpeg-linux-arm64.gz`，25568691 字节，`754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0`
  - `ffmpeg-darwin-x64.gz`，25296431 字节，`929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106`
  - `ffmpeg-darwin-arm64.gz`，19246198 字节，`8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa`
- 只安装 ffmpeg。ffprobe 继续按 `tools/ffprobe` → `PATH` 解析，本特性不写 `tools/ffprobe`。
- 面板写操作仅管理员。普通用户返回 `403`。
- 下载地址只允许 `https`。回环地址（`127.0.0.1`、`localhost`、`[::1]`）允许 `http`，供测试使用。
- 压缩包与解压结果上限均为 200 MB。
- 不新增 Cargo 或 npm 依赖。
- 安装目录是 `<数据库目录>/tools`。备份不包含该目录。
- 界面与诊断文案用中文。文档用短句。

## Review Focus

- 下载中断、正文截断或大小不符：任务失败，不产生 `tools/ffmpeg`，旧版本保留。Task 2 覆盖。
- 镜像返回 `200` 的 HTML 错误页：SHA-256 不符，任务失败，旧版本保留。Task 2 覆盖。
- 解压或写入失败：任务进入 `failed`，临时文件清理，旧版本保留。Task 2 与 Task 3 覆盖。
- 面板二进制存在但不可执行或架构不符：状态为 `installed=false`、`source=managed`，不静默回退；删除或重装可恢复。Task 1 与 Task 4 覆盖。
- 并发安装与权限：第二个安装或删除请求返回 `409`；普通用户返回 `403`。Task 3 与 Task 4 覆盖。

---

### Task 1: 统一 ffmpeg 路径解析

**Files:**
- Create: `src/tools.rs`
- Modify: `src/lib.rs`（模块表，`pub mod tools;`）
- Modify: `src/state.rs`（`AppState` 方法）
- Modify: `src/http/routes/media.rs:111-118, 670, 706-712, 800-806`
- Modify: `src/schedule.rs:445, 511, 639`
- Modify: `src/logo_anim.rs:39-56, 139, 212-261`
- Modify: `src/http/routes/logo.rs:405-414`
- Test: `src/tools.rs`（内联单元测试）

**Interfaces:**
- Consumes: 无。
- Produces:
  - `pub fn data_dir(db_path: &Path) -> PathBuf`
  - `pub fn dir(data_dir: &Path) -> PathBuf`
  - `pub fn is_executable(path: &Path) -> bool`
  - `pub fn managed_path(data_dir: &Path, name: &str) -> Option<PathBuf>`
  - `pub fn ffmpeg(data_dir: &Path) -> PathBuf`
  - `pub fn ffprobe(data_dir: &Path) -> PathBuf`
  - `AppState::data_dir() -> PathBuf`
  - `AppState::tools_dir() -> PathBuf`
  - `AppState::ffmpeg() -> PathBuf`
  - `AppState::ffprobe() -> PathBuf`

- [ ] **Step 1: 写失败的单元测试**

在 `src/tools.rs` 末尾写测试。测试只支持 Unix，项目也只发布 Unix 平台。

```rust
#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tv-buddy-tools-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn executable(path: &std::path::Path) {
        let mut permissions = fs::metadata(path).expect("读属性").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("设权限");
    }

    #[test]
    fn managed_binary_wins_over_path() {
        let dir = temp_dir();
        fs::create_dir_all(super::dir(&dir)).expect("建 tools");
        let managed = super::dir(&dir).join("ffmpeg");
        fs::write(&managed, b"#!/bin/sh\n").expect("写文件");
        executable(&managed);
        assert_eq!(super::ffmpeg(&dir), managed);
        assert_eq!(super::ffprobe(&dir), PathBuf::from("ffprobe"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_non_executable_falls_back_to_path() {
        let dir = temp_dir();
        assert_eq!(super::ffmpeg(&dir), PathBuf::from("ffmpeg"));

        fs::create_dir_all(super::dir(&dir)).expect("建 tools");
        let managed = super::dir(&dir).join("ffmpeg");
        fs::write(&managed, b"#!/bin/sh\n").expect("写文件");
        fs::set_permissions(&managed, fs::Permissions::from_mode(0o644)).expect("设权限");
        assert_eq!(super::ffmpeg(&dir), PathBuf::from("ffmpeg"));
        let _ = fs::remove_dir_all(dir);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test --offline --lib tools::tests`
Expected: 编译失败，提示 `super::dir`、`super::ffmpeg` 等函数不存在。

- [ ] **Step 3: 实现路径解析**

在 `src/tools.rs` 写：

```rust
//! 外部工具路径：面板安装的版本优先，系统 PATH 兜底。

use std::path::{Path, PathBuf};

/// 数据目录：数据库文件所在目录。
pub fn data_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 面板安装的工具目录。
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tools")
}

/// Unix 可执行判定：普通文件且任一执行位为 1。
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// 面板安装的二进制；不存在或不可执行时返回 `None`。
pub fn managed_path(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir(data_dir).join(name);
    is_executable(&path).then_some(path)
}

pub fn ffmpeg(data_dir: &Path) -> PathBuf {
    managed_path(data_dir, "ffmpeg").unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

pub fn ffprobe(data_dir: &Path) -> PathBuf {
    managed_path(data_dir, "ffprobe").unwrap_or_else(|| PathBuf::from("ffprobe"))
}
```

在 `src/lib.rs` 的模块表加入 `pub mod tools;`，放在 `pub mod timeshift;` 后。

在 `src/state.rs` 的 `impl AppState` 内、`snapshot` 前加入：

```rust
/// 数据目录：数据库文件所在目录。面板安装的外部工具放在这里。
pub fn data_dir(&self) -> PathBuf {
    crate::tools::data_dir(&self.db_path)
}

pub fn tools_dir(&self) -> PathBuf {
    crate::tools::dir(&self.data_dir())
}

pub fn ffmpeg(&self) -> PathBuf {
    crate::tools::ffmpeg(&self.data_dir())
}

pub fn ffprobe(&self) -> PathBuf {
    crate::tools::ffprobe(&self.data_dir())
}
```

`src/state.rs` 的 `use std::path::PathBuf` 已存在，不需新增导入。

- [ ] **Step 4: 运行单元测试**

Run: `cargo test --offline --lib tools::tests`
Expected: 2 项通过。

- [ ] **Step 5: 替换调用点**

`src/http/routes/media.rs`：

- 增加 `async fn probe_version(path: &FsPath) -> Option<String>`：运行 `path -version`，退出码为 0 时取 stdout 第一行非空文本。
- `async fn ffmpeg_probe(state: &AppState) -> Option<PathBuf>` 改为：

```rust
async fn ffmpeg_probe(state: &AppState) -> Option<PathBuf> {
    let path = state.ffmpeg();
    probe_version(&path).await.map(|_| path)
}
```

- `list` 里的调用改为 `ffmpeg_probe(&state).await`。`"ffmpeg_path"` 改为 `ffmpeg.as_ref().map(|path| path.display().to_string())`。
- `convert` 里的调用改为 `ffmpeg_probe(&state).await.is_none()`。
- `run_ffmpeg` 增加参数 `ffmpeg: &FsPath`，放在参数表末尾：`run_ffmpeg(input, playlist, segments, copy, ffmpeg: &FsPath)`。把 `Command::new("ffmpeg")` 改为 `Command::new(ffmpeg)`。
- `convert` 的两次调用改为 `run_ffmpeg(&input, &playlist, &segments, true, &ffmpeg)` 与 `run_ffmpeg(&input, &playlist, &segments, false, &ffmpeg)`。调用前先 `let ffmpeg = state.ffmpeg();`。

`src/schedule.rs`：

- 3 处 `tokio::process::Command::new("ffmpeg")` 改为 `tokio::process::Command::new(state.ffmpeg())`。

`src/logo_anim.rs`：

- `animate` 参数表在 `cache_dir` 后加 `ffmpeg: &Path, ffprobe: &Path`。参数变为 9 个，在函数上加 `#[allow(clippy::too_many_arguments)]`，避免新增 clippy 警告。
- 第 139 行改为 `decode_ffmpeg(path, ffmpeg, ffprobe)`。
- `decode_ffmpeg(path: &Path, ffmpeg: &Path, ffprobe: &Path)`：第 213 行改为 `probe_ffmpeg(path, ffprobe)?`；第 214 行改为 `Command::new(ffmpeg)`。
- `probe_ffmpeg(path: &Path, ffprobe: &Path)`：第 238 行改为 `Command::new(ffprobe)`。

`src/http/routes/logo.rs` 第 405 行的 `animate` 调用在 `&config.cache.disk_path` 后加状态参数：

```rust
&state.ffmpeg(),
&state.ffprobe(),
```

调用前不需要绑定临时值；借用持续到函数返回。

- [ ] **Step 6: 编译并运行相关测试**

Run: `cargo test --offline --lib && cargo test --offline --test http_contract media_assets_subtitles_and_resolver_scripts`
Expected: 全部通过。

- [ ] **Step 7: 提交**

```bash
git add src/tools.rs src/lib.rs src/state.rs src/http/routes/media.rs src/schedule.rs src/logo_anim.rs src/http/routes/logo.rs docs/superpowers/specs/2026-11-18-panel-ffmpeg-install-design.md docs/superpowers/plans/2026-11-18-panel-ffmpeg-install.md
git commit -m "feat(tools): 统一解析 ffmpeg 路径"
```

---

### Task 2: 下载、校验与解压

**Files:**
- Create: `src/tools/install.rs`
- Modify: `src/tools.rs`（Task 2 的 Step 3 加 `pub mod install;`）
- Test: `src/tools/install.rs`（内联单元测试）

**Interfaces:**
- Consumes: `crate::tools::dir`、`crate::tools::is_executable`。
- Produces:
  - `pub const DEFAULT_BASE_URL: &str`
  - `pub struct Artifact { pub file: String, pub bytes: u64, pub sha256: String }`
  - `pub fn artifact_for(os: &str, arch: &str) -> Option<Artifact>`
  - `pub fn base_url() -> String`
  - `pub fn remove_managed(data_dir: &Path) -> anyhow::Result<bool>`
  - `pub(crate) async fn install(client: &reqwest::Client, base_url: &str, artifact: &Artifact, data_dir: &Path, progress: &(dyn Fn(&str, u64, Option<u64>) + Send + Sync)) -> anyhow::Result<()>`

- [ ] **Step 1: 写失败的单元测试**

测试放 `install.rs` 的 `#[cfg(test)] mod tests`。先写平台映射、成功安装、哈希失败、大小失败、旧版本保留五组。

```rust
fn fixture_artifact() -> (Artifact, Vec<u8>) {
    use flate2::{write::GzEncoder, Compression};
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let script = b"#!/bin/sh\necho \"ffmpeg version test\"\n";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(script).expect("压缩");
    let gz = encoder.finish().expect("压缩结束");
    let sha256 = hex::encode(Sha256::digest(&gz));
    (
        Artifact {
            file: "ffmpeg-test.gz".into(),
            bytes: gz.len() as u64,
            sha256,
        },
        gz,
    )
}

async fn serve(bytes: Vec<u8>) -> String {
    let app = axum::Router::new().route(
        "/ffmpeg-test.gz",
        axum::routing::get(move || {
            let bytes = bytes.clone();
            async move { bytes }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定端口");
    let addr = listener.local_addr().expect("取端口");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tv-buddy-install-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

#[test]
fn artifact_table_covers_four_platforms() {
    assert_eq!(artifact_for("linux", "x86_64").expect("linux amd64").file, "ffmpeg-linux-x64.gz");
    assert_eq!(artifact_for("linux", "aarch64").expect("linux arm64").file, "ffmpeg-linux-arm64.gz");
    assert_eq!(artifact_for("macos", "x86_64").expect("macos amd64").file, "ffmpeg-darwin-x64.gz");
    assert_eq!(artifact_for("macos", "aarch64").expect("macos arm64").file, "ffmpeg-darwin-arm64.gz");
    assert!(artifact_for("windows", "x86_64").is_none());
}

#[tokio::test]
async fn install_places_verified_binary() {
    let (artifact, gz) = fixture_artifact();
    let base = serve(gz).await;
    let dir = temp_dir();
    let client = reqwest::Client::new();

    install(&client, &base, &artifact, &dir, &|_, _, _| {}).await.expect("安装");

    let dest = crate::tools::dir(&dir).join("ffmpeg");
    assert!(crate::tools::is_executable(&dest));
    assert!(!crate::tools::dir(&dir).join(".ffmpeg.part").exists());
    assert!(!crate::tools::dir(&dir).join(".ffmpeg.unpack").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn install_rejects_hash_mismatch() {
    let (mut artifact, gz) = fixture_artifact();
    artifact.sha256 = "0".repeat(64);
    let base = serve(gz).await;
    let dir = temp_dir();
    let client = reqwest::Client::new();

    let error = install(&client, &base, &artifact, &dir, &|_, _, _| {})
        .await
        .expect_err("哈希必须失败");
    assert!(format!("{error:#}").contains("SHA-256"));
    assert!(!crate::tools::dir(&dir).join("ffmpeg").exists());
    assert!(!crate::tools::dir(&dir).join(".ffmpeg.part").exists());
    assert!(!crate::tools::dir(&dir).join(".ffmpeg.unpack").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn install_rejects_size_mismatch() {
    let (mut artifact, gz) = fixture_artifact();
    artifact.bytes += 1;
    let base = serve(gz).await;
    let dir = temp_dir();
    let client = reqwest::Client::new();

    let error = install(&client, &base, &artifact, &dir, &|_, _, _| {})
        .await
        .expect_err("大小必须失败");
    assert!(format!("{error:#}").contains("大小"));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn failed_install_keeps_old_binary() {
    let (mut artifact, gz) = fixture_artifact();
    artifact.sha256 = "0".repeat(64);
    let base = serve(gz).await;
    let dir = temp_dir();
    let tools = crate::tools::dir(&dir);
    std::fs::create_dir_all(&tools).expect("建 tools");
    std::fs::write(tools.join("ffmpeg"), b"old").expect("写旧文件");

    install(&reqwest::Client::new(), &base, &artifact, &dir, &|_, _, _| {})
        .await
        .expect_err("必须失败");
    assert_eq!(std::fs::read(tools.join("ffmpeg")).expect("读旧文件"), b"old");
    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test --offline --lib tools::install::tests`
Expected: 编译失败，模块与函数不存在。

- [ ] **Step 3: 实现安装源表与下载**

先在 `src/tools.rs` 顶部加 `pub mod install;`。再在 `src/tools/install.rs` 实现固定表、URL 校验与 `install`。关键值必须与 Global Constraints 一致。

```rust
pub const DEFAULT_BASE_URL: &str =
    "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1";
pub const PHASE_DOWNLOADING: &str = "downloading";
pub const PHASE_VERIFYING: &str = "verifying";
pub const PHASE_INSTALLING: &str = "installing";

const PART: &str = ".ffmpeg.part";
const UNPACK: &str = ".ffmpeg.unpack";
const DEST: &str = "ffmpeg";
const MAX_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Artifact {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}
```

`artifact_for` 用 `match (os, arch)` 返回上表的值。`base_url` 读取 `TV_BUDDY_FFMPEG_BASE_URL`，去空白，空值回退 `DEFAULT_BASE_URL`。

`install` 按以下顺序执行：

1. `tokio::fs::create_dir_all(crate::tools::dir(data_dir))`。
2. 删除旧的 `PART` 与 `UNPACK`。
3. 用 `url::Url::parse` 拼出 `{base}/{file}`。校验：`https`，或 `http` 且 host 是 `127.0.0.1`、`localhost`、`[::1]`。
4. `client.get(url).send().await?.error_for_status()?`。再校验 `response.url()`。
5. 读 `response.content_length()` 作为 `total`。
6. 循环 `response.chunk().await?`，不用 `bytes_stream`（项目没开 reqwest 的 `stream` 特性）。每块累加 `downloaded`，超过 `MAX_BYTES` 报错；更新 `Sha256`；写入 `PART`；调用 `progress(PHASE_DOWNLOADING, downloaded, total)`。
7. 关闭文件。校验 `downloaded == artifact.bytes`，否则报「下载大小不符」。
8. 校验 `hex::encode(hasher.finalize()) == artifact.sha256`，否则报「SHA-256 校验失败」。
9. `progress(PHASE_VERIFYING, downloaded, total)`。
10. 在 `tokio::task::spawn_blocking` 内用 `flate2::read::GzDecoder` 解压 `PART` 到 `UNPACK`。用 `decoder.take(MAX_BYTES + 1)` 限制输出；复制字节超过 `MAX_BYTES` 就报错。
11. `tokio::fs::set_permissions(&UNPACK, Permissions::from_mode(0o755))`。
12. `progress(PHASE_INSTALLING, downloaded, total)`。
13. 运行 `UNPACK -version`。要求退出码为 0，且 stdout 含 `ffmpeg version`。
14. `tokio::fs::rename(&UNPACK, dest)`。删除 `PART`。
15. 任一步失败：删除 `PART` 与 `UNPACK`，保留旧 `DEST`。

`remove_managed`：

```rust
pub fn remove_managed(data_dir: &std::path::Path) -> anyhow::Result<bool> {
    let tools = crate::tools::dir(data_dir);
    let dest = tools.join(DEST);
    let existed = dest.is_file();
    if existed {
        std::fs::remove_file(&dest).with_context(|| format!("删除 {}", dest.display()))?;
    }
    let _ = std::fs::remove_file(tools.join(PART));
    let _ = std::fs::remove_file(tools.join(UNPACK));
    Ok(existed)
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test --offline --lib tools::install::tests`
Expected: 5 项通过。

- [ ] **Step 5: 提交**

```bash
git add src/tools/install.rs
git commit -m "feat(tools): 增加 ffmpeg 下载与校验"
```

---

### Task 3: 安装任务状态

**Files:**
- Modify: `src/tools/install.rs`
- Modify: `src/state.rs`（新增字段与初始化）
- Test: `src/tools/install.rs`（内联单元测试）

**Interfaces:**
- Consumes: Task 2 的 `Artifact`、`artifact_for`、`base_url`、`install`、`remove_managed`。
- Produces:
  - `pub struct JobSnapshot { pub phase: String, pub downloaded: u64, pub total: Option<u64>, pub error: Option<String>, pub started_at: u64, pub finished_at: Option<u64> }`，实现 `Clone`、`Debug`、`Serialize`。
  - `pub enum StartError { Busy, Unsupported(String) }`，实现 `Display`。
  - `pub struct Installer`
  - `Installer::new() -> Self`
  - `Installer::set_base_url(&self, base: impl Into<String>)`
  - `Installer::job(&self) -> Option<JobSnapshot>`：`idle` 返回 `None`。
  - `Installer::running(&self) -> bool`
  - `Installer::start(self: &Arc<Self>, data_dir: PathBuf) -> Result<JobSnapshot, StartError>`
  - `AppState.ffmpeg_install: Arc<crate::tools::install::Installer>`

- [ ] **Step 1: 写失败的单元测试**

```rust
#[tokio::test]
async fn start_rejects_second_job() {
    let installer = Arc::new(Installer::new());
    let base = serve_with_delay(b"not-gzip".to_vec(), 1000).await;
    installer.set_base_url(base);
    let dir = temp_dir();

    installer.start(dir.clone()).expect("第一个任务");
    assert!(matches!(installer.start(dir.clone()), Err(StartError::Busy)));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while installer.running() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!installer.running());
    let job = installer.job().expect("任务记录");
    assert_eq!(job.phase, "failed");
    assert!(job.error.as_deref().unwrap_or("").len() > 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn finished_job_reports_done() {
    let (artifact, gz) = fixture_artifact();
    let base = serve(gz).await;
    let dir = temp_dir();
    let installer = Arc::new(Installer::new());
    installer.artifact = Some(artifact);
    installer.set_base_url(base);

    installer.start(dir.clone()).expect("启动");
    let job = wait_job(&installer, "done").await;
    assert!(job.error.is_none());
    assert!(crate::tools::is_executable(&crate::tools::dir(&dir).join("ffmpeg")));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn failed_job_reports_error() {
    let (mut artifact, gz) = fixture_artifact();
    artifact.sha256 = "0".repeat(64);
    let base = serve(gz).await;
    let dir = temp_dir();
    let installer = Arc::new(Installer::new());
    installer.artifact = Some(artifact);
    installer.set_base_url(base);

    installer.start(dir.clone()).expect("启动");
    let job = wait_job(&installer, "failed").await;
    assert!(job.error.as_deref().unwrap_or("").contains("SHA-256"));
    let _ = std::fs::remove_dir_all(dir);
}

async fn wait_job(installer: &Arc<Installer>, phase: &str) -> JobSnapshot {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(job) = installer.job() {
            if job.phase == phase {
                return job;
            }
            assert_ne!(job.phase, "failed", "任务失败：{:?}", job.error);
        }
        assert!(std::time::Instant::now() < deadline, "等待超时：{phase}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
```

`serve_with_delay` 与 `serve` 相同，但延迟后返回正文。延迟用 `tokio::time::sleep`。它必须匹配任意路径，用 `Router::new().fallback(handler)`，因为 `start` 使用当前平台的真实制品文件名。

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test --offline --lib tools::install::tests`
Expected: 编译失败，`Installer` 与 `JobSnapshot` 不存在。

- [ ] **Step 3: 实现 Installer**

关键结构：

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct JobSnapshot {
    pub phase: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
}

impl Default for JobSnapshot {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            downloaded: 0,
            total: None,
            error: None,
            started_at: 0,
            finished_at: None,
        }
    }
}

pub struct Installer {
    job: std::sync::Mutex<JobSnapshot>,
    running: std::sync::atomic::AtomicBool,
    base_url: std::sync::RwLock<String>,
    client: reqwest::Client,
    #[cfg(test)]
    pub(crate) artifact: Option<Artifact>,
}
```

- `new()` 建专用客户端：`connect_timeout(15s)`、`read_timeout(60s)`，不设总超时。User-Agent 用 `crate::state::DEFAULT_USER_AGENT`。不要用 `AppState::http`（总超时 25 秒）。`#[cfg(test)]` 的 `artifact` 字段在 `new()` 中初始化为 `None`。
- `set_base_url` 写锁替换。
- `job()` 深拷贝；`phase == "idle"` 返回 `None`。
- `running()` 读原子值。
- `start`：
  1. `running.compare_exchange(false, true, SeqCst, SeqCst)` 失败返回 `StartError::Busy`。
  2. 取平台制品。生产用 `artifact_for(std::env::consts::OS, std::env::consts::ARCH)`；测试优先用 `self.artifact`。没有则复位 `running`，返回 `StartError::Unsupported(format!("{}/{}", OS, ARCH))`。
  3. 重置任务：`phase=downloading`、`downloaded=0`、`total=None`、`error=None`、`started_at=now_secs()`、`finished_at=None`。
  4. 取 base URL 与数据目录，克隆 `Arc<Self>`，`tokio::spawn` 执行 `run`。
  5. 返回当前快照。
- `run` 调用 Task 2 的 `install`，progress 闭包更新 `phase/downloaded/total`。成功写 `done`；失败写 `failed` 与 `format!("{error:#}")`。两条路径都写 `finished_at`，并把 `running` 置回 `false`。
- `now_secs()`：`SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()`。

在 `src/state.rs`：

- 字段区加 `pub ffmpeg_install: Arc<crate::tools::install::Installer>,`。
- `Self { ... }` 初始化加 `ffmpeg_install: Arc::new(crate::tools::install::Installer::new()),`。

- [ ] **Step 4: 运行测试**

Run: `cargo test --offline --lib tools::install::tests`
Expected: 8 项通过（Task 2 的 5 项加本任务的 3 项）。

- [ ] **Step 5: 提交**

```bash
git add src/tools/install.rs src/state.rs
git commit -m "feat(tools): 增加 ffmpeg 安装任务"
```

---

### Task 4: 面板接口

**Files:**
- Modify: `src/http/routes/media.rs:229-255`（状态）与文件末尾（新增处理函数）
- Modify: `src/http.rs:220-223`（路由）
- Test: `tests/http_contract.rs`（新增 6 个契约测试）

**Interfaces:**
- Consumes: `state.ffmpeg_install`、`Installer::start`、`Installer::job`、`Installer::running`、`StartError`、`remove_managed`、`tools::managed_path`、`state.ffmpeg()`、`state.data_dir()`。
- Produces:
  - `GET /panel/api/assets/ffmpeg-status`
  - `POST /panel/api/tools/ffmpeg/install`
  - `POST /panel/api/tools/ffmpeg/remove`

- [ ] **Step 1: 写失败的契约测试**

在 `tests/http_contract.rs` 加 6 个测试。复用现有 `fixture()`、`login()` 与 `non_admin_is_forbidden_on_admin_api` 的登录写法。

```rust
#[tokio::test]
async fn ffmpeg_status_reports_source_and_job() {
    let fixture = fixture().await;
    let session = login(&fixture).await;
    let body: serde_json::Value = serde_json::from_str(
        &fixture.client
            .get(format!("{}/panel/api/assets/ffmpeg-status", fixture.base))
            .header("authorization", format!("Bearer {session}"))
            .send().await.expect("请求").text().await.expect("正文"),
    ).expect("JSON");
    assert!(body["installed"].is_boolean());
    assert!(body["managed_installed"].is_boolean());
    assert!(body["system_installed"].is_boolean());
    assert!(body["source"].is_null() || body["source"].is_string());
    assert!(body["path"].is_null() || body["path"].is_string());
    assert!(body["job"].is_null() || body["job"].is_object());
    assert_eq!(body["remux_ready"], true);
}

#[tokio::test]
async fn ffmpeg_install_requires_admin() {
    let fixture = fixture().await;
    let login = |path: &'static str, body: &'static str| {
        fixture.client.post(format!("{}{path}", fixture.base))
            .header("content-type", "application/json").body(body).send()
    };
    let session: String = serde_json::from_str::<serde_json::Value>(
        &login("/panel/api/login", r#"{"username":"viewer","password":"viewer"}"#)
            .await.expect("登录").text().await.expect("正文"),
    ).expect("JSON")["token"].as_str().expect("token").to_string();
    let response = fixture.client
        .post(format!("{}/panel/api/tools/ffmpeg/install", fixture.base))
        .header("authorization", format!("Bearer {session}"))
        .send().await.expect("请求");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ffmpeg_remove_deletes_managed_binary() {
    let fixture = fixture().await;
    let session = login(&fixture).await;
    let tools = fixture.dir.join("tools");
    std::fs::create_dir_all(&tools).expect("建 tools");
    std::fs::write(tools.join("ffmpeg"), b"#!/bin/sh\n").expect("写文件");
    std::fs::write(tools.join(".ffmpeg.part"), b"part").expect("写临时文件");
    std::fs::write(tools.join(".ffmpeg.unpack"), b"unpack").expect("写临时文件");

    let response = fixture.client
        .post(format!("{}/panel/api/tools/ffmpeg/remove", fixture.base))
        .header("authorization", format!("Bearer {session}"))
        .send().await.expect("请求");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!tools.join("ffmpeg").exists());
    assert!(!tools.join(".ffmpeg.part").exists());
    assert!(!tools.join(".ffmpeg.unpack").exists());
}

#[tokio::test]
async fn ffmpeg_install_job_fails_on_unreachable_source() {
    let fixture = fixture().await;
    let session = login(&fixture).await;
    fixture.state.ffmpeg_install.set_base_url("http://127.0.0.1:1");
    let response = fixture.client
        .post(format!("{}/panel/api/tools/ffmpeg/install", fixture.base))
        .header("authorization", format!("Bearer {session}"))
        .send().await.expect("请求");
    assert_eq!(response.status(), StatusCode::OK);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let body: serde_json::Value = serde_json::from_str(
            &fixture.client
                .get(format!("{}/panel/api/assets/ffmpeg-status", fixture.base))
                .header("authorization", format!("Bearer {session}"))
                .send().await.expect("请求").text().await.expect("正文"),
        ).expect("JSON");
        if body["job"]["phase"] == "failed" {
            assert!(body["job"]["error"].as_str().unwrap_or("").len() > 0);
            break;
        }
        assert!(Instant::now() < deadline, "任务未失败");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!fixture.dir.join("tools/ffmpeg").exists());
}

#[tokio::test]
async fn ffmpeg_install_rejects_second_job() {
    let fixture = fixture().await;
    let session = login(&fixture).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await.expect("绑定端口");
    let addr = listener.local_addr().expect("取端口");
    tokio::spawn(async move {
        let app = axum::Router::new().fallback(|| async {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            b"not-gzip".to_vec()
        });
        let _ = axum::serve(listener, app).await;
    });
    fixture.state.ffmpeg_install.set_base_url(format!("http://{addr}"));

    let post = || {
        fixture.client
            .post(format!("{}/panel/api/tools/ffmpeg/install", fixture.base))
            .header("authorization", format!("Bearer {session}"))
            .send()
    };
    assert_eq!(post().await.expect("第一次").status(), StatusCode::OK);
    assert_eq!(post().await.expect("第二次").status(), StatusCode::CONFLICT);

    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.state.ffmpeg_install.running() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!fixture.state.ffmpeg_install.running());
}
```

另外加一个「面板版本存在但损坏」的状态测试。这个覆盖 Review Focus 第 4 条。

```rust
#[tokio::test]
async fn ffmpeg_status_does_not_fall_back_from_broken_managed_binary() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = fixture().await;
    let session = login(&fixture).await;
    let tools = fixture.dir.join("tools");
    std::fs::create_dir_all(&tools).expect("建 tools");
    let broken = tools.join("ffmpeg");
    std::fs::write(&broken, b"#!/bin/sh\nexit 1\n").expect("写文件");
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755))
        .expect("设权限");

    let body: serde_json::Value = serde_json::from_str(
        &fixture.client
            .get(format!("{}/panel/api/assets/ffmpeg-status", fixture.base))
            .header("authorization", format!("Bearer {session}"))
            .send().await.expect("请求").text().await.expect("正文"),
    ).expect("JSON");
    assert_eq!(body["managed_installed"], true);
    assert_eq!(body["installed"], false);
    assert_eq!(body["source"], "managed");
    assert!(body["path"].is_null());
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test --offline --test http_contract ffmpeg_`
Expected: 状态字段缺失，安装与删除接口返回 `404`。

- [ ] **Step 3: 实现状态接口**

`ffmpeg_status` 改为接收 `PanelAuth` 与 `State(state)`。复用 Task 1 的 `probe_version(path)`。

响应字段：

```json
{
  "installed": true,
  "version": "ffmpeg version 7.0.2-static ...",
  "source": "managed",
  "path": "/data/tools/ffmpeg",
  "managed_installed": true,
  "system_installed": true,
  "job": { "phase": "downloading", "downloaded": 123, "total": 29354986, "error": null, "started_at": 0, "finished_at": null },
  "remux_ready": true,
  "note": "可转封装"
}
```

规则：

- `managed_installed`：`tools::managed_path(&state.data_dir(), "ffmpeg").is_some()`。
- `system_installed`：`probe_version(Path::new("ffmpeg")).await.is_some()`。
- `source`：面板版本存在时为 `"managed"`；否则系统可用时为 `"system"`；否则 `null`。
- `installed`：有面板版本时看面板版本能否运行；否则看系统版本。
- `path`：`installed=true` 时给实际路径字符串，否则 `null`。
- `version`：实际使用版本的 `-version` 首行。
- `job`：`state.ffmpeg_install.job()`。
- `remux_ready`：固定 `true`。remux 是纯 Rust，不依赖 ffmpeg。
- `note`：已安装为「可转封装」；未安装为「未安装 ffmpeg；DASH 录制转 MKV 与素材转换不可用；remux 不受影响」。

- [ ] **Step 4: 实现安装与删除接口**

```rust
pub async fn ffmpeg_install(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError>

pub async fn ffmpeg_remove(
    actor: PanelUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError>
```

- 非管理员：`Err(ApiError::Forbidden("需要管理员权限".into()))`。
- 安装：`state.ffmpeg_install.start(state.data_dir())`。`StartError::Busy` → `ApiError::conflict("安装任务正在进行")`；`StartError::Unsupported(platform)` → `ApiError::bad_request(format!("不支持此平台：{platform}"))`。成功返回 `{"ok": true, "job": <snapshot>}`。
- 删除：`running()` 为真返回 `ApiError::conflict("安装任务正在进行")`。否则 `remove_managed(&state.data_dir())`，返回 `{"ok": true, "removed": <bool>}`。失败返回 `ApiError::internal`。

在 `src/http.rs` 的 ffmpeg-status 路由后注册：

```rust
.route(
    "/panel/api/tools/ffmpeg/install",
    post(routes::media::ffmpeg_install),
)
.route(
    "/panel/api/tools/ffmpeg/remove",
    post(routes::media::ffmpeg_remove),
)
```

`media.rs` 的 `use crate::{http::{error::ApiError, ext::PanelAuth}, ...}` 加入 `ext::PanelUser`。

- [ ] **Step 5: 运行契约测试**

Run: `cargo test --offline --test http_contract ffmpeg_`
Expected: 6 项通过（含原有媒体素材测试中的 ffmpeg 断言）。

- [ ] **Step 6: 提交**

```bash
git add src/http/routes/media.rs src/http.rs tests/http_contract.rs
git commit -m "feat(panel): 增加 ffmpeg 安装与删除接口"
```

---

### Task 5: 面板界面

**Files:**
- Create: `frontend/src/ffmpeg.tsx`
- Modify: `frontend/src/system.tsx:1, 59, 71-78, 181-194`
- Modify: `frontend/src/assets.tsx:1-10, 534-562`
- Test: `cd frontend && pnpm build`（类型检查与打包）

**Interfaces:**
- Consumes:
  - `GET /panel/api/assets/ffmpeg-status`
  - `POST /panel/api/tools/ffmpeg/install`
  - `POST /panel/api/tools/ffmpeg/remove`
  - `api`、`getRole`、`formatBytes`、`Badge`、`Confirm`、`useToast`
- Produces:
  - `export function useFfmpeg(): { status: FfmpegStatus | null; busy: boolean; error: string; install: () => Promise<void>; remove: () => Promise<void>; refresh: () => Promise<void> }`
  - `export function FfmpegCard({ state }: { state: ReturnType<typeof useFfmpeg> })`

```ts
type FfmpegJob = {
  phase: string;
  downloaded: number;
  total: number | null;
  error: string | null;
  started_at: number;
  finished_at: number | null;
};

type FfmpegStatus = {
  installed: boolean;
  version: string | null;
  source: 'managed' | 'system' | null;
  path: string | null;
  managed_installed: boolean;
  system_installed: boolean;
  job: FfmpegJob | null;
};
```

- [ ] **Step 1: 写组件**

`useFfmpeg` 行为：

- 组件挂载时 `refresh()`。失败时 `error` 存文本。
- `status.job.phase` 是 `downloading`、`verifying`、`installing` 时，每秒调用 `refresh()`；结束或卸载时清定时器。
- `install()`：`POST /tools/ffmpeg/install`，随后 `refresh()`。捕获错误写入 `error`。
- `remove()`：`POST /tools/ffmpeg/remove`，随后 `refresh()`。
- `busy` 覆盖请求进行中。

`FfmpegCard` 渲染：

- 标题「ffmpeg」。
- 已安装：`Badge tone="ok">已安装</Badge>`、版本、来源（「面板安装」或「系统」）、路径。
- 未安装：`Badge tone="warn">未安装</Badge>`，并显示影响说明。
- 管理员按钮：未安装显示「安装 ffmpeg」；已安装显示「重新安装」与「删除面板版本」。删除用 `Confirm`，文案说明只删除面板安装的版本。
- 任务进行中：显示阶段中文名与 `formatBytes(downloaded)`，有 `total` 时显示百分比。按钮禁用。
- `job.phase === 'failed'`：显示 `job.error` 与「重试」按钮。
- `managed_installed && !installed`：显示「面板安装的版本不可用，请重新安装或删除」。
- 卡片底部用次要文字显示「来源：eugeneware/ffmpeg-static（GPLv3）」。
- 普通用户（`getRole() !== 'admin'`）不显示写操作按钮。

阶段中文名：`downloading` → 下载中；`verifying` → 校验中；`installing` → 启用中；`done` → 完成；`failed` → 失败。

- [ ] **Step 2: 接入系统状态页**

`frontend/src/system.tsx`：

- 删除 `ffmpeg` 的 `useState`、`Promise.all` 中的 ffmpeg 请求与 `setFfmpeg`。
- 删除「运行与能力」卡片里的 `<dt>ffmpeg</dt>` 与对应 `<dd>`。
- 导入 `FfmpegCard, useFfmpeg`。
- 在 `const { node, ok, err } = useToast();` 后加 `const ffmpeg = useFfmpeg();`。
- 在「运行与能力」卡片之后渲染 `<FfmpegCard state={ffmpeg} />`。

- [ ] **Step 3: 接入素材页**

`frontend/src/assets.tsx`：

- 导入 `FfmpegCard, useFfmpeg`。
- 删除 `AssetsPage` 里的 ffmpeg `useState` 与 `useEffect`。
- 加 `const ffmpeg = useFfmpeg();`。
- 用 `<FfmpegCard state={ffmpeg} />` 替换「运行环境」卡片。
- `CandidatesCard`、`Library`、`ConvertCard` 的 `ready` 改为 `Boolean(ffmpeg.status?.installed)`。

- [ ] **Step 4: 构建前端**

Run: `cd frontend && pnpm build`
Expected: `tsc --noEmit` 与 `vite build` 通过。

- [ ] **Step 5: 构建 Rust 以嵌入新前端**

Run: `cargo build --offline`
Expected: 编译通过。

- [ ] **Step 6: 提交**

```bash
git add frontend/src/ffmpeg.tsx frontend/src/system.tsx frontend/src/assets.tsx
git commit -m "feat(panel): 增加 ffmpeg 安装界面"
```

`frontend/dist` 在 `.gitignore` 中，不入库。

---

### Task 6: 文档、质量门与发布

**Files:**
- Modify: `README.md:104-110, 233-240, 760-766`
- Modify: `docs/design/03-http-contract.md:475-481`
- Modify: `PROGRESS.md:1-10, 677-692, 762-778`

- [ ] **Step 1: 更新 README**

在可选依赖表后加一段：

> 面板可以安装 ffmpeg。打开「系统状态」或「视频素材」页，点「安装 ffmpeg」。
> 安装到数据库目录的 `tools/ffmpeg`，不需要 root。
> 面板安装的版本优先于系统 `PATH`。删除后回退系统版本。
> 下载源是 `eugeneware/ffmpeg-static` 的静态构建，GPLv3。
> 镜像可用 `TV_BUDDY_FFMPEG_BASE_URL` 覆盖。

Docker 段落把「需要 ffmpeg 时加 `--build-arg WITH_FFMPEG=1`」补成「或在面板安装」。

接口表加：

```markdown
| `POST` | `/panel/api/tools/ffmpeg/{install,remove}` | 安装 / 删除面板 ffmpeg（管理员） |
```

- [ ] **Step 2: 更新 HTTP 契约文档**

在 `docs/design/03-http-contract.md` 的面板路由表「素材」或「系统状态」行补上两个接口。在 §6.9 素材与脚本里加三行行为说明：安装位置、解析顺序、管理员限制。

- [ ] **Step 3: 更新 PROGRESS**

- 修改首部「最后更新」为当天日期。
- 在 §5.3 后加 `## 5.4 面板安装 ffmpeg（v2026.50）`，写 4 条：
  - 面板下载静态 ffmpeg 到 `<数据库目录>/tools/ffmpeg`，校验 SHA-256。
  - 解析顺序：面板版本 → 系统 `PATH`。删除面板版本后回退。
  - 仅管理员。Docker 非 root 可用。安装源可换镜像。
  - 质量门结果（填实际命令与通过数）。

- [ ] **Step 4: 运行完整质量门**

```bash
cargo fmt --check
cargo test --offline --all-targets
cargo clippy --offline --all-targets
cd frontend && pnpm build
```

Expected: 全部通过。记录测试总数。

- [ ] **Step 5: 手工验收**

在 Docker 中验证：

```bash
docker build --build-arg WITH_FFMPEG=0 -t tv-buddy:ffmpeg-test .
docker run --rm -p 9900:9900 -v tv-buddy-ffmpeg-test:/data tv-buddy:ffmpeg-test
```

登录面板，打开「系统状态」，点「安装 ffmpeg」。确认：

1. 进度到「完成」。
2. 状态显示「已安装」、来源「面板安装」、路径 `/data/tools/ffmpeg`。
3. 上传一个视频素材并转换成功。
4. 若有 DASH 测试频道，录一段并确认产物是 MKV。
5. 点「删除面板版本」，状态回退「未安装」或「系统」。

若在 systemd 主机，再装一次，确认 `/opt/tv-buddy/tools/ffmpeg` 可写。`ProtectSystem=strict` 不应阻止安装。

若无法跑 Docker，在 macOS 本机启动二进制执行同样的 1、2、4 步。

- [ ] **Step 6: 提交文档**

```bash
git add README.md docs/design/03-http-contract.md PROGRESS.md
git commit -m "docs: 补充面板安装 ffmpeg"
```

- [ ] **Step 7: 推送与打标签**

项目规则：每次提交后推送 `main`（`PROGRESS.md` §6.5）。本特性完成即打标签。

```bash
git push origin main
git tag -a v2026.50 -m "feat(tools): 面板安装 ffmpeg"
git push origin v2026.50
```

如果远端已有更高版本，按 `v2026.N` 递增，不重排。
