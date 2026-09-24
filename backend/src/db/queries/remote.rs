use super::super::{Database, now};
use crate::models::RemoteTaskInfo;
use anyhow::Result;
use sqlx::Row;

pub async fn create_remote_task(
    db: &Database,
    server_id: &str,
    command: &str,
    username: &str,
) -> Result<RemoteTaskInfo> {
    let task = RemoteTaskInfo {
        id: uuid::Uuid::new_v4().to_string(),
        server_id: server_id.to_string(),
        command: command.to_string(),
        status: "pending".to_string(),
        requested_by: username.to_string(),
        requested_at: now(),
        started_at: None,
        completed_at: None,
        result: String::new(),
        exit_code: None,
    };
    sqlx::query(db.sql(
        "INSERT INTO remote_tasks(id, server_id, command, status, requested_by, requested_at) \
         VALUES (?, ?, ?, 'pending', ?, ?)",
    ))
    .bind(&task.id)
    .bind(&task.server_id)
    .bind(&task.command)
    .bind(&task.requested_by)
    .bind(task.requested_at)
    .execute(db.pool())
    .await?;
    Ok(task)
}

pub async fn remote_task(db: &Database, id: &str) -> Result<Option<RemoteTaskInfo>> {
    let row = sqlx::query(db.sql(
        "SELECT id, server_id, command, status, requested_by, requested_at, started_at, \
         completed_at, result, exit_code FROM remote_tasks WHERE id=?",
    ))
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    row.as_ref()
        .map(remote_task_from_row)
        .transpose()
        .map_err(Into::into)
}
pub(crate) fn remote_task_from_row(
    row: &sqlx::any::AnyRow,
) -> std::result::Result<RemoteTaskInfo, sqlx::Error> {
    Ok(RemoteTaskInfo {
        id: row.try_get("id")?,
        server_id: row.try_get("server_id")?,
        command: row.try_get("command")?,
        status: row.try_get("status")?,
        requested_by: row.try_get("requested_by")?,
        requested_at: row.try_get("requested_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        result: row.try_get("result")?,
        exit_code: row.try_get("exit_code")?,
    })
}

pub async fn mark_remote_task_sent(db: &Database, id: &str, server_id: &str) -> Result<()> {
    sqlx::query(db.sql(
        "UPDATE remote_tasks SET status='sent', started_at=? \
         WHERE id=? AND server_id=? AND status='pending'",
    ))
    .bind(now())
    .bind(id)
    .bind(server_id)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn update_remote_task_result(
    db: &Database,
    server_id: &str,
    id: &str,
    status: &str,
    result: &str,
    exit_code: Option<i64>,
) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "UPDATE remote_tasks SET status=?, result=?, exit_code=?, completed_at=? \
         WHERE id=? AND server_id=? AND status IN ('pending','sent')",
    ))
    .bind(status)
    .bind(result)
    .bind(exit_code)
    .bind(now())
    .bind(id)
    .bind(server_id)
    .execute(db.pool())
    .await?;
    if result.rows_affected() > 0 {
        return Ok(true);
    }
    let completed = sqlx::query_scalar::<_, i64>(db.sql(
        "SELECT COUNT(*) FROM remote_tasks \
         WHERE id=? AND server_id=? AND status IN ('success','failed')",
    ))
    .bind(id)
    .bind(server_id)
    .fetch_one(db.pool())
    .await?;
    Ok(completed > 0)
}
