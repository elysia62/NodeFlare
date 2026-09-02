mod auth;
mod cloudflare;
mod db;
mod exchange;
mod latency;
mod live;
mod models;
mod notify;
mod outbound;
mod routes;
mod theme;
mod turnstile;

use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use futures_util::TryStreamExt;
use serde::{de::DeserializeOwned, Serialize};
use worker::*;

use crate::auth::{
    bearer_token, is_admin, random_salt, sha256_hex, verify_turnstile_proof, ADMIN_SESSION_SECONDS,
};
use crate::models::{
    AgentDiskMetric, AgentGpuMetric, AgentReport, AlertRuleInput, ApiError, LatencyTaskInput,
    ServerInput, ServerView, ThemeInput,
};

pub(crate) const ADMIN_HTML: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.html"));
pub(crate) const ADMIN_SCRIPT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.js"));
pub(crate) const ADMIN_STYLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.css"));
pub(crate) const API_JSON_MAX_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_AGENT_SAMPLES: usize = 720;
const SERVER_RENEWAL_INTERVAL_SECONDS: i64 = 60 * 60;
const REMOTE_THEME_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const VERSION: &str = match option_env!("NODEFLARE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Serialize)]
struct PublicServer {
    id: String,
    name: String,
    region: String,
    group_name: String,
    tags: String,
    expires_at: Option<i64>,
    traffic_limit: i64,
    traffic_limit_type: String,
    price: f64,
    billing_cycle: i64,
    currency: String,
    auto_renewal: bool,
    reset_day: i64,
    timestamp: Option<i64>,
    cpu: Option<f64>,
    load1: Option<f64>,
    load5: Option<f64>,
    load15: Option<f64>,
    mem_used: Option<i64>,
    mem_total: Option<i64>,
    swap_used: Option<i64>,
    swap_total: Option<i64>,
    disk_used: Option<i64>,
    disk_total: Option<i64>,
    net_in: Option<f64>,
    net_out: Option<f64>,
    net_rx_total: Option<i64>,
    net_tx_total: Option<i64>,
    uptime: Option<i64>,
    processes: Option<i64>,
    tcp_connections: Option<i64>,
    udp_connections: Option<i64>,
    cpu_cores: Option<i64>,
    cpu_model: Option<String>,
    os: Option<String>,
    kernel: Option<String>,
    arch: Option<String>,
    virtualization: Option<String>,
    gpu_usage: Option<f64>,
    gpu_model: Option<String>,
    agent_version: Option<String>,
    disk_read_bps: Option<f64>,
    disk_write_bps: Option<f64>,
    disk_read_iops: Option<f64>,
    disk_write_iops: Option<f64>,
    disk_await_ms: Option<f64>,
    disk_utilization: Option<f64>,
    disks: Vec<AgentDiskMetric>,
    gpus: Vec<AgentGpuMetric>,
    latency: Vec<latency::LatencySample>,
}

pub(crate) fn now() -> i64 {
    Date::now().as_millis() as i64 / 1000
}

fn env_text(env: &Env, key: &str, fallback: &str) -> String {
    env.var(key)
        .ok()
        .map(|v| v.to_string())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_number(env: &Env, key: &str, fallback: i64) -> i64 {
    env_text(env, key, &fallback.to_string())
        .parse()
        .unwrap_or(fallback)
}

fn env_secret_text(env: &Env, key: &str) -> String {
    env.secret(key)
        .or_else(|_| env.var(key))
        .map(|value| value.to_string())
        .unwrap_or_default()
}

pub(crate) fn settings_for_admin_response(
    mut settings: db::SettingsView,
    env: &Env,
) -> db::SettingsView {
    for (field, variable) in [
        (&mut settings.turnstile_site_key, "TURNSTILE_SITE_KEY"),
        (&mut settings.turnstile_secret_key, "TURNSTILE_SECRET_KEY"),
        (&mut settings.cloudflare_account_id, "CF_USAGE_ACCOUNT_ID"),
        (&mut settings.cloudflare_api_token, "CF_USAGE_API_TOKEN"),
    ] {
        if field.trim().is_empty() && !env_secret_text(env, variable).trim().is_empty() {
            *field = db::SECRET_MASK.to_string();
        }
    }
    settings
}

fn json<T: Serialize>(value: &T, status: u16) -> Result<Response> {
    let mut response = Response::from_json(value)?.with_status(status);
    set_api_headers(&mut response)?;
    Ok(response)
}

fn set_api_headers(response: &mut Response) -> Result<()> {
    let headers = response.headers_mut();
    headers.set("Cache-Control", "no-store")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("Referrer-Policy", "no-referrer")?;
    headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()",
    )?;
    Ok(())
}

pub(crate) fn no_content() -> Result<Response> {
    let mut response = Response::empty()?.with_status(204);
    set_api_headers(&mut response)?;
    Ok(response)
}

pub(crate) fn set_admin_session_cookie(
    response: &mut Response,
    req: &Request,
    token: Option<&str>,
) -> Result<()> {
    let secure = req.url().ok().is_some_and(|url| url.scheme() == "https");
    let (value, max_age) = token
        .map(|value| (value, ADMIN_SESSION_SECONDS))
        .unwrap_or(("", 0));
    response.headers_mut().append(
        "Set-Cookie",
        &format!(
            "nodeflare_admin={value}; Path=/; Max-Age={max_age}; HttpOnly{}; SameSite=Strict",
            if secure { "; Secure" } else { "" }
        ),
    )?;
    Ok(())
}

async fn request_json<T: DeserializeOwned>(req: &mut Request, limit: usize) -> Result<T> {
    if req
        .headers()
        .get("Content-Length")?
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length == 0 || length > limit)
    {
        return Err(Error::RustError(
            "JSON request body size is invalid".to_string(),
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = req.stream()?;
    while let Some(mut chunk) = stream.try_next().await? {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > limit)
        {
            return Err(Error::RustError(
                "JSON request body is too large".to_string(),
            ));
        }
        bytes.append(&mut chunk);
    }
    serde_json::from_slice(&bytes).map_err(|error| Error::RustError(error.to_string()))
}

pub(crate) fn error(message: &str, status: u16) -> Result<Response> {
    json(
        &ApiError {
            error: message,
            request_id: None,
        },
        status,
    )
}

fn internal_error(request_id: &str) -> Result<Response> {
    let mut response = json(
        &ApiError {
            error: "服务暂时不可用，请稍后重试",
            request_id: Some(request_id),
        },
        500,
    )?;
    response.headers_mut().set("X-Request-Id", request_id)?;
    Ok(response)
}

fn request_id(req: &Request) -> String {
    req.headers()
        .get("CF-Ray")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .or_else(random_salt)
        .unwrap_or_else(|| format!("local-{}", Date::now().as_millis() as i64))
}

fn rate_limited() -> Result<Response> {
    let mut response = error("请求过于频繁，请稍后重试", 429)?;
    response.headers_mut().set("Retry-After", "60")?;
    Ok(response)
}

fn mutable_response(response: Response) -> Result<Response> {
    let status = response.status_code();
    let headers = Headers::new();
    for (name, value) in response.headers().entries() {
        headers.append(&name, &value)?;
    }
    let (_, body) = response.into_parts();
    Ok(Response::from_body(body)?
        .with_status(status)
        .with_headers(headers))
}

pub(crate) fn embedded_admin_response(body: &[u8], content_type: &str) -> Result<Response> {
    let mut response = Response::from_bytes(body.to_vec())?;
    let headers = response.headers_mut();
    headers.set("Content-Type", content_type)?;
    headers.set("Cache-Control", "no-store")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("X-Frame-Options", "DENY")?;
    headers.set("Referrer-Policy", "strict-origin-when-cross-origin")?;
    headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()",
    )?;
    headers.set(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self'; frame-src https://challenges.cloudflare.com; font-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
    )?;
    Ok(response)
}

pub(crate) fn secure_public_response(mut response: Response) -> Result<Response> {
    let headers = response.headers_mut();
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("X-Frame-Options", "DENY")?;
    headers.set("Referrer-Policy", "strict-origin-when-cross-origin")?;
    headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()",
    )?;
    headers.set(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self' ws: wss:; frame-src https://challenges.cloudflare.com; font-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
    )?;
    Ok(response)
}

fn remote_theme_content_type(path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    if path == "index.html" {
        return Some("text/html; charset=utf-8");
    }
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("js" | "mjs") => Some("application/javascript; charset=utf-8"),
        Some("css") => Some("text/css; charset=utf-8"),
        Some("json" | "map") => Some("application/json; charset=utf-8"),
        Some("svg") => Some("image/svg+xml"),
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("ico") => Some("image/x-icon"),
        Some("woff") => Some("font/woff"),
        Some("woff2") => Some("font/woff2"),
        Some("ttf") => Some("font/ttf"),
        Some("wasm") => Some("application/wasm"),
        _ => None,
    }
}

fn remote_theme_failure(message: &str) -> Result<Response> {
    let mut response = Response::error(message, 502)?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    secure_public_response(response)
}

fn rewrite_remote_theme_index(html: &str, asset_prefix: &str) -> String {
    html.replace("\"/assets/", &format!("\"{asset_prefix}"))
        .replace("'/assets/", &format!("'{asset_prefix}"))
}

async fn remote_theme_response(path: &str, base: &str) -> Result<Option<Response>> {
    let (remote_url, relative) =
        if path == "/" || path == "/index.html" || path.starts_with("/instance/") {
            let Some(index) = theme::index_url(base) else {
                return Ok(Some(remote_theme_failure("远程主题来源不受支持")?));
            };
            (index, "index.html".to_string())
        } else if let Some(url) = theme::asset_url(base, path) {
            (url, path.trim_start_matches('/').to_string())
        } else {
            return Ok(Some(remote_theme_failure("远程主题资源路径不受支持")?));
        };
    let is_index = relative == "index.html";
    let cache = Cache::default();
    let cache_key = format!("{remote_url}?nodeflare-theme-cache=2");
    if let Ok(Some(response)) = cache.get(&cache_key, false).await {
        return Ok(Some(secure_public_response(mutable_response(response)?)?));
    }
    let request = Request::new(&remote_url, Method::Get)?;
    let mut response = match outbound::fetch_with_timeout(request, REMOTE_THEME_FETCH_TIMEOUT).await
    {
        Ok(Some(response)) if (200..300).contains(&response.status_code()) => response,
        Ok(Some(response)) => {
            console_warn!(
                "remote theme returned HTTP {} for {}",
                response.status_code(),
                remote_url
            );
            return if is_index {
                Ok(None)
            } else {
                Ok(Some(remote_theme_failure("远程主题资源暂时不可用")?))
            };
        }
        Ok(None) => {
            console_warn!("remote theme request timed out for {remote_url}");
            return if is_index {
                Ok(None)
            } else {
                Ok(Some(remote_theme_failure("远程主题资源请求超时")?))
            };
        }
        Err(error) => {
            console_warn!("remote theme request failed for {remote_url}: {error}");
            return if is_index {
                Ok(None)
            } else {
                Ok(Some(remote_theme_failure("远程主题资源加载失败")?))
            };
        }
    };
    let limit = if is_index {
        theme::INDEX_MAX_BYTES
    } else {
        theme::ASSET_MAX_BYTES
    };
    let Some(body) = outbound::read_response_limited(&mut response, limit).await? else {
        return if is_index {
            Ok(None)
        } else {
            Ok(Some(remote_theme_failure("远程主题资源大小无效")?))
        };
    };
    let body = if is_index {
        let html = String::from_utf8(body)
            .map_err(|_| Error::RustError("主题 index.html 不是 UTF-8 文本".to_string()))?;
        rewrite_remote_theme_index(&html, "/__theme-active/assets/").into_bytes()
    } else {
        body
    };
    let mut response = Response::from_bytes(body)?;
    let headers = response.headers_mut();
    headers.set("Cache-Control", "public, max-age=300")?;
    if let Some(content_type) = remote_theme_content_type(&relative) {
        headers.set("Content-Type", content_type)?;
    }
    let mut response = secure_public_response(response)?;
    if let Ok(cached) = response.cloned() {
        let _ = cache.put(&cache_key, cached).await;
    }
    Ok(Some(response))
}

async fn remote_theme_preview_response(
    relative: &str,
    base: &str,
    preview_prefix: &str,
) -> Result<Response> {
    if relative.starts_with("assets/") {
        let path = format!("/{relative}");
        return Ok(remote_theme_response(&path, base)
            .await?
            .unwrap_or(remote_theme_failure("主题预览资源不存在")?));
    }
    if !relative.is_empty() && relative != "index.html" && !relative.starts_with("instance/") {
        return Response::error("主题预览资源不存在", 404);
    }
    let Some(index) = theme::index_url(base) else {
        return remote_theme_failure("远程主题来源不受支持");
    };
    let request = Request::new(&index, Method::Get)?;
    let Some(mut response) =
        outbound::fetch_with_timeout(request, REMOTE_THEME_FETCH_TIMEOUT).await?
    else {
        return remote_theme_failure("主题预览页面请求超时");
    };
    if !(200..300).contains(&response.status_code()) {
        return remote_theme_failure("主题预览页面暂时不可用");
    }
    let Some(body) = outbound::read_response_limited(&mut response, theme::INDEX_MAX_BYTES).await?
    else {
        return remote_theme_failure("主题预览页面大小无效");
    };
    let html = String::from_utf8(body)
        .map_err(|_| Error::RustError("主题 index.html 不是 UTF-8 文本".to_string()))?;
    let asset_prefix = format!("{preview_prefix}/assets/");
    let html = rewrite_remote_theme_index(&html, &asset_prefix);
    let mut response = Response::from_html(html)?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    secure_public_response(response)
}

fn valid_ping_target(value: &str) -> bool {
    let raw = value.trim();
    if raw.is_empty() {
        return false;
    }
    if raw.len() > 60
        || raw.contains("://")
        || raw
            .chars()
            .any(|character| character.is_whitespace() || ":/@?#\\[]".contains(character))
    {
        return false;
    }

    if raw.len() > 50 || raw.starts_with('.') || raw.ends_with('.') {
        return false;
    }

    let labels = raw.split('.').collect::<Vec<_>>();
    let ipv4_like = labels.len() == 4
        && labels
            .iter()
            .all(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()));
    if ipv4_like {
        return raw.parse::<Ipv4Addr>().is_ok_and(public_probe_ipv4);
    }
    let lower = raw.to_ascii_lowercase();
    if labels.len() < 2
        || ["local", "localhost", "internal", "lan", "localdomain"]
            .iter()
            .any(|suffix| lower == *suffix || lower.ends_with(&format!(".{suffix}")))
        || lower == "home.arpa"
        || lower.ends_with(".home.arpa")
    {
        return false;
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
            && label
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric())
            && label
                .chars()
                .last()
                .is_some_and(|character| character.is_ascii_alphanumeric())
    })
}

fn public_probe_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

pub(crate) fn valid_cloudflare_account_id(value: &str) -> bool {
    let value = value.trim();
    value == db::SECRET_MASK
        || value.is_empty()
        || (value.len() == 32 && value.chars().all(|character| character.is_ascii_hexdigit()))
}

pub(crate) fn valid_cloudflare_api_token(value: &str) -> bool {
    let value = value.trim();
    value.chars().count() <= 512 && !value.chars().any(char::is_whitespace)
}

pub(crate) fn valid_password_derived(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn valid_public_asset_url(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    if value.chars().count() > 1000 {
        return false;
    }
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some_and(|host| !host.is_empty())
            && url.username().is_empty()
            && url.password().is_none()
    })
}

pub(crate) fn valid_background_urls(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    if value.chars().count() > 1000 {
        return false;
    }
    let parts = value.split('|').collect::<Vec<_>>();
    parts.len() <= 2
        && parts.iter().any(|part| !part.trim().is_empty())
        && parts
            .iter()
            .all(|part| part.trim().is_empty() || valid_public_asset_url(part))
}

fn valid_server_price(value: f64) -> bool {
    value.is_finite() && (-1.0..=1_000_000_000.0).contains(&value)
}

fn submitted_secret<'a>(submitted: Option<&'a str>, stored: &'a str) -> &'a str {
    match submitted.map(str::trim) {
        Some(db::SECRET_MASK) | None => stored.trim(),
        Some(value) => value,
    }
}

fn same_origin(origin: &str, request_url: &Url) -> bool {
    origin.trim().trim_end_matches('/') == request_url.origin().ascii_serialization()
}

pub(crate) fn validate_latency_task(input: &LatencyTaskInput) -> Option<&'static str> {
    let name_len = input.name.trim().chars().count();
    if !(1..=80).contains(&name_len) {
        return Some("任务名称长度应为 1 至 80 个字符");
    }
    if !matches!(input.task_type.as_str(), "tcp" | "icmp") {
        return Some("延迟类型仅支持 TCP 或 ICMP");
    }
    let target = input.target.trim();
    if target.is_empty() || !valid_ping_target(target) {
        return Some("目标应为公网域名或公网 IPv4");
    }
    if input.task_type == "tcp" {
        if input.port.is_none_or(|port| !(1..=65535).contains(&port)) {
            return Some("TCP 端口应为 1 至 65535");
        }
    } else if input.port.is_some() {
        return Some("ICMP 检测不使用端口");
    }
    if !(30..=3600).contains(&input.interval_seconds) {
        return Some("延迟检测间隔应为 30 至 3600 秒");
    }
    if input.server_ids.len() > 500
        || input
            .server_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 80)
        || input.server_ids.iter().collect::<HashSet<_>>().len() != input.server_ids.len()
    {
        return Some("服务器选择列表无效");
    }
    None
}

pub(crate) fn validate_alert_rule(input: &AlertRuleInput) -> Option<&'static str> {
    if !(1..=80).contains(&input.name.trim().chars().count()) {
        return Some("规则名称长度应为 1 至 80 个字符");
    }
    if !matches!(
        input.metric.as_str(),
        "cpu" | "memory" | "disk" | "net_in" | "net_out"
    ) {
        return Some("告警指标无效");
    }
    let maximum = if matches!(input.metric.as_str(), "cpu" | "memory" | "disk") {
        100.0
    } else {
        1_000_000.0
    };
    if !input.threshold.is_finite() || input.threshold <= 0.0 || input.threshold > maximum {
        return Some("告警阈值超出允许范围");
    }
    if !(1..=1440).contains(&input.duration_minutes) {
        return Some("告警时间窗口应为 1 至 1440 分钟");
    }
    if !matches!(input.aggregation.as_str(), "average" | "continuous") {
        return Some("告警判断方式无效");
    }
    if input.server_ids.len() > 500
        || input
            .server_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 80)
        || input.server_ids.iter().collect::<HashSet<_>>().len() != input.server_ids.len()
    {
        return Some("告警服务器列表无效");
    }
    None
}

pub(crate) fn validate_theme(input: &ThemeInput) -> Option<&'static str> {
    if !(1..=80).contains(&input.name.trim().chars().count()) {
        return Some("主题名称长度应为 1 至 80 个字符");
    }
    if input.description.trim().chars().count() > 300 {
        return Some("主题说明不能超过 300 个字符");
    }
    if theme::normalize_url(&input.url).is_none() {
        return Some("主题 URL 仅支持 GitHub 地址");
    }
    None
}

pub(crate) fn validate_server(input: &ServerInput) -> Option<&'static str> {
    let name_len = input.name.trim().chars().count();
    if !(1..=80).contains(&name_len) {
        return Some("节点名称长度应为 1 至 80 个字符");
    }
    if input.region.chars().count() > 16 || input.group_name.chars().count() > 40 {
        return Some("地区或分组字段过长");
    }
    if input.tags.chars().count() > 240 {
        return Some("标签字段过长");
    }
    if input.traffic_limit < 0 {
        return Some("流量限额不能为负数");
    }
    if !matches!(
        input.traffic_limit_type.as_str(),
        "sum" | "max" | "min" | "up" | "down"
    ) {
        return Some("流量计算方式无效");
    }
    if !valid_server_price(input.price) {
        return Some("价格无效");
    }
    if !(0..=3650).contains(&input.billing_cycle) {
        return Some("计费周期应为 0（一次性）或 1 至 3650 天");
    }
    if input.currency.len() != 3
        || !input
            .currency
            .chars()
            .all(|value| value.is_ascii_alphabetic())
    {
        return Some("币种应为 3 位字母代码");
    }
    if !(1..=31).contains(&input.reset_day) {
        return Some("流量重置日应为 1 至 31");
    }
    if !(15..=3600).contains(&input.report_interval) {
        return Some("上报间隔应为 15 至 3600 秒");
    }
    if !(1..=60).contains(&input.collect_interval) || input.collect_interval > input.report_interval
    {
        return Some("采样间隔应为 1 至 60 秒且不能大于上报间隔");
    }
    if (input.report_interval + input.collect_interval - 1) / input.collect_interval
        > MAX_AGENT_SAMPLES as i64
    {
        return Some("每个上报周期最多采集 720 个样本");
    }
    if input.network_interface.chars().count() > 160 {
        return Some("统计网卡配置字段过长");
    }
    if !valid_agent_mirror(&input.agent_mirror) {
        return Some("Agent 下载加速地址必须是无凭据、查询参数和片段的 HTTPS URL");
    }
    None
}

pub(crate) fn valid_agent_mirror(value: &str) -> bool {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return true;
    }
    if value.len() > 2048
        || value.chars().any(char::is_whitespace)
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_./:@-".contains(character))
    {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    if url.scheme() == "https" {
        return true;
    }
    url.scheme() == "http"
        && matches!(
            url.host_str(),
            Some(host) if host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
        )
}

pub(crate) fn validate_report(report: &AgentReport) -> Option<&'static str> {
    let floats = [
        report.cpu,
        report.load1,
        report.load5,
        report.load15,
        report.net_in,
        report.net_out,
        report.gpu_usage,
        report.disk_read_bps,
        report.disk_write_bps,
        report.disk_read_iops,
        report.disk_write_iops,
        report.disk_await_ms,
        report.disk_utilization,
    ];
    if floats
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Some("指标包含非法数值");
    }
    if !(0.0..=100.0).contains(&report.cpu) || !(0.0..=100.0).contains(&report.gpu_usage) {
        return Some("CPU 或 GPU 使用率超出范围");
    }
    let counters = [
        report.mem_used,
        report.mem_total,
        report.swap_used,
        report.swap_total,
        report.disk_used,
        report.disk_total,
        report.net_rx_total,
        report.net_tx_total,
        report.uptime,
        report.processes,
        report.tcp_connections,
        report.udp_connections,
        report.cpu_cores,
    ];
    if counters.iter().any(|value| *value < 0) {
        return Some("计数指标不能为负数");
    }
    if report.mem_used > report.mem_total
        || report.swap_used > report.swap_total
        || report.disk_used > report.disk_total
    {
        return Some("已用资源不能超过总量");
    }
    if report.gpu_model.chars().count() > 240 {
        return Some("GPU 型号字段过长");
    }
    if report.agent_version.chars().count() > 80 {
        return Some("Agent 版本字段过长");
    }
    if report.ip_v4.chars().count() > 15
        || (!report.ip_v4.is_empty() && report.ip_v4.parse::<Ipv4Addr>().is_err())
        || report.ip_v6.chars().count() > 45
        || (!report.ip_v6.is_empty() && report.ip_v6.parse::<Ipv6Addr>().is_err())
    {
        return Some("公网地址字段无效");
    }
    if report.disks.len() > 64
        || report.gpus.len() > 32
        || report.disks.iter().any(|disk| {
            disk.name.chars().count() > 120
                || disk.mount_point.chars().count() > 240
                || disk.used < 0
                || disk.total < 0
                || disk.used > disk.total
                || ![
                    disk.read_bps,
                    disk.write_bps,
                    disk.read_iops,
                    disk.write_iops,
                    disk.await_ms,
                    disk.utilization,
                ]
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
        })
        || report.gpus.iter().any(|gpu| {
            gpu.model.chars().count() > 240
                || gpu.memory_used < 0
                || gpu.memory_total < 0
                || gpu.memory_used > gpu.memory_total
                || gpu
                    .usage
                    .is_some_and(|usage| !usage.is_finite() || !(0.0..=100.0).contains(&usage))
        })
    {
        return Some("磁盘或 GPU 明细无效");
    }
    if report.latency_results.len() > 128
        || report
            .latency_results
            .iter()
            .map(|result| &result.task_id)
            .collect::<HashSet<_>>()
            .len()
            != report.latency_results.len()
    {
        return Some("延迟结果列表无效");
    }
    if report.latency_results.iter().any(|result| {
        result.task_id.is_empty()
            || result.task_id.len() > 80
            || result.timestamp <= 0
            || !result.latency_ms.is_finite()
            || !result.packet_loss.is_finite()
            || !(-1.0..=100_000.0).contains(&result.latency_ms)
            || !(0.0..=100.0).contains(&result.packet_loss)
    }) {
        return Some("延迟结果数值无效");
    }
    None
}

pub(crate) fn public_server(
    server: ServerView,
    latency: Vec<latency::LatencySample>,
) -> PublicServer {
    let disks = server
        .disk_info
        .as_deref()
        .and_then(|value| serde_json::from_str::<Vec<AgentDiskMetric>>(value).ok())
        .unwrap_or_default();
    let gpus = server
        .gpu_info
        .as_deref()
        .and_then(|value| serde_json::from_str::<Vec<AgentGpuMetric>>(value).ok())
        .unwrap_or_default();
    PublicServer {
        id: server.id,
        name: server.name,
        region: server.region,
        group_name: server.group_name,
        tags: server.tags,
        expires_at: server.expires_at,
        traffic_limit: server.traffic_limit,
        traffic_limit_type: server.traffic_limit_type,
        price: server.price,
        billing_cycle: server.billing_cycle,
        currency: server.currency,
        auto_renewal: server.auto_renewal != 0,
        reset_day: server.reset_day,
        timestamp: server.timestamp,
        cpu: server.cpu,
        load1: server.load1,
        load5: server.load5,
        load15: server.load15,
        mem_used: server.mem_used,
        mem_total: server.mem_total,
        swap_used: server.swap_used,
        swap_total: server.swap_total,
        disk_used: server.disk_used,
        disk_total: server.disk_total,
        net_in: server.net_in,
        net_out: server.net_out,
        net_rx_total: server.net_rx_total,
        net_tx_total: server.net_tx_total,
        uptime: server.uptime,
        processes: server.processes,
        tcp_connections: server.tcp_connections,
        udp_connections: server.udp_connections,
        cpu_cores: server.cpu_cores,
        cpu_model: server.cpu_model,
        os: server.os,
        kernel: server.kernel,
        arch: server.arch,
        virtualization: server.virtualization,
        gpu_usage: server.gpu_usage,
        gpu_model: server.gpu_model,
        agent_version: server.agent_version,
        disk_read_bps: server.disk_read_bps,
        disk_write_bps: server.disk_write_bps,
        disk_read_iops: server.disk_read_iops,
        disk_write_iops: server.disk_write_iops,
        disk_await_ms: server.disk_await_ms,
        disk_utilization: server.disk_utilization,
        disks,
        gpus,
        latency,
    }
}

fn request_cookie(req: &Request, name: &str) -> Option<String> {
    let raw = req.headers().get("Cookie").ok().flatten()?;
    raw.split(';').find_map(|entry| {
        let (key, value) = entry.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

pub(crate) fn client_ip(req: &Request) -> Option<String> {
    req.headers()
        .get("CF-Connecting-IP")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("X-Forwarded-For").ok().flatten())
        .map(|value| value.split(',').next().unwrap_or("").trim().to_string())
        .filter(|value| !value.is_empty())
}

fn rate_limit_binding(method: Method, path: &str) -> Option<&'static str> {
    if method == Method::Post && matches!(path, "/api/admin/login" | "/api/turnstile/verify") {
        Some("AUTH_RATE_LIMITER")
    } else if method == Method::Get && path == "/api/agent/ws" {
        Some("AGENT_RATE_LIMITER")
    } else if path.starts_with("/api/") {
        Some("API_RATE_LIMITER")
    } else {
        None
    }
}

fn rate_limit_key(req: &Request) -> String {
    let identity = client_ip(req).unwrap_or_else(|| {
        req.headers()
            .get("User-Agent")
            .ok()
            .flatten()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "anonymous".to_string())
    });
    sha256_hex(&identity)
}

fn agent_rate_limit_key(req: &Request) -> String {
    bearer_token(req)
        .filter(|value| !value.is_empty())
        .map(|value| sha256_hex(&value))
        .unwrap_or_else(|| rate_limit_key(req))
}

fn allow_on_rate_limit_failure(binding: &str) -> bool {
    let _ = binding;
    false
}

async fn request_within_rate_limit(env: &Env, binding: &str, key: String) -> bool {
    let limiter = match env.rate_limiter(binding) {
        Ok(value) => value,
        Err(error) => {
            console_warn!("rate limit binding {binding} unavailable: {error}");
            return allow_on_rate_limit_failure(binding);
        }
    };
    match limiter.limit(key).await {
        Ok(outcome) => outcome.success,
        Err(error) => {
            console_warn!("rate limit check failed for {binding}: {error}");
            allow_on_rate_limit_failure(binding)
        }
    }
}

pub(crate) fn requested_hours(req: &Request, maximum: i64) -> Result<i64> {
    Ok(req
        .url()?
        .query_pairs()
        .find(|(key, _)| key == "hours")
        .and_then(|(_, value)| value.parse::<i64>().ok())
        .unwrap_or(24)
        .clamp(1, maximum))
}

pub(crate) fn history_cache_key(
    request_url: &Url,
    kind: &str,
    server_id: &str,
    hours: i64,
    version: i64,
) -> String {
    let mut url = request_url.clone();
    url.set_path(&format!("/__nodeflare-cache/{kind}/{server_id}"));
    url.set_query(Some(&format!("hours={hours}&version={version}")));
    url.set_fragment(None);
    url.to_string()
}

pub(crate) fn history_cache_seconds(hours: i64) -> i64 {
    match hours {
        120.. => 10 * 60,
        60.. => 5 * 60,
        30.. => 3 * 60,
        _ => 60,
    }
}

async fn cached_history_response(key: &str) -> Option<Response> {
    match Cache::default().get(key, false).await {
        Ok(Some(response)) => {
            let mut response = mutable_response(response).ok()?;
            response
                .headers_mut()
                .set("Cache-Control", "no-store")
                .ok()?;
            response.headers_mut().set("X-Cache", "HIT").ok()?;
            Some(response)
        }
        Ok(None) => None,
        Err(error) => {
            console_warn!("history cache read failed: {error}");
            None
        }
    }
}

async fn store_history_response(key: &str, response: &mut Response, ttl: i64) {
    if response.headers_mut().set("X-Cache", "MISS").is_err() {
        return;
    }
    let Ok(mut cached) = response.cloned() else {
        return;
    };
    if cached
        .headers_mut()
        .set(
            "Cache-Control",
            &format!("public, max-age={}", ttl.clamp(60, 10 * 60)),
        )
        .is_err()
    {
        return;
    }
    if let Err(error) = Cache::default().put(key, cached).await {
        console_warn!("history cache write failed: {error}");
    }
}

fn request_turnstile_proof(req: &Request) -> Option<String> {
    request_cookie(req, "nodeflare_turnstile")
}

pub(crate) fn server_id(path: &str, prefix: &str) -> Option<String> {
    let id = path.strip_prefix(prefix)?.trim_matches('/');
    if id.is_empty() || id.contains('/') || id.len() > 80 {
        None
    } else {
        Some(id.to_string())
    }
}

async fn handle(req: Request, env: Env) -> Result<Response> {
    let method = req.method();
    let path = req.path();

    if method == Method::Get {
        if let Some(response) = routes::site::embedded_asset(&path)? {
            return Ok(response);
        }
    }

    let database = env.d1("DB")?;

    if method == Method::Get && path == "/api/agent/ws" {
        return routes::agent::websocket(req, &env, &database).await;
    }

    let default_name = env_text(&env, "SITE_NAME", "NodeFlare");
    let default_threshold = env_number(&env, "OFFLINE_THRESHOLD_SECONDS", 180).clamp(30, 3600);
    let default_retention =
        env_number(&env, "HISTORY_RETENTION_DAYS", 30).clamp(1, db::MAX_HISTORY_RETENTION_DAYS);
    let default_username = env_text(&env, "ADMIN_USERNAME", "");
    let settings = db::settings(
        &database,
        &default_name,
        default_threshold,
        default_retention,
        &default_username,
    )
    .await?;
    let environment_turnstile_site_key = env_secret_text(&env, "TURNSTILE_SITE_KEY");
    let environment_turnstile_secret_key = env_secret_text(&env, "TURNSTILE_SECRET_KEY");
    let turnstile_site_key = if settings.turnstile_site_key.trim().is_empty() {
        environment_turnstile_site_key.clone()
    } else {
        settings.turnstile_site_key.clone()
    };
    let turnstile_secret_key = if settings.turnstile_secret_key.trim().is_empty() {
        environment_turnstile_secret_key.clone()
    } else {
        settings.turnstile_secret_key.clone()
    };
    let turnstile_configured =
        !turnstile_site_key.trim().is_empty() && !turnstile_secret_key.trim().is_empty();
    let public_turnstile_enabled = settings.turnstile_enabled && turnstile_configured;
    let login_protection_enabled = settings.turnstile_login_enabled && turnstile_configured;

    let admin = is_admin(&req, &env, &settings.admin_password_hash);
    let turnstile_verified = !public_turnstile_enabled
        || admin
        || request_turnstile_proof(&req).is_some_and(|value| {
            verify_turnstile_proof(&value, &env, &settings.admin_password_hash)
        });

    let context = routes::RouteContext {
        env: env.clone(),
        database,
        settings,
        admin,
        turnstile_verified,
        turnstile_site_key,
        turnstile_secret_key,
        public_turnstile_enabled,
        login_protection_enabled,
        environment_turnstile_site_key,
        environment_turnstile_secret_key,
        default_name,
        default_threshold,
        default_retention,
        default_username,
    };

    let req = match routes::public::route(req, &context).await? {
        routes::RouteOutcome::Handled(response) => return Ok(response),
        routes::RouteOutcome::Unmatched(req) => req,
    };
    let req = match routes::admin::route(req, &context).await? {
        routes::RouteOutcome::Handled(response) => return Ok(response),
        routes::RouteOutcome::Unmatched(req) => req,
    };
    if path.starts_with("/api/") {
        return error("接口不存在", 404);
    }
    let req = match routes::site::route(req, &context).await? {
        routes::RouteOutcome::Handled(response) => return Ok(response),
        routes::RouteOutcome::Unmatched(req) => req,
    };
    let response = env.assets("ASSETS")?.fetch_request(req).await?;
    secure_public_response(mutable_response(response)?)
}
#[event(fetch, respond_with_errors)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let method = req.method();
    let method_name = format!("{method:?}");
    let path = req.path();
    let request_id = request_id(&req);
    let request_url = req.url().ok();
    let origin = req.headers().get("Origin").ok().flatten();

    if path.starts_with("/api/")
        && origin
            .as_deref()
            .zip(request_url.as_ref())
            .is_some_and(|(origin, url)| !same_origin(origin, url))
    {
        return error("请求来源未获授权", 403);
    }

    if let Some(binding) = rate_limit_binding(method, &path) {
        let key = if binding == "AGENT_RATE_LIMITER" {
            agent_rate_limit_key(&req)
        } else {
            rate_limit_key(&req)
        };
        if !request_within_rate_limit(&env, binding, key).await {
            return rate_limited();
        }
    }

    match handle(req, env).await {
        Ok(response) => Ok(response),
        Err(err) => {
            console_error!(
                "{}",
                serde_json::json!({
                    "event": "request_failed",
                    "request_id": request_id,
                    "method": method_name,
                    "path": path,
                    "error": err.to_string(),
                })
            );
            internal_error(&request_id)
        }
    }
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    let Ok(database) = env.d1("DB") else {
        return;
    };
    let default_name = env_text(&env, "SITE_NAME", "NodeFlare");
    let default_threshold = env_number(&env, "OFFLINE_THRESHOLD_SECONDS", 180).clamp(30, 3600);
    let default_retention =
        env_number(&env, "HISTORY_RETENTION_DAYS", 30).clamp(1, db::MAX_HISTORY_RETENTION_DAYS);
    let default_username = env_text(&env, "ADMIN_USERNAME", "");
    let settings = db::settings(
        &database,
        &default_name,
        default_threshold,
        default_retention,
        &default_username,
    )
    .await;
    let retention = match &settings {
        Ok(settings) => settings.history_retention_days,
        Err(_) => default_retention,
    };
    let current = now();
    let last_cleanup = db::get_setting(&database, "last_history_cleanup")
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if current.saturating_sub(last_cleanup) >= db::HISTORY_ROTATION_INTERVAL_SECONDS {
        if let Err(err) = db::cleanup_history(&database, retention).await {
            console_error!("history cleanup failed: {err}");
        } else {
            let _ = db::save_setting(&database, "last_history_cleanup", &current.to_string()).await;
        }
        if let Err(err) = notify::cleanup(&database, current).await {
            console_error!("notification cleanup failed: {err}");
        }
    }
    match exchange::refresh(&database, current, false).await {
        Ok((rates, true)) => console_log!(
            "exchange rates updated from {} for {}",
            rates.source,
            rates.date
        ),
        Ok((_, false)) => {}
        Err(err) => console_error!("exchange-rate refresh failed: {err}"),
    }
    let last_server_renewal = db::get_setting(&database, "last_server_renewal")
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if current.saturating_sub(last_server_renewal) >= SERVER_RENEWAL_INTERVAL_SECONDS {
        if let Err(err) = notify::renew_servers(&database).await {
            console_error!("server renewal failed: {err}");
        } else {
            let _ = db::save_setting(&database, "last_server_renewal", &current.to_string()).await;
        }
    }
    if let Ok(settings) = settings {
        if let Err(err) = notify::check_alerts(&database, &env, &settings).await {
            console_error!("alert check failed: {err}");
        }
        if settings.notification_enabled {
            if let Err(err) = notify::process_pending(&database, current).await {
                console_error!("notification delivery processing failed: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        allow_on_rate_limit_failure, history_cache_key, history_cache_seconds, rate_limit_binding,
        rewrite_remote_theme_index, same_origin, submitted_secret, valid_agent_mirror,
        valid_background_urls, valid_cloudflare_account_id, valid_cloudflare_api_token,
        valid_password_derived, valid_ping_target, valid_public_asset_url, validate_latency_task,
        validate_report, validate_server,
    };
    use crate::{
        db::SECRET_MASK, models::AgentReport, models::LatencyTaskInput, models::ServerInput,
    };
    use worker::{Method, Url};

    #[test]
    fn validates_report_public_ips() {
        let mut report = AgentReport::default();
        assert_eq!(validate_report(&report), None);
        report.ip_v4 = "203.0.113.7".into();
        report.ip_v6 = "2001:db8::1".into();
        assert_eq!(validate_report(&report), None);
        report.ip_v4 = "2001:db8::1".into();
        assert_eq!(validate_report(&report), Some("公网地址字段无效"));
        report.ip_v4 = "203.0.113.7".into();
        report.ip_v6 = "203.0.113.7".into();
        assert_eq!(validate_report(&report), Some("公网地址字段无效"));
        report.ip_v4 = "not-an-ip".into();
        report.ip_v6.clear();
        assert_eq!(validate_report(&report), Some("公网地址字段无效"));
        report.ip_v4.clear();
        assert_eq!(validate_report(&report), None);
    }

    #[test]
    fn requires_public_ip_fields_in_agent_reports() {
        for field in ["ip_v4", "ip_v6"] {
            let mut report = serde_json::to_value(AgentReport::default()).unwrap();
            report.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<AgentReport>(report).is_err());
        }
    }

    #[test]
    fn assigns_rate_limits_by_route() {
        assert_eq!(
            rate_limit_binding(Method::Post, "/api/admin/login"),
            Some("AUTH_RATE_LIMITER")
        );
        assert_eq!(
            rate_limit_binding(Method::Get, "/api/agent/ws"),
            Some("AGENT_RATE_LIMITER")
        );
        assert_eq!(
            rate_limit_binding(Method::Get, "/api/history/node-a"),
            Some("API_RATE_LIMITER")
        );
        assert_eq!(rate_limit_binding(Method::Get, "/"), None);
        assert!(!allow_on_rate_limit_failure("AUTH_RATE_LIMITER"));
        assert!(!allow_on_rate_limit_failure("API_RATE_LIMITER"));
    }

    #[test]
    fn builds_versioned_history_cache_keys() {
        let request_url =
            Url::parse("https://status.example/api/history/node-a?hours=1").expect("request URL");
        assert_eq!(
            history_cache_key(&request_url, "metrics", "node-a", 24, 7),
            "https://status.example/__nodeflare-cache/metrics/node-a?hours=24&version=7"
        );
        assert!(same_origin("https://status.example", &request_url));
        assert!(!same_origin("https://other.example", &request_url));
        assert_eq!(history_cache_seconds(1), 60);
        assert_eq!(history_cache_seconds(30), 180);
        assert_eq!(history_cache_seconds(60), 300);
        assert_eq!(history_cache_seconds(120), 600);
    }

    #[test]
    fn bounds_samples_buffered_between_reports() {
        let mut server = ServerInput {
            name: "node-a".to_string(),
            region: String::new(),
            group_name: String::new(),
            tags: String::new(),
            hidden: false,
            expires_at: None,
            traffic_limit: 0,
            traffic_limit_type: "sum".to_string(),
            price: 0.0,
            billing_cycle: 30,
            currency: "CNY".to_string(),
            auto_renewal: false,
            network_interface: String::new(),
            reset_day: 1,
            report_interval: 3600,
            collect_interval: 5,
            rx_correction: 0,
            tx_correction: 0,
            agent_mirror: String::new(),
            offline_notify_disabled: false,
            auto_update: true,
        };
        assert_eq!(validate_server(&server), None);
        server.collect_interval = 4;
        assert_eq!(
            validate_server(&server),
            Some("每个上报周期最多采集 720 个样本")
        );
    }

    #[test]
    fn validates_agent_mirrors() {
        for mirror in [
            "",
            "https://mirror.example.com",
            "https://mirror.example.com/github",
            "http://127.0.0.1:8080",
            "http://localhost:8080",
        ] {
            assert!(
                valid_agent_mirror(mirror),
                "expected valid mirror: {mirror}"
            );
        }
        for mirror in [
            "https://",
            "http://mirror.example.com",
            "https://user@mirror.example.com",
            "https://mirror.example.com/?token=secret",
            "https://mirror.example.com/#fragment",
            "https://bad host.example.com",
        ] {
            assert!(
                !valid_agent_mirror(mirror),
                "expected invalid mirror: {mirror}"
            );
        }
    }

    #[test]
    fn validates_latency_targets() {
        for target in ["gd-ct-dualstack.ip.zstaticcdn.com", "1.1.1.1"] {
            assert!(valid_ping_target(target), "expected valid target: {target}");
        }

        for target in [
            "",
            "https://example.com",
            "example.com/path",
            "example.com:443",
            "example.com:0",
            "example.com:65536",
            "example.com:abc",
            "999.1.1.1",
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "localhost",
            "router.local",
            "metadata.google.internal",
            "Metadata.Google.INTERNAL",
            "router_1.example.com",
            "[::1]:443",
            "bad host",
            "-example.com",
        ] {
            assert!(
                !valid_ping_target(target),
                "expected invalid target: {target}"
            );
        }
    }

    #[test]
    fn validates_dynamic_latency_tasks() {
        let mut task = LatencyTaskInput {
            name: "Cloudflare TCP".to_string(),
            task_type: "tcp".to_string(),
            target: "1.1.1.1".to_string(),
            port: Some(443),
            interval_seconds: 60,
            default_enabled: false,
            server_ids: vec!["node-a".to_string()],
        };
        assert_eq!(validate_latency_task(&task), None);

        task.task_type = "icmp".to_string();
        assert_eq!(validate_latency_task(&task), Some("ICMP 检测不使用端口"));
        task.port = None;
        task.server_ids.clear();
        // 空服务器列表且未开默认分配：任务先保存，之后可再分配。
        assert_eq!(validate_latency_task(&task), None);
        task.default_enabled = true;
        assert_eq!(validate_latency_task(&task), None);
    }

    #[test]
    fn validates_cloudflare_usage_credentials() {
        assert!(valid_cloudflare_account_id(""));
        assert!(valid_cloudflare_account_id(SECRET_MASK));
        assert!(valid_cloudflare_account_id(
            "0123456789abcdef0123456789ABCDEF"
        ));
        assert!(!valid_cloudflare_account_id("account-id"));
        assert!(valid_cloudflare_api_token(""));
        assert!(valid_cloudflare_api_token("token_abc-123"));
        assert!(!valid_cloudflare_api_token("token with spaces"));
        assert_eq!(submitted_secret(Some(SECRET_MASK), "stored"), "stored");
        assert_eq!(
            submitted_secret(Some("replacement"), "stored"),
            "replacement"
        );
        assert_eq!(submitted_secret(Some(""), "stored"), "");
    }

    #[test]
    fn validates_client_password_derivations() {
        assert!(valid_password_derived(&"a1".repeat(32)));
        assert!(!valid_password_derived(&"A1".repeat(32)));
        assert!(!valid_password_derived(&"a1".repeat(31)));
        assert!(!valid_password_derived(&"zz".repeat(32)));
    }

    #[test]
    fn validates_public_asset_urls() {
        assert!(valid_public_asset_url(""));
        assert!(valid_public_asset_url("https://cdn.example.com/logo.svg"));
        assert!(valid_public_asset_url(
            "https://cdn.example.com/background.webp?size=large#hero"
        ));
        assert!(!valid_public_asset_url("http://cdn.example.com/logo.svg"));
        assert!(!valid_public_asset_url("/logo.svg"));
        assert!(!valid_public_asset_url(
            "https://user:pass@example.com/logo.svg"
        ));

        assert!(valid_background_urls(""));
        assert!(valid_background_urls(
            "https://cdn.example.com/light.webp | https://cdn.example.com/dark.webp"
        ));
        assert!(valid_background_urls(
            "https://cdn.example.com/light.webp |"
        ));
        assert!(valid_background_urls("| https://cdn.example.com/dark.webp"));
        assert!(!valid_background_urls("|"));
        assert!(!valid_background_urls(
            "https://cdn.example.com/a.webp | https://cdn.example.com/b.webp | https://cdn.example.com/c.webp"
        ));
        assert!(!valid_background_urls("javascript:alert(1)"));
    }

    #[test]
    fn namespaces_remote_theme_assets() {
        let html = r#"<script src="/assets/app.js"></script><link href='/assets/app.css'>"#;
        assert_eq!(
            rewrite_remote_theme_index(html, "/__theme-active/assets/"),
            r#"<script src="/__theme-active/assets/app.js"></script><link href='/__theme-active/assets/app.css'>"#
        );
    }
}
