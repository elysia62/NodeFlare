use std::time::Duration;

use serde_json::Value;
use worker::{Error, Method, Request, Result, Url};

use crate::outbound::{fetch_with_timeout, read_response_limited};

pub const BUILTIN_THEME_ID: &str = "builtin-nodeflare-glass";
pub const BUILTIN_THEME_NAME: &str = "NodeFlare Glass";
pub const INDEX_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const ASSET_MAX_BYTES: usize = 16 * 1024 * 1024;
const SETTINGS_MAX_BYTES: usize = 64 * 1024;
const REMOTE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ResolvedTheme {
    pub source_url: String,
    pub resolved_url: String,
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
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }

    let path = parsed.path().trim_matches('/');
    let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() == 2 {
        // 纯仓库地址默认使用 main 分支。
        parts.push("tree");
        parts.push("main");
    }
    if parts.len() < 4
        || parts[2] != "tree"
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
                || matches!(*part, "." | "..")
        })
    {
        return None;
    }
    Some(format!("https://github.com/{}", parts.join("/")))
}

pub fn resolve_url(value: &str) -> Result<ResolvedTheme> {
    let source_url =
        normalize_url(value).ok_or_else(|| Error::RustError("主题 URL 格式无效".to_string()))?;
    let parsed = Url::parse(&source_url)?;

    let parts: Vec<_> = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let owner = parts[0];
    let repository = parts[1];
    let reference = parts[3];
    let subdirectory = parts[4..].join("/");
    let suffix = if subdirectory.is_empty() {
        String::new()
    } else {
        format!("/{subdirectory}")
    };
    Ok(ResolvedTheme {
        source_url,
        resolved_url: format!(
            "https://raw.githubusercontent.com/{owner}/{repository}/{reference}{suffix}"
        ),
    })
}

fn remote_url(base: &str, relative: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), relative)
}

fn raw_github_base(base: &str) -> Option<Url> {
    let parsed = Url::parse(&format!("{}/", base.trim_end_matches('/'))).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("raw.githubusercontent.com")
        || parsed.port().is_some()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let parts = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (parts.len() >= 3
        && parts.iter().all(|part| {
            !matches!(*part, "." | "..")
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
        }))
    .then_some(parsed)
}

pub fn asset_url(base: &str, path: &str) -> Option<String> {
    let relative = path.strip_prefix("/assets/")?;
    if relative.is_empty()
        || relative.len() > 1024
        || relative
            .chars()
            .any(|character| character.is_control() || matches!(character, '%' | '\\' | '?' | '#'))
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }

    let base = raw_github_base(base)?;
    let remote = base.join(&format!("assets/{relative}")).ok()?;
    let expected_path = format!("{}assets/{relative}", base.path());
    if remote.origin() != base.origin() || remote.path() != expected_path {
        return None;
    }
    Some(remote.to_string())
}

pub fn index_url(base: &str) -> Option<String> {
    raw_github_base(base).map(|base| remote_url(base.as_str(), "index.html"))
}

fn settings_url(base: &str) -> Option<String> {
    raw_github_base(base).map(|base| remote_url(base.as_str(), "theme.json"))
}

pub async fn validate_remote(base: &str) -> Result<()> {
    let index = index_url(base)
        .ok_or_else(|| Error::RustError("主题资源地址不是 GitHub Raw 地址".to_string()))?;
    let request = Request::new(&index, Method::Get)?;
    let Some(mut response) = fetch_with_timeout(request, REMOTE_FETCH_TIMEOUT).await? else {
        return Err(Error::RustError("主题 index.html 请求超时".to_string()));
    };
    let status = response.status_code();
    if !(200..300).contains(&status) {
        return Err(Error::RustError(format!(
            "主题 index.html 返回 HTTP {status}"
        )));
    }
    let body = read_response_limited(&mut response, INDEX_MAX_BYTES)
        .await?
        .ok_or_else(|| Error::RustError("主题 index.html 内容无效".to_string()))?;
    let body = String::from_utf8(body)
        .map_err(|_| Error::RustError("主题 index.html 不是 UTF-8 文本".to_string()))?;
    if body.trim().is_empty() {
        return Err(Error::RustError("主题 index.html 内容无效".to_string()));
    }
    Ok(())
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
                "key": "assetCurrency",
                "label": "资产折算币种",
                "type": "select",
                "default": "CNY",
                "options": currencies.iter().map(|currency| {
                    serde_json::json!({ "label": currency, "value": currency })
                }).collect::<Vec<_>>()
            },
            {
                "key": "enableBlur",
                "label": "启用毛玻璃效果",
                "type": "toggle",
                "default": true
            },
            {
                "key": "showOnline",
                "label": "总览显示在线节点",
                "type": "toggle",
                "default": true
            },
            {
                "key": "showCarrierLatency",
                "label": "节点卡片分线路显示延迟",
                "type": "toggle",
                "default": false
            },
            {
                "key": "telecomLatencyTask",
                "label": "电信线路任务名称",
                "type": "text",
                "default": "",
                "placeholder": "留空时按任务名称自动匹配"
            },
            {
                "key": "mobileLatencyTask",
                "label": "移动线路任务名称",
                "type": "text",
                "default": "",
                "placeholder": "留空时按任务名称自动匹配"
            },
            {
                "key": "unicomLatencyTask",
                "label": "联通线路任务名称",
                "type": "text",
                "default": "",
                "placeholder": "留空时按任务名称自动匹配"
            }
        ]
    })
}

fn empty_settings_schema(source: &str) -> Value {
    serde_json::json!({ "schema": 1, "source": source, "settings": [] })
}

fn valid_setting_value(value: &Value) -> bool {
    value.is_string() || value.is_boolean() || value.as_f64().is_some()
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
    let allowed_types = [
        "text", "textarea", "url", "color", "select", "toggle", "number",
    ];
    let mut keys = std::collections::HashSet::new();
    for field in fields {
        let field = field.as_object()?;
        let key = field.get("key")?.as_str()?;
        let label = field.get("label")?.as_str()?;
        let field_type = field.get("type")?.as_str()?;
        if key.is_empty()
            || key.len() > 64
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
            || !keys.insert(key)
            || label.is_empty()
            || label.chars().count() > 80
            || !allowed_types.contains(&field_type)
            || field.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "key"
                        | "label"
                        | "type"
                        | "default"
                        | "placeholder"
                        | "options"
                        | "min"
                        | "max"
                        | "step"
                )
            })
            || field
                .get("default")
                .is_some_and(|value| !valid_setting_value(value))
            || field
                .get("placeholder")
                .is_some_and(|value| value.as_str().is_none_or(|text| text.chars().count() > 160))
            || ["min", "max", "step"].iter().any(|key| {
                field
                    .get(*key)
                    .is_some_and(|value| value.as_f64().is_none())
            })
        {
            return None;
        }
        let options = field.get("options").and_then(Value::as_array);
        if field_type == "select"
            && options.is_none_or(|choices| {
                choices.is_empty()
                    || choices.len() > 64
                    || choices.iter().any(|choice| {
                        choice.as_object().is_none_or(|choice| {
                            choice.len() != 2
                                || choice
                                    .get("label")
                                    .and_then(Value::as_str)
                                    .is_none_or(|text| text.is_empty() || text.chars().count() > 80)
                                || choice
                                    .get("value")
                                    .and_then(Value::as_str)
                                    .is_none_or(|text| {
                                        text.is_empty() || text.chars().count() > 160
                                    })
                        })
                    })
            })
        {
            return None;
        }
    }
    Some(serde_json::json!({
        "schema": 1,
        "source": "remote",
        "settings": fields
    }))
}

pub async fn remote_settings_schema(base: &str) -> Result<Value> {
    let url = settings_url(base)
        .ok_or_else(|| Error::RustError("主题资源地址不是 GitHub Raw 地址".to_string()))?;
    let request = Request::new(&url, Method::Get)?;
    let Some(mut response) = fetch_with_timeout(request, REMOTE_FETCH_TIMEOUT).await? else {
        return Err(Error::RustError("主题 theme.json 请求超时".to_string()));
    };
    if response.status_code() == 404 {
        return Ok(empty_settings_schema("remote"));
    }
    if !(200..300).contains(&response.status_code()) {
        return Err(Error::RustError(format!(
            "主题 theme.json 返回 HTTP {}",
            response.status_code()
        )));
    }
    let body = read_response_limited(&mut response, SETTINGS_MAX_BYTES)
        .await?
        .ok_or_else(|| Error::RustError("主题 theme.json 内容无效".to_string()))?;
    let value = serde_json::from_slice(body.as_slice())
        .map_err(|_| Error::RustError("主题 theme.json 不是有效 JSON".to_string()))?;
    validate_settings_schema(value)
        .ok_or_else(|| Error::RustError("主题 theme.json 设置格式无效".to_string()))
}

/// 读取远程主题 theme.json 中的 version 字段；无 theme.json 或无版本时返回 None。
pub async fn remote_theme_version(base: &str) -> Option<String> {
    let url = settings_url(base)?;
    let request = Request::new(&url, Method::Get).ok()?;
    let mut response = fetch_with_timeout(request, REMOTE_FETCH_TIMEOUT)
        .await
        .ok()??;
    if !(200..300).contains(&response.status_code()) {
        return None;
    }
    let body = read_response_limited(&mut response, SETTINGS_MAX_BYTES)
        .await
        .ok()??;
    let value: Value = serde_json::from_slice(body.as_slice()).ok()?;
    let version = value.get("version")?.as_str()?.trim();
    if version.is_empty()
        || version.len() > 40
        || !version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_+ ".contains(character))
    {
        return None;
    }
    Some(version.to_string())
}

#[cfg(test)]
mod tests {
    use super::{asset_url, normalize_url, resolve_url, validate_settings_schema};

    #[test]
    fn validates_remote_theme_settings() {
        let schema = validate_settings_schema(serde_json::json!({
            "schema": 1,
            "settings": [{
                "key": "accent",
                "label": "强调色",
                "type": "color",
                "default": "#00aaff"
            }]
        }))
        .expect("valid schema");
        assert_eq!(schema["source"], "remote");
        assert!(validate_settings_schema(serde_json::json!({
            "schema": 1,
            "settings": [{ "key": "../bad", "label": "Bad", "type": "text" }]
        }))
        .is_none());
        assert!(validate_settings_schema(serde_json::json!({
            "schema": 1,
            "settings": [{ "key": "mode", "label": "Mode", "type": "select" }]
        }))
        .is_none());
    }

    #[test]
    fn accepts_only_github_urls() {
        assert_eq!(
            normalize_url("https://github.com/acme/theme/tree/abc123").as_deref(),
            Some("https://github.com/acme/theme/tree/abc123")
        );
        assert!(normalize_url("https://themes.example.com/nodeflare").is_none());
        assert!(normalize_url("http://themes.example.com/theme").is_none());
        assert!(normalize_url("https://user@example.com/theme").is_none());
        assert_eq!(
            normalize_url("https://github.com/acme/theme").as_deref(),
            Some("https://github.com/acme/theme/tree/main")
        );
        assert_eq!(
            resolve_url("https://github.com/acme/theme/tree/main/dist")
                .expect("GitHub theme URL")
                .resolved_url,
            "https://raw.githubusercontent.com/acme/theme/main/dist"
        );
    }

    #[test]
    fn limits_proxy_paths_to_theme_assets() {
        assert_eq!(
            asset_url(
                "https://raw.githubusercontent.com/acme/theme/main",
                "/assets/app.js"
            )
            .as_deref(),
            Some("https://raw.githubusercontent.com/acme/theme/main/assets/app.js")
        );
        assert!(asset_url("https://example.com/theme", "/assets/app.js").is_none());
        assert!(asset_url("https://example.com/theme", "/logo.svg").is_none());
        assert!(asset_url("https://example.com/theme", "/assets/../secret").is_none());
        assert!(asset_url("https://example.com/theme", "/assets/%2e%2e/secret").is_none());
        assert!(asset_url("https://example.com/theme", "/assets/..\\secret").is_none());
        assert!(asset_url("https://example.com/theme", "/assets/a%2fb.js").is_none());
    }
}
