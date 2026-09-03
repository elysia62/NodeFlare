use super::ApiResponse;
use crate::middleware::AuthenticatedUser;
use crate::models::CreateRemoteTaskRequest;
use crate::AppState;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::sync::Arc;

pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(input): Json<CreateRemoteTaskRequest>,
) -> Result<Response, ApiResponse> {
    let command = input.command.trim();
    let script = input.script.trim();
    if input.server_id.is_empty()
        || input.server_id.len() > 80
        || command.is_empty() == script.is_empty()
        || command.len() > 16_384
        || script.len() > 256 * 1024
    {
        return Err(ApiResponse::bad_request(
            "请选择一个节点，并且只填写命令或脚本中的一项",
        ));
    }
    if let Some((secret, enabled)) = crate::db::queries::get_totp_secret(&state.db, &user.username)
        .await
        .map_err(ApiResponse::internal)?
    {
        if enabled
            && (input.totp_code.trim().is_empty()
                || !crate::totp::verify_totp(&secret, input.totp_code.trim())
                    .map_err(ApiResponse::internal)?)
        {
            return Err(ApiResponse::unauthorized("远程执行需要有效的两步验证码"));
        }
    }
    if !crate::db::queries::all_server_ids(&state.db)
        .await
        .map_err(ApiResponse::internal)?
        .contains(&input.server_id)
    {
        return Err(ApiResponse::not_found("节点不存在"));
    }
    let task = crate::db::queries::create_remote_task(
        &state.db,
        &input.server_id,
        command,
        script,
        &user.username,
    )
    .await
    .map_err(ApiResponse::internal)?;
    if state.send_remote_task(&task).await {
        crate::db::queries::mark_remote_task_sent(&state.db, &task.id, &task.server_id)
            .await
            .map_err(ApiResponse::internal)?;
    }
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"task_id": task.id})),
    )
        .into_response())
}

pub async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    let task = crate::db::queries::remote_task(&state.db, &id)
        .await
        .map_err(ApiResponse::internal)?
        .ok_or_else(|| ApiResponse::not_found("任务不存在"))?;
    Ok(Json(task).into_response())
}

pub async fn get_server_tasks(
    State(state): State<Arc<AppState>>,
    Path(server_id): Path<String>,
) -> Result<Response, ApiResponse> {
    Ok(Json(
        crate::db::queries::server_remote_tasks(&state.db, &server_id, 50)
            .await
            .map_err(ApiResponse::internal)?,
    )
    .into_response())
}
