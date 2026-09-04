use super::{ApiResponse, admin_cookie, request_is_secure};
use crate::AppState;
use crate::db::SECRET_MASK;
use crate::middleware::AuthenticatedUser;
use crate::models::{
    AlertRuleInput, LatencyTaskInput, ServerBatchInput, ServerInput, ServerOrderInput,
    SettingsInput, TelegramSettingsInput, ThemeInput, ThemeUploadInput,
};
use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Extension, Path, Query, Request, State};
use axum::http::{
    HeaderMap, HeaderValue, StatusCode,
    header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, SET_COOKIE},
};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio_util::io::ReaderStream;
use url::Url;

#[derive(Deserialize)]
pub struct DatabaseRestoreInput {
    filename: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseMigrationInput {
    database_url: String,
}

struct DatabaseOperationFlag<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for DatabaseOperationFlag<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub async fn servers_get(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let servers = crate::db::queries::list_servers(&state.db, true)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"servers": servers})).into_response())
}

pub async fn servers_post(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ServerInput>,
) -> Result<Response, ApiResponse> {
    validate_server(&input).map_err(ApiResponse::bad_request)?;
    let (id, token) = crate::db::queries::create_server(&state.db, &input)
        .await
        .map_err(ApiResponse::internal)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": id, "agent_token": token})),
    )
        .into_response())
}

pub async fn server_patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<ServerInput>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    validate_server(&input).map_err(ApiResponse::bad_request)?;
    if !crate::db::queries::update_server(&state.db, &id, &input)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("节点不存在"));
    }
    state.push_agent_config(&id).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn server_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    if !crate::db::queries::delete_server(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("节点不存在"));
    }
    state.disconnect_agent(&id).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn servers_delete(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ServerBatchInput>,
) -> Result<Response, ApiResponse> {
    validate_ids(&input.ids)?;
    crate::db::queries::delete_servers(&state.db, &input.ids)
        .await
        .map_err(ApiResponse::internal)?;
    for id in input.ids {
        state.disconnect_agent(&id).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn servers_order(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ServerOrderInput>,
) -> Result<Response, ApiResponse> {
    validate_ids(&input.ids)?;
    let current = crate::db::queries::all_server_ids(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    if current.len() != input.ids.len() || input.ids.iter().any(|id| !current.contains(id)) {
        return Err(ApiResponse::bad_request("排序列表必须包含全部节点"));
    }
    crate::db::queries::reorder_servers(&state.db, &input.ids)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn server_token_rotate(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    let token = crate::db::queries::rotate_server_token(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
        .ok_or_else(|| ApiResponse::not_found("节点不存在"))?;
    state.disconnect_agent(&id).await;
    Ok(Json(serde_json::json!({"agent_token": token})).into_response())
}

pub async fn settings_get(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(settings.admin_view()).into_response())
}

pub async fn settings_patch(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    Json(mut input): Json<SettingsInput>,
) -> Result<Response, ApiResponse> {
    let current = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    validate_settings(&state, &current, &input).await?;
    let password_hash = input
        .new_password_derived
        .as_deref()
        .map(crate::auth::hash_password)
        .transpose()
        .map_err(ApiResponse::internal)?;
    let next_username = input
        .admin_username
        .as_deref()
        .map_or(current.admin_username.as_str(), str::trim)
        .to_string();
    let username_changed = next_username != user.username;
    if !username_changed {
        input.admin_username = None;
    }
    crate::db::update_settings(&state.db, &input, password_hash.as_deref())
        .await
        .map_err(ApiResponse::internal)?;
    crate::db::queries::rename_totp_user(&state.db, &user.username, &next_username)
        .await
        .map_err(ApiResponse::internal)?;
    let credentials_changed = password_hash.is_some() || username_changed;
    let token = if credentials_changed {
        Some(
            crate::db::create_session(
                &state.db,
                &next_username,
                state.config.session_ttl_hours,
                &super::auth::session_device(&headers, peer),
            )
            .await
            .map_err(ApiResponse::internal)?,
        )
    } else {
        None
    };
    let updated = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    let mut response = Json(serde_json::json!({
        "settings": updated.admin_view(),
        "token": token,
    }))
    .into_response();
    if let Some(token) = token {
        response.headers_mut().append(
            SET_COOKIE,
            HeaderValue::from_str(&admin_cookie(
                &token,
                state.config.session_ttl_hours * 3600,
                request_is_secure(&headers),
            ))
            .map_err(ApiResponse::internal)?,
        );
    }
    Ok(response)
}

pub async fn latency_tasks_get(
    State(state): State<Arc<AppState>>,
) -> Result<Response, ApiResponse> {
    let tasks = crate::db::queries::list_latency_tasks(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"tasks": tasks})).into_response())
}

pub async fn latency_tasks_post(
    State(state): State<Arc<AppState>>,
    Json(input): Json<LatencyTaskInput>,
) -> Result<Response, ApiResponse> {
    validate_latency_task(&state, &input).await?;
    let id = crate::db::queries::create_latency_task(&state.db, &input)
        .await
        .map_err(ApiResponse::internal)?;
    for server_id in &input.server_ids {
        state.push_agent_config(server_id).await;
    }
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))).into_response())
}

pub async fn latency_task_patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<LatencyTaskInput>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    validate_latency_task(&state, &input).await?;
    let mut affected = crate::db::queries::latency_task_server_ids(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?;
    affected.extend(input.server_ids.iter().cloned());
    affected.sort();
    affected.dedup();
    if !crate::db::queries::update_latency_task(&state.db, &id, &input)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("延迟任务不存在"));
    }
    for server_id in affected {
        state.push_agent_config(&server_id).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn latency_task_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    let affected = crate::db::queries::latency_task_server_ids(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?;
    if !crate::db::queries::delete_latency_task(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("延迟任务不存在"));
    }
    for server_id in affected {
        state.push_agent_config(&server_id).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn alert_rules_get(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let rules = crate::db::queries::list_alert_rules(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"rules": rules})).into_response())
}

pub async fn alert_rules_post(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AlertRuleInput>,
) -> Result<Response, ApiResponse> {
    validate_alert_rule(&state, &input).await?;
    if crate::db::queries::list_alert_rules(&state.db)
        .await
        .map_err(ApiResponse::internal)?
        .len()
        >= 20
    {
        return Err(ApiResponse::bad_request("最多可创建 20 条资源告警规则"));
    }
    let id = crate::db::queries::create_alert_rule(&state.db, &input)
        .await
        .map_err(ApiResponse::internal)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))).into_response())
}

pub async fn alert_rule_patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<AlertRuleInput>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    validate_alert_rule(&state, &input).await?;
    if !crate::db::queries::update_alert_rule(&state.db, &id, &input)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("告警规则不存在"));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn alert_rule_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    if !crate::db::queries::delete_alert_rule(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("告警规则不存在"));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn telegram_get(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let telegram = crate::db::queries::telegram_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"telegram": telegram})).into_response())
}

pub async fn telegram_put(
    State(state): State<Arc<AppState>>,
    Json(input): Json<TelegramSettingsInput>,
) -> Result<Response, ApiResponse> {
    if input.bot_token.trim().is_empty()
        || input.bot_token.len() > 512
        || input.bot_token.chars().any(char::is_whitespace)
        || input.chat_id.trim().is_empty()
        || input.chat_id.len() > 128
        || input.template.trim().is_empty()
        || input.template.chars().count() > 4000
        || input.message_thread_id.is_some_and(|value| value <= 0)
    {
        return Err(ApiResponse::bad_request("Telegram 配置格式无效"));
    }
    crate::db::queries::save_telegram_settings(&state.db, &input)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn telegram_test(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    crate::notify::test_telegram(&state.db, &state.http)
        .await
        .map_err(|error| ApiResponse::error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn themes_get(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    let themes = crate::db::queries::list_themes(&state.db, &settings.active_theme_id)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"themes": themes})).into_response())
}

pub async fn themes_post(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ThemeInput>,
) -> Result<Response, ApiResponse> {
    validate_theme_metadata(&input.name, &input.description)?;
    let source_url = crate::theme::normalize_repository_url(&input.url)
        .map_err(|error| ApiResponse::bad_request(error.to_string()))?;
    let digest = Sha256::digest(source_url.as_bytes());
    let id = format!("theme-{}", &hex::encode(digest)[..16]);
    if crate::db::queries::theme_exists(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::conflict("该主题已添加"));
    }
    let downloaded = crate::theme::download_latest_release(&state.http, &source_url)
        .await
        .map_err(|error| ApiResponse::unprocessable(error.to_string()))?;
    create_installed_theme(
        &state,
        id,
        ThemeInput {
            url: downloaded.source_url,
            ..input
        },
        downloaded.archive,
        downloaded.release_version,
    )
    .await
}

pub async fn themes_upload(
    State(state): State<Arc<AppState>>,
    Query(input): Query<ThemeUploadInput>,
    request: Request,
) -> Result<Response, ApiResponse> {
    validate_theme_metadata(&input.name, &input.description)?;
    let filename = validate_theme_filename(&input.filename)?;
    if request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > crate::theme::THEME_ZIP_MAX_BYTES)
    {
        return Err(ApiResponse::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "主题 ZIP 不能超过 32 MiB",
        ));
    }
    let archive = to_bytes(request.into_body(), crate::theme::THEME_ZIP_MAX_BYTES)
        .await
        .map_err(|_| ApiResponse::error(StatusCode::PAYLOAD_TOO_LARGE, "主题 ZIP 不能超过 32 MiB"))?
        .to_vec();
    if archive.is_empty() {
        return Err(ApiResponse::bad_request("请选择非空 ZIP 文件"));
    }
    let digest = Sha256::digest(&archive);
    let id = format!("theme-{}", &hex::encode(digest)[..16]);
    if crate::db::queries::theme_exists(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::conflict("该主题 ZIP 已添加"));
    }
    create_installed_theme(
        &state,
        id,
        ThemeInput {
            name: input.name,
            description: input.description,
            url: format!("upload:{filename}"),
        },
        archive,
        String::new(),
    )
    .await
}

pub async fn theme_activate(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    if id != crate::theme::BUILTIN_THEME_ID {
        let url = crate::db::queries::theme_resolved_url(&state.db, &id)
            .await
            .map_err(ApiResponse::internal)?
            .ok_or_else(|| ApiResponse::not_found("主题不存在"))?;
        crate::theme::validate(&state.config.theme_dir, &url)
            .await
            .map_err(|error| ApiResponse::unprocessable(error.to_string()))?;
    }
    if !crate::db::queries::set_active_theme(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("主题不存在"));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn theme_preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    if id == crate::theme::BUILTIN_THEME_ID
        || !crate::db::queries::theme_exists(&state.db, &id)
            .await
            .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("主题不存在"));
    }
    let proof = crate::db::queries::create_theme_preview(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({
        "preview_url": format!("/__theme-preview/{proof}/")
    }))
    .into_response())
}

pub async fn theme_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    validate_id(&id)?;
    if id == crate::theme::BUILTIN_THEME_ID {
        return Err(ApiResponse::bad_request("内置主题不能删除"));
    }
    let reference = crate::db::queries::theme_resolved_url(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
        .ok_or_else(|| ApiResponse::not_found("主题不存在"))?;
    if !crate::db::queries::delete_theme(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("主题不存在"));
    }
    crate::theme::remove_installed(&state.config.theme_dir, &reference)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn theme_settings(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    if settings.active_theme_id == crate::theme::BUILTIN_THEME_ID {
        return Ok(Json(crate::theme::builtin_settings_schema()).into_response());
    }
    let url = crate::db::queries::theme_resolved_url(&state.db, &settings.active_theme_id)
        .await
        .map_err(ApiResponse::internal)?
        .ok_or_else(|| ApiResponse::not_found("当前主题不存在"))?;
    Ok(Json(
        crate::theme::settings_schema(&state.config.theme_dir, &url)
            .await
            .map_err(|error| ApiResponse::unprocessable(error.to_string()))?,
    )
    .into_response())
}

async fn create_installed_theme(
    state: &AppState,
    id: String,
    input: ThemeInput,
    archive: Vec<u8>,
    fallback_version: String,
) -> Result<Response, ApiResponse> {
    crate::theme::install_archive(&state.config.theme_dir, &id, archive)
        .await
        .map_err(|error| ApiResponse::unprocessable(error.to_string()))?;
    let reference = crate::theme::local_reference(&id).map_err(ApiResponse::internal)?;
    let validation = async {
        crate::theme::validate(&state.config.theme_dir, &reference).await?;
        crate::theme::settings_schema(&state.config.theme_dir, &reference).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = validation {
        let _ = crate::theme::remove_installed(&state.config.theme_dir, &reference).await;
        return Err(ApiResponse::unprocessable(error.to_string()));
    }
    let version = crate::theme::version(&state.config.theme_dir, &reference)
        .await
        .unwrap_or(fallback_version);
    if let Err(error) =
        crate::db::queries::create_theme(&state.db, &id, &input, &reference, &version).await
    {
        let _ = crate::theme::remove_installed(&state.config.theme_dir, &reference).await;
        return Err(ApiResponse::internal(error));
    }
    Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id}))).into_response())
}

fn validate_theme_metadata(name: &str, description: &str) -> Result<(), ApiResponse> {
    if !(1..=80).contains(&name.trim().chars().count()) || description.trim().chars().count() > 300
    {
        return Err(ApiResponse::bad_request("主题名称或说明无效"));
    }
    Ok(())
}

fn validate_theme_filename(value: &str) -> Result<String, ApiResponse> {
    let filename = value.trim();
    if filename.is_empty()
        || filename.chars().count() > 255
        || filename.contains('/')
        || filename.contains('\\')
        || filename.chars().any(char::is_control)
        || !filename.to_ascii_lowercase().ends_with(".zip")
    {
        return Err(ApiResponse::bad_request(
            "上传文件必须是名称有效的 ZIP 文件",
        ));
    }
    Ok(filename.to_string())
}

pub async fn exchange_refresh(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    Ok(Json(
        crate::exchange::refresh(&state.db, &state.http, true)
            .await
            .map_err(|error| ApiResponse::error(StatusCode::BAD_GATEWAY, error.to_string()))?,
    )
    .into_response())
}

pub async fn database_stats(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    Ok(Json(state.db.stats().await.map_err(ApiResponse::internal)?).into_response())
}

pub async fn database_reclaim(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let _maintenance = state.database_maintenance.lock().await;
    state
        .database_maintenance_active
        .store(true, Ordering::Release);
    let _operation = DatabaseOperationFlag(&state.database_maintenance_active);
    state.disconnect_agents().await;

    let before = state.db.stats().await.map_err(ApiResponse::internal)?;
    state
        .db
        .reclaim_space()
        .await
        .map_err(ApiResponse::internal)?;
    let database = state.db.stats().await.map_err(ApiResponse::internal)?;
    let reclaimed_bytes = before.size_bytes.saturating_sub(database.size_bytes);
    Ok(Json(serde_json::json!({
        "database": database,
        "reclaimed_bytes": reclaimed_bytes,
    }))
    .into_response())
}

pub async fn database_migrate(
    State(state): State<Arc<AppState>>,
    Json(input): Json<DatabaseMigrationInput>,
) -> Result<Response, ApiResponse> {
    if state.database_overridden {
        return Err(ApiResponse::bad_request(
            "使用 --database 启动时无法自动切换配置",
        ));
    }
    let value = input.database_url.trim();
    if value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control) {
        return Err(ApiResponse::bad_request("请输入有效的目标数据库 URL"));
    }
    let base = state
        .config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let target_url = crate::config::resolve_database_url(base, value)
        .map_err(|_| ApiResponse::bad_request("目标数据库 URL 格式无效"))?;
    let target_kind = crate::db::database_kind(&target_url)
        .map_err(|_| ApiResponse::bad_request("目标数据库 URL 格式无效"))?;
    if target_kind == state.db.kind() {
        return Err(ApiResponse::bad_request("目标必须使用另一种数据库类型"));
    }

    let _maintenance = state.database_maintenance.lock().await;
    state
        .database_maintenance_active
        .store(true, Ordering::Release);
    let _operation = DatabaseOperationFlag(&state.database_maintenance_active);
    state.disconnect_agents().await;

    let target = crate::db::connect(&target_url)
        .await
        .map_err(|_| ApiResponse::unprocessable("无法连接目标数据库，请检查地址、账号和网络"))?;
    target
        .migrate()
        .await
        .map_err(|_| ApiResponse::unprocessable("无法初始化目标数据库，请检查账号权限"))?;
    let migrated_rows = crate::backup::copy_database(&state.db, &target)
        .await
        .map_err(|error| ApiResponse::unprocessable(format!("迁移失败：{error}")))?;
    let database = target.stats().await.map_err(ApiResponse::internal)?;
    crate::config::update_database_url(&state.config_path, &target_url)
        .map_err(ApiResponse::internal)?;

    Ok(Json(serde_json::json!({
        "migrated_rows": migrated_rows,
        "target_kind": target_kind.as_str(),
        "size_bytes": database.size_bytes,
        "restart_required": true,
    }))
    .into_response())
}

pub async fn database_backup(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let _maintenance = state.database_maintenance.lock().await;
    let archive = crate::backup::export_archive(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    let disposition =
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", archive.filename))
            .map_err(ApiResponse::internal)?;
    let body = Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(archive.file)));
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/zip")
        .header(CONTENT_DISPOSITION, disposition)
        .header(CONTENT_LENGTH, archive.size)
        .header(CACHE_CONTROL, "no-store")
        .body(body)
        .map_err(ApiResponse::internal)
}

pub async fn database_restore(
    State(state): State<Arc<AppState>>,
    Query(input): Query<DatabaseRestoreInput>,
    request: Request,
) -> Result<Response, ApiResponse> {
    let filename = input.filename.trim();
    if filename.is_empty()
        || filename.chars().count() > 255
        || filename.contains('/')
        || filename.contains('\\')
        || filename.chars().any(char::is_control)
        || !filename.to_ascii_lowercase().ends_with(".zip")
    {
        return Err(ApiResponse::bad_request("请选择有效的数据库备份 ZIP"));
    }
    if request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > crate::backup::DATABASE_BACKUP_MAX_BYTES)
    {
        return Err(ApiResponse::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "数据库备份 ZIP 不能超过 512 MiB",
        ));
    }
    let archive = to_bytes(
        request.into_body(),
        crate::backup::DATABASE_BACKUP_MAX_BYTES,
    )
    .await
    .map_err(|_| {
        ApiResponse::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "数据库备份 ZIP 不能超过 512 MiB",
        )
    })?;

    let _maintenance = state.database_maintenance.lock().await;
    state
        .database_maintenance_active
        .store(true, Ordering::Release);
    let _operation = DatabaseOperationFlag(&state.database_maintenance_active);
    state.disconnect_agents().await;
    let restored_rows = crate::backup::restore_archive(&state.db, &archive)
        .await
        .map_err(|error| ApiResponse::unprocessable(format!("恢复失败：{error}")))?;
    Ok(Json(serde_json::json!({"restored_rows": restored_rows})).into_response())
}

fn validate_id(id: &str) -> Result<(), ApiResponse> {
    if id.is_empty() || id.len() > 80 || id.contains('/') {
        Err(ApiResponse::bad_request("ID 无效"))
    } else {
        Ok(())
    }
}

fn validate_ids(ids: &[String]) -> Result<(), ApiResponse> {
    if ids.is_empty()
        || ids.len() > 500
        || ids.iter().any(|id| validate_id(id).is_err())
        || ids.iter().collect::<HashSet<_>>().len() != ids.len()
    {
        Err(ApiResponse::bad_request("节点列表无效"))
    } else {
        Ok(())
    }
}

fn validate_server(input: &ServerInput) -> Result<(), &'static str> {
    if !(1..=80).contains(&input.name.trim().chars().count()) {
        return Err("节点名称长度应为 1 至 80 个字符");
    }
    if input.region.chars().count() > 16
        || input.group_name.chars().count() > 40
        || input.tags.chars().count() > 240
    {
        return Err("地区、分组或标签字段过长");
    }
    if input.traffic_limit < 0
        || !matches!(
            input.traffic_limit_type.as_str(),
            "sum" | "max" | "min" | "up" | "down"
        )
    {
        return Err("流量配置无效");
    }
    if !input.price.is_finite()
        || !(-1.0..=1_000_000_000.0).contains(&input.price)
        || !(0..=3650).contains(&input.billing_cycle)
    {
        return Err("价格或计费周期无效");
    }
    if input.currency.len() != 3
        || !input
            .currency
            .chars()
            .all(|value| value.is_ascii_alphabetic())
    {
        return Err("币种应为 3 位字母代码");
    }
    if !(1..=31).contains(&input.reset_day)
        || !(15..=3600).contains(&input.report_interval)
        || !(1..=60).contains(&input.collect_interval)
        || input.collect_interval > input.report_interval
        || (input.report_interval + input.collect_interval - 1) / input.collect_interval > 720
    {
        return Err("采样、上报或流量重置配置无效");
    }
    if input.network_interface.chars().count() > 160 || !valid_agent_mirror(&input.agent_mirror) {
        return Err("网卡或 Agent 下载地址无效");
    }
    Ok(())
}

async fn validate_latency_task(
    state: &AppState,
    input: &LatencyTaskInput,
) -> Result<(), ApiResponse> {
    if !(1..=80).contains(&input.name.trim().chars().count())
        || !matches!(input.task_type.as_str(), "tcp" | "icmp")
        || !valid_ping_target(input.target.trim())
        || !(30..=3600).contains(&input.interval_seconds)
        || input.task_type == "tcp" && input.port.is_none_or(|port| !(1..=65535).contains(&port))
        || input.task_type == "icmp" && input.port.is_some()
    {
        return Err(ApiResponse::bad_request("延迟任务格式无效"));
    }
    validate_ids_allow_empty(&input.server_ids)?;
    let servers = crate::db::queries::all_server_ids(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    if input.server_ids.iter().any(|id| !servers.contains(id)) {
        return Err(ApiResponse::bad_request("延迟任务包含不存在的节点"));
    }
    Ok(())
}

async fn validate_alert_rule(state: &AppState, input: &AlertRuleInput) -> Result<(), ApiResponse> {
    let maximum = if matches!(input.metric.as_str(), "cpu" | "memory" | "disk") {
        100.0
    } else {
        1_000_000.0
    };
    if !(1..=80).contains(&input.name.trim().chars().count())
        || !matches!(
            input.metric.as_str(),
            "cpu" | "memory" | "disk" | "net_in" | "net_out"
        )
        || !input.threshold.is_finite()
        || input.threshold <= 0.0
        || input.threshold > maximum
        || !(1..=1440).contains(&input.duration_minutes)
        || !matches!(input.aggregation.as_str(), "average" | "continuous")
    {
        return Err(ApiResponse::bad_request("告警规则格式无效"));
    }
    validate_ids_allow_empty(&input.server_ids)?;
    let servers = crate::db::queries::all_server_ids(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    if input.server_ids.iter().any(|id| !servers.contains(id)) {
        return Err(ApiResponse::bad_request("告警规则包含不存在的节点"));
    }
    Ok(())
}

fn validate_ids_allow_empty(ids: &[String]) -> Result<(), ApiResponse> {
    if ids.len() > 500
        || ids.iter().any(|id| validate_id(id).is_err())
        || ids.iter().collect::<HashSet<_>>().len() != ids.len()
    {
        Err(ApiResponse::bad_request("节点选择列表无效"))
    } else {
        Ok(())
    }
}

async fn validate_settings(
    state: &AppState,
    current: &crate::db::Settings,
    input: &SettingsInput,
) -> Result<(), ApiResponse> {
    if input
        .site_name
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 80)
        || input
            .site_description
            .as_ref()
            .is_some_and(|value| value.chars().count() > 240)
        || input
            .site_announcement
            .as_ref()
            .is_some_and(|value| value.chars().count() > 1000)
        || input
            .logo_url
            .as_deref()
            .is_some_and(|value| !valid_https_url(value))
        || input
            .background_url
            .as_deref()
            .is_some_and(|value| !valid_background_urls(value))
        || input
            .locale
            .as_deref()
            .is_some_and(|value| !matches!(value, "zh-CN" | "en"))
        || input
            .default_theme
            .as_deref()
            .is_some_and(|value| !matches!(value, "system" | "light" | "dark"))
    {
        return Err(ApiResponse::bad_request("站点设置格式无效"));
    }
    if input.admin_username.as_ref().is_some_and(|value| {
        value.trim().is_empty()
            || value.chars().count() > 64
            || value.chars().any(char::is_whitespace)
    }) {
        return Err(ApiResponse::bad_request("管理员用户名格式无效"));
    }
    if input
        .new_password_derived
        .as_deref()
        .is_some_and(|value| !crate::auth::valid_password_derived(value))
    {
        return Err(ApiResponse::bad_request("新密码摘要格式无效"));
    }
    if input.theme_options.as_ref().is_some_and(|value| {
        let Some(object) = value.as_object() else {
            return true;
        };
        object.len() > 40 || value.to_string().len() > 12_000
    }) {
        return Err(ApiResponse::bad_request("主题设置格式无效"));
    }
    if let Some(id) = input.active_theme_id.as_deref()
        && id != crate::theme::BUILTIN_THEME_ID
        && !crate::db::queries::theme_exists(&state.db, id)
            .await
            .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::bad_request("活动主题不存在"));
    }
    let site_key = submitted_secret(
        input.turnstile_site_key.as_deref(),
        &current.turnstile_site_key,
    );
    let secret_key = submitted_secret(
        input.turnstile_secret_key.as_deref(),
        &current.turnstile_secret_key,
    );
    if site_key.is_empty() != secret_key.is_empty() {
        return Err(ApiResponse::bad_request(
            "Turnstile Site Key 和 Secret Key 必须同时填写或同时留空",
        ));
    }
    Ok(())
}

fn submitted_secret<'a>(submitted: Option<&'a str>, current: &'a str) -> &'a str {
    match submitted.map(str::trim) {
        None | Some(SECRET_MASK) => current,
        Some(value) => value,
    }
}

fn valid_https_url(value: &str) -> bool {
    let value = value.trim();
    value.is_empty()
        || value.len() <= 1000
            && Url::parse(value).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
}

fn valid_background_urls(value: &str) -> bool {
    let parts = value.split('|').collect::<Vec<_>>();
    value.trim().is_empty()
        || parts.len() <= 2
            && parts.iter().any(|part| !part.trim().is_empty())
            && parts.iter().all(|part| valid_https_url(part))
}

fn valid_agent_mirror(value: &str) -> bool {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return true;
    }
    Url::parse(value).is_ok_and(|url| {
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && (url.scheme() == "https"
                || url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")))
    })
}

fn valid_ping_target(value: &str) -> bool {
    let labels = value.split('.').collect::<Vec<_>>();
    if labels.len() == 4
        && labels
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_digit()))
    {
        return value.parse::<Ipv4Addr>().is_ok_and(|ip| {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a == 169 && b == 254
                || a == 172 && (16..=31).contains(&b)
                || a == 192 && b == 168
                || a == 192 && b == 0 && (c == 0 || c == 2)
                || a >= 224)
        });
    }
    let lower = value.to_ascii_lowercase();
    labels.len() >= 2
        && !["local", "localhost", "internal", "lan", "localdomain"]
            .iter()
            .any(|suffix| lower == *suffix || lower.ends_with(&format!(".{suffix}")))
        && labels.iter().all(|label| {
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
