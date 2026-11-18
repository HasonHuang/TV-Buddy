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

/// 当前平台对应的固定制品。
pub fn artifact_for(os: &str, arch: &str) -> Option<Artifact> {
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
        file: file.into(),
        bytes,
        sha256: sha256.into(),
    })
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

/// 删除面板安装的 ffmpeg 与临时文件。返回是否删除了正式文件。
pub fn remove_managed(data_dir: &Path) -> Result<bool> {
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

/// 下载、校验并启用 ffmpeg。
///
/// `progress` 收到阶段名、已下载字节与总字节（源未给 `Content-Length` 时为 `None`）。
pub(crate) async fn install(
    client: &reqwest::Client,
    base_url: &str,
    artifact: &Artifact,
    data_dir: &Path,
    progress: &(dyn Fn(&str, u64, Option<u64>) + Send + Sync),
) -> Result<()> {
    let tools = crate::tools::dir(data_dir);
    tokio::fs::create_dir_all(&tools)
        .await
        .context("建工具目录失败")?;
    let part = tools.join(PART);
    let unpack = tools.join(UNPACK);
    let _ = tokio::fs::remove_file(&part).await;
    let _ = tokio::fs::remove_file(&unpack).await;

    let result = install_inner(client, base_url, artifact, &tools, progress).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
        let _ = tokio::fs::remove_file(&unpack).await;
    }
    result
}

async fn install_inner(
    client: &reqwest::Client,
    base_url: &str,
    artifact: &Artifact,
    tools: &Path,
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

    let total = response.content_length();
    let part = tools.join(PART);
    let unpack = tools.join(UNPACK);
    let mut file = tokio::fs::File::create(&part)
        .await
        .context("建临时文件失败")?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    while let Some(chunk) = response.chunk().await.context("读取下载正文失败")? {
        downloaded += chunk.len() as u64;
        if downloaded > MAX_BYTES {
            bail!("下载超过 200 MB");
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.context("写临时文件失败")?;
        progress(PHASE_DOWNLOADING, downloaded, total);
    }
    file.flush().await.context("写临时文件失败")?;
    drop(file);

    if downloaded != artifact.bytes {
        bail!("下载大小不符：{} != {}", downloaded, artifact.bytes);
    }
    if hex::encode(hasher.finalize()) != artifact.sha256 {
        bail!("SHA-256 校验失败");
    }
    progress(PHASE_VERIFYING, downloaded, total);

    let source = part.clone();
    let target = unpack.clone();
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

    tokio::fs::set_permissions(&unpack, std::fs::Permissions::from_mode(0o755))
        .await
        .context("设置权限失败")?;
    progress(PHASE_INSTALLING, downloaded, total);

    let output = tokio::process::Command::new(&unpack)
        .arg("-version")
        .output()
        .await
        .context("执行下载的 ffmpeg 失败")?;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout).contains("ffmpeg version")
    {
        bail!("下载的二进制不是可用的 ffmpeg");
    }

    tokio::fs::rename(&unpack, tools.join(DEST))
        .await
        .context("启用 ffmpeg 失败")?;
    let _ = tokio::fs::remove_file(&part).await;
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

/// 单实例安装任务。
pub struct Installer {
    job: std::sync::Mutex<JobSnapshot>,
    running: std::sync::atomic::AtomicBool,
    base_url: std::sync::RwLock<String>,
    client: reqwest::Client,
    #[cfg(test)]
    pub(crate) artifact: Option<Artifact>,
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
            artifact: None,
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
        let Some(artifact) = self.selected_artifact() else {
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
                &artifact,
                &data_dir,
                &|phase, downloaded, total| installer.update(phase, downloaded, total),
            )
            .await;
            installer.finish(outcome);
        });
        Ok(snapshot)
    }

    fn selected_artifact(&self) -> Option<Artifact> {
        #[cfg(test)]
        if let Some(artifact) = &self.artifact {
            return Some(artifact.clone());
        }
        artifact_for(std::env::consts::OS, std::env::consts::ARCH)
    }

    fn update(&self, phase: &str, downloaded: u64, total: Option<u64>) {
        let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
        job.phase = phase.into();
        job.downloaded = downloaded;
        if total.is_some() {
            job.total = total;
        }
    }

    fn finish(&self, outcome: anyhow::Result<()>) {
        {
            let mut job = self.job.lock().unwrap_or_else(|error| error.into_inner());
            job.finished_at = Some(now_secs());
            match outcome {
                Ok(()) => {
                    job.phase = "done".into();
                    job.error = None;
                }
                Err(error) => {
                    job.phase = "failed".into();
                    job.error = Some(format!("{error:#}"));
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
    fn artifact_table_covers_four_platforms() {
        assert_eq!(
            artifact_for("linux", "x86_64").expect("linux amd64").file,
            "ffmpeg-linux-x64.gz"
        );
        assert_eq!(
            artifact_for("linux", "aarch64").expect("linux arm64").file,
            "ffmpeg-linux-arm64.gz"
        );
        assert_eq!(
            artifact_for("macos", "x86_64").expect("macos amd64").file,
            "ffmpeg-darwin-x64.gz"
        );
        assert_eq!(
            artifact_for("macos", "aarch64").expect("macos arm64").file,
            "ffmpeg-darwin-arm64.gz"
        );
        assert!(artifact_for("windows", "x86_64").is_none());
    }

    #[tokio::test]
    async fn install_places_verified_binary() {
        let (artifact, gz) = fixture_artifact();
        let base = serve(gz).await;
        let dir = temp_dir();
        let client = reqwest::Client::new();

        install(&client, &base, &artifact, &dir, &|_, _, _| {})
            .await
            .expect("安装");

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

        install(
            &reqwest::Client::new(),
            &base,
            &artifact,
            &dir,
            &|_, _, _| {},
        )
        .await
        .expect_err("必须失败");
        assert_eq!(
            std::fs::read(tools.join("ffmpeg")).expect("读旧文件"),
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
        let (artifact, gz) = fixture_artifact();
        let base = serve(gz).await;
        let dir = temp_dir();
        let mut installer = Arc::new(Installer::new());
        Arc::get_mut(&mut installer).expect("独占").artifact = Some(artifact);
        installer.set_base_url(base);

        installer.start(dir.clone()).expect("启动");
        let job = wait_job(&installer, "done").await;
        assert!(job.error.is_none());
        assert!(crate::tools::is_executable(
            &crate::tools::dir(&dir).join("ffmpeg")
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn failed_job_reports_error() {
        let (mut artifact, gz) = fixture_artifact();
        artifact.sha256 = "0".repeat(64);
        let base = serve(gz).await;
        let dir = temp_dir();
        let mut installer = Arc::new(Installer::new());
        Arc::get_mut(&mut installer).expect("独占").artifact = Some(artifact);
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
}
