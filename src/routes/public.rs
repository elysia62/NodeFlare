use std::collections::HashMap;

use worker::{Method, Request, Response, Result};

use crate::auth::{
    create_admin_jwt, create_turnstile_proof, hash_password, random_salt, verify_credentials,
};
use crate::client_ip;
use crate::models::LoginRequest;
use crate::routes::{RouteContext, RouteOutcome};
use crate::turnstile;
use crate::{
    cached_history_response, db, error, history_cache_key, history_cache_seconds, json, latency,
    live, no_content, now, public_server, request_json, requested_hours, server_id,
    set_admin_session_cookie, store_history_response, API_JSON_MAX_BYTES,
};

fn request_hostname(req: &Request) -> Option<String> {
    req.url()
        .ok()?
        .host_str()
        .map(|hostname| hostname.to_string())
}

pub(crate) async fn route(mut req: Request, ctx: &RouteContext) -> Result<RouteOutcome> {
    let method = req.method();
    let path = req.path();

    if method == Method::Get && path == "/api/bootstrap" {
        return Ok(RouteOutcome::Handled(bootstrap(ctx).await?));
    }
    if method == Method::Get && path == "/api/config" {
        return Ok(RouteOutcome::Handled(config(ctx)?));
    }
    if method == Method::Post && path == "/api/admin/login" {
        return Ok(RouteOutcome::Handled(admin_login(&mut req, ctx).await?));
    }
    if method == Method::Post && path == "/api/turnstile/verify" {
        return Ok(RouteOutcome::Handled(
            turnstile_verify(&mut req, ctx).await?,
        ));
    }
    if method == Method::Get && path == "/api/exchange-rates" {
        return Ok(RouteOutcome::Handled(exchange_rates(ctx).await?));
    }
    if method == Method::Get && path == "/api/ws" {
        if !ctx.turnstile_verified {
            return Ok(RouteOutcome::Handled(error(
                "请先完成 Cloudflare 人机验证",
                403,
            )?));
        }
        if !ctx.settings.public_dashboard && !ctx.admin {
            return Ok(RouteOutcome::Handled(error("此仪表盘需要登录后访问", 401)?));
        }
        return Ok(RouteOutcome::Handled(live::upgrade(req, &ctx.env).await?));
    }
    if method == Method::Get && path == "/api/servers" {
        return Ok(RouteOutcome::Handled(servers(ctx).await?));
    }
    if method == Method::Get && path.starts_with("/api/history/") {
        return Ok(RouteOutcome::Handled(history(&req, ctx).await?));
    }
    if method == Method::Get && path.starts_with("/api/latency/") {
        return Ok(RouteOutcome::Handled(latency(&req, ctx).await?));
    }
    Ok(RouteOutcome::Unmatched(req))
}

fn public_access_denied(ctx: &RouteContext) -> Option<Result<Response>> {
    if !ctx.turnstile_verified {
        return Some(error("请先完成 Cloudflare 人机验证", 403));
    }
    if !ctx.settings.public_dashboard && !ctx.admin {
        return Some(error("此仪表盘需要登录后访问", 401));
    }
    None
}

fn config_value(ctx: &RouteContext) -> serde_json::Value {
    let settings = &ctx.settings;
    serde_json::json!({
        "site_name": settings.site_name,
        "site_description": settings.site_description,
        "site_announcement": settings.site_announcement,
        "logo_url": settings.logo_url,
        "favicon_url": settings.favicon_url,
        "locale": settings.locale,
        "public_dashboard": settings.public_dashboard,
        "offline_threshold_seconds": settings.offline_threshold_seconds,
        "history_retention_days": settings.history_retention_days,
        "default_theme": settings.default_theme,
        "active_theme_id": settings.active_theme_id,
        "background_url": settings.background_url,
        "theme_options": settings.theme_options,
        "show_search": settings.show_search,
        "show_groups": settings.show_groups,
        "show_stats": settings.show_stats,
        "show_assets": settings.show_assets,
        "show_traffic": settings.show_traffic,
        "show_speed": settings.show_speed,
        "show_price": settings.show_price,
        "show_expiry": settings.show_expiry,
        "show_latency": settings.show_latency,
        "show_uptime": settings.show_uptime,
        "turnstile_enabled": ctx.public_turnstile_enabled,
        "turnstile_login_enabled": ctx.login_protection_enabled || ctx.public_turnstile_enabled,
        "turnstile_site_key": ctx.turnstile_site_key,
        "password_client_salt": settings.password_client_salt
    })
}

fn config(ctx: &RouteContext) -> Result<Response> {
    let mut response = json(&config_value(ctx), 200)?;
    response
        .headers_mut()
        .set("X-NodeFlare-Server-Time", &now().to_string())?;
    Ok(response)
}

async fn server_values(ctx: &RouteContext) -> Result<Vec<serde_json::Value>> {
    let raw_servers = db::list_servers(&ctx.database, false).await?;
    let mut latency_by_server: HashMap<String, Vec<latency::LatencySample>> = HashMap::new();
    for sample in latency::latest_all(&ctx.database).await? {
        latency_by_server
            .entry(sample.server_id.clone())
            .or_default()
            .push(sample);
    }
    raw_servers
        .into_iter()
        .map(|server| {
            let samples = latency_by_server.remove(&server.id).unwrap_or_default();
            serde_json::to_value(public_server(server, samples)).map_err(Into::into)
        })
        .collect()
}

async fn bootstrap(ctx: &RouteContext) -> Result<Response> {
    let access = if !ctx.turnstile_verified {
        "turnstile"
    } else if !ctx.settings.public_dashboard && !ctx.admin {
        "login"
    } else {
        "ok"
    };
    let config = config_value(ctx);
    if access != "ok" {
        return json(
            &serde_json::json!({
                "config": config,
                "access": access,
                "servers": [],
                "exchange_rates": null
            }),
            200,
        );
    }
    let servers = server_values(ctx).await?;
    let exchange_rates = crate::exchange::current(&ctx.database, now()).await?;
    json(
        &serde_json::json!({
            "config": config,
            "access": access,
            "servers": servers,
            "exchange_rates": exchange_rates
        }),
        200,
    )
}

async fn admin_login(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    let settings = &ctx.settings;
    if settings.admin_username.trim().is_empty() {
        return error("尚未配置 ADMIN_USERNAME", 503);
    }
    if settings.admin_password_hash.is_empty() && ctx.env.secret("ADMIN_PASSWORD").is_err() {
        return error("尚未配置 ADMIN_PASSWORD", 503);
    }
    let input: LoginRequest = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("请求格式无效", 400),
    };
    if input.username.trim().is_empty()
        || input.password.is_empty()
        || !crate::valid_password_derived(&input.password_derived)
    {
        return error("请输入用户名和密码", 400);
    }
    let login_turnstile_enabled = ctx.public_turnstile_enabled || ctx.login_protection_enabled;
    if login_turnstile_enabled {
        let Some(hostname) = request_hostname(req) else {
            return error("请求主机名无效", 400);
        };
        let verified = turnstile::verify(
            &input.turnstile_token,
            &ctx.turnstile_secret_key,
            client_ip(req).as_deref(),
            &hostname,
            turnstile::ADMIN_LOGIN_ACTION,
        )
        .await
        .unwrap_or(false);
        if !verified {
            return error("Cloudflare 人机验证失败，请重试", 403);
        }
    }
    if !verify_credentials(
        &ctx.env,
        &settings.admin_username,
        &settings.admin_password_hash,
        &input.username,
        &input.password,
        &input.password_derived,
    ) {
        return error("用户名或密码错误", 401);
    }
    let session_password_hash = if settings.admin_password_hash.is_empty() {
        let Some(salt) = random_salt() else {
            return error("运行时随机源不可用，无法初始化管理员密码", 503);
        };
        let password_hash = hash_password(&input.password_derived, &salt);
        db::save_setting(&ctx.database, "admin_password_hash", &password_hash).await?;
        password_hash
    } else {
        settings.admin_password_hash.clone()
    };
    let Some(token) = create_admin_jwt(&ctx.env, &session_password_hash) else {
        return error("会话密钥未配置", 503);
    };
    let mut response = json(&serde_json::json!({ "token": token }), 200)?;
    set_admin_session_cookie(&mut response, req, Some(&token))?;
    Ok(response)
}

async fn turnstile_verify(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    if !ctx.public_turnstile_enabled {
        return error("全站人机验证未启用", 400);
    }
    let input: crate::models::TurnstileVerifyRequest =
        match request_json(req, API_JSON_MAX_BYTES).await {
            Ok(value) => value,
            Err(_) => return error("验证请求格式无效", 400),
        };
    let Some(hostname) = request_hostname(req) else {
        return error("请求主机名无效", 400);
    };
    let verified = turnstile::verify(
        &input.token,
        &ctx.turnstile_secret_key,
        client_ip(req).as_deref(),
        &hostname,
        turnstile::PUBLIC_DASHBOARD_ACTION,
    )
    .await
    .unwrap_or(false);
    if !verified {
        return error("Cloudflare 人机验证失败，请重试", 403);
    }
    let Some(proof) = create_turnstile_proof(&ctx.env, &ctx.settings.admin_password_hash) else {
        return error("验证凭据签名失败", 503);
    };
    let secure = req.url().ok().is_some_and(|url| url.scheme() == "https");
    let mut response = no_content()?;
    response.headers_mut().append(
        "Set-Cookie",
        &format!(
            "nodeflare_turnstile={proof}; Path=/; Max-Age=3600; HttpOnly{}; SameSite=Strict",
            if secure { "; Secure" } else { "" }
        ),
    )?;
    Ok(response)
}

async fn exchange_rates(ctx: &RouteContext) -> Result<Response> {
    if let Some(denied) = public_access_denied(ctx) {
        return denied;
    }
    let rates = crate::exchange::current(&ctx.database, now()).await?;
    let mut response = json(&rates, 200)?;
    if ctx.settings.public_dashboard {
        response
            .headers_mut()
            .set("Cache-Control", "public, max-age=300")?;
    }
    Ok(response)
}

async fn servers(ctx: &RouteContext) -> Result<Response> {
    if let Some(denied) = public_access_denied(ctx) {
        return denied;
    }
    let servers = server_values(ctx).await?;
    json(&serde_json::json!({ "servers": servers }), 200)
}

async fn history(req: &Request, ctx: &RouteContext) -> Result<Response> {
    if let Some(denied) = public_access_denied(ctx) {
        return denied;
    }
    let path = req.path();
    let Some(id) = server_id(&path, "/api/history/") else {
        return error("节点 ID 无效", 400);
    };
    if db::get_server(&ctx.database, &id, false).await?.is_none() {
        return error("节点不存在", 404);
    }
    let hours = requested_hours(req, 24 * 30)?;
    let cache_key = if ctx.settings.public_dashboard {
        Some(history_cache_key(
            &req.url()?,
            "metrics",
            &id,
            hours,
            ctx.settings.history_cache_version,
        ))
    } else {
        None
    };
    if let Some(cache_key) = cache_key.as_deref() {
        if let Some(response) = cached_history_response(cache_key).await {
            return Ok(response);
        }
    }
    let points = db::history(&ctx.database, &id, hours).await?;
    let mut response = json(&serde_json::json!({ "points": points }), 200)?;
    if let Some(cache_key) = cache_key.as_deref() {
        store_history_response(cache_key, &mut response, history_cache_seconds(hours)).await;
    }
    Ok(response)
}

async fn latency(req: &Request, ctx: &RouteContext) -> Result<Response> {
    if let Some(denied) = public_access_denied(ctx) {
        return denied;
    }
    let path = req.path();
    let Some(id) = server_id(&path, "/api/latency/") else {
        return error("节点 ID 无效", 400);
    };
    if db::get_server(&ctx.database, &id, false).await?.is_none() {
        return error("节点不存在", 404);
    }
    let hours = requested_hours(req, 24 * db::MAX_HISTORY_RETENTION_DAYS)?;
    let cache_key = if ctx.settings.public_dashboard {
        Some(history_cache_key(
            &req.url()?,
            "latency",
            &id,
            hours,
            ctx.settings.history_cache_version,
        ))
    } else {
        None
    };
    if let Some(cache_key) = cache_key.as_deref() {
        if let Some(response) = cached_history_response(cache_key).await {
            return Ok(response);
        }
    }
    let tasks = latency::tasks_for_server(&ctx.database, &id).await?;
    let points = latency::history(&ctx.database, &id, hours).await?;
    let mut response = json(
        &serde_json::json!({ "tasks": tasks, "points": points }),
        200,
    )?;
    if let Some(cache_key) = cache_key.as_deref() {
        store_history_response(cache_key, &mut response, history_cache_seconds(hours)).await;
    }
    Ok(response)
}
