use super::*;
use sqlx::{AssertSqlSafe, Row};

async fn setup() -> Database {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();
    sqlx::query("INSERT INTO servers(id,name,token_hash,created_at,updated_at) VALUES ('node','Node','hash',1,1)")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO latency_tasks(id,name,task_type,target,interval_seconds,created_at,updated_at) VALUES ('task','Task','icmp','example.com',60,1,1)")
        .execute(db.pool()).await.unwrap();
    db
}

async fn timestamps(db: &Database, table: &str) -> Vec<i64> {
    sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT timestamp FROM {table} ORDER BY timestamp"
    )))
    .fetch_all(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn indexed_cleanup_keeps_summaries_crossing_or_ending_at_cutoff() {
    let db = setup().await;
    let cutoff = 1_800_000_000;
    for (timestamp, last_timestamp) in [
        (cutoff - 3600, cutoff - 1),
        (cutoff - 300, cutoff + 30),
        (cutoff - 120, cutoff),
        (cutoff, cutoff + 59),
        (cutoff + 60, cutoff + 61),
    ] {
        sqlx::query("INSERT INTO metric_history(server_id,timestamp,first_timestamp,last_timestamp) VALUES ('node',?,?,?)")
            .bind(timestamp).bind(timestamp).bind(last_timestamp)
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss,last_timestamp) VALUES ('task','node',?,10.0,0.0,?)")
            .bind(timestamp).bind(last_timestamp).execute(db.pool()).await.unwrap();
    }
    for statement in &CLEANUP_STATEMENTS[..2] {
        assert_eq!(
            sqlx::query(db.sql(statement))
                .bind(cutoff)
                .bind(cutoff)
                .bind(CLEANUP_DELETE_BATCH_ROWS)
                .execute(db.pool())
                .await
                .unwrap()
                .rows_affected(),
            1
        );
    }
    let expected = vec![cutoff - 300, cutoff - 120, cutoff, cutoff + 60];
    assert_eq!(timestamps(&db, "metric_history").await, expected);
    assert_eq!(timestamps(&db, "latency_results").await, expected);
}

#[tokio::test]
async fn indexed_latency_cleanup_preserves_raw_sample_fallback_and_batch_limit() {
    let db = setup().await;
    let cutoff = 1_800_000_000;
    for timestamp in [cutoff - 2, cutoff - 1, cutoff, cutoff + 1] {
        sqlx::query("INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss) VALUES ('task','node',?,10.0,0.0)")
            .bind(timestamp).execute(db.pool()).await.unwrap();
    }
    let statement = db.sql(CLEANUP_STATEMENTS[1]);
    for remaining in [
        vec![cutoff - 1, cutoff, cutoff + 1],
        vec![cutoff, cutoff + 1],
    ] {
        assert_eq!(
            sqlx::query(statement)
                .bind(cutoff)
                .bind(cutoff)
                .bind(1_i64)
                .execute(db.pool())
                .await
                .unwrap()
                .rows_affected(),
            1
        );
        assert_eq!(timestamps(&db, "latency_results").await, remaining);
    }
    assert_eq!(
        sqlx::query(statement)
            .bind(cutoff)
            .bind(cutoff)
            .bind(1_i64)
            .execute(db.pool())
            .await
            .unwrap()
            .rows_affected(),
        0
    );
}

#[tokio::test]
async fn cleanup_uses_time_range_indexes_when_all_history_is_retained() {
    let db = setup().await;
    let cutoff = 1_800_000_000_i64;
    sqlx::query(
        "WITH RECURSIVE seq(n) AS (SELECT 0 UNION ALL SELECT n+1 FROM seq WHERE n<9999) \
        INSERT INTO metric_history(server_id,timestamp,first_timestamp,last_timestamp) \
        SELECT 'node',?+n,?+n,?+n FROM seq",
    )
    .bind(cutoff)
    .bind(cutoff)
    .bind(cutoff)
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss) \
        SELECT 'task',server_id,timestamp,10.0,0.0 FROM metric_history",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query("ANALYZE").execute(db.pool()).await.unwrap();

    // Check the actual DELETE, not a duplicate SELECT or a wall-clock threshold:
    // retained rows must sit beyond an indexed upper bound, not a full scan.
    for (statement, index) in CLEANUP_STATEMENTS[..2]
        .iter()
        .zip(["metric_history_time", "latency_results_time"])
    {
        let plan = sqlx::query(AssertSqlSafe(format!("EXPLAIN QUERY PLAN {statement}")))
            .bind(cutoff)
            .bind(cutoff)
            .bind(CLEANUP_DELETE_BATCH_ROWS)
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>();
        assert!(
            plan.iter().any(|step| step.contains("SEARCH")
                && step.contains(index)
                && step.contains("timestamp<?")),
            "history cleanup must use the bounded time index: {plan:?}"
        );
        assert_eq!(
            sqlx::query(db.sql(statement))
                .bind(cutoff)
                .bind(cutoff)
                .bind(CLEANUP_DELETE_BATCH_ROWS)
                .execute(db.pool())
                .await
                .unwrap()
                .rows_affected(),
            0
        );
    }
    for table in ["metric_history", "latency_results"] {
        assert_eq!(timestamps(&db, table).await.len(), 10_000);
    }
}
