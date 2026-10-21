//! 翻译服务客户端：兼容 MTranServer v5 与旧批量接口。
//!
//! 批量请求：`POST <base>/translate/batch`。
//! - v5 正文：`{from, to, texts}`，响应 `{results}`。
//! - 旧接口正文：`{texts, source_lang, target_lang}`。
//! - 单条接口：`POST <base>/translate`，正文 `{from, to, text}`，响应 `{result}`。
//!
//! 批量失败时回退单条接口，便于只开放 `/translate` 的代理。
//! 响应接受 `results`/`translations`/`texts`/`result`/`text`/`data`。

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// 去掉首尾空白与末尾斜杠。
fn normalize(base: &str) -> &str {
    base.trim().trim_end_matches('/')
}

/// 批量翻译端点。接受服务根地址或完整 `/translate` 地址。
pub fn endpoint(base: &str) -> String {
    let base = normalize(base);
    if base.is_empty() {
        return String::new();
    }
    if base.ends_with("/translate/batch") {
        base.to_string()
    } else if base.ends_with("/translate") {
        format!("{base}/batch")
    } else {
        format!("{base}/translate/batch")
    }
}

/// 单条翻译端点。接受服务根地址或完整 `/translate[/batch]` 地址。
pub fn single_endpoint(base: &str) -> String {
    let base = normalize(base);
    if base.is_empty() {
        return String::new();
    }
    if let Some(root) = base.strip_suffix("/translate/batch") {
        format!("{root}/translate")
    } else if base.ends_with("/translate") {
        base.to_string()
    } else {
        format!("{base}/translate")
    }
}

/// 批量翻译；失败时返回错误，空译文回退原文。
pub async fn batch(
    client: &reqwest::Client,
    base: &str,
    source: &str,
    target: &str,
    texts: &[String],
) -> Result<Vec<String>> {
    batch_with_token(client, base, source, target, texts, None).await
}

/// 带 Token 的批量翻译。
///
/// 先按 MTranServer v5 形状请求；400/422 时再试旧形状。
/// 批量端点不可用或响应无法解析时，逐条请求 `/translate`。
pub async fn batch_with_token(
    client: &reqwest::Client,
    base: &str,
    source: &str,
    target: &str,
    texts: &[String],
    token: Option<&str>,
) -> Result<Vec<String>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let url = endpoint(base);
    if url.is_empty() {
        bail!("翻译服务地址为空");
    }

    // 同时带 v5 与旧字段。多数实现会忽略多余字段。
    let body = json!({
        "from": source,
        "to": target,
        "texts": texts,
        "source_lang": source,
        "target_lang": target,
    });
    let response = post_json(client, &url, token, &body).await?;
    let status = response.status();
    if status.is_success() {
        if let Ok(values) = decode(response, texts).await {
            return Ok(values);
        }
        return single_batch(client, base, source, target, texts, token).await;
    }

    if status == reqwest::StatusCode::BAD_REQUEST
        || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
    {
        let legacy = json!({
            "texts": texts,
            "source_lang": source,
            "target_lang": target,
        });
        let response = post_json(client, &url, token, &legacy).await?;
        if response.status().is_success() {
            if let Ok(values) = decode(response, texts).await {
                return Ok(values);
            }
        }
    }

    if matches!(
        status,
        reqwest::StatusCode::NOT_FOUND
            | reqwest::StatusCode::METHOD_NOT_ALLOWED
            | reqwest::StatusCode::NOT_IMPLEMENTED
    ) {
        return single_batch(client, base, source, target, texts, token).await;
    }

    let detail = response.text().await.unwrap_or_default();
    let detail = detail.trim();
    if detail.is_empty() {
        bail!("翻译服务返回 {status}");
    }
    let detail: String = detail.chars().take(200).collect();
    bail!("翻译服务返回 {status}: {detail}");
}

/// 逐条调用单条接口。
async fn single_batch(
    client: &reqwest::Client,
    base: &str,
    source: &str,
    target: &str,
    texts: &[String],
    token: Option<&str>,
) -> Result<Vec<String>> {
    let url = single_endpoint(base);
    if url.is_empty() {
        bail!("翻译服务地址为空");
    }
    let mut output = Vec::with_capacity(texts.len());
    for text in texts {
        let body = json!({
            "from": source,
            "to": target,
            "text": text,
            "source_lang": source,
            "target_lang": target,
        });
        let response = post_json(client, &url, token, &body).await?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            let detail = detail.trim();
            if detail.is_empty() {
                bail!("翻译服务返回 {status}");
            }
            let detail: String = detail.chars().take(200).collect();
            bail!("翻译服务返回 {status}: {detail}");
        }
        let value: Value =
            serde_json::from_str(&response.text().await.context("读取翻译响应失败")?)
                .context("翻译响应不是 JSON")?;
        let translated = extract(&value).into_iter().next().unwrap_or_default();
        output.push(if translated.trim().is_empty() {
            text.clone()
        } else {
            translated
        });
    }
    Ok(output)
}

/// 发送 JSON 请求；Token 使用 `Authorization: Bearer`。
async fn post_json(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    body: &Value,
) -> Result<reqwest::Response> {
    let payload = serde_json::to_vec(body).context("序列化翻译请求失败")?;
    let mut request = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload);
    if let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) {
        request = request.bearer_auth(token);
    }
    request.send().await.context("请求翻译服务失败")
}

/// 解析响应并按原文补齐空译文。
async fn decode(response: reqwest::Response, texts: &[String]) -> Result<Vec<String>> {
    let text = response.text().await.context("读取翻译响应失败")?;
    let value: Value = serde_json::from_str(&text).context("翻译响应不是 JSON")?;
    let mut values = extract(&value);
    if values.is_empty() {
        bail!("翻译响应缺少译文");
    }
    values.truncate(texts.len());
    values.resize(texts.len(), String::new());
    for (value, original) in values.iter_mut().zip(texts) {
        if value.trim().is_empty() {
            *value = original.clone();
        }
    }
    Ok(values)
}

/// 从响应中提取译文列表；未知形状返回空列表。
pub fn extract(value: &Value) -> Vec<String> {
    let array = value.as_array().or_else(|| {
        ["results", "translations", "texts", "result", "text", "data"]
            .iter()
            .find_map(|key| value.get(*key).and_then(|item| item.as_array()))
    });
    if let Some(items) = array {
        return items
            .iter()
            .map(|item| match item {
                Value::String(text) => text.clone(),
                other => other
                    .get("text")
                    .or_else(|| other.get("translation"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect();
    }
    for key in ["result", "translation", "text"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return vec![text.to_string()];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_trims_trailing_slash() {
        assert_eq!(
            endpoint("http://127.0.0.1:8989/"),
            "http://127.0.0.1:8989/translate/batch"
        );
    }

    #[test]
    fn endpoint_accepts_full_translate_paths() {
        assert_eq!(
            endpoint("https://translate.example/translate"),
            "https://translate.example/translate/batch"
        );
        assert_eq!(
            endpoint("https://translate.example/translate/batch/"),
            "https://translate.example/translate/batch"
        );
        assert_eq!(
            single_endpoint("https://translate.example/translate/batch"),
            "https://translate.example/translate"
        );
        assert_eq!(
            single_endpoint("https://translate.example"),
            "https://translate.example/translate"
        );
    }

    #[test]
    fn extract_accepts_common_shapes() {
        let array = serde_json::json!(["一", "二"]);
        assert_eq!(extract(&array), vec!["一", "二"]);

        let wrapped = serde_json::json!({ "translations": ["一", "二"] });
        assert_eq!(extract(&wrapped), vec!["一", "二"]);

        let objects = serde_json::json!({ "result": [{"text": "一"}, {"translation": "二"}] });
        assert_eq!(extract(&objects), vec!["一", "二"]);

        let unknown = serde_json::json!({ "value": "x" });
        assert!(extract(&unknown).is_empty());
    }

    #[test]
    fn extract_accepts_mtran_v5_shapes() {
        let batch = serde_json::json!({
            "results": ["你好，世界", "早上好"],
            "detected_languages": ["en", "en"]
        });
        assert_eq!(extract(&batch), vec!["你好，世界", "早上好"]);

        let single = serde_json::json!({ "result": "你好，世界", "time_cost": 18 });
        assert_eq!(extract(&single), vec!["你好，世界"]);
    }
}
