//! 多 VPS 数据分享：`GET /api/share/pull` 与主从同步任务。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §6.10。
//! 证据：F-013（S-SHARE-01…06）。
//!
//! 载荷形状固定为 `{"token","token_old","bans":[],"unbans":[]}`。
//! 门禁：`key` 必须等于 `[share].key`，源 IP 必须在 `[share].trust_ips`。

use std::{net::IpAddr, str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{auth::ip_guard::MAX_BAN_SECS, state::AppState};

/// 同步载荷。两个角色同形。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Payload {
    /// 当前 Token；未启用时为空。
    #[serde(default)]
    pub token: Option<String>,
    /// 宽限期内的旧 Token。
    #[serde(default)]
    pub token_old: Option<String>,
    /// 当前封禁中的地址。
    #[serde(default)]
    pub bans: Vec<String>,
    /// 最近解封的地址。
    #[serde(default)]
    pub unbans: Vec<String>,
}

/// 一次同步的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    /// 是否采用了新 Token。
    pub token_changed: bool,
    /// 新增或续期的封禁数。
    pub banned: usize,
    /// 解封数。
    pub unbanned: usize,
}

/// 判断源 IP 是否命中信任列表。
///
/// 支持精确地址、IPv4/IPv6 CIDR 与 `*`。
pub fn ip_allowed(ip: &str, trust: &[String]) -> bool {
    trust.iter().any(|entry| match_ip(ip.trim(), entry.trim()))
}

fn match_ip(ip: &str, entry: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    if entry == "*" {
        return true;
    }
    if let Some((base, bits)) = entry.split_once('/') {
        let (Ok(address), Ok(base), Ok(bits)) = (
            IpAddr::from_str(ip),
            IpAddr::from_str(base),
            bits.parse::<u8>(),
        ) else {
            return false;
        };
        return cidr_contains(address, base, bits);
    }
    ip == entry
}

fn cidr_contains(address: IpAddr, base: IpAddr, bits: u8) -> bool {
    match (address, base) {
        (IpAddr::V4(address), IpAddr::V4(base)) if bits <= 32 => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(address) & mask == u32::from(base) & mask
        }
        (IpAddr::V6(address), IpAddr::V6(base)) if bits <= 128 => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(address) & mask == u128::from(base) & mask
        }
        _ => false,
    }
}

/// 生成本机载荷。`window` 决定解封回溯范围。
pub fn payload(state: &AppState, window: Duration) -> Payload {
    let (current, old) = state.play_tokens.snapshot();
    Payload {
        token: Some(current),
        token_old: old,
        bans: state.ip_guard.banned_ips(),
        unbans: state.ip_guard.recent_unbans(window),
    }
}

/// 应用对端载荷。`adopt_token` 为假时忽略 Token（master 角色）。
pub fn apply(state: &AppState, payload: &Payload, adopt_token: bool) -> Applied {
    let mut applied = Applied::default();
    if adopt_token {
        if let Some(token) = payload.token.as_ref().filter(|value| !value.is_empty()) {
            if state
                .play_tokens
                .adopt(token.clone(), payload.token_old.clone())
            {
                applied.token_changed = true;
                info!("【共享】采用主机 Token");
            }
        }
    }
    for ip in &payload.bans {
        if ip.trim().is_empty() {
            continue;
        }
        state.ip_guard.ban(ip.trim(), MAX_BAN_SECS);
        applied.banned += 1;
    }
    for ip in &payload.unbans {
        if state.ip_guard.unban(ip.trim()) {
            applied.unbanned += 1;
        }
    }
    applied
}

/// 从对端拉取载荷。
pub async fn pull_from(state: &AppState, base: &str, key: &str) -> Result<Payload> {
    let url = format!("{}/api/share/pull", base.trim_end_matches('/'));
    let response = state
        .http
        .get(&url)
        .query(&[("key", key)])
        .send()
        .await
        .with_context(|| format!("请求 {url} 失败"))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("{url} 返回 {status}");
    }
    let body = response
        .text()
        .await
        .with_context(|| format!("读取 {url} 正文失败"))?;
    serde_json::from_str::<Payload>(&body).with_context(|| format!("解析 {url} 载荷失败"))
}

/// 起同步任务；`[share].enabled` 为假时只保留空转。
pub fn spawn(state: Arc<AppState>) {
    if !state.snapshot().share.enabled {
        return;
    }
    tokio::spawn(async move {
        let role = state.snapshot().share.role.to_ascii_lowercase();
        if role == "follower" {
            info!("【共享】本机为 follower，token 由主机接管，已禁用本地轮换");
        }
        let interval = state.snapshot().share.poll_secs.max(5);
        info!(
            "【共享】同步任务运行中(角色={}, 间隔={}s)",
            state.snapshot().share.role,
            interval
        );
        // 等监听与对端就绪，避免启动竞态。
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            let share = state.snapshot().share.clone();
            if !share.enabled {
                tokio::time::sleep(Duration::from_secs(share.poll_secs.max(5))).await;
                continue;
            }
            if share.role.eq_ignore_ascii_case("follower") && !share.master_url.is_empty() {
                match pull_from(&state, &share.master_url, &share.key).await {
                    Ok(payload) => {
                        let applied = apply(&state, &payload, true);
                        info!(
                            token = applied.token_changed,
                            banned = applied.banned,
                            unbanned = applied.unbanned,
                            "【共享】已从主机同步"
                        );
                    }
                    Err(error) => {
                        warn!("【共享】拉取主机失败 {}: {error:#}", share.master_url);
                    }
                }
            } else if share.role.eq_ignore_ascii_case("master") {
                for base in &share.follower_urls {
                    match pull_from(&state, base, &share.key).await {
                        Ok(payload) => {
                            let applied = apply(&state, &payload, false);
                            info!(
                                follower = %base,
                                banned = applied.banned,
                                unbanned = applied.unbanned,
                                "【共享】已汇总从机"
                            );
                        }
                        Err(error) => {
                            warn!("【共享】拉取从机失败 {base}: {error:#}");
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(share.poll_secs.max(5))).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_list_matches_exact_and_cidr() {
        let trust = vec![
            "192.168.1.5".to_string(),
            "10.0.0.0/8".to_string(),
            "2001:db8::/32".to_string(),
        ];
        assert!(ip_allowed("192.168.1.5", &trust));
        assert!(!ip_allowed("192.168.1.6", &trust));
        assert!(ip_allowed("10.2.3.4", &trust));
        assert!(!ip_allowed("11.2.3.4", &trust));
        assert!(ip_allowed("2001:db8::1", &trust));
        assert!(!ip_allowed("2001:db9::1", &trust));
        assert!(ip_allowed("203.0.113.9", &["*".to_string()]));
        assert!(!ip_allowed("203.0.113.9", &[]));
    }

    #[test]
    fn payload_roundtrip() {
        let text = r#"{"token":null,"token_old":null,"bans":[],"unbans":[]}"#;
        let payload: Payload = serde_json::from_str(text).expect("解析");
        assert_eq!(payload, Payload::default());
        assert_eq!(
            serde_json::to_string(&payload).expect("序列化"),
            r#"{"token":null,"token_old":null,"bans":[],"unbans":[]}"#
        );
    }
}
