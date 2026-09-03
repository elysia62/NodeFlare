use super::ApiResponse;
use crate::AppState;
use crate::middleware::AuthenticatedUser;
use crate::models::CreateRemoteTaskRequest;
use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;

const MAX_REMOTE_TARGETS: usize = 128;

#[derive(Serialize)]
struct CreatedRemoteTask {
    server_id: String,
    task_id: String,
}

fn remote_server_ids(input: &CreateRemoteTaskRequest) -> Vec<String> {
    let mut seen = HashSet::new();
    input
        .server_ids
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .filter(|id| seen.insert((*id).to_string()))
        .map(str::to_string)
        .collect()
}

fn validate_remote_totp(status: Option<(&str, bool)>, code: &str) -> Result<(), ApiResponse> {
    let Some((secret, true)) = status else {
        return Err(ApiResponse::error(
            StatusCode::PRECONDITION_REQUIRED,
            "请先在登录与安全中启用 TOTP 两步验证",
        ));
    };
    if code.trim().is_empty()
        || !crate::totp::verify_totp(secret, code.trim()).map_err(ApiResponse::internal)?
    {
        // 远程任务接口带管理员认证；这里不能返回 401，否则前端会把
        // “验证码错误”误判成登录会话失效并清除管理员令牌。
        return Err(ApiResponse::unprocessable("两步验证码错误"));
    }
    Ok(())
}

pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(input): Json<CreateRemoteTaskRequest>,
) -> Result<Response, ApiResponse> {
    let command = input.command.trim();
    let server_ids = remote_server_ids(&input);
    if server_ids.is_empty()
        || server_ids.len() > MAX_REMOTE_TARGETS
        || server_ids.iter().any(|id| id.len() > 80)
        || command.is_empty()
        || command.len() > 16_384
    {
        return Err(ApiResponse::bad_request("请选择 1 至 128 个节点并填写命令"));
    }
    let totp = crate::db::queries::get_totp_secret(&state.db, &user.username)
        .await
        .map_err(ApiResponse::internal)?;
    validate_remote_totp(
        totp.as_ref()
            .map(|(secret, enabled)| (secret.as_str(), *enabled)),
        &input.totp_code,
    )?;
    let known_server_ids = crate::db::queries::all_server_ids(&state.db)
        .await
        .map_err(ApiResponse::internal)?
        .into_iter()
        .collect::<HashSet<_>>();
    if let Some(missing) = server_ids
        .iter()
        .find(|server_id| !known_server_ids.contains(*server_id))
    {
        return Err(ApiResponse::not_found(format!("节点不存在: {missing}")));
    }

    let mut tasks = Vec::with_capacity(server_ids.len());
    for server_id in server_ids {
        let task =
            crate::db::queries::create_remote_task(&state.db, &server_id, command, &user.username)
                .await
                .map_err(ApiResponse::internal)?;
        if state.send_remote_task(&task).await {
            crate::db::queries::mark_remote_task_sent(&state.db, &task.id, &task.server_id)
                .await
                .map_err(ApiResponse::internal)?;
        }
        tasks.push(CreatedRemoteTask {
            server_id,
            task_id: task.id,
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"tasks": tasks})),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_execution_requires_enabled_totp() {
        let missing = validate_remote_totp(None, "123456").unwrap_err();
        assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED);

        let disabled =
            validate_remote_totp(Some(("JBSWY3DPEHPK3PXP", false)), "123456").unwrap_err();
        assert_eq!(disabled.status, StatusCode::PRECONDITION_REQUIRED);
    }

    #[test]
    fn invalid_remote_totp_does_not_invalidate_the_admin_session() {
        let error =
            validate_remote_totp(Some(("JBSWY3DPEHPK3PXP", true)), "not-a-code").unwrap_err();
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn accepts_a_current_remote_totp() {
        let secret = "JBSWY3DPEHPK3PXP";
        let secret_bytes = data_encoding::BASE32_NOPAD
            .decode(secret.as_bytes())
            .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let code = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &secret_bytes, now);

        assert!(validate_remote_totp(Some((secret, true)), &code).is_ok());
    }

    #[test]
    fn trims_batch_server_ids_without_duplicates() {
        let input = CreateRemoteTaskRequest {
            server_ids: vec![
                " server-a ".to_string(),
                "server-a".to_string(),
                "server-b".to_string(),
            ],
            command: "uptime".to_string(),
            totp_code: "123456".to_string(),
        };

        assert_eq!(remote_server_ids(&input), vec!["server-a", "server-b"]);
    }
}
