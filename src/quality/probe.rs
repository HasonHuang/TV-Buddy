//! 质量探测：HTTP 读取、HLS 解析与测速。

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
                (Some(width), Some(height)) => {
                    u32::from(width) * u32::from(height) <= max
                }
                _ => true,
            })
            .cloned()
            .collect::<Vec<_>>()
    });

    let candidates = within.filter(|items| !items.is_empty()).unwrap_or(variants);
    candidates.into_iter().max_by_key(|variant| variant.bandwidth)
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

/// 只读不存。用于测速样本。
async fn read_counted(
    mut response: Response,
    max_bytes: usize,
    max_secs: f64,
) -> Result<(u64, Duration), String> {
    let started = Instant::now();
    let mut total = 0u64;
    loop {
        if total >= max_bytes as u64 || started.elapsed().as_secs_f64() >= max_secs {
            break;
        }
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = max_bytes.saturating_sub(total as usize);
                let take = chunk.len().min(remaining);
                total = total.saturating_add(take as u64);
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

/// 探测一个地址。
pub async fn probe(
    client: &Client,
    url: &str,
    headers: &[(String, String)],
    mode: ProbeMode,
    max_resolution_px: Option<u32>,
    limits: &ProbeLimits,
) -> ProbeOutput {
    let started = Instant::now();
    let url = clean_url(url);
    let mut total_bytes = 0u64;
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

    let (media_url, media_body, variant_width, variant_height) =
        if body_text.contains("#EXT-X-STREAM-INF") {
            let Some(variant) = parse_master(&body_text, final_url.as_str(), max_resolution_px)
            else {
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
            (
                final_url.to_string(),
                body_text.to_string(),
                None,
                None,
            )
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
        let request = with_headers(client.get(&segments[0]).timeout(limits.speed_timeout), headers);
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
    for segment in segments {
        let request = with_headers(
            client.get(&segment).timeout(limits.speed_timeout),
            headers,
        );
        let segment_started = Instant::now();
        let response = match request.send().await {
            Ok(response) if response.status().is_success() => response,
            _ => continue,
        };
        if first_delay.is_none() {
            first_delay = Some(segment_started.elapsed().as_millis() as u32);
        }
        let (bytes, elapsed) = match read_counted(
            response,
            limits.max_sample_bytes as usize,
            limits.max_sample_secs,
        )
        .await
        {
            Ok(value) => value,
            Err(_) => continue,
        };
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
        )
        .await;
        assert!(!out.measurement.reachable);
        assert!(!out.measurement.probe_error);
    }
}
