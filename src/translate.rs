//! 翻译服务客户端：调用兼容 MTranServer 的批量接口。
//!
//! 请求：`POST <base>/translate/batch`，正文 `{texts, source_lang, target_lang}`。
//! 响应接受顶层数组，或 `translations`/`texts`/`result`/`text`/`data` 数组。
//! 元素可为字符串，或含 `text`/`translation` 的对象。

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// 批量翻译端点。
pub fn endpoint(base: &str) -> String {
    format!("{}/translate/batch", base.trim().trim_end_matches('/'))
}

/// 批量翻译；失败时返回错误，空译文回退原文。
pub async fn batch(
    client: &reqwest::Client,
    base: &str,
    source: &str,
    target: &str,
    texts: &[String],
) -> Result<Vec<String>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let body = serde_json::to_vec(&serde_json::json!({
        "texts": texts,
        "source_lang": source,
        "target_lang": target,
    }))?;
    let response = client
        .post(endpoint(base))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .context("请求翻译服务失败")?;
    if !response.status().is_success() {
        bail!("翻译服务返回 {}", response.status());
    }
    let text = response.text().await.context("读取翻译响应失败")?;
    let value: Value = serde_json::from_str(&text).context("翻译响应不是 JSON")?;
    let mut values = extract(&value);
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
        ["translations", "texts", "result", "text", "data"]
            .iter()
            .find_map(|key| value.get(*key).and_then(|item| item.as_array()))
    });
    array
        .map(|items| {
            items
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
                .collect()
        })
        .unwrap_or_default()
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
}
