use super::super::{Database, now};
use anyhow::Result;

const REMOTE_TASK_ACTIVE_TTL_SECONDS: i64 = 24 * 60 * 60;
const CLEANUP_DELETE_BATCH_ROWS: i64 = 1000;
const CLEANUP_MAX_PASSES: usize = 10;
const CLEANUP_TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

pub async fn cleanup_database(db: &Database, retention_days: i64) -> Result<()> {
    cleanup_database_with_budget(db, retention_days, CLEANUP_MAX_PASSES, CLEANUP_TIME_BUDGET).await
}
pub(crate) async fn cleanup_database_with_budget(
    db: &Database,
    retention_days: i64,
    max_passes: usize,
    budget: std::time::Duration,
) -> Result<()> {
    let current = now();
    let cutoff = current - retention_days.clamp(1, 3650) * 86_400;
    let active_task_cutoff = current - REMOTE_TASK_ACTIVE_TTL_SECONDS;
    let statements = [
        "DELETE FROM metric_history WHERE (server_id, timestamp) IN ( \
           SELECT server_id, timestamp FROM metric_history WHERE \
           last_timestamp<? \
           ORDER BY timestamp LIMIT ?)",
        "DELETE FROM latency_results WHERE (task_id, server_id, timestamp) IN ( \
           SELECT task_id, server_id, timestamp FROM latency_results WHERE timestamp<? \
           ORDER BY timestamp LIMIT ?)",
        "DELETE FROM remote_tasks WHERE status IN ('success','failed') AND id IN ( \
           SELECT id FROM remote_tasks WHERE status IN ('success','failed') AND completed_at<? \
           ORDER BY completed_at LIMIT ?)",
        "DELETE FROM alert_states WHERE active=0 AND state_key IN ( \
           SELECT state_key FROM alert_states WHERE active=0 AND updated_at<? \
           AND NOT EXISTS (SELECT 1 FROM notification_outbox WHERE notification_outbox.state_key=alert_states.state_key) \
           ORDER BY updated_at LIMIT ?)",
    ];
    let started = std::time::Instant::now();
    let mut done = [false; 5];
    // Each statement commits its own bounded batch; rotate tables so a large
    // metrics backlog cannot starve latency, task or alert cleanup.
    for _ in 0..max_passes {
        for (index, finished) in done.iter_mut().enumerate() {
            if started.elapsed() >= budget {
                return Ok(());
            }
            if *finished {
                continue;
            }
            let affected = if index == 0 {
                sqlx::query(db.sql(
                    "UPDATE remote_tasks SET status='failed', completed_at=?, \
                     result=CASE WHEN result='' THEN '超过 24 小时未收到执行结果，无法确认命令状态' ELSE result END \
                     WHERE status IN ('pending','sent') AND id IN ( \
                       SELECT id FROM remote_tasks WHERE status IN ('pending','sent') AND requested_at<? \
                       ORDER BY requested_at LIMIT ?)",
                ))
                .bind(current)
                .bind(active_task_cutoff)
                .bind(CLEANUP_DELETE_BATCH_ROWS)
                .execute(db.pool())
                .await?
                .rows_affected()
            } else {
                sqlx::query(db.sql(statements[index - 1]))
                    .bind(cutoff)
                    .bind(CLEANUP_DELETE_BATCH_ROWS)
                    .execute(db.pool())
                    .await?
                    .rows_affected()
            };
            *finished = affected < CLEANUP_DELETE_BATCH_ROWS as u64;
            tokio::task::yield_now().await;
        }
        if done.iter().all(|finished| *finished) {
            break;
        }
    }
    Ok(())
}
