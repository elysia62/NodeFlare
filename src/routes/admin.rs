use std::collections::HashSet;

use worker::{console_error, console_warn, Method, Request, Response, Result};

use crate::auth::{
    create_admin_jwt, create_theme_preview_proof, hash_password, random_salt, sha256_hex,
};
use crate::models::{
    AlertRuleInput, LatencyTaskInput, ServerBatchInput, ServerInput, ServerOrderInput,
    SettingsInput, ThemeInput, ThemeView,
};
use crate::routes::{RouteContext, RouteOutcome};
use crate::{
    cloudflare, db, env_secret_text, error, exchange, json, latency, live, no_content, notify, now,
    request_json, server_id, settings_for_admin_response, submitted_secret, theme,
    valid_background_urls, valid_cloudflare_account_id, valid_cloudflare_api_token,
    valid_password_derived, valid_public_asset_url, validate_alert_rule, validate_latency_task,
    validate_server, validate_theme, API_JSON_MAX_BYTES,
};

pub(crate) async fn route(mut req: Request, ctx: &RouteContext) -> Result<RouteOutcome> {
    let method = req.method();
    let path = req.path();

    if !path.starts_with("/api/admin/") {
        return Ok(RouteOutcome::Unmatched(req));
    }
    // 登出不需要管理员会话（仅清除 Cookie），其余管理接口一律要求授权。
    if method == Method::Post && path == "/api/admin/logout" {
        let mut response = no_content()?;
        crate::set_admin_session_cookie(&mut response, &req, None)?;
        return Ok(RouteOutcome::Handled(response));
    }
    if !ctx.admin {
        return Ok(RouteOutcome::Handled(error("未授权", 401)?));
    }

    if method == Method::Get && path == "/api/admin/latency-tasks" {
        return handled(json(
            &serde_json::json!({ "tasks": latency::list_tasks(&ctx.database).await? }),
            200,
        ));
    }
    if method == Method::Post && path == "/api/admin/latency-tasks" {
        return handled(latency_tasks_post(&mut req, ctx).await);
    }
    if (method == Method::Patch || method == Method::Delete)
        && path.starts_with("/api/admin/latency-tasks/")
    {
        return handled(latency_tasks_patch_delete(&mut req, ctx, &method, &path).await);
    }

    if method == Method::Get && path == "/api/admin/alert-rules" {
        return handled(json(
            &serde_json::json!({ "rules": db::list_alert_rules(&ctx.database).await? }),
            200,
        ));
    }
    if method == Method::Post && path == "/api/admin/alert-rules" {
        return handled(alert_rules_post(&mut req, ctx).await);
    }
    if (method == Method::Patch || method == Method::Delete)
        && path.starts_with("/api/admin/alert-rules/")
    {
        return handled(alert_rules_patch_delete(&mut req, ctx, &method, &path).await);
    }

    if method == Method::Get && path == "/api/admin/telegram" {
        return handled(json(
            &serde_json::json!({ "telegram": notify::telegram_settings(&ctx.database).await? }),
            200,
        ));
    }
    if method == Method::Put && path == "/api/admin/telegram" {
        return handled(telegram_put(&mut req, ctx).await);
    }
    if method == Method::Post && path == "/api/admin/telegram/test" {
        return handled(telegram_test(ctx).await);
    }

    if method == Method::Get && path == "/api/admin/servers" {
        let servers = db::list_servers(&ctx.database, true).await?;
        return handled(json(&serde_json::json!({ "servers": servers }), 200));
    }
    if method == Method::Delete && path == "/api/admin/servers" {
        return handled(servers_batch_delete(&mut req, ctx).await);
    }
    if method == Method::Patch && path == "/api/admin/servers/order" {
        return handled(servers_order_patch(&mut req, ctx).await);
    }
    if method == Method::Post && path == "/api/admin/servers" {
        return handled(servers_post(&mut req, ctx).await);
    }
    if method == Method::Get && path.starts_with("/api/admin/servers/") && path.ends_with("/token")
    {
        return handled(server_token_get(ctx, &path).await);
    }
    if (method == Method::Patch || method == Method::Delete)
        && path.starts_with("/api/admin/servers/")
    {
        return handled(server_patch_delete(&mut req, ctx, &method, &path).await);
    }

    if method == Method::Get && path == "/api/admin/settings" {
        return handled(json(
            &settings_for_admin_response(ctx.settings.clone(), &ctx.env),
            200,
        ));
    }
    if method == Method::Patch && path == "/api/admin/settings" {
        return handled(settings_patch(&mut req, ctx).await);
    }

    if method == Method::Get && path == "/api/admin/themes" {
        return handled(themes_get(ctx).await);
    }
    if method == Method::Post && path == "/api/admin/themes" {
        return handled(themes_post(&mut req, ctx).await);
    }
    if method == Method::Post
        && path.starts_with("/api/admin/themes/")
        && path.ends_with("/preview")
    {
        return handled(theme_preview_post(ctx, &path).await);
    }
    if method == Method::Post
        && path.starts_with("/api/admin/themes/")
        && path.ends_with("/activate")
    {
        return handled(theme_activate_post(ctx, &path).await);
    }
    if method == Method::Delete && path.starts_with("/api/admin/themes/") {
        return handled(theme_delete(ctx, &path).await);
    }

    if method == Method::Get && path == "/api/admin/theme-settings" {
        return handled(theme_settings_get(ctx).await);
    }
    if method == Method::Post && path == "/api/admin/exchange-rates/refresh" {
        return handled(exchange_rates_refresh(ctx).await);
    }
    if method == Method::Get && path == "/api/admin/database" {
        let stats =
            db::database_stats(&ctx.database, ctx.settings.offline_threshold_seconds).await?;
        return handled(json(&stats, 200));
    }
    if method == Method::Get && path == "/api/admin/cloudflare-usage" {
        return handled(cloudflare_usage(ctx).await);
    }
    if method == Method::Delete && path == "/api/admin/history" {
        db::clear_history(&ctx.database).await?;
        live::clear_alert_windows(&ctx.env).await?;
        db::increment_setting(&ctx.database, "history_cache_version").await?;
        return handled(no_content());
    }
    Ok(RouteOutcome::Unmatched(req))
}

fn handled(result: Result<Response>) -> Result<RouteOutcome> {
    result.map(RouteOutcome::Handled)
}

async fn existing_server_ids(ctx: &RouteContext) -> Result<HashSet<String>> {
    Ok(db::list_servers(&ctx.database, true)
        .await?
        .into_iter()
        .map(|server| server.id)
        .collect())
}

fn short_durable_id(ctx: &RouteContext) -> Result<String> {
    Ok(ctx
        .env
        .durable_object("LIVE_HUB")?
        .unique_id()?
        .to_string()
        .chars()
        .take(16)
        .collect::<String>())
}

fn unique_admin_id(ctx: &RouteContext, namespace: &str) -> Result<String> {
    let source = format!(
        "{namespace}:{}:{}",
        short_durable_id(ctx)?,
        worker::Date::now().as_millis()
    );
    Ok(sha256_hex(&source).chars().take(24).collect())
}

async fn latency_tasks_post(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    let input: LatencyTaskInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("延迟任务格式无效", 400),
    };
    if let Some(message) = validate_latency_task(&input) {
        return error(message, 400);
    }
    if latency::task_count(&ctx.database).await? >= latency::MAX_LATENCY_TASKS as i64 {
        return error("最多可创建 128 个延迟任务", 400);
    }
    let server_ids = existing_server_ids(ctx).await?;
    if input.server_ids.iter().any(|id| !server_ids.contains(id)) {
        return error("延迟任务包含不存在的服务器", 400);
    }
    let assigned_server_ids = input.server_ids.clone();
    let id = unique_admin_id(ctx, "latency-task")?;
    latency::create_task(&ctx.database, &id, &input, now()).await?;
    if let Err(error) = live::disconnect_agents(&ctx.env, &assigned_server_ids).await {
        console_error!("failed to refresh Agents after latency task creation: {error}");
    }
    db::increment_setting(&ctx.database, "history_cache_version").await?;
    json(&serde_json::json!({ "id": id }), 201)
}

async fn latency_tasks_patch_delete(
    req: &mut Request,
    ctx: &RouteContext,
    method: &Method,
    path: &str,
) -> Result<Response> {
    let Some(id) = server_id(path, "/api/admin/latency-tasks/") else {
        return error("延迟任务 ID 无效", 400);
    };
    if *method == Method::Delete {
        let assigned_server_ids = latency::server_ids_for_task(&ctx.database, &id).await?;
        return if latency::delete_task(&ctx.database, &id).await? {
            if let Err(error) = live::disconnect_agents(&ctx.env, &assigned_server_ids).await {
                console_error!("failed to refresh Agents after latency task deletion: {error}");
            }
            db::increment_setting(&ctx.database, "history_cache_version").await?;
            no_content()
        } else {
            error("延迟任务不存在", 404)
        };
    }
    let input: LatencyTaskInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("延迟任务格式无效", 400),
    };
    if let Some(message) = validate_latency_task(&input) {
        return error(message, 400);
    }
    let server_ids = existing_server_ids(ctx).await?;
    if input
        .server_ids
        .iter()
        .any(|server_id| !server_ids.contains(server_id))
    {
        return error("延迟任务包含不存在的服务器", 400);
    }
    let mut affected_server_ids = latency::server_ids_for_task(&ctx.database, &id).await?;
    affected_server_ids.extend(input.server_ids.iter().cloned());
    affected_server_ids.sort();
    affected_server_ids.dedup();
    if latency::update_task(&ctx.database, &id, &input, now()).await? {
        if let Err(error) = live::disconnect_agents(&ctx.env, &affected_server_ids).await {
            console_error!("failed to refresh Agents after latency task update: {error}");
        }
        db::increment_setting(&ctx.database, "history_cache_version").await?;
        no_content()
    } else {
        error("延迟任务不存在", 404)
    }
}

async fn alert_rules_post(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    if db::list_alert_rules(&ctx.database).await?.len() >= 20 {
        return error("最多可创建 20 条资源告警规则", 400);
    }
    let input: AlertRuleInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("告警规则格式无效", 400),
    };
    if let Some(message) = validate_alert_rule(&input) {
        return error(message, 400);
    }
    let server_ids = existing_server_ids(ctx).await?;
    if input.server_ids.iter().any(|id| !server_ids.contains(id)) {
        return error("告警规则包含不存在的服务器", 400);
    }
    let id = short_durable_id(ctx)?;
    db::create_alert_rule(&ctx.database, &id, &input).await?;
    json(&serde_json::json!({ "id": id }), 201)
}

async fn alert_rules_patch_delete(
    req: &mut Request,
    ctx: &RouteContext,
    method: &Method,
    path: &str,
) -> Result<Response> {
    let Some(id) = server_id(path, "/api/admin/alert-rules/") else {
        return error("告警规则 ID 无效", 400);
    };
    if *method == Method::Delete {
        return if db::delete_alert_rule(&ctx.database, &id).await? {
            no_content()
        } else {
            error("告警规则不存在", 404)
        };
    }
    let input: AlertRuleInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("告警规则格式无效", 400),
    };
    if let Some(message) = validate_alert_rule(&input) {
        return error(message, 400);
    }
    let server_ids = existing_server_ids(ctx).await?;
    if input
        .server_ids
        .iter()
        .any(|server_id| !server_ids.contains(server_id))
    {
        return error("告警规则包含不存在的服务器", 400);
    }
    if db::update_alert_rule(&ctx.database, &id, &input).await? {
        no_content()
    } else {
        error("告警规则不存在", 404)
    }
}

async fn servers_batch_delete(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    let input: ServerBatchInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("批量删除格式无效", 400),
    };
    if input.ids.is_empty()
        || input.ids.len() > 500
        || input.ids.iter().any(|id| id.is_empty() || id.len() > 80)
        || input.ids.iter().collect::<HashSet<_>>().len() != input.ids.len()
    {
        return error("批量删除列表无效", 400);
    }
    db::delete_servers(&ctx.database, &input.ids).await?;
    if let Err(error) = live::disconnect_agents(&ctx.env, &input.ids).await {
        console_error!("failed to disconnect deleted Agents: {error}");
    }
    if let Err(error) = live::remove_alert_samples(&ctx.env, &input.ids).await {
        console_error!("failed to remove deleted Agent alert samples: {error}");
    }
    no_content()
}

async fn servers_order_patch(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    let input: ServerOrderInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("排序格式无效", 400),
    };
    if input.ids.len() > 500
        || input.ids.iter().any(|id| id.is_empty() || id.len() > 80)
        || input.ids.iter().collect::<HashSet<_>>().len() != input.ids.len()
    {
        return error("节点排序列表无效", 400);
    }
    let current = db::list_servers(&ctx.database, true).await?;
    let current_ids: HashSet<_> = current.iter().map(|server| &server.id).collect();
    if input.ids.len() != current_ids.len() || input.ids.iter().any(|id| !current_ids.contains(id))
    {
        return error("排序列表必须包含全部节点", 400);
    }
    db::reorder_servers(&ctx.database, &input.ids).await?;
    no_content()
}

async fn servers_post(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    let input: ServerInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("节点格式无效", 400),
    };
    if let Some(message) = validate_server(&input) {
        return error(message, 400);
    }
    let id = short_durable_id(ctx)?;
    let token = ctx.env.durable_object("LIVE_HUB")?.unique_id()?.to_string();
    db::create_server(&ctx.database, &id, &token, &input).await?;
    latency::assign_defaults(&ctx.database, &id).await?;
    json(&serde_json::json!({ "id": id, "agent_token": token }), 201)
}

async fn server_token_get(ctx: &RouteContext, path: &str) -> Result<Response> {
    let id = path
        .strip_prefix("/api/admin/servers/")
        .and_then(|value| value.strip_suffix("/token"))
        .unwrap_or_default();
    if id.is_empty() || id.contains('/') || id.len() > 80 {
        return error("节点 ID 无效", 400);
    }
    match db::get_agent_token(&ctx.database, id).await? {
        Some(token) => json(&serde_json::json!({ "agent_token": token }), 200),
        None => error("节点不存在", 404),
    }
}

async fn server_patch_delete(
    req: &mut Request,
    ctx: &RouteContext,
    method: &Method,
    path: &str,
) -> Result<Response> {
    let Some(id) = server_id(path, "/api/admin/servers/") else {
        return error("节点 ID 无效", 400);
    };
    if *method == Method::Delete {
        if !db::delete_server(&ctx.database, &id).await? {
            return error("节点不存在", 404);
        }
        if let Err(error) = live::disconnect_agents(&ctx.env, std::slice::from_ref(&id)).await {
            console_error!("failed to disconnect deleted Agent: {error}");
        }
        if let Err(error) = live::remove_alert_samples(&ctx.env, std::slice::from_ref(&id)).await {
            console_error!("failed to remove deleted Agent alert samples: {error}");
        }
        return no_content();
    }
    let input: ServerInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("节点格式无效", 400),
    };
    if let Some(message) = validate_server(&input) {
        return error(message, 400);
    }
    if !db::update_server(&ctx.database, &id, &input).await? {
        return error("节点不存在", 404);
    }
    if let Err(error) = live::disconnect_agents(&ctx.env, std::slice::from_ref(&id)).await {
        console_error!("failed to reconnect updated Agent: {error}");
    }
    no_content()
}

async fn themes_get(ctx: &RouteContext) -> Result<Response> {
    let mut themes = vec![ThemeView {
        id: theme::BUILTIN_THEME_ID.to_string(),
        name: theme::BUILTIN_THEME_NAME.to_string(),
        description: "默认主题".to_string(),
        url: String::new(),
        version: crate::VERSION.to_string(),
        builtin: true,
        active: ctx.settings.active_theme_id == theme::BUILTIN_THEME_ID,
    }];
    themes.extend(db::list_themes(&ctx.database, &ctx.settings.active_theme_id).await?);
    json(&serde_json::json!({ "themes": themes }), 200)
}

async fn themes_post(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    if db::list_themes(&ctx.database, &ctx.settings.active_theme_id)
        .await?
        .len()
        >= 32
    {
        return error("最多可添加 32 个第三方主题", 400);
    }
    let input: ThemeInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("主题格式无效", 400),
    };
    if let Some(message) = validate_theme(&input) {
        return error(message, 400);
    }
    let resolved = match theme::resolve_url(&input.url) {
        Ok(value) => value,
        Err(err) => {
            console_warn!("theme URL resolution failed: {err}");
            return error("无法解析主题地址，请检查 URL", 422);
        }
    };
    let digest = sha256_hex(&resolved.source_url);
    let id = format!("theme-{}", &digest[..16]);
    if db::theme_exists(&ctx.database, &id).await? {
        return error("该主题 URL 已添加", 409);
    }
    if let Err(err) = theme::validate_remote(&resolved.resolved_url).await {
        console_warn!(
            "theme validation failed for {}: {err}",
            resolved.resolved_url
        );
        return error("无法读取主题 index.html，请检查 URL 和主题构建产物", 422);
    }
    let version = theme::remote_theme_version(&resolved.resolved_url)
        .await
        .unwrap_or_default();
    db::create_theme(
        &ctx.database,
        &id,
        &input,
        &resolved.source_url,
        &version,
        now(),
    )
    .await?;
    json(&serde_json::json!({ "id": id }), 201)
}

async fn theme_preview_post(ctx: &RouteContext, path: &str) -> Result<Response> {
    let id = path
        .strip_prefix("/api/admin/themes/")
        .and_then(|value| value.strip_suffix("/preview"))
        .unwrap_or("")
        .trim_matches('/');
    if id.is_empty() || id.contains('/') || id.len() > 80 {
        return error("主题 ID 无效", 400);
    }
    if id == theme::BUILTIN_THEME_ID || !db::theme_exists(&ctx.database, id).await? {
        return error("远程主题不存在", 404);
    }
    let Some(proof) = create_theme_preview_proof(&ctx.env, &ctx.settings.admin_password_hash, id)
    else {
        return error("无法创建主题预览凭据", 503);
    };
    json(
        &serde_json::json!({
            "preview_url": format!("/__theme-preview/{proof}/")
        }),
        200,
    )
}

async fn theme_activate_post(ctx: &RouteContext, path: &str) -> Result<Response> {
    let id = path
        .strip_prefix("/api/admin/themes/")
        .and_then(|value| value.strip_suffix("/activate"))
        .unwrap_or("")
        .trim_matches('/');
    if id.is_empty() || id.contains('/') || id.len() > 80 {
        return error("主题 ID 无效", 400);
    }
    if id != theme::BUILTIN_THEME_ID {
        let Some(url) = db::theme_url(&ctx.database, id).await? else {
            return error("主题不存在", 404);
        };
        if let Err(err) = theme::validate_remote(&url).await {
            console_warn!("theme activation failed for {url}: {err}");
            return error("主题当前不可访问，未切换主题", 422);
        }
        if let Some(version) = theme::remote_theme_version(&url).await {
            if let Err(err) = db::update_theme_version(&ctx.database, id, &version).await {
                console_warn!("theme version refresh failed for {id}: {err}");
            }
        }
    }
    if !db::set_active_theme(&ctx.database, id).await? {
        return error("主题不存在", 404);
    }
    no_content()
}

async fn theme_delete(ctx: &RouteContext, path: &str) -> Result<Response> {
    let Some(id) = server_id(path, "/api/admin/themes/") else {
        return error("主题 ID 无效", 400);
    };
    if id == theme::BUILTIN_THEME_ID {
        return error("内置主题不能删除", 400);
    }
    if db::delete_theme(&ctx.database, &id).await? {
        no_content()
    } else {
        error("主题不存在", 404)
    }
}

async fn theme_settings_get(ctx: &RouteContext) -> Result<Response> {
    if ctx.settings.active_theme_id == theme::BUILTIN_THEME_ID {
        return json(&theme::builtin_settings_schema(), 200);
    }
    let Some(url) = db::theme_url(&ctx.database, &ctx.settings.active_theme_id).await? else {
        return error("当前主题不存在", 404);
    };
    match theme::remote_settings_schema(&url).await {
        Ok(schema) => json(&schema, 200),
        Err(err) => {
            console_warn!("theme settings validation failed for {url}: {err}");
            error("当前主题的 theme.json 设置格式无效", 422)
        }
    }
}

async fn exchange_rates_refresh(ctx: &RouteContext) -> Result<Response> {
    match exchange::refresh(&ctx.database, now(), true).await {
        Ok((rates, _)) => json(&rates, 200),
        Err(err) => {
            console_error!("manual exchange-rate refresh failed: {err}");
            error("汇率更新失败，已保留数据库中的旧汇率", 502)
        }
    }
}

async fn cloudflare_usage(ctx: &RouteContext) -> Result<Response> {
    let account_id = if ctx.settings.cloudflare_account_id.trim().is_empty() {
        env_secret_text(&ctx.env, "CF_USAGE_ACCOUNT_ID")
    } else {
        ctx.settings.cloudflare_account_id.clone()
    };
    let token = if ctx.settings.cloudflare_api_token.trim().is_empty() {
        env_secret_text(&ctx.env, "CF_USAGE_API_TOKEN")
    } else {
        ctx.settings.cloudflare_api_token.clone()
    };
    if account_id.trim().is_empty() || token.trim().is_empty() {
        return error("尚未配置 Cloudflare 用量查询凭据", 503);
    }
    match cloudflare::usage(token.trim(), account_id.trim(), now()).await {
        Ok(usage) => json(&usage, 200),
        Err(err) => {
            console_error!("cloudflare usage query failed: {err}");
            let detail = err.to_string().chars().take(240).collect::<String>();
            error(&format!("Cloudflare 用量查询失败：{detail}"), 502)
        }
    }
}

async fn telegram_put(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    let input: crate::models::TelegramNotificationInput =
        match request_json(req, API_JSON_MAX_BYTES).await {
            Ok(value) => value,
            Err(_) => return error("Telegram 配置格式无效", 400),
        };
    match notify::save_telegram(&ctx.database, &input, now()).await? {
        Ok(()) => no_content(),
        Err(message) => error(&message, 400),
    }
}

async fn telegram_test(ctx: &RouteContext) -> Result<Response> {
    match notify::test_telegram(
        &ctx.database,
        &unique_admin_id(ctx, "notification-test")?,
        now(),
    )
    .await?
    {
        Ok(()) => no_content(),
        Err(message) => error(&format!("Telegram 测试消息发送失败：{message}"), 502),
    }
}

async fn settings_patch(req: &mut Request, ctx: &RouteContext) -> Result<Response> {
    let settings = &ctx.settings;
    let input: SettingsInput = match request_json(req, API_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("设置格式无效", 400),
    };
    if input
        .site_name
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 80)
    {
        return error("站点名称长度应为 1 至 80 个字符", 400);
    }
    if input
        .site_description
        .as_ref()
        .is_some_and(|value| value.chars().count() > 240)
    {
        return error("站点描述不能超过 240 个字符", 400);
    }
    if input
        .site_announcement
        .as_ref()
        .is_some_and(|value| value.chars().count() > 1000)
    {
        return error("站点公告不能超过 1000 个字符", 400);
    }
    if input
        .logo_url
        .as_deref()
        .is_some_and(|value| !valid_public_asset_url(value))
    {
        return error("站点 Logo 必须使用 HTTPS 地址", 400);
    }
    if input
        .default_theme
        .as_deref()
        .is_some_and(|value| !matches!(value, "system" | "light" | "dark"))
    {
        return error("默认主题无效", 400);
    }
    if let Some(id) = input.active_theme_id.as_deref() {
        let valid = id == theme::BUILTIN_THEME_ID || db::theme_exists(&ctx.database, id).await?;
        if !valid {
            return error("活动主题不存在", 400);
        }
    }
    if input
        .background_url
        .as_deref()
        .is_some_and(|value| !valid_background_urls(value))
    {
        return error(
            "背景地址无效：请使用 HTTPS 地址，浅色和深色可用 | 分隔",
            400,
        );
    }
    if input.theme_options.as_ref().is_some_and(|value| {
        let Some(options) = value.as_object() else {
            return true;
        };
        options.len() > 40
            || value.to_string().len() > 12_000
            || options.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 64
                    || !key
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
                    || !(value.is_boolean()
                        || value.is_number()
                        || value
                            .as_str()
                            .is_some_and(|text| text.chars().count() <= 500))
            })
    }) {
        return error("主题设置格式无效", 400);
    }
    if input.admin_username.as_ref().is_some_and(|value| {
        value.trim().is_empty()
            || value.chars().count() > 64
            || value.chars().any(char::is_whitespace)
    }) {
        return error("用户名应为 1 至 64 个不含空格的字符", 400);
    }
    if input
        .new_password
        .as_ref()
        .is_some_and(|value| !value.is_empty() && !(8..=128).contains(&value.chars().count()))
    {
        return error("新密码长度应为 8 至 128 个字符", 400);
    }
    if input
        .new_password
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        && input
            .new_password_derived
            .as_deref()
            .is_none_or(|value| !valid_password_derived(value))
    {
        return error("新密码派生值无效", 400);
    }
    let configured_site_key = submitted_secret(
        input.turnstile_site_key.as_deref(),
        &settings.turnstile_site_key,
    );
    let configured_secret_key = submitted_secret(
        input.turnstile_secret_key.as_deref(),
        &settings.turnstile_secret_key,
    );
    let site_key = if configured_site_key.is_empty() {
        ctx.environment_turnstile_site_key.trim()
    } else {
        configured_site_key
    };
    let secret_key = if configured_secret_key.is_empty() {
        ctx.environment_turnstile_secret_key.trim()
    } else {
        configured_secret_key
    };
    let protection_activated = input.turnstile_enabled == Some(true) && !settings.turnstile_enabled
        || input.turnstile_login_enabled == Some(true) && !settings.turnstile_login_enabled;
    if protection_activated && (site_key.is_empty() || secret_key.is_empty()) {
        return error("启用 Turnstile 前必须填写站点密钥和私钥", 400);
    }
    if input
        .traffic_alert_percentage
        .is_some_and(|value| !(50..=100).contains(&value))
    {
        return error("流量提醒起始阈值应为 50 至 100", 400);
    }
    if input
        .cloudflare_account_id
        .as_deref()
        .is_some_and(|value| !valid_cloudflare_account_id(value))
    {
        return error("Cloudflare Account ID 应为 32 位十六进制字符", 400);
    }
    if input
        .cloudflare_api_token
        .as_deref()
        .is_some_and(|value| value.trim() != db::SECRET_MASK && !valid_cloudflare_api_token(value))
    {
        return error("Cloudflare API Token 格式无效", 400);
    }
    if input
        .locale
        .as_deref()
        .is_some_and(|value| !matches!(value, "zh-CN" | "en"))
    {
        return error("界面语言仅支持简体中文或 English", 400);
    }
    if input
        .favicon_url
        .as_deref()
        .is_some_and(|value| !valid_public_asset_url(value))
    {
        return error("站点图标必须使用 HTTPS 地址", 400);
    }
    let password_hash = if input
        .new_password
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        let password_derived = input.new_password_derived.as_deref().unwrap_or_default();
        let Some(salt) = random_salt() else {
            return error("运行时随机源不可用，无法保存新密码", 503);
        };
        Some(hash_password(password_derived, &salt))
    } else {
        None
    };
    db::update_settings(&ctx.database, &input, password_hash.as_deref()).await?;
    let updated = db::settings(
        &ctx.database,
        &ctx.default_name,
        ctx.default_threshold,
        ctx.default_retention,
        &ctx.default_username,
    )
    .await?;
    let token = if password_hash.is_some() {
        create_admin_jwt(&ctx.env, &updated.admin_password_hash)
    } else {
        None
    };
    let response_settings = settings_for_admin_response(updated, &ctx.env);
    let mut response = json(
        &serde_json::json!({ "settings": response_settings, "token": token }),
        200,
    )?;
    if let Some(token) = token.as_deref() {
        crate::set_admin_session_cookie(&mut response, req, Some(token))?;
    }
    Ok(response)
}
