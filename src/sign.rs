//! 切片路径签名：时间窗共享签名。
//!
//! 目的：切片请求逐次校验，同时让同一时间窗内所有用户拿到相同 URL，
//! 这样 CDN 只缓存一份。
//!
//! 签名只覆盖 `gid|cid|exp`，不绑定用户。窗口内拿到 URL 的人都能播。
//! 按用户撤销做不到，只能等窗口过期。

use sha2::{Digest, Sha256};

use crate::config::Config;

/// 签名覆盖的协议版本。改格式时升级它，旧链接自然失效。
const VERSION: &str = "v1";

/// 签名长度：16 字节 → 32 个十六进制字符。
const SIG_LEN: usize = 32;

/// 时间窗下限与上限（秒）。
pub const WINDOW_MIN: u64 = 30;
pub const WINDOW_MAX: u64 = 3600;

/// 是否启用切片签名。
///
/// 令牌总开关关闭时不签名，保持旧行为。
/// 密钥缺失或无效时也按关闭处理，避免签不出名却拒绝所有切片。
pub fn enabled(config: &Config) -> bool {
    config.auth.token_enabled && config.auth.slice_sign_enabled && secret_bytes(config).is_some()
}

/// 是否缺少密钥。
///
/// 启用签名但密钥为空时，保存配置会生成一个。
pub fn needs_secret(config: &Config) -> bool {
    config.auth.token_enabled
        && config.auth.slice_sign_enabled
        && config.auth.slice_sign_secret.trim().is_empty()
}

/// 生成 32 字节随机密钥，返回十六进制。
pub fn generate_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// 补空密钥。已存在或不需要时不动。
pub fn ensure_secret(config: &mut Config) {
    if needs_secret(config) {
        config.auth.slice_sign_secret = generate_secret();
    }
}

/// 当前时间窗对应的切片路径前缀，形如 `l/<exp>/<sig>`。
///
/// 关闭签名时返回空串。调用方把它插在 `{gid}/{cid}` 与切片名之间。
pub fn prefix(config: &Config, gid: &str, cid: &str) -> String {
    if !enabled(config) {
        return String::new();
    }
    let Some(secret) = secret_bytes(config) else {
        return String::new();
    };
    let exp = window_end(now_secs(), window_secs(config));
    let sig = signature(&secret, gid, cid, exp);
    format!("l/{exp}/{sig}")
}

/// 校验路径里的 `exp` 与 `sig`。
///
/// 关闭签名时一律通过。
pub fn verify(config: &Config, gid: &str, cid: &str, exp: &str, sig: &str) -> bool {
    if !enabled(config) {
        return true;
    }
    let Ok(exp) = exp.parse::<u64>() else {
        return false;
    };
    let now = now_secs();
    let window = window_secs(config);
    // 只接受未来 1–2 个窗口内的 `exp`，两头都留余量。
    if exp <= now || exp > now.saturating_add(window.saturating_mul(2)) {
        return false;
    }
    let Some(secret) = secret_bytes(config) else {
        return false;
    };
    let expected = signature(&secret, gid, cid, exp);
    constant_time_eq(expected.as_bytes(), sig.as_bytes())
}

/// 时间窗长度，钳到允许范围。
pub fn window_secs(config: &Config) -> u64 {
    config
        .auth
        .slice_sign_window_secs
        .clamp(WINDOW_MIN, WINDOW_MAX)
}

/// 当前时间对应的窗口结束时刻。
///
/// 取下两个窗口边界，使链接有 1–2 个窗口的寿命。
pub fn window_end(now: u64, window: u64) -> u64 {
    let window = window.max(1);
    (now / window + 2) * window
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0)
}

fn secret_bytes(config: &Config) -> Option<Vec<u8>> {
    let text = config.auth.slice_sign_secret.trim();
    if text.is_empty() {
        return None;
    }
    let bytes = hex::decode(text).ok()?;
    (bytes.len() >= 16).then_some(bytes)
}

/// `hex(HMAC-SHA256(secret, "v1|gid|cid|exp"))` 的前 16 字节。
fn signature(secret: &[u8], gid: &str, cid: &str, exp: u64) -> String {
    let message = format!("{VERSION}|{gid}|{cid}|{exp}");
    let mac = hmac_sha256(secret, message.as_bytes());
    hex::encode(&mac[..SIG_LEN / 2])
}

/// HMAC-SHA256。用 `sha2` 手写，避免新增 `hmac` 依赖。
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    let mut outer = Sha256::new();
    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    inner.update(inner_pad);
    inner.update(data);
    let inner_digest = inner.finalize();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn config() -> Config {
        let mut config = Config::default();
        config.auth.token_enabled = true;
        config.auth.slice_sign_enabled = true;
        config.auth.slice_sign_secret = "00112233445566778899aabbccddeeff".into();
        config
    }

    #[test]
    fn hmac_matches_rfc_4231_vector() {
        // RFC 4231 Test Case 1。
        let key = [0x0bu8; 20];
        let mac = hmac_sha256(&key, b"Hi There");
        assert_eq!(
            hex::encode(mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn same_window_same_uid_same_signature() {
        // 同一 (gid, cid, exp) 必须给出同一签名。
        let secret = hex::decode(config().auth.slice_sign_secret).expect("密钥");
        let one = signature(&secret, "g1", "c1", 1_790_000_000);
        let two = signature(&secret, "g1", "c1", 1_790_000_000);
        assert_eq!(one, two);
        assert_eq!(one.len(), SIG_LEN);
        assert!(one.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn signature_changes_with_channel_and_exp() {
        let secret = hex::decode(config().auth.slice_sign_secret).expect("密钥");
        assert_ne!(
            signature(&secret, "g1", "c1", 100),
            signature(&secret, "g1", "c2", 100)
        );
        assert_ne!(
            signature(&secret, "g1", "c1", 100),
            signature(&secret, "g1", "c1", 130)
        );
    }

    #[test]
    fn window_end_is_aligned_and_ahead() {
        assert_eq!(window_end(0, 300), 600);
        assert_eq!(window_end(301, 300), 900);
        // 每个时刻都在窗口结束前 1–2 个窗口内。
        for now in [0u64, 1, 299, 300, 301, 1_790_000_123] {
            let exp = window_end(now, 300);
            assert!(exp > now);
            assert!(exp <= now + 600);
            assert_eq!(exp % 300, 0);
        }
    }

    #[test]
    fn verify_accepts_fresh_and_rejects_tampered() {
        let config = config();
        let exp = window_end(now_secs(), window_secs(&config));
        let sig = signature(
            &hex::decode(&config.auth.slice_sign_secret).expect("密钥"),
            "g1",
            "c1",
            exp,
        );
        assert!(verify(&config, "g1", "c1", &exp.to_string(), &sig));
        assert!(!verify(&config, "g1", "c2", &exp.to_string(), &sig));
        assert!(!verify(
            &config,
            "g1",
            "c1",
            &(exp + window_secs(&config)).to_string(),
            &sig
        ));
        assert!(!verify(&config, "g1", "c1", "bad", &sig));
    }

    #[test]
    fn disabled_signing_passes_and_yields_empty_prefix() {
        let mut config = config();
        config.auth.slice_sign_enabled = false;
        assert!(prefix(&config, "g1", "c1").is_empty());
        assert!(verify(&config, "g1", "c1", "0", "bad"));
    }

    #[test]
    fn missing_secret_disables_signing() {
        let mut config = config();
        config.auth.slice_sign_secret.clear();
        assert!(!enabled(&config));
        assert!(prefix(&config, "g1", "c1").is_empty());
        assert!(verify(&config, "g1", "c1", "0", "bad"));
    }

    #[test]
    fn missing_secret_is_filled_once() {
        let mut config = Config::default();
        config.auth.token_enabled = true;
        assert!(needs_secret(&config));
        ensure_secret(&mut config);
        assert_eq!(config.auth.slice_sign_secret.len(), 64);
        assert!(!needs_secret(&config));
    }
}
