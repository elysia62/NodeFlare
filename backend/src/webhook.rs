use crate::models::{WebhookPreset, WebhookSettingsInput};
use anyhow::{Result, ensure};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::time::Duration;

pub const DEFAULT_BODY: &str = r#"{"event":"{{event}}","node":"{{node}}","title":"{{title}}","message":"{{message}}","site":"{{site}}","time":"{{time}}"}"#;

pub struct Message<'a> {
    pub title: &'a str,
    pub node: &'a str,
    pub message: &'a str,
    pub event: &'a str,
    pub site: &'a str,
}

pub fn client() -> Result<reqwest::Client> {
    Ok(client_builder().build()?)
}

fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(format!("NodeFlare/{}", crate::config::VERSION))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
}

fn request_headers(input: &str) -> Result<HeaderMap> {
    ensure!(
        input.len() <= 8192 && input.lines().count() <= 64,
        "Webhook 请求头过长"
    );
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("Webhook 请求头应为 Name: value 格式"))?;
        let name = HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|_| anyhow::anyhow!("Webhook 请求头名称无效"))?;
        ensure!(
            !matches!(
                name.as_str(),
                "host" | "content-length" | "transfer-encoding" | "connection"
            ),
            "Webhook 请求头不能覆盖连接或长度信息"
        );
        let value = HeaderValue::from_str(value.trim())
            .map_err(|_| anyhow::anyhow!("Webhook 请求头内容无效"))?;
        headers.insert(name, value);
    }
    Ok(headers)
}

pub fn render(template: &str, message: &Message<'_>) -> Result<String> {
    let timestamp = time::OffsetDateTime::now_utc();
    let time = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        timestamp.year(),
        timestamp.month() as u8,
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second()
    );
    let mut remaining = template;
    let mut output = String::with_capacity(template.len());
    while let Some(start) = remaining.find("{{") {
        output.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(end) = remaining.find("}}") else {
            break;
        };
        let key = &remaining[2..end];
        let value = match key {
            "title" => Some(message.title),
            "server" | "node" => Some(message.node),
            "message" => Some(message.message),
            "event" => Some(message.event),
            "site" => Some(message.site),
            "time" => Some(time.as_str()),
            _ => None,
        };
        if let Some(value) = value {
            // Scan only the template: replacement data may itself contain
            // {{tokens}} and must never be expanded a second time.
            let encoded = serde_json::to_string(value)?;
            output.push_str(&encoded[1..encoded.len() - 1]);
        } else {
            output.push_str(&remaining[..end + 2]);
        }
        remaining = &remaining[end + 2..];
    }
    output.push_str(remaining);
    serde_json::from_str::<Value>(&output)
        .map_err(|_| anyhow::anyhow!("Webhook 请求体必须是有效 JSON，占位符需放在引号内"))?;
    Ok(output)
}

pub fn validate(settings: &WebhookSettingsInput) -> Result<()> {
    ensure!(settings.url.len() <= 2048, "Webhook URL 无效");
    let url = url::Url::parse(&settings.url).map_err(|_| anyhow::anyhow!("Webhook URL 无效"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "Webhook URL 无效"
    );
    request_headers(&settings.headers)?;
    ensure!(
        !settings.body.trim().is_empty() && settings.body.len() <= 16 * 1024,
        "Webhook 请求体为空或超过 16 KiB"
    );
    // Quotes and newlines expose unquoted placeholders that happen to work
    // with a simple preview string but would corrupt a real notification.
    render(
        &settings.body,
        &Message {
            title: "测试 \"标题\"\n下一行",
            node: "节点\\路径\n\"一\"",
            message: "正文 \"quoted\"\n{{event}}",
            event: "test",
            site: "NodeFlare \"站点\"",
        },
    )?;
    Ok(())
}

pub async fn send(
    client: &reqwest::Client,
    settings: &WebhookSettingsInput,
    message: &Message<'_>,
) -> Result<()> {
    validate(settings)?;
    let mut response = client
        .post(&settings.url)
        .timeout(Duration::from_secs(10))
        .headers(request_headers(&settings.headers)?)
        .body(render(&settings.body, message)?)
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;
    let status = response.status();
    ensure!(
        !status.is_redirection(),
        "Webhook 地址发生跳转（HTTP {}），请填写最终地址",
        status.as_u16()
    );
    ensure!(
        status.is_success(),
        "Webhook 请求失败（HTTP {}）",
        status.as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)?
    {
        ensure!(
            bytes.len() + chunk.len() <= 64 * 1024,
            "Webhook 响应超过 64 KiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let expected = match settings.preset {
        WebhookPreset::Bark => Some(("code", 200)),
        WebhookPreset::Wecom | WebhookPreset::Dingtalk => Some(("errcode", 0)),
        WebhookPreset::Feishu => Some(("code", 0)),
        _ => None,
    };
    if let Some((field, expected)) = expected {
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Webhook 返回了无效的服务响应"))?;
        let code = value
            .get(field)
            .or_else(|| {
                matches!(settings.preset, WebhookPreset::Feishu)
                    .then(|| value.get("StatusCode"))
                    .flatten()
            })
            .and_then(Value::as_i64);
        // Do not echo a remote response: it can contain the URL, headers or
        // device key. A numeric service code is sufficient to diagnose refusal.
        ensure!(
            code == Some(expected),
            "Webhook 服务拒收通知（{}={}）",
            field,
            code.map(|value| value.to_string())
                .unwrap_or_else(|| "missing".into())
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, response::IntoResponse, routing::post};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn escapes_json_values_without_expanding_replacement_data() {
        let message = Message {
            title: "标题 \"quoted\"",
            node: "node\\path",
            message: "line1\n{{event}}",
            event: "offline",
            site: "站点",
        };
        let body = render(DEFAULT_BODY, &message).unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["title"], message.title);
        assert_eq!(value["node"], message.node);
        assert_eq!(value["message"], message.message);
        assert_eq!(value["event"], "offline");
        assert_eq!(value["site"], message.site);
        assert!(render(r#"{"message":{{message}}}"#, &message).is_err());
        assert_eq!(
            render(r#"{"unknown":"{{unknown}}"}"#, &message).unwrap(),
            r#"{"unknown":"{{unknown}}"}"#
        );
    }

    #[test]
    fn validates_templates_and_headers_before_saving() {
        let mut settings = WebhookSettingsInput {
            enabled: true,
            preset: WebhookPreset::Custom,
            url: "https://example.com/push?key=secret".into(),
            headers: "Authorization: Bearer a:b\nContent-Type: application/vnd.api+json".into(),
            body: DEFAULT_BODY.into(),
            clear_headers: false,
        };
        validate(&settings).unwrap();
        assert_eq!(
            request_headers(&settings.headers).unwrap()["authorization"],
            "Bearer a:b"
        );
        for body in [r#"{"title":{{title}}}"#, "not json"] {
            settings.body = body.into();
            assert!(validate(&settings).is_err());
        }
        settings.body = DEFAULT_BODY.into();
        for headers in ["no delimiter", "Content-Length: 10", "X Key: value"] {
            settings.headers = headers.into();
            assert!(validate(&settings).is_err());
        }
        settings.headers.clear();
        for url in [
            "file:///tmp/key",
            "https://user:secret@example.com",
            "https://example.com#fragment",
            "not a URL",
        ] {
            settings.url = url.into();
            assert!(validate(&settings).is_err());
        }
    }

    #[tokio::test]
    async fn sends_custom_json_checks_bark_codes_and_never_follows_redirects() {
        // Tests do not run main(), which installs the process-wide TLS provider.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = received.clone();
        let redirected = Arc::new(AtomicUsize::new(0));
        let redirects = redirected.clone();
        let app = Router::new()
            .route(
                "/push",
                post(
                    move |headers: axum::http::HeaderMap, Json(payload): Json<Value>| {
                        captured.lock().unwrap().push((headers, payload.clone()));
                        async move {
                            Json(if payload["body"] == "reject" {
                                serde_json::json!({"code":400,"message":"private-key"})
                            } else {
                                serde_json::json!({"code":200})
                            })
                        }
                    },
                ),
            )
            .route(
                "/redirect",
                post(|| async {
                    (
                        axum::http::StatusCode::TEMPORARY_REDIRECT,
                        [("Location", "/target")],
                    )
                        .into_response()
                }),
            )
            .route(
                "/target",
                post(move || {
                    redirects.fetch_add(1, Ordering::Relaxed);
                    async { "ok" }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut settings = WebhookSettingsInput {
            enabled: true,
            preset: WebhookPreset::Bark,
            url: format!("http://{address}/push"),
            headers: "X-Token: secret-header".into(),
            body: r#"{"device_key":"private-key","title":"{{title}}","body":"{{message}}"}"#.into(),
            clear_headers: false,
        };
        let message = Message {
            title: "离线 \"节点\"",
            node: "Node",
            message: "line1\nline2",
            event: "offline",
            site: "NodeFlare",
        };
        // Keep local fixtures independent of the machine's HTTP_PROXY settings.
        let http = client_builder().no_proxy().build().unwrap();
        let success = send(&http, &settings, &message).await;
        let rejection = send(
            &http,
            &settings,
            &Message {
                message: "reject",
                ..message
            },
        )
        .await;
        settings.url = format!("http://{address}/redirect");
        let redirect = send(&http, &settings, &message).await;
        task.abort();
        success.unwrap();
        assert!(rejection.unwrap_err().to_string().contains("code=400"));
        assert!(redirect.unwrap_err().to_string().contains("307"));
        assert_eq!(redirected.load(Ordering::Relaxed), 0);
        let requests = received.lock().unwrap();
        assert_eq!(requests[0].0[CONTENT_TYPE], "application/json");
        assert_eq!(requests[0].0["x-token"], "secret-header");
        assert_eq!(requests[0].1["title"], message.title);
        assert_eq!(requests[0].1["body"], message.message);
    }
}
