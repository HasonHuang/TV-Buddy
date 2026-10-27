//! 备份与恢复。
//!
//! 备份是 gzip 压缩的 tar，根目录 `tv-buddy-backup/`（F-013 §8）。
//! 作用域：`all`（默认）｜`epg-scripts`｜`resolver`；其它值回退 `all`。
//! tar 读写由本模块内置，不引入第三方库。

mod tar;

use std::{
    fs,
    io::{Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};

use crate::{config::Config, logo};

/// 备份根目录。
const ROOT: &str = "tv-buddy-backup";

/// 恢复报告。
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub files: Vec<String>,
    pub skipped: Vec<String>,
}

/// 作用域归一化。
pub fn scope(value: &str) -> &'static str {
    match value {
        "epg-scripts" => "epg-scripts",
        "resolver" => "resolver",
        _ => "all",
    }
}

/// 生成 gzip(tar) 备份。
pub fn create(
    config_path: &Path,
    cache_dir: &str,
    include: &str,
    version: &str,
) -> Result<Vec<u8>> {
    let scope = scope(include);
    let cache = Path::new(cache_dir);
    let mut entries: Vec<tar::Entry> = Vec::new();

    if scope == "all" {
        push_file(&mut entries, config_path, &format!("{ROOT}/config.toml"))?;
        push_dir(
            &mut entries,
            &logo::dir(cache_dir),
            &format!("{ROOT}/logos"),
        )?;
    }
    if scope == "all" || scope == "epg-scripts" {
        push_dir(
            &mut entries,
            &cache.join("epg").join("scripts"),
            &format!("{ROOT}/epg/scripts"),
        )?;
        push_file(
            &mut entries,
            &cache.join("epg").join("epg_map.json"),
            &format!("{ROOT}/epg/epg_map.json"),
        )?;
    }
    if scope == "all" || scope == "resolver" {
        push_dir(
            &mut entries,
            &cache.join("resolvers"),
            &format!("{ROOT}/resolvers"),
        )?;
    }

    let manifest = serde_json::json!({
        "generator": "tv-buddy",
        "version": version,
        "created": now_secs(),
        "scope": scope,
    });
    entries.push(tar::Entry {
        name: format!("{ROOT}/manifest.json"),
        data: manifest.to_string().into_bytes(),
    });

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&tar::build(&entries))
        .context("写 tar 失败")?;
    encoder.finish().context("写 gzip 失败")
}

/// 文件名：`tv-buddy-backup-YYYYmmdd-HHMMSS.tar.gz`（UTC）。
pub fn file_name() -> String {
    let secs = now_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "tv-buddy-backup-{year:04}{month:02}{day:02}-{:02}{:02}{:02}.tar.gz",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// 解包并应用备份：先校验配置，再写 `config.toml.bak`，最后落盘台标。
///
/// 返回恢复后的配置与报告；调用方负责重载快照。
pub fn restore(config_path: &Path, cache_dir: &str, data: &[u8]) -> Result<(Config, Report)> {
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).context("解压失败")?;
    let entries = tar::parse(&raw).context("读取 tar 失败")?;

    let mut config_text: Option<String> = None;
    let mut logos: Vec<(String, Vec<u8>)> = Vec::new();
    let mut report = Report::default();

    for entry in entries {
        report.files.push(entry.name.clone());
        let name = entry
            .name
            .rsplit('/')
            .next()
            .unwrap_or(&entry.name)
            .to_string();
        if name == "config.toml" {
            config_text = Some(String::from_utf8(entry.data).context("配置不是 UTF-8")?);
            continue;
        }
        if let Some((_, file)) = entry.name.rsplit_once("/logos/") {
            let Some(ext) = file.rsplit_once('.').map(|(_, ext)| ext) else {
                report.skipped.push(entry.name);
                continue;
            };
            if logo::sanitize(file).is_none() || logo::check_ext(ext).is_none() {
                report.skipped.push(entry.name);
                continue;
            }
            logos.push((file.to_string(), entry.data));
        }
    }

    let Some(text) = config_text else {
        bail!("备份中没有 config.toml");
    };
    let config = crate::config::parse(&text).context("备份配置不合法")?;

    if config_path.is_file() {
        let backup = config_path.with_extension("toml.bak");
        fs::copy(config_path, &backup).with_context(|| format!("写 {}", backup.display()))?;
    }
    crate::config::save(config_path, &config).context("写入配置失败")?;

    for (name, bytes) in &logos {
        let base = name.rsplit_once('.').map(|(base, _)| base).unwrap_or(name);
        let ext = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
        if let Err(error) = logo::write(cache_dir, base, ext, bytes) {
            report.skipped.push(format!("logos/{name}: {error:#}"));
        }
    }

    Ok((config, report))
}

fn push_file(entries: &mut Vec<tar::Entry>, path: &Path, name: &str) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let data = fs::read(path).with_context(|| format!("读 {}", path.display()))?;
    entries.push(tar::Entry {
        name: name.to_string(),
        data,
    });
    Ok(())
}

fn push_dir(entries: &mut Vec<tar::Entry>, path: &Path, prefix: &str) -> Result<()> {
    let Ok(read) = fs::read_dir(path) else {
        return Ok(());
    };
    let mut names: Vec<String> = read
        .filter_map(|item| item.ok())
        .map(|item| item.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    for name in names {
        let full = path.join(&name);
        if full.is_file() {
            push_file(entries, &full, &format!("{prefix}/{name}"))?;
        }
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_falls_back_to_all() {
        assert_eq!(super::scope("epg-scripts"), "epg-scripts");
        assert_eq!(super::scope("resolver"), "resolver");
        assert_eq!(super::scope("all"), "all");
        assert_eq!(super::scope("其它"), "all");
    }

    #[test]
    fn file_name_has_timestamp_shape() {
        let name = file_name();
        assert!(name.starts_with("tv-buddy-backup-"));
        assert!(name.ends_with(".tar.gz"));
        assert_eq!(name.len(), "tv-buddy-backup-20260101-000000.tar.gz".len());
    }

    #[test]
    fn roundtrip_config_and_logo() {
        let dir = std::env::temp_dir().join(format!("backup-test-{}", uuid::Uuid::new_v4()));
        let cache = dir.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let config_path = dir.join("config.toml");
        let text = "[server]\nnot_found_url = \"http://a/404.mp4\"\n[cache]\ndisk_path = \"x\"\n";
        fs::write(&config_path, text).unwrap();
        logo::write(&cache.to_string_lossy(), "t1", "png", b"img").unwrap();

        let data = create(&config_path, &cache.to_string_lossy(), "all", "test").unwrap();
        assert_eq!(&data[..2], &[0x1f, 0x8b]);

        // 改动配置后恢复。
        fs::write(
            &config_path,
            "[server]\nnot_found_url = \"http://b/404.mp4\"\n",
        )
        .unwrap();
        let (config, report) = restore(&config_path, &cache.to_string_lossy(), &data).unwrap();
        assert!(report
            .files
            .iter()
            .any(|file| file.ends_with("config.toml")));
        assert!(report
            .files
            .iter()
            .any(|file| file.ends_with("logos/t1.png")));
        assert_eq!(config.server.not_found_url, "http://a/404.mp4");
        assert!(dir.join("config.toml.bak").is_file());
        assert!(logo::dir(&cache.to_string_lossy()).join("t1.png").is_file());

        // 非法数据。
        assert!(restore(&config_path, &cache.to_string_lossy(), b"not gzip").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
