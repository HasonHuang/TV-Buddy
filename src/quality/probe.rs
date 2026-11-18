//! 质量探测：HTTP 读取、HLS 解析与测速。

use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::{
    header::{HeaderName, HeaderValue, CONTENT_TYPE},
    Client, RequestBuilder, Response,
};
use url::Url;

use crate::quality::rules::Measurement;

/// 探测档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeMode {
    Reachability,
    Playlist,
    Speed,
    Full,
}

/// 单次探测的上限。
#[derive(Debug, Clone)]
pub struct ProbeLimits {
    pub timeout: Duration,
    pub speed_timeout: Duration,
    pub max_playlist_bytes: usize,
    pub max_sample_bytes: u64,
    pub max_sample_secs: f64,
    pub sample_segments: usize,
    pub ffprobe: bool,
    pub ffprobe_ready: bool,
    pub ffprobe_path: std::path::PathBuf,
    pub ffprobe_timeout: Duration,
    pub ffprobe_sample_bytes: usize,
    pub ffprobe_min_speed_mib: f64,
}

/// 单轮探测的并发与次数句柄。
pub struct ProbeGates {
    pub probe_semaphore: Arc<tokio::sync::Semaphore>,
    pub ffprobe_counter: Arc<std::sync::atomic::AtomicU64>,
    pub max_ffprobe_per_run: u64,
}

impl ProbeGates {
    pub fn unlimited() -> Self {
        Self {
            probe_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
            ffprobe_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            max_ffprobe_per_run: 0,
        }
    }
}

/// 探测输出。
#[derive(Debug, Clone)]
pub struct ProbeOutput {
    pub measurement: Measurement,
    pub bytes: u64,
    pub duration_ms: u64,
}

/// 主清单中的一个变体。
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub url: String,
    pub bandwidth: u64,
    pub width: Option<u16>,
    pub height: Option<u16>,
}

/// 解析主清单。选择最高码率；有上限时优先选择不超上限的最高码率。
pub fn parse_master(body: &str, base: &str, max_resolution_px: Option<u32>) -> Option<Variant> {
    let base = Url::parse(base).ok()?;
    let mut variants = Vec::new();
    let mut pending: Option<(u64, Option<u16>, Option<u16>)> = None;

    for line in body.lines() {
        let line = line.trim();
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let (bandwidth, width, height) = parse_stream_inf(attrs);
            pending = Some((bandwidth, width, height));
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((bandwidth, width, height)) = pending.take() else {
            continue;
        };
        let Ok(url) = base.join(line) else {
            continue;
        };
        variants.push(Variant {
            url: url.to_string(),
            bandwidth,
            width,
            height,
        });
    }

    if variants.is_empty() {
        return None;
    }

    let within = max_resolution_px.map(|max| {
        variants
            .iter()
            .filter(|variant| match (variant.width, variant.height) {
                (Some(width), Some(height)) => u32::from(width) * u32::from(height) <= max,
                _ => true,
            })
            .cloned()
            .collect::<Vec<_>>()
    });

    let candidates = within.filter(|items| !items.is_empty()).unwrap_or(variants);
    candidates
        .into_iter()
        .max_by_key(|variant| variant.bandwidth)
}

fn parse_stream_inf(attrs: &str) -> (u64, Option<u16>, Option<u16>) {
    let mut bandwidth = 0;
    let mut width = None;
    let mut height = None;
    for item in attrs.split(',') {
        let Some((key, value)) = item.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "BANDWIDTH" => bandwidth = value.parse().unwrap_or(0),
            "RESOLUTION" => {
                if let Some((w, h)) = value.split_once('x') {
                    width = w.trim().parse().ok();
                    height = h.trim().parse().ok();
                }
            }
            _ => {}
        }
    }
    (bandwidth, width, height)
}

/// 解析媒体清单中的切片地址。跳过最后一个切片，避免取到未写完的切片。
///
/// 只保留末尾 `limit + 1` 个地址，避免长清单占用过多内存。
pub fn parse_media_segments(body: &str, base: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let Ok(base) = Url::parse(base) else {
        return Vec::new();
    };
    let mut urls: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for line in body.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Ok(url) = base.join(line) else {
            continue;
        };
        urls.push_back(url.to_string());
        if urls.len() > limit.saturating_add(1) {
            urls.pop_front();
        }
    }
    if urls.len() > 1 {
        urls.pop_back();
    }
    urls.into_iter().collect()
}

fn with_headers(mut request: RequestBuilder, headers: &[(String, String)]) -> RequestBuilder {
    for (name, value) in headers {
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(value) else {
            continue;
        };
        request = request.header(name, value);
    }
    request
}

async fn read_bounded(
    mut response: Response,
    max_bytes: usize,
    max_secs: f64,
) -> Result<(Vec<u8>, Duration), String> {
    let started = Instant::now();
    let mut body = Vec::new();
    loop {
        if body.len() >= max_bytes || started.elapsed().as_secs_f64() >= max_secs {
            break;
        }
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = max_bytes.saturating_sub(body.len());
                if chunk.len() > remaining {
                    body.extend_from_slice(&chunk[..remaining]);
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok((body, started.elapsed()))
}

/// 首块读出后的判定结果。连续流不保留正文，只计数。
enum ProbeBody {
    Hls(Vec<u8>),
    Stream { bytes: u64, elapsed: Duration },
}

fn starts_hls(chunk: &[u8]) -> bool {
    String::from_utf8_lossy(chunk)
        .trim_start()
        .starts_with("#EXTM3U")
}

/// 读首块判定 HLS。HLS 保留正文到清单上限；连续流只累计字节数。
async fn read_probe_body(
    mut response: Response,
    hls_hint: bool,
    playlist_limit: usize,
    stream_limit: usize,
    max_secs: f64,
) -> Result<ProbeBody, String> {
    let started = Instant::now();
    let mut head = Vec::new();
    let mut total = 0u64;
    let mut decided = hls_hint;
    let mut is_hls = hls_hint;
    loop {
        let limit = if is_hls { playlist_limit } else { stream_limit } as u64;
        if total >= limit || started.elapsed().as_secs_f64() >= max_secs {
            break;
        }
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if !decided {
                    is_hls = hls_hint || starts_hls(&chunk);
                    decided = true;
                }
                let limit = if is_hls { playlist_limit } else { stream_limit } as u64;
                let remaining = limit.saturating_sub(total) as usize;
                let take = chunk.len().min(remaining);
                total = total.saturating_add(take as u64);
                if is_hls {
                    head.extend_from_slice(&chunk[..take]);
                }
                if take < chunk.len() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    if is_hls {
        Ok(ProbeBody::Hls(head))
    } else {
        Ok(ProbeBody::Stream {
            bytes: total,
            elapsed: started.elapsed(),
        })
    }
}

/// 只读不存。用于测速样本。可选采集切片开头，供 ffprobe 使用。
async fn read_counted(
    mut response: Response,
    max_bytes: usize,
    max_secs: f64,
    capture: Option<&mut Vec<u8>>,
    capture_limit: usize,
) -> Result<(u64, Duration), String> {
    let started = Instant::now();
    let mut total = 0u64;
    let mut capture = capture;
    loop {
        if total >= max_bytes as u64 || started.elapsed().as_secs_f64() >= max_secs {
            break;
        }
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = max_bytes.saturating_sub(total as usize);
                let take = chunk.len().min(remaining);
                total = total.saturating_add(take as u64);
                if let Some(buffer) = capture.as_deref_mut() {
                    if capture_limit > 0 && buffer.len() < capture_limit {
                        let room = capture_limit.saturating_sub(buffer.len());
                        let copied = take.min(room);
                        buffer.extend_from_slice(&chunk[..copied]);
                    }
                }
                if take < chunk.len() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok((total, started.elapsed()))
}

fn content_type(response: &Response) -> String {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 去掉地址末尾的 `$描述` 段。上游只认基地址。
pub fn clean_url(url: &str) -> &str {
    url.split('$').next().unwrap_or(url).trim()
}

pub(crate) fn is_dash(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".mpd")
}

fn looks_like_hls(url: &str, content_type: &str) -> bool {
    url.to_ascii_lowercase().contains(".m3u8")
        || content_type.contains("mpegurl")
        || content_type.contains("x-mpegurl")
}

fn probe_error(message: impl Into<String>, reachable: bool) -> Measurement {
    Measurement {
        reachable,
        probe_error: true,
        http_status: None,
        delay_ms: None,
        speed_mib: None,
        width: None,
        height: None,
        error: Some(message.into()),
    }
}

/// 判断字节是否以 MPEG-TS 同步字节开头，并在每 188 字节处重复。
fn is_mpegts(bytes: &[u8]) -> bool {
    const PACKET: usize = 188;
    bytes.len() > PACKET * 2
        && [0, PACKET, PACKET * 2]
            .iter()
            .all(|offset| bytes[*offset] == 0x47)
}

/// 取第一个有效的视频流宽高。
fn parse_ffprobe_json(stdout: &[u8]) -> Option<(u16, u16)> {
    let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    value.get("streams")?.as_array()?.iter().find_map(|stream| {
        if stream.get("codec_type")?.as_str()? != "video" {
            return None;
        }
        let width = stream.get("width")?.as_u64()?;
        let height = stream.get("height")?.as_u64()?;
        (width > 0 && height > 0 && width <= u16::MAX as u64 && height <= u16::MAX as u64)
            .then(|| (width as u16, height as u16))
    })
}

/// 预留一次 ffprobe 尝试。`max` 为 0 表示不限制。
fn reserve_ffprobe_attempt(counter: &std::sync::atomic::AtomicU64, max: u64) -> bool {
    use std::sync::atomic::Ordering;
    if max == 0 {
        return true;
    }
    let previous = counter.fetch_add(1, Ordering::SeqCst);
    if previous >= max {
        counter.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}

/// 用采集的切片字节运行 ffprobe，识别分辨率。字节只经标准输入传递。
async fn detect_resolution(
    path: &std::path::Path,
    bytes: &[u8],
    timeout: Duration,
) -> Result<(u16, u16), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut command = tokio::process::Command::new(path);
    command.arg("-v").arg("error");
    if is_mpegts(bytes) {
        command.arg("-f").arg("mpegts");
    }
    command
        .arg("-probesize")
        .arg("512K")
        .arg("-analyzeduration")
        .arg("1000000")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=codec_type,width,height")
        .arg("-of")
        .arg("json")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "ffprobe 标准输入不可用".to_string())?;

    // 超时覆盖写入与等待。子进程不读标准输入时，写入不会永久阻塞。
    let output = tokio::time::timeout(timeout, async move {
        stdin
            .write_all(bytes)
            .await
            .map_err(|error| error.to_string())?;
        drop(stdin);
        child
            .wait_with_output()
            .await
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "ffprobe 超时".to_string())??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let summary: String = stderr.trim().chars().take(4096).collect();
        return Err(format!("ffprobe 失败: {summary}"));
    }
    parse_ffprobe_json(&output.stdout).ok_or_else(|| "ffprobe 无视频流".to_string())
}

/// 探测一个地址。
pub async fn probe(
    client: &Client,
    url: &str,
    headers: &[(String, String)],
    mode: ProbeMode,
    max_resolution_px: Option<u32>,
    limits: &ProbeLimits,
    gates: &ProbeGates,
) -> ProbeOutput {
    let started = Instant::now();
    let url = clean_url(url);
    let mut total_bytes = 0u64;
    let effective_mode = if mode == ProbeMode::Full && !limits.ffprobe {
        ProbeMode::Speed
    } else {
        mode
    };
    let mut measurement = Measurement {
        reachable: false,
        probe_error: false,
        http_status: None,
        delay_ms: None,
        speed_mib: None,
        width: None,
        height: None,
        error: None,
    };

    // P1 不解析 DASH。连通档只检查 HTTP，仍可判定地址是否在线。
    if is_dash(url) && mode != ProbeMode::Reachability {
        return ProbeOutput {
            measurement: probe_error("DASH 清单暂不支持", true),
            bytes: 0,
            duration_ms: started.elapsed().as_millis() as u64,
        };
    }

    let request = with_headers(client.get(url).timeout(limits.timeout), headers);
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            measurement.error = Some(error.to_string());
            return ProbeOutput {
                measurement,
                bytes: 0,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
    };

    measurement.http_status = Some(response.status().as_u16());
    if !response.status().is_success() {
        measurement.error = Some(format!("HTTP {}", response.status()));
        return ProbeOutput {
            measurement,
            bytes: 0,
            duration_ms: started.elapsed().as_millis() as u64,
        };
    }

    measurement.reachable = true;
    measurement.delay_ms = Some(started.elapsed().as_millis() as u32);
    let final_url = response.url().clone();
    let content_type = content_type(&response);

    if mode == ProbeMode::Reachability {
        return ProbeOutput {
            measurement,
            bytes: 0,
            duration_ms: started.elapsed().as_millis() as u64,
        };
    }

    let hls = looks_like_hls(url, &content_type);
    let stream_limit = if mode == ProbeMode::Playlist {
        limits.max_playlist_bytes
    } else {
        limits.max_sample_bytes as usize
    };
    let read_secs = if hls || mode == ProbeMode::Playlist {
        limits.timeout.as_secs_f64()
    } else {
        limits.max_sample_secs
    };

    let body = match read_probe_body(
        response,
        hls,
        limits.max_playlist_bytes,
        stream_limit,
        read_secs,
    )
    .await
    {
        Ok(ProbeBody::Hls(body)) => body,
        Ok(ProbeBody::Stream { bytes, elapsed }) => {
            total_bytes = total_bytes.saturating_add(bytes);
            if mode == ProbeMode::Playlist {
                return ProbeOutput {
                    measurement,
                    bytes: total_bytes,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
            if bytes == 0 {
                measurement.reachable = false;
                measurement.error = Some("空响应".to_string());
                return ProbeOutput {
                    measurement,
                    bytes: total_bytes,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
            measurement.speed_mib = Some(if elapsed > Duration::ZERO {
                bytes as f64 / elapsed.as_secs_f64() / 1024.0 / 1024.0
            } else {
                0.0
            });
            return ProbeOutput {
                measurement,
                bytes: total_bytes,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
        Err(error) => {
            measurement.reachable = false;
            measurement.error = Some(error);
            return ProbeOutput {
                measurement,
                bytes: total_bytes,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
    };
    total_bytes = total_bytes.saturating_add(body.len() as u64);

    let body_text = String::from_utf8_lossy(&body);

    let (media_url, media_body, variant_width, variant_height) = if body_text
        .contains("#EXT-X-STREAM-INF")
    {
        let Some(variant) = parse_master(&body_text, final_url.as_str(), max_resolution_px) else {
            measurement = probe_error("主清单没有可用变体", true);
            return ProbeOutput {
                measurement,
                bytes: total_bytes,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        };
        let request = with_headers(
            client.get(&variant.url).timeout(limits.speed_timeout),
            headers,
        );
        let response = match request.send().await {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                measurement = probe_error(format!("变体 HTTP {}", response.status()), true);
                return ProbeOutput {
                    measurement,
                    bytes: total_bytes,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
            Err(error) => {
                measurement.reachable = false;
                measurement.error = Some(error.to_string());
                return ProbeOutput {
                    measurement,
                    bytes: total_bytes,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
        };
        let (media, _) = match read_bounded(
            response,
            limits.max_playlist_bytes,
            limits.timeout.as_secs_f64(),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                measurement.reachable = false;
                measurement.error = Some(error);
                return ProbeOutput {
                    measurement,
                    bytes: total_bytes,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
        };
        total_bytes = total_bytes.saturating_add(media.len() as u64);
        (
            variant.url.clone(),
            String::from_utf8_lossy(&media).to_string(),
            variant.width,
            variant.height,
        )
    } else {
        (final_url.to_string(), body_text.to_string(), None, None)
    };

    // 变体与切片地址已提取。释放清单正文，降低并发探测的峰值内存。
    drop(body_text);
    drop(body);
    measurement.width = variant_width;
    measurement.height = variant_height;
    let segments = parse_media_segments(&media_body, &media_url, limits.sample_segments);
    if segments.is_empty() {
        measurement = probe_error("媒体清单没有可用切片", true);
        return ProbeOutput {
            measurement,
            bytes: total_bytes,
            duration_ms: started.elapsed().as_millis() as u64,
        };
    }

    if mode == ProbeMode::Playlist {
        let request = with_headers(
            client.get(&segments[0]).timeout(limits.speed_timeout),
            headers,
        );
        return match request.send().await {
            Ok(response) if response.status().is_success() => ProbeOutput {
                measurement,
                bytes: total_bytes,
                duration_ms: started.elapsed().as_millis() as u64,
            },
            Ok(response) => {
                measurement.reachable = false;
                measurement.error = Some(format!("切片 HTTP {}", response.status()));
                ProbeOutput {
                    measurement,
                    bytes: total_bytes,
                    duration_ms: started.elapsed().as_millis() as u64,
                }
            }
            Err(error) => {
                measurement.reachable = false;
                measurement.error = Some(error.to_string());
                ProbeOutput {
                    measurement,
                    bytes: total_bytes,
                    duration_ms: started.elapsed().as_millis() as u64,
                }
            }
        };
    }

    let mut sample_bytes = 0u64;
    let mut sample_time = Duration::ZERO;
    let mut first_delay = None;
    let mut capture_permit = None;
    let mut capture: Option<Vec<u8>> = None;
    let mut captured: Option<Vec<u8>> = None;
    let geometry_unknown = measurement.width.is_none() || measurement.height.is_none();
    if effective_mode == ProbeMode::Full
        && limits.ffprobe
        && limits.ffprobe_ready
        && geometry_unknown
        && reserve_ffprobe_attempt(&gates.ffprobe_counter, gates.max_ffprobe_per_run)
    {
        if let Ok(permit) = gates.probe_semaphore.clone().try_acquire_owned() {
            capture_permit = Some(permit);
            capture = Some(Vec::new());
        }
    }
    for segment in segments {
        let request = with_headers(client.get(&segment).timeout(limits.speed_timeout), headers);
        let segment_started = Instant::now();
        let response = match request.send().await {
            Ok(response) if response.status().is_success() => response,
            _ => continue,
        };
        if first_delay.is_none() {
            first_delay = Some(segment_started.elapsed().as_millis() as u32);
        }
        let slot = if captured.is_none() {
            capture.as_mut()
        } else {
            None
        };
        let (bytes, elapsed) = match read_counted(
            response,
            limits.max_sample_bytes as usize,
            limits.max_sample_secs,
            slot,
            limits.ffprobe_sample_bytes,
        )
        .await
        {
            Ok(value) => value,
            Err(_) => continue,
        };
        if capture.as_ref().is_some_and(|buffer| !buffer.is_empty()) {
            captured = capture.take();
        }
        sample_bytes = sample_bytes.saturating_add(bytes);
        sample_time = sample_time.saturating_add(elapsed);
    }
    total_bytes = total_bytes.saturating_add(sample_bytes);
    if sample_bytes == 0 || sample_time.is_zero() {
        // 清单可读但所有样本切片都失败：按探测错误处理，不判不可达。
        measurement = probe_error("切片全部失败", true);
    } else {
        measurement.speed_mib =
            Some(sample_bytes as f64 / sample_time.as_secs_f64() / 1024.0 / 1024.0);
        measurement.delay_ms = first_delay.or(measurement.delay_ms);
    }

    if let Some(bytes) = captured {
        let speed_ok = measurement
            .speed_mib
            .is_some_and(|speed| speed >= limits.ffprobe_min_speed_mib);
        if speed_ok && !bytes.is_empty() {
            match detect_resolution(&limits.ffprobe_path, &bytes, limits.ffprobe_timeout).await {
                Ok((width, height)) => {
                    measurement.width = measurement.width.or(Some(width));
                    measurement.height = measurement.height.or(Some(height));
                    tracing::debug!(source = "ffprobe", width, height, "分辨率识别完成");
                }
                Err(error) => {
                    tracing::debug!(source = "unknown", %error, "分辨率识别失败");
                }
            }
        }
    }
    drop(capture_permit);

    ProbeOutput {
        measurement,
        bytes: total_bytes,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    impl ProbeLimits {
        pub(crate) fn for_test() -> Self {
            Self {
                timeout: Duration::from_secs(2),
                speed_timeout: Duration::from_secs(2),
                max_playlist_bytes: 64 * 1024,
                max_sample_bytes: 64 * 1024,
                max_sample_secs: 1.0,
                sample_segments: 2,
                ffprobe: false,
                ffprobe_ready: false,
                ffprobe_path: std::path::PathBuf::from("ffprobe"),
                ffprobe_timeout: Duration::from_secs(2),
                ffprobe_sample_bytes: 64 * 1024,
                ffprobe_min_speed_mib: 0.0,
            }
        }
    }

    #[test]
    fn master_picks_highest_bandwidth_and_reads_resolution() {
        let body = "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=1280x720\n\
low/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1920x1080\n\
high/index.m3u8\n";
        let variant =
            parse_master(body, "http://example.com/live/master.m3u8", None).expect("variant");
        assert_eq!(variant.url, "http://example.com/live/high/index.m3u8");
        assert_eq!(variant.width, Some(1920));
        assert_eq!(variant.height, Some(1080));

        let variant = parse_master(
            body,
            "http://example.com/live/master.m3u8",
            Some(1280 * 720),
        )
        .expect("variant");
        assert_eq!(variant.url, "http://example.com/live/low/index.m3u8");
        assert_eq!(variant.width, Some(1280));
        assert_eq!(variant.height, Some(720));
    }

    #[test]
    fn media_segments_are_absolutized_and_skip_last() {
        let body = "#EXTM3U\n#EXTINF:4,\nseg1.ts\n#EXTINF:4,\nseg2.ts\n#EXTINF:4,\nseg3.ts\n";
        let urls = parse_media_segments(body, "http://example.com/live/index.m3u8", 2);
        assert_eq!(
            urls,
            vec![
                "http://example.com/live/seg1.ts".to_string(),
                "http://example.com/live/seg2.ts".to_string(),
            ]
        );
    }

    async fn serve_once(
        path: &'static str,
        content_type: &'static str,
        body: String,
        status: &'static str,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{addr}{path}")
    }

    #[tokio::test]
    async fn probe_marks_http_error_unreachable() {
        let url = serve_once(
            "/live.m3u8",
            "application/vnd.apple.mpegurl",
            String::new(),
            "404 Not Found",
        )
        .await;
        let client = reqwest::Client::new();
        let out = probe(
            &client,
            &url,
            &[],
            ProbeMode::Speed,
            None,
            &ProbeLimits::for_test(),
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(!out.measurement.reachable);
        assert!(out.measurement.error.is_some());
    }

    #[tokio::test]
    async fn probe_measures_continuous_stream() {
        let url = serve_once("/live.ts", "video/mp2t", "x".repeat(64 * 1024), "200 OK").await;
        let client = reqwest::Client::new();
        let out = probe(
            &client,
            &url,
            &[],
            ProbeMode::Speed,
            None,
            &ProbeLimits::for_test(),
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(out.measurement.reachable);
        assert!(!out.measurement.probe_error);
        assert!(out.measurement.speed_mib.unwrap_or(0.0) > 0.0);
        assert!(out.bytes >= 64 * 1024);
    }

    /// 只接受指定路径；其他路径返回 404，用来验证请求地址已被规范化。
    async fn serve_path(expected: &'static str, body: String) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 1024];
            let read = socket.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..read]);
            let ok = request.starts_with(&format!("GET {expected} "));
            let (status, payload) = if ok {
                ("200 OK", body)
            } else {
                ("404 Not Found", String::new())
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn probe_strips_dollar_description_before_request() {
        let base = serve_path("/live.ts", "x".repeat(4096)).await;
        let client = reqwest::Client::new();
        let out = probe(
            &client,
            &format!("{base}/live.ts$CCTV"),
            &[],
            ProbeMode::Speed,
            None,
            &ProbeLimits::for_test(),
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(out.measurement.reachable, "{:?}", out.measurement.error);
    }

    #[tokio::test]
    async fn probe_marks_dash_playlist_as_probe_error() {
        let client = reqwest::Client::new();
        let out = probe(
            &client,
            "http://127.0.0.1:9/live.mpd",
            &[],
            ProbeMode::Speed,
            None,
            &ProbeLimits::for_test(),
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(out.measurement.probe_error);
        assert!(out.measurement.reachable, "解析能力缺失不算不可达");
    }

    #[tokio::test]
    async fn probe_marks_all_failed_segments_as_probe_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            for _ in 0..3 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 1024];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]);
                let (status, mime, payload) = if request.starts_with("GET /live.m3u8 ") {
                    (
                        "200 OK",
                        "application/vnd.apple.mpegurl",
                        "#EXTM3U\n#EXTINF:1,\nseg1.ts\n#EXTINF:1,\nseg2.ts\n#EXTINF:1,\nseg3.ts\n",
                    )
                } else {
                    ("404 Not Found", "text/plain", "")
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        let client = reqwest::Client::new();
        let out = probe(
            &client,
            &format!("http://{addr}/live.m3u8"),
            &[],
            ProbeMode::Speed,
            None,
            &ProbeLimits::for_test(),
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(out.measurement.reachable, "清单本身可读");
        assert!(out.measurement.probe_error, "切片全部失败按探测错误处理");
        assert_eq!(out.measurement.error.as_deref(), Some("切片全部失败"));
    }

    #[tokio::test]
    async fn playlist_mode_checks_first_segment() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 1024];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]);
                let (status, mime, payload) = if request.starts_with("GET /live.m3u8 ") {
                    (
                        "200 OK",
                        "application/vnd.apple.mpegurl",
                        "#EXTM3U\n#EXTINF:1,\nseg1.ts\n#EXTINF:1,\nseg2.ts\n",
                    )
                } else {
                    ("404 Not Found", "text/plain", "")
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        let client = reqwest::Client::new();
        let out = probe(
            &client,
            &format!("http://{addr}/live.m3u8"),
            &[],
            ProbeMode::Playlist,
            None,
            &ProbeLimits::for_test(),
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(!out.measurement.reachable, "首个切片不可访问应判不可达");
        assert!(!out.measurement.probe_error);
    }

    #[tokio::test]
    async fn probe_reachability_checks_dash_http_status() {
        let url = serve_once("/live.mpd", "application/dash+xml", "mpd".into(), "200 OK").await;
        let client = reqwest::Client::new();
        let out = probe(
            &client,
            &url,
            &[],
            ProbeMode::Reachability,
            None,
            &ProbeLimits::for_test(),
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(out.measurement.reachable);
        assert!(!out.measurement.probe_error);
    }

    #[tokio::test]
    async fn probe_marks_empty_stream_unreachable() {
        let url = serve_once("/empty.ts", "video/mp2t", String::new(), "200 OK").await;
        let client = reqwest::Client::new();
        let out = probe(
            &client,
            &url,
            &[],
            ProbeMode::Speed,
            None,
            &ProbeLimits::for_test(),
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(!out.measurement.reachable);
        assert!(!out.measurement.probe_error);
    }

    #[test]
    fn is_mpegts_detects_sync_bytes() {
        let mut bytes = vec![0u8; 400];
        bytes[0] = 0x47;
        bytes[188] = 0x47;
        bytes[376] = 0x47;
        assert!(is_mpegts(&bytes));
        bytes[188] = 0;
        assert!(!is_mpegts(&bytes));
    }

    #[test]
    fn parse_ffprobe_json_picks_first_video() {
        let json = br#"{"streams":[{"codec_type":"audio"},{"codec_type":"video","width":1920,"height":1080}]}"#;
        assert_eq!(parse_ffprobe_json(json), Some((1920, 1080)));
        assert_eq!(parse_ffprobe_json(br#"{"streams":[]}"#), None);
        assert_eq!(parse_ffprobe_json(b"not json"), None);
    }

    #[test]
    fn ffprobe_cap_counts_attempts() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let counter = AtomicU64::new(0);
        assert!(reserve_ffprobe_attempt(&counter, 1));
        assert!(!reserve_ffprobe_attempt(&counter, 1));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert!(reserve_ffprobe_attempt(&counter, 0), "0 表示不限制");
    }

    #[test]
    fn probe_semaphore_bounds_capture_permits() {
        use std::sync::Arc;
        let gates = ProbeGates {
            probe_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
            ffprobe_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            max_ffprobe_per_run: 0,
        };
        let first = gates
            .probe_semaphore
            .clone()
            .try_acquire_owned()
            .expect("第一个许可");
        assert!(gates.probe_semaphore.clone().try_acquire_owned().is_err());
        drop(first);
        assert!(gates.probe_semaphore.clone().try_acquire_owned().is_ok());
    }

    #[tokio::test]
    async fn detect_resolution_reads_stdin() {
        let dir = std::env::temp_dir().join(format!("tvb-ffprobe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建目录");
        let script = dir.join("ffprobe");
        std::fs::write(
            &script,
            b"#!/bin/sh\ncat > /dev/null\nprintf '{\"streams\":[{\"codec_type\":\"video\",\"width\":1280,\"height\":720}]}'\n",
        )
        .expect("写脚本");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("权限");

        let result = detect_resolution(&script, &[0x47; 400], Duration::from_secs(2)).await;
        assert_eq!(result, Ok((1280, 720)));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 提供媒体清单与一个 TS 切片。清单不含 RESOLUTION。
    async fn serve_hls_segment() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 1024];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]);
                let (mime, payload): (&str, Vec<u8>) = if request.starts_with("GET /live.m3u8 ") {
                    (
                        "application/vnd.apple.mpegurl",
                        b"#EXTM3U\n#EXTINF:1,\nseg1.ts\n#EXTINF:1,\nseg2.ts\n".to_vec(),
                    )
                } else {
                    let mut segment = vec![0u8; 512];
                    segment[0] = 0x47;
                    segment[188] = 0x47;
                    segment[376] = 0x47;
                    ("video/mp2t", segment)
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&payload).await;
            }
        });
        format!("http://{addr}/live.m3u8")
    }

    fn write_script(dir: &std::path::Path, body: &[u8]) -> std::path::PathBuf {
        let script = dir.join("ffprobe");
        std::fs::write(&script, body).expect("写脚本");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("权限");
        script
    }

    #[tokio::test]
    async fn full_mode_runs_ffprobe_on_captured_segment() {
        let dir = std::env::temp_dir().join(format!("tvb-ffprobe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建目录");
        let script = write_script(
            &dir,
            b"#!/bin/sh\ncat > /dev/null\nprintf '{\"streams\":[{\"codec_type\":\"video\",\"width\":1280,\"height\":720}]}'\n",
        );
        let client = reqwest::Client::new();
        let limits = ProbeLimits {
            ffprobe: true,
            ffprobe_ready: true,
            ffprobe_path: script,
            ..ProbeLimits::for_test()
        };
        let url = serve_hls_segment().await;
        let out = probe(
            &client,
            &url,
            &[],
            ProbeMode::Full,
            None,
            &limits,
            &ProbeGates::unlimited(),
        )
        .await;
        assert_eq!(out.measurement.width, Some(1280));
        assert_eq!(out.measurement.height, Some(720));
        assert!(!out.measurement.probe_error);

        // ffprobe 关闭时 Full 退化为 Speed：不运行进程，分辨率保持未知。
        let marker = dir.join("ran");
        let script = write_script(
            &dir,
            format!(
                "#!/bin/sh\ncat > /dev/null\ntouch '{}'\nprintf '{{\"streams\":[{{\"codec_type\":\"video\",\"width\":1280,\"height\":720}}]}}'\n",
                marker.display()
            )
            .as_bytes(),
        );
        let limits = ProbeLimits {
            ffprobe: false,
            ffprobe_ready: true,
            ffprobe_path: script,
            ..ProbeLimits::for_test()
        };
        let url = serve_hls_segment().await;
        let out = probe(
            &client,
            &url,
            &[],
            ProbeMode::Full,
            None,
            &limits,
            &ProbeGates::unlimited(),
        )
        .await;
        assert!(out.measurement.reachable);
        assert!(out.measurement.width.is_none());
        assert!(!marker.exists(), "ffprobe 关闭时不得运行进程");
        let _ = std::fs::remove_dir_all(dir);
    }
}
