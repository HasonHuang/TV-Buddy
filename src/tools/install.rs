//! ffmpeg 安装：固定制品、下载、校验与原子启用。

use std::{
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

pub const DEFAULT_BASE_URL: &str =
    "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1";
pub const PHASE_DOWNLOADING: &str = "downloading";
pub const PHASE_VERIFYING: &str = "verifying";
pub const PHASE_INSTALLING: &str = "installing";

const MAX_BYTES: u64 = 200 * 1024 * 1024;

/// 面板安装并维护的二进制名。
const MANAGED: [&str; 2] = ["ffprobe", "ffmpeg"];

#[derive(Debug, Clone)]
pub struct Artifact {
    pub name: String,
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}

fn part_path(tools: &Path, name: &str) -> PathBuf {
    tools.join(format!(".{name}.part"))
}

fn unpack_path(tools: &Path, name: &str) -> PathBuf {
    tools.join(format!(".{name}.unpack"))
}

/// 现有 ffmpeg 制品表。只增加 `name` 字段。
fn ffmpeg_artifact_for(os: &str, arch: &str) -> Option<Artifact> {
    let (file, bytes, sha256) = match (os, arch) {
        ("linux", "x86_64") => (
            "ffmpeg-linux-x64.gz",
            29_354_986,
            "bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa",
        ),
        ("linux", "aarch64") => (
            "ffmpeg-linux-arm64.gz",
            25_568_691,
            "754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0",
        ),
        ("macos", "x86_64") => (
            "ffmpeg-darwin-x64.gz",
            25_296_431,
            "929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106",
        ),
        ("macos", "aarch64") => (
            "ffmpeg-darwin-arm64.gz",
            19_246_198,
            "8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa",
        ),
        _ => return None,
    };
    Some(Artifact {
        name: "ffmpeg".into(),
        file: file.into(),
        bytes,
        sha256: sha256.into(),
    })
}

fn ffprobe_artifact_for(os: &str, arch: &str) -> Option<Artifact> {
    let (file, bytes, sha256) = match (os, arch) {
        ("linux", "x86_64") => (
            "ffprobe-linux-x64.gz",
            29_276_839,
            "25d9b6ccb05e3d9de9e04e31e2506d8dd7f9f0418981965ac6df12e8d3afd067",
        ),
        ("linux", "aarch64") => (
            "ffprobe-linux-arm64.gz",
            25_493_573,
            "2ab6aba60ee84412dff9188720703376cb4e7aaf7e0b5e43aa8249f2acae5bf8",
        ),
        ("macos", "x86_64") => (
            "ffprobe-darwin-x64.gz",
            25_239_438,
            "d4da574d6e2e197bd259b47d69cf262df9e312af24ad960444f6d806d3d4c186",
        ),
        ("macos", "aarch64") => (
            "ffprobe-darwin-arm64.gz",
            19_207_077,
            "d986a8ec7b030899fe66a8a288ed809a3543338705a3ce178cfb85869c5d80be",
        ),
        _ => return None,
    };
    Some(Artifact {
        name: "ffprobe".into(),
        file: file.into(),
        bytes,
        sha256: sha256.into(),
    })
}

pub fn artifacts_for(os: &str, arch: &str) -> Option<Vec<Artifact>> {
    Some(vec![
        ffprobe_artifact_for(os, arch)?,
        ffmpeg_artifact_for(os, arch)?,
    ])
}

/// 下载前缀。环境变量 `TV_BUDDY_FFMPEG_BASE_URL` 可指向镜像。
pub fn base_url() -> String {
    std::env::var("TV_BUDDY_FFMPEG_BASE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn ensure_allowed(url: &url::Url) -> Result<()> {
    match url.scheme() {
        "https" => Ok(()),
        "http"
            if matches!(
                url.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
            ) =>
        {
            Ok(())
        }
        _ => bail!("下载地址必须是 https"),
    }
}

/// 删除面板安装的 ffmpeg、ffprobe 与临时文件。返回是否删除了正式文件。
pub fn remove_managed(data_dir: &Path) -> Result<bool> {
    let tools = crate::tools::dir(data_dir);
    let mut existed = false;
    for name in MANAGED {
        let dest = tools.join(name);
        if dest.is_file() {
            std::fs::remove_file(&dest).with_context(|| format!("删除 {}", dest.display()))?;
            existed = true;
        }
        let _ = std::fs::remove_file(part_path(&tools, name));
        let _ = std::fs::remove_file(unpack_path(&tools, name));
    }
    Ok(existed)
}

/// 下载、校验并启用 ffmpeg 与 ffprobe。
///
/// `progress` 收到阶段名、累计已下载字节与总字节（总数为制品大小之和）。
pub(crate) async fn install(
    client: &reqwest::Client,
    base_url: &str,
    artifacts: &[Artifact],
    data_dir: &Path,
    progress: &(dyn Fn(&str, u64, Option<u64>) + Send + Sync),
) -> Result<()> {
    let tools = crate::tools::dir(data_dir);
    tokio::fs::create_dir_all(&tools)
        .await
        .context("建工具目录失败")?;
    for artifact in artifacts {
        let _ = tokio::fs::remove_file(part_path(&tools, &artifact.name)).await;
        let _ = tokio::fs::remove_file(unpack_path(&tools, &artifact.name)).await;
    }
    let total = artifacts.iter().map(|artifact| artifact.bytes).sum();

    let result = install_inner(client, base_url, artifacts, &tools, total, progress).await;
    if result.is_err() {
        for artifact in artifacts {
            let _ = tokio::fs::remove_file(part_path(&tools, &artifact.name)).await;
            let _ = tokio::fs::remove_file(unpack_path(&tools, &artifact.name)).await;
        }
    }
    result
}

async fn install_inner(
    client: &reqwest::Client,
    base_url: &str,
    artifacts: &[Artifact],
    tools: &Path,
    total: u64,
    progress: &(dyn Fn(&str, u64, Option<u64>) + Send + Sync),
) -> Result<()> {
    let mut downloaded = 0u64;
    for artifact in artifacts {
        download_artifact(
            client,
            base_url,
            artifact,
            tools,
            &mut downloaded,
            total,
            progress,
        )
        .await?;
    }

    for artifact in artifacts {
        unpack_artifact(artifact, tools).await?;
    }

    progress(PHASE_INSTALLING, downloaded, Some(total));
    for artifact in artifacts {
        verify_artifact(artifact, tools).await?;
    }

    for artifact in artifacts {
        tokio::fs::rename(
            unpack_path(tools, &artifact.name),
            tools.join(&artifact.name),
        )
        .await
        .with_context(|| format!("启用 {} 失败", artifact.name))?;
        let _ = tokio::fs::remove_file(part_path(tools, &artifact.name)).await;
    }
    Ok(())
}

async fn download_artifact(
    client: &reqwest::Client,
    base_url: &str,
    artifact: &Artifact,
    tools: &Path,
    downloaded: &mut u64,
    total: u64,
    progress: &(dyn Fn(&str, u64, Option<u64>) + Send + Sync),
) -> Result<()> {
    let url = url::Url::parse(&format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        artifact.file
    ))
    .context("下载地址不合法")?;
    ensure_allowed(&url)?;

    let mut response = client
        .get(url)
        .send()
        .await
        .context("连接下载源失败")?
        .error_for_status()
        .context("下载源返回错误状态")?;
    ensure_allowed(response.url())?;

    let part = part_path(tools, &artifact.name);
    let mut file = tokio::fs::File::create(&part)
        .await
        .context("建临时文件失败")?;
    let mut hasher = Sha256::new();
    let mut current = 0u64;
    while let Some(chunk) = response.chunk().await.context("读取下载正文失败")? {
        current += chunk.len() as u64;
        if current > MAX_BYTES {
            bail!("下载超过 200 MB");
        }
        *downloaded += chunk.len() as u64;
        hasher.update(&chunk);
        file.write_all(&chunk).await.context("写临时文件失败")?;
        progress(PHASE_DOWNLOADING, *downloaded, Some(total));
    }
    file.flush().await.context("写临时文件失败")?;
    drop(file);

    if current != artifact.bytes {
        bail!("下载大小不符：{} != {}", current, artifact.bytes);
    }
    if hex::encode(hasher.finalize()) != artifact.sha256 {
        bail!("SHA-256 校验失败");
    }
    progress(PHASE_VERIFYING, *downloaded, Some(total));
    Ok(())
}

async fn unpack_artifact(artifact: &Artifact, tools: &Path) -> Result<()> {
    let source = part_path(tools, &artifact.name);
    let target = unpack_path(tools, &artifact.name);
    tokio::task::spawn_blocking(move || -> Result<()> {
        let input = std::fs::File::open(&source).context("打开临时文件失败")?;
        let decoder = flate2::read::GzDecoder::new(std::io::BufReader::new(input));
        let mut output = std::fs::File::create(&target).context("建解压文件失败")?;
        let copied =
            std::io::copy(&mut decoder.take(MAX_BYTES + 1), &mut output).context("解压失败")?;
        if copied > MAX_BYTES {
            bail!("解压后超过 200 MB");
        }
        Ok(())
    })
    .await
    .context("解压任务失败")??;

    tokio::fs::set_permissions(
        unpack_path(tools, &artifact.name),
        std::fs::Permissions::from_mode(0o755),
    )
    .await
    .context("设置权限失败")?;
    Ok(())
}

async fn verify_artifact(artifact: &Artifact, tools: &Path) -> Result<()> {
    let unpack = unpack_path(tools, &artifact.name);
    let output = tokio::process::Command::new(&unpack)
        .arg("-version")
        .output()
        .await
        .with_context(|| format!("执行下载的 {} 失败", artifact.name))?;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout).contains(&format!("{} version", artifact.name))
    {
        bail!("下载的二进制不是可用的 {}", artifact.name);
    }
    Ok(())
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    Busy,
    Unsupported(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("安装任务正在进行"),
            Self::Unsupported(platform) => write!(formatter, "不支持此平台：{platform}"),
        }
    }
}

impl std::error::Error for StartError {}

#[derive(Debug)]
pub enum RemoveError {
    Busy,
    Failed(String),
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("安装任务正在进行"),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RemoveError {}

/// 单实例安装任务。
pub struct Installer {
    job: std::sync::Mutex<JobSnapshot>,
    running: std::sync::atomic::AtomicBool,
    base_url: std::sync::RwLock<String>,
    client: reqwest::Client,
    #[cfg(test)]
    pub(crate) artifacts: Option<Vec<Artifact>>,
}

impl Default for Installer {
    fn default() -> Self {
        Self::new()
    }
}

impl Installer {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(60))
            .user_agent(crate::state::DEFAULT_USER_AGENT)
            .build()
            .unwrap_or_default();
        Self {
            job: std::sync::Mutex::new(JobSnapshot::default()),
            running: std::sync::atomic::AtomicBool::new(false),
            base_url: std::sync::RwLock::new(base_url()),
            client,
            #[cfg(test)]
            artifacts: None,
        }
    }

    /// 覆盖下载前缀。测试与镜像使用。
    pub fn set_base_url(&self, base: impl Into<String>) {
        let mut guard = self
            .base_url
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *guard = base.into();
    }

    /// 最近一次任务；`idle` 时返回 `None`。
    pub fn job(&self) -> Option<JobSnapshot> {
        let job = self
            .job
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        (job.phase != "idle").then_some(job)
    }

    pub fn running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 删除面板安装的 ffmpeg。与安装共用同一个占位，避免 TOCTOU。
    pub fn remove(&self, data_dir: &Path) -> Result<bool, RemoveError> {
        if self
            .running
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err()
        {
            return Err(RemoveError::Busy);
        }
        let result = remove_managed(data_dir);
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        match result {
            Ok(removed) => {
                tracing::info!(removed, "删除面板 ffmpeg");
                Ok(removed)
            }
            Err(error) => {
                let message = format!("{error:#}");
                tracing::warn!(error = %message, "删除面板 ffmpeg 失败");
                Err(RemoveError::Failed(message))
            }
        }
    }

    /// 启动安装任务。立即返回初始快照。
    pub fn start(self: &Arc<Self>, data_dir: PathBuf) -> Result<JobSnapshot, StartError> {
        if self
            .running
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err()
        {
            return Err(StartError::Busy);
        }
        let Some(artifacts) = self.selected_artifacts() else {
            self.running
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Err(StartError::Unsupported(format!(
                "{}/{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )));
        };
        let base = self
            .base_url
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        tracing::info!(
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            count = artifacts.len(),
            base = %base,
            "开始安装 ffmpeg 与 ffprobe"
        );
        let snapshot = {
            let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
            *job = JobSnapshot {
                phase: PHASE_DOWNLOADING.into(),
                downloaded: 0,
                total: None,
                error: None,
                started_at: now_secs(),
                finished_at: None,
            };
            job.clone()
        };
        let installer = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = install(
                &installer.client,
                &base,
                &artifacts,
                &data_dir,
                &|phase, downloaded, total| installer.update(phase, downloaded, total),
            )
            .await;
            installer.finish(outcome);
        });
        Ok(snapshot)
    }

    fn selected_artifacts(&self) -> Option<Vec<Artifact>> {
        #[cfg(test)]
        if let Some(artifacts) = &self.artifacts {
            return Some(artifacts.clone());
        }
        artifacts_for(std::env::consts::OS, std::env::consts::ARCH)
    }

    fn update(&self, phase: &str, downloaded: u64, total: Option<u64>) {
        let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
        let changed = job.phase != phase;
        job.phase = phase.into();
        job.downloaded = downloaded;
        if total.is_some() {
            job.total = total;
        }
        drop(job);
        if changed {
            tracing::info!(phase, downloaded, total = ?total, "ffmpeg 安装阶段");
        }
    }

    fn finish(&self, outcome: anyhow::Result<()>) {
        {
            let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
            job.finished_at = Some(now_secs());
            match outcome {
                Ok(()) => {
                    tracing::info!("ffmpeg 安装完成");
                    job.phase = "done".into();
                    job.error = None;
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    tracing::warn!(error = %message, "ffmpeg 安装失败");
                    job.phase = "failed".into();
                    job.error = Some(message);
                }
            }
        }
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn fixture_artifact(name: &str, version: &str) -> (Artifact, Vec<u8>) {
        use flate2::{write::GzEncoder, Compression};
        use sha2::{Digest, Sha256};
        use std::io::Write;

        let script = format!("#!/bin/sh\necho \"{name} version {version}\"\n");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(script.as_bytes()).expect("压缩");
        let gz = encoder.finish().expect("压缩结束");
        let sha256 = hex::encode(Sha256::digest(&gz));
        (
            Artifact {
                name: name.into(),
                file: format!("{name}-test.gz"),
                bytes: gz.len() as u64,
                sha256,
            },
            gz,
        )
    }

    fn fixture_pair() -> Vec<(Artifact, Vec<u8>)> {
        vec![
            fixture_artifact("ffmpeg", "test"),
            fixture_artifact("ffprobe", "test"),
        ]
    }

    fn only_artifacts(fixtures: &[(Artifact, Vec<u8>)]) -> Vec<Artifact> {
        fixtures
            .iter()
            .map(|(artifact, _)| artifact.clone())
            .collect()
    }

    async fn serve(items: Vec<(String, Vec<u8>)>) -> String {
        let mut app = axum::Router::new();
        for (file, bytes) in items {
            let path = format!("/{file}");
            app = app.route(
                &path,
                axum::routing::get(move || {
                    let bytes = bytes.clone();
                    async move { bytes }
                }),
            );
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定端口");
        let addr = listener.local_addr().expect("取端口");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    async fn serve_fixtures(fixtures: &[(Artifact, Vec<u8>)]) -> String {
        serve(
            fixtures
                .iter()
                .map(|(artifact, bytes)| (artifact.file.clone(), bytes.clone()))
                .collect(),
        )
        .await
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tv-buddy-install-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    async fn serve_with_delay(bytes: Vec<u8>, delay_ms: u64) -> String {
        let app = axum::Router::new().fallback(move || {
            let bytes = bytes.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                bytes
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定端口");
        let addr = listener.local_addr().expect("取端口");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    #[test]
    fn artifact_table_includes_ffprobe_for_all_platforms() {
        for (os, arch) in [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("macos", "x86_64"),
            ("macos", "aarch64"),
        ] {
            let items = artifacts_for(os, arch).expect("平台");
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].name, "ffprobe");
            assert_eq!(items[1].name, "ffmpeg");
        }
        assert!(artifacts_for("windows", "x86_64").is_none());
    }

    #[test]
    fn remove_managed_removes_both_binaries() {
        let dir = temp_dir();
        let tools = crate::tools::dir(&dir);
        std::fs::create_dir_all(&tools).expect("建 tools");
        std::fs::write(tools.join("ffmpeg"), b"ffmpeg").expect("写 ffmpeg");
        std::fs::write(tools.join("ffprobe"), b"ffprobe").expect("写 ffprobe");
        assert!(remove_managed(&dir).expect("删除"));
        assert!(!tools.join("ffmpeg").exists());
        assert!(!tools.join("ffprobe").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn install_places_verified_binary() {
        let fixtures = fixture_pair();
        let base = serve_fixtures(&fixtures).await;
        let dir = temp_dir();
        let client = reqwest::Client::new();

        install(
            &client,
            &base,
            &only_artifacts(&fixtures),
            &dir,
            &|_, _, _| {},
        )
        .await
        .expect("安装");

        let tools = crate::tools::dir(&dir);
        for name in ["ffmpeg", "ffprobe"] {
            assert!(
                crate::tools::is_executable(&tools.join(name)),
                "{name} 必须可用"
            );
            assert!(!tools.join(format!(".{name}.part")).exists());
            assert!(!tools.join(format!(".{name}.unpack")).exists());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn install_rejects_hash_mismatch() {
        let mut fixtures = fixture_pair();
        fixtures[1].0.sha256 = "0".repeat(64);
        let base = serve_fixtures(&fixtures).await;
        let dir = temp_dir();
        let client = reqwest::Client::new();

        let error = install(
            &client,
            &base,
            &only_artifacts(&fixtures),
            &dir,
            &|_, _, _| {},
        )
        .await
        .expect_err("哈希必须失败");
        assert!(format!("{error:#}").contains("SHA-256"));
        let tools = crate::tools::dir(&dir);
        for name in ["ffmpeg", "ffprobe"] {
            assert!(!tools.join(name).exists());
            assert!(!tools.join(format!(".{name}.part")).exists());
            assert!(!tools.join(format!(".{name}.unpack")).exists());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn install_rejects_size_mismatch() {
        let mut fixtures = fixture_pair();
        fixtures[0].0.bytes += 1;
        let base = serve_fixtures(&fixtures).await;
        let dir = temp_dir();
        let client = reqwest::Client::new();

        let error = install(
            &client,
            &base,
            &only_artifacts(&fixtures),
            &dir,
            &|_, _, _| {},
        )
        .await
        .expect_err("大小必须失败");
        assert!(format!("{error:#}").contains("大小"));
        let tools = crate::tools::dir(&dir);
        for name in ["ffmpeg", "ffprobe"] {
            assert!(!tools.join(format!(".{name}.part")).exists());
            assert!(!tools.join(format!(".{name}.unpack")).exists());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn failed_install_keeps_old_binary() {
        let mut fixtures = fixture_pair();
        fixtures[0].0.sha256 = "0".repeat(64);
        let base = serve_fixtures(&fixtures).await;
        let dir = temp_dir();
        let tools = crate::tools::dir(&dir);
        std::fs::create_dir_all(&tools).expect("建 tools");
        std::fs::write(tools.join("ffmpeg"), b"old").expect("写旧 ffmpeg");
        std::fs::write(tools.join("ffprobe"), b"old").expect("写旧 ffprobe");

        install(
            &reqwest::Client::new(),
            &base,
            &only_artifacts(&fixtures),
            &dir,
            &|_, _, _| {},
        )
        .await
        .expect_err("必须失败");
        assert_eq!(
            std::fs::read(tools.join("ffmpeg")).expect("读旧文件"),
            b"old"
        );
        assert_eq!(
            std::fs::read(tools.join("ffprobe")).expect("读旧文件"),
            b"old"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn start_rejects_second_job() {
        let installer = Arc::new(Installer::new());
        let base = serve_with_delay(b"not-gzip".to_vec(), 1000).await;
        installer.set_base_url(base);
        let dir = temp_dir();

        installer.start(dir.clone()).expect("第一个任务");
        assert!(matches!(
            installer.start(dir.clone()),
            Err(StartError::Busy)
        ));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while installer.running() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!installer.running());
        let job = installer.job().expect("任务记录");
        assert_eq!(job.phase, "failed");
        assert!(!job.error.as_deref().unwrap_or("").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn finished_job_reports_done() {
        let fixtures = fixture_pair();
        let base = serve_fixtures(&fixtures).await;
        let dir = temp_dir();
        let mut installer = Arc::new(Installer::new());
        Arc::get_mut(&mut installer).expect("独占").artifacts = Some(only_artifacts(&fixtures));
        installer.set_base_url(base);

        installer.start(dir.clone()).expect("启动");
        let job = wait_job(&installer, "done").await;
        assert!(job.error.is_none());
        let tools = crate::tools::dir(&dir);
        for name in ["ffmpeg", "ffprobe"] {
            assert!(
                crate::tools::is_executable(&tools.join(name)),
                "{name} 必须可用"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn failed_job_reports_error() {
        let mut fixtures = fixture_pair();
        fixtures[1].0.sha256 = "0".repeat(64);
        let base = serve_fixtures(&fixtures).await;
        let dir = temp_dir();
        let mut installer = Arc::new(Installer::new());
        Arc::get_mut(&mut installer).expect("独占").artifacts = Some(only_artifacts(&fixtures));
        installer.set_base_url(base);

        installer.start(dir.clone()).expect("启动");
        let job = wait_job(&installer, "failed").await;
        assert!(job.error.as_deref().unwrap_or("").contains("SHA-256"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn url_rules_allow_https_and_loopback_http() {
        assert!(ensure_allowed(&url::Url::parse("https://example.com/x").expect("url")).is_ok());
        assert!(ensure_allowed(&url::Url::parse("http://127.0.0.1/x").expect("url")).is_ok());
        assert!(ensure_allowed(&url::Url::parse("http://localhost/x").expect("url")).is_ok());
        assert!(ensure_allowed(&url::Url::parse("http://example.com/x").expect("url")).is_err());
        assert!(ensure_allowed(&url::Url::parse("ftp://example.com/x").expect("url")).is_err());
    }

    #[tokio::test]
    async fn install_rejects_decompression_error_and_cleans_temp() {
        use sha2::{Digest, Sha256};

        let (ffmpeg, gz) = fixture_artifact("ffmpeg", "test");
        let bytes = b"not-gzip".to_vec();
        let ffprobe = Artifact {
            name: "ffprobe".into(),
            file: "ffprobe-test.gz".into(),
            bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
        };
        let base = serve(vec![
            (ffmpeg.file.clone(), gz),
            (ffprobe.file.clone(), bytes),
        ])
        .await;
        let dir = temp_dir();

        let error = install(
            &reqwest::Client::new(),
            &base,
            &[ffmpeg, ffprobe],
            &dir,
            &|_, _, _| {},
        )
        .await
        .expect_err("必须失败");
        assert!(format!("{error:#}").contains("解压"));
        let tools = crate::tools::dir(&dir);
        for name in ["ffmpeg", "ffprobe"] {
            assert!(!tools.join(name).exists());
            assert!(!tools.join(format!(".{name}.part")).exists());
            assert!(!tools.join(format!(".{name}.unpack")).exists());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn reinstall_replaces_existing_binary() {
        let fixtures = fixture_pair();
        let base = serve_fixtures(&fixtures).await;
        let dir = temp_dir();
        let tools = crate::tools::dir(&dir);
        std::fs::create_dir_all(&tools).expect("建 tools");
        std::fs::write(tools.join("ffmpeg"), b"old").expect("写旧 ffmpeg");
        std::fs::write(tools.join("ffprobe"), b"old").expect("写旧 ffprobe");

        install(
            &reqwest::Client::new(),
            &base,
            &only_artifacts(&fixtures),
            &dir,
            &|_, _, _| {},
        )
        .await
        .expect("重装");
        for name in ["ffmpeg", "ffprobe"] {
            let dest = tools.join(name);
            assert_ne!(std::fs::read(&dest).expect("读文件"), b"old");
            assert!(crate::tools::is_executable(&dest));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn remove_is_busy_while_installing() {
        let installer = Arc::new(Installer::new());
        let base = serve_with_delay(b"not-gzip".to_vec(), 1000).await;
        installer.set_base_url(base);
        let dir = temp_dir();

        installer.start(dir.clone()).expect("启动");
        assert!(matches!(installer.remove(&dir), Err(RemoveError::Busy)));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while installer.running() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!installer.running());
        installer.remove(&dir).expect("删除");
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
}
