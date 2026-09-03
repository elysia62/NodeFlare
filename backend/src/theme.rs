use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;
use url::Url;

pub const BUILTIN_THEME_ID: &str = "builtin-nodeflare-glass";
const INDEX_MAX_BYTES: usize = 4 * 1024 * 1024;
const ASSET_MAX_BYTES: usize = 16 * 1024 * 1024;
const SETTINGS_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ResolvedTheme {
    pub source_url: String,
    pub resolved_url: String,
}

pub fn resolve_url(value: &str) -> Result<ResolvedTheme> {
    let source_url = normalize_url(value).context("主题 URL 仅支持 GitHub 仓库地址")?;
    let parsed = Url::parse(&source_url)?;
    let parts = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let suffix = if parts.len() > 4 {
        format!("/{}", parts[4..].join("/"))
    } else {
        String::new()
    };
    Ok(ResolvedTheme {
        source_url,
        resolved_url: format!(
            "https://raw.githubusercontent.com/{}/{}/{}{}",
            parts[0], parts[1], parts[3], suffix
        ),
    })
}

pub fn normalize_url(value: &str) -> Option<String> {
    let raw = value.trim();
    if !(12..=2048).contains(&raw.len()) || raw.contains('\\') {
        return None;
    }
    let parsed = Url::parse(raw).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let mut parts = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() == 2 {
        parts.extend(["tree", "main"]);
    }
    if parts.len() < 4
        || parts[2] != "tree"
        || parts.iter().any(|part| {
            matches!(*part, "." | "..")
                || part.is_empty()
                || !part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
        })
    {
        return None;
    }
    Some(format!("https://github.com/{}", parts.join("/")))
}

pub async fn validate_remote(client: &reqwest::Client, base: &str) -> Result<()> {
    let (body, _) = fetch_remote(client, base, "index.html", INDEX_MAX_BYTES).await?;
    let html = String::from_utf8(body).context("主题 index.html 不是 UTF-8 文本")?;
    if html.trim().is_empty() {
        anyhow::bail!("主题 index.html 为空");
    }
    Ok(())
}

pub async fn remote_settings_schema(client: &reqwest::Client, base: &str) -> Result<Value> {
    let response = client
        .get(remote_url(base, "theme.json")?)
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(serde_json::json!({"schema": 1, "source": "remote", "settings": []}));
    }
    let response = response.error_for_status()?;
    let body = bounded_body(response, SETTINGS_MAX_BYTES).await?;
    let value: Value = serde_json::from_slice(&body)?;
    validate_settings_schema(value).context("主题 theme.json 设置格式无效")
}

pub async fn remote_version(client: &reqwest::Client, base: &str) -> Option<String> {
    let value = remote_settings_schema(client, base).await.ok()?;
    let version = value.get("version")?.as_str()?.trim();
    (!version.is_empty()
        && version.len() <= 40
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_+ ".contains(character)))
    .then(|| version.to_string())
}

pub async fn fetch_theme_path(
    client: &reqwest::Client,
    base: &str,
    relative: &str,
    prefix: &str,
) -> Result<(Vec<u8>, String)> {
    let relative = sanitize_relative(relative)?;
    let target = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let limit = if target == "index.html" {
        INDEX_MAX_BYTES
    } else {
        ASSET_MAX_BYTES
    };
    let (body, content_type) = fetch_remote(client, base, target, limit).await?;
    if target == "index.html" {
        let html = String::from_utf8(body)?;
        return Ok((rewrite_index(&html, prefix).into_bytes(), content_type));
    }
    Ok((body, content_type))
}

fn sanitize_relative(value: &str) -> Result<&str> {
    let relative = value.trim_start_matches('/');
    if relative.len() > 1024
        || relative.contains('\\')
        || relative.contains('%')
        || relative.contains('?')
        || relative.contains('#')
        || relative
            .split('/')
            .any(|part| part == "." || part == ".." || part.is_empty() && !relative.is_empty())
    {
        anyhow::bail!("主题资源路径无效");
    }
    Ok(relative)
}

async fn fetch_remote(
    client: &reqwest::Client,
    base: &str,
    relative: &str,
    limit: usize,
) -> Result<(Vec<u8>, String)> {
    let response = client
        .get(remote_url(base, relative)?)
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    Ok((bounded_body(response, limit).await?, content_type))
}

async fn bounded_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        anyhow::bail!("远程主题资源过大");
    }
    let body = response.bytes().await?;
    if body.len() > limit {
        anyhow::bail!("远程主题资源过大");
    }
    Ok(body.to_vec())
}

fn remote_url(base: &str, relative: &str) -> Result<Url> {
    let base = Url::parse(&format!("{}/", base.trim_end_matches('/')))?;
    if base.scheme() != "https" || base.host_str() != Some("raw.githubusercontent.com") {
        anyhow::bail!("主题资源地址无效");
    }
    let target = base.join(relative)?;
    if target.origin() != base.origin() || !target.path().starts_with(base.path()) {
        anyhow::bail!("主题资源越界");
    }
    Ok(target)
}

fn rewrite_index(html: &str, prefix: &str) -> String {
    let asset_prefix = format!("{}/assets/", prefix.trim_end_matches('/'));
    html.replace("\"/assets/", &format!("\"{asset_prefix}"))
        .replace("'/assets/", &format!("'{asset_prefix}"))
}

pub fn builtin_settings_schema() -> Value {
    let currencies = [
        "CNY", "USD", "HKD", "EUR", "GBP", "JPY", "RUB", "CHF", "INR", "VND", "THB", "CAD",
    ];
    serde_json::json!({
        "schema": 1,
        "source": "builtin",
        "settings": [
            {
                "key": "assetCurrency", "label": "资产折算币种", "type": "select",
                "default": "CNY",
                "options": currencies.iter().map(|currency| serde_json::json!({
                    "label": currency, "value": currency
                })).collect::<Vec<_>>()
            },
            {"key": "enableBlur", "label": "启用毛玻璃效果", "type": "toggle", "default": true},
            {"key": "showOnline", "label": "总览显示在线节点", "type": "toggle", "default": true},
            {"key": "showCarrierLatency", "label": "节点卡片分线路显示延迟", "type": "toggle", "default": false},
            {"key": "telecomLatencyTask", "label": "电信线路任务名称", "type": "text", "default": "", "placeholder": "留空时按任务名称自动匹配"},
            {"key": "mobileLatencyTask", "label": "移动线路任务名称", "type": "text", "default": "", "placeholder": "留空时按任务名称自动匹配"},
            {"key": "unicomLatencyTask", "label": "联通线路任务名称", "type": "text", "default": "", "placeholder": "留空时按任务名称自动匹配"}
        ]
    })
}

fn validate_settings_schema(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    if object.get("schema")?.as_u64()? != 1 {
        return None;
    }
    let fields = object.get("settings")?.as_array()?;
    if fields.len() > 40 {
        return None;
    }
    let allowed = [
        "text", "textarea", "url", "color", "select", "toggle", "number",
    ];
    let mut keys = HashSet::new();
    for field in fields {
        let field = field.as_object()?;
        let key = field.get("key")?.as_str()?;
        let label = field.get("label")?.as_str()?;
        let kind = field.get("type")?.as_str()?;
        if key.is_empty()
            || key.len() > 64
            || label.is_empty()
            || label.chars().count() > 80
            || !allowed.contains(&kind)
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
            || !keys.insert(key)
        {
            return None;
        }
        if kind == "select" && !field.get("options").is_some_and(Value::is_array) {
            return None;
        }
    }
    let mut result = value;
    result
        .as_object_mut()?
        .insert("source".to_string(), Value::String("remote".to_string()));
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_github_theme_urls() {
        assert_eq!(
            resolve_url("https://github.com/acme/theme/tree/main/dist")
                .unwrap()
                .resolved_url,
            "https://raw.githubusercontent.com/acme/theme/main/dist"
        );
        assert!(resolve_url("https://example.com/theme").is_err());
    }
}
