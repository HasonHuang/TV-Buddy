//! `tv-buddy update`：从发布包替换当前二进制。
//!
//! 只替换二进制，不更新管理脚本、服务单元或容器镜像。
//! 容器内替换只对当前容器有效；生产升级请用 `docker compose pull`。

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DEFAULT_REPO: &str = "HasonHuang/TV-Buddy";
const USER_AGENT: &str = concat!("tv-buddy/", env!("TV_BUDDY_VERSION"));

/// `update` 的参数。
#[derive(Default)]
struct Options {
    check: bool,
    force: bool,
    restart: bool,
    tag: Option<String>,
    repo: Option<String>,
    archive: Option<String>,
}

impl Options {
    fn parse(raw: &[String]) -> Result<Self> {
        let mut options = Self::default();
        let mut index = 0;
        while index < raw.len() {
            match raw[index].as_str() {
                "--check" => options.check = true,
                "--force" => options.force = true,
                "--restart" => options.restart = true,
                "--version" | "--tag" => {
                    index += 1;
                    options.tag = Some(
                        raw.get(index)
                            .context("--version 缺少 tag")?
                            .to_string(),
                    );
                }
                "--repo" => {
                    index += 1;
                    options.repo = Some(
                        raw.get(index)
                            .context("--repo 缺少 owner/name")?
                            .to_string(),
                    );
                }
                "--archive" => {
                    index += 1;
                    options.archive = Some(
                        raw.get(index)
                            .context("--archive 缺少路径或 URL")?
                            .to_string(),
                    );
                }
                other => bail!("未知参数：{other}\n用法：tv-buddy update [--check] [--version <tag>] [--repo <owner/name>] [--archive <路径|URL>] [--restart] [--force]"),
            }
            index += 1;
        }
        Ok(options)
    }
}

/// 执行更新。
pub async fn run(raw: &[String]) -> Result<()> {
    let mut options = Options::parse(raw)?;
    if options.repo.is_none() {
        options.repo = std::env::var("TV_BUDDY_REPO")
            .ok()
            .filter(|v| !v.trim().is_empty());
    }
    if options.tag.is_none() {
        options.tag = std::env::var("TV_BUDDY_VERSION")
            .ok()
            .filter(|v| !v.trim().is_empty());
    }
    if options.archive.is_none() {
        options.archive = std::env::var("TV_BUDDY_ARCHIVE")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                std::env::var("TV_BUDDY_URL")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            });
    }

    let suffix = platform_suffix()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .context("创建 HTTP 客户端失败")?;

    let temp = TempDir::new()?;
    let archive_path;
    let sha_path;
    let new_version;

    if let Some(source) = &options.archive {
        let (path, sha) = fetch_archive(&client, source, &temp.0).await?;
        archive_path = path;
        sha_path = sha;
        new_version = options.tag.clone().unwrap_or_else(|| "发布包".to_string());
        if options.check {
            println!("当前版本：{}", crate::VERSION);
            println!("更新源：{source}");
            println!("本地/指定归档不查询最新版本；确认后去掉 --check 执行。");
            return Ok(());
        }
    } else {
        let repo = options
            .repo
            .clone()
            .unwrap_or_else(|| DEFAULT_REPO.to_string());
        let release = resolve_release(&client, &repo, options.tag.as_deref(), suffix).await?;
        new_version = release.tag.clone();

        if normalize(&new_version) == normalize(crate::VERSION) && !options.force {
            println!("已是最新版本：{}", crate::VERSION);
            return Ok(());
        }
        if options.check {
            println!("当前版本：{}", crate::VERSION);
            println!("最新版本：{}", new_version);
            println!("安装包：{}", release.asset_url);
            return Ok(());
        }

        archive_path = temp.0.join("pkg.tar.gz");
        let bytes = fetch(&client, &release.asset_url).await?;
        std::fs::write(&archive_path, &bytes).context("写入安装包失败")?;
        sha_path = match fetch_text(&client, &format!("{}.sha256", release.asset_url)).await {
            Ok(text) => {
                verify_sha(&bytes, &text)?;
                None
            }
            Err(_) => {
                eprintln!("警告：未取到 .sha256，跳过校验");
                None
            }
        };
    }

    if let Some(sha) = &sha_path {
        let bytes = std::fs::read(&archive_path).context("读取安装包失败")?;
        let text = std::fs::read_to_string(sha).context("读取 .sha256 失败")?;
        verify_sha(&bytes, &text)?;
    }

    let extract = temp.0.join("extract");
    std::fs::create_dir_all(&extract).context("创建解包目录失败")?;
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive_path)
        .arg("-C")
        .arg(&extract)
        .status()
        .context("运行 tar 失败")?;
    if !status.success() {
        bail!("解包失败：{}", archive_path.display());
    }
    let new_bin = find_binary(&extract).context("安装包内没有 tv-buddy")?;
    ensure_binary(&new_bin)?;

    let backup = replace_current(&new_bin)?;
    println!("已更新二进制：{} → {}", crate::VERSION, new_version);
    println!("旧二进制备份：{}", backup.display());
    println!("提示：只替换二进制。管理脚本与服务单元请用发布包内 install.sh。");

    if Path::new("/.dockerenv").exists() {
        eprintln!("警告：容器内替换只对当前容器有效。生产升级请用 docker compose pull。");
    }

    if options.restart {
        restart_service()?;
    } else {
        println!("重启服务后生效：");
        println!("  sudo systemctl restart tv-buddy");
        println!("  或 sudo rc-service tv-buddy restart");
        println!("  容器内：docker compose restart");
    }
    Ok(())
}

struct Release {
    tag: String,
    asset_url: String,
}

async fn resolve_release(
    client: &reqwest::Client,
    repo: &str,
    tag: Option<&str>,
    suffix: &str,
) -> Result<Release> {
    let base = format!("https://api.github.com/repos/{repo}/releases");
    let wanted = tag
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "latest");
    let url = match wanted {
        Some(tag) => format!("{base}/tags/{tag}"),
        None => format!("{base}/latest"),
    };
    let value = match get_json(client, &url).await {
        Ok(value) => value,
        Err(error) if wanted.is_none() => {
            eprintln!("获取最新正式版失败：{error:#}");
            Value::Null
        }
        Err(error) => return Err(error),
    };
    if let Some(release) = pick_release(&value, suffix) {
        return Ok(release);
    }
    if wanted.is_none() {
        // `releases/latest` 只认正式版；只有预发布时退回最新一条。
        let list = get_json(client, &format!("{base}?per_page=30")).await?;
        if let Some(release) = pick_release(&list, suffix) {
            eprintln!("未找到正式版，改用最新 Release：{}", release.tag);
            return Ok(release);
        }
    }
    bail!("Release 中没有当前平台的安装包（{suffix}）")
}

fn pick_release(value: &Value, suffix: &str) -> Option<Release> {
    let releases: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![value],
        _ => return None,
    };
    for release in releases {
        let Some(tag) = release.get("tag_name").and_then(Value::as_str) else {
            continue;
        };
        let Some(assets) = release.get("assets").and_then(Value::as_array) else {
            continue;
        };
        for asset in assets {
            let name = asset
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !name.ends_with(suffix) {
                continue;
            }
            let Some(url) = asset.get("browser_download_url").and_then(Value::as_str) else {
                continue;
            };
            return Some(Release {
                tag: tag.to_string(),
                asset_url: url.to_string(),
            });
        }
    }
    None
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value> {
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .with_context(|| format!("请求 GitHub API 失败：{url}"))?;
    if !response.status().is_success() {
        bail!(
            "GitHub API 返回 HTTP {}：{url}（可能触发限流）",
            response.status()
        );
    }
    let text = response.text().await.context("读取 GitHub API 响应失败")?;
    serde_json::from_str(&text).context("解析 GitHub API 响应失败")
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .with_context(|| format!("下载失败：{url}"))?;
    if !response.status().is_success() {
        bail!("下载失败：{url}（HTTP {}）", response.status());
    }
    Ok(response.bytes().await.context("读取下载内容失败")?.to_vec())
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let bytes = fetch(client, url).await?;
    String::from_utf8(bytes).context(".sha256 不是 UTF-8")
}

async fn fetch_archive(
    client: &reqwest::Client,
    source: &str,
    dir: &Path,
) -> Result<(PathBuf, Option<PathBuf>)> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let path = dir.join("pkg.tar.gz");
        let bytes = fetch(client, source).await?;
        std::fs::write(&path, &bytes).context("写入安装包失败")?;
        let sha = match fetch_text(client, &format!("{source}.sha256")).await {
            Ok(text) => {
                verify_sha(&bytes, &text)?;
                None
            }
            Err(_) => {
                eprintln!("警告：未取到 .sha256，跳过校验");
                None
            }
        };
        return Ok((path, sha));
    }
    let path = PathBuf::from(source);
    if !path.is_file() {
        bail!("安装包不存在：{source}");
    }
    let sha = path.with_extension(format!(
        "{}.sha256",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
    ));
    let sha = if sha.is_file() {
        Some(sha)
    } else {
        eprintln!("警告：未找到 {}.sha256，跳过校验", path.display());
        None
    };
    Ok((path, sha))
}

fn verify_sha(bytes: &[u8], text: &str) -> Result<()> {
    let expected = text.split_whitespace().next().context(".sha256 文件为空")?;
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("SHA-256 校验失败：期望 {expected}，实际 {actual}");
    }
    Ok(())
}

fn platform_suffix() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("-linux-amd64.tar.gz"),
        ("linux", "aarch64") => Ok("-linux-arm64.tar.gz"),
        ("macos", "aarch64") => Ok("-macos-arm64.tar.gz"),
        (os, arch) => bail!("不支持的平台：{os}/{arch}。只支持 Linux amd64/arm64 与 macOS arm64"),
    }
}

fn normalize(version: &str) -> &str {
    version.trim().trim_start_matches('v')
}

fn find_binary(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary(&path) {
                return Some(found);
            }
        } else if path.file_name().and_then(|name| name.to_str()) == Some("tv-buddy") {
            return Some(path);
        }
    }
    None
}

fn ensure_binary(path: &Path) -> Result<()> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .with_context(|| format!("打开安装包内二进制失败：{}", path.display()))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .with_context(|| format!("读取文件头失败：{}", path.display()))?;
    let ok = magic == [0x7f, b'E', b'L', b'F']
        || matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe] | [0xfe, 0xed, 0xfa, 0xcf] | [0xca, 0xfe, 0xba, 0xbe]
        );
    if !ok {
        bail!("不是可执行二进制：{}", path.display());
    }
    Ok(())
}

fn replace_current(new_bin: &Path) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("取当前可执行文件路径失败")?;
    let dir = exe.parent().context("当前可执行文件没有父目录")?;
    let pid = std::process::id();
    let staged = suffix_path(&exe, &format!(".new.{pid}"));
    let backup = suffix_path(&exe, ".bak");

    std::fs::copy(new_bin, &staged).with_context(|| {
        format!(
            "无法写入 {}。请用 root 运行；容器内请用 docker compose pull",
            dir.display()
        )
    })?;
    set_executable(&staged)?;
    std::fs::copy(&exe, &backup)
        .with_context(|| format!("备份旧二进制失败：{}", backup.display()))?;
    if let Err(error) = std::fs::rename(&staged, &exe) {
        let _ = std::fs::remove_file(&staged);
        return Err(error).with_context(|| format!("替换 {} 失败", exe.display()));
    }
    Ok(backup)
}

fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("设置执行权限失败：{}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn restart_service() -> Result<()> {
    if Path::new("/run/systemd/system").exists() && command_exists("systemctl") {
        let status = Command::new("systemctl")
            .args(["restart", "tv-buddy"])
            .status()
            .context("运行 systemctl 失败")?;
        if status.success() {
            println!("已重启 tv-buddy（systemd）");
            return Ok(());
        }
        bail!("systemctl restart tv-buddy 失败；请用 root 运行");
    }
    if command_exists("rc-service") {
        let status = Command::new("rc-service")
            .args(["tv-buddy", "restart"])
            .status()
            .context("运行 rc-service 失败")?;
        if status.success() {
            println!("已重启 tv-buddy（OpenRC）");
            return Ok(());
        }
        bail!("rc-service tv-buddy restart 失败；请用 root 运行");
    }
    if Path::new("/.dockerenv").exists() {
        println!("容器内请重启容器：docker compose restart");
        return Ok(());
    }
    println!("未检测到 systemd/OpenRC；请手动重启服务。");
    Ok(())
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let path = dir.join(name);
                path.is_file()
            })
        })
        .unwrap_or(false)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("tv-buddy-update-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)
            .with_context(|| format!("创建临时目录失败：{}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_asset_for_platform() {
        let value = serde_json::json!({
            "tag_name": "v2026.10",
            "assets": [
                {"name": "tv-buddy-2026.10-linux-amd64.tar.gz", "browser_download_url": "https://example/amd64"},
                {"name": "tv-buddy-2026.10-linux-arm64.tar.gz", "browser_download_url": "https://example/arm64"}
            ]
        });
        let release = pick_release(&value, "-linux-arm64.tar.gz").expect("资产");
        assert_eq!(release.tag, "v2026.10");
        assert_eq!(release.asset_url, "https://example/arm64");
    }

    #[test]
    fn picks_first_release_with_asset() {
        let value = serde_json::json!([
            {"tag_name": "v1", "assets": [{"name": "other.tar.gz"}]},
            {"tag_name": "v2", "assets": [{"name": "tv-buddy-2-linux-amd64.tar.gz", "browser_download_url": "https://example/v2"}]}
        ]);
        let release = pick_release(&value, "-linux-amd64.tar.gz").expect("资产");
        assert_eq!(release.tag, "v2");
    }

    #[test]
    fn sha256_matches_and_rejects() {
        let text = format!("{}  pkg.tar.gz\n", hex::encode(Sha256::digest(b"abc")));
        verify_sha(b"abc", &text).expect("校验");
        assert!(verify_sha(b"abd", &text).is_err());
    }

    #[test]
    fn version_normalization() {
        assert_eq!(normalize("v2026.10"), "2026.10");
        assert_eq!(normalize(" 2026.10 "), "2026.10");
    }

    #[test]
    fn finds_binary_in_nested_directory() {
        let dir = std::env::temp_dir().join(format!("tv-buddy-find-{}", uuid::Uuid::new_v4()));
        let nested = dir.join("pkg/bin");
        std::fs::create_dir_all(&nested).expect("建目录");
        let bin = nested.join("tv-buddy");
        std::fs::write(&bin, b"x").expect("写文件");
        assert_eq!(find_binary(&dir).as_deref(), Some(bin.as_path()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
