use super::*;
use crate::db::queries::{history, latency_history};
use crate::models::AgentReport;

async fn setup(db: &Database) {
    db.migrate().await.unwrap();
    sqlx::query("INSERT INTO servers(id,name,token_hash,created_at,updated_at) VALUES ('node','Node','hash',1,1)")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO latency_tasks(id,name,task_type,target,interval_seconds,created_at,updated_at) VALUES ('task','Task','icmp','example.com',60,1,1)")
        .execute(db.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO latency_task_servers(task_id,server_id,assigned_at) VALUES ('task','node',1)",
    )
    .execute(db.pool())
    .await
    .unwrap();
}

async fn seed(db: &Database, base: i64) {
    let reports = [(1, 10.0, 100), (2, 30.0, 200), (31, 80.0, 7), (61, 60.0, 3)].map(
        |(offset, cpu, traffic)| AgentReport {
            timestamp: base + offset,
            cpu,
            load1: cpu / 10.0,
            gpu_usage: cpu,
            mem_used: cpu as i64 * 10,
            mem_total: 1000,
            disk_used: cpu as i64 * 10,
            disk_total: 1000,
            net_in: cpu,
            net_out: cpu * 2.0,
            net_rx_total: traffic,
            net_tx_total: traffic,
            uptime: traffic,
            ..AgentReport::default()
        },
    );
    let rows = super::super::history::aggregate_history(&reports, 15);
    let mut tx = db.pool().begin().await.unwrap();
    super::super::ingest::save_history_rows(db, &mut tx, "node", &rows)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for (offset, latency, loss) in [
        (1, 10.0, 0.0),
        (2, -1.0, 100.0),
        (31, 90.0, 0.0),
        (61, 110.0, 0.0),
    ] {
        sqlx::query(db.sql("INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss) VALUES ('task','node',?,?,?)"))
            .bind(base+offset).bind(latency).bind(loss).execute(db.pool()).await.unwrap();
    }
}

async fn compact(db: &Database, current: i64) {
    compact_history_at(db, 365, current, Duration::from_secs(10))
        .await
        .unwrap();
}

async fn assert_tiers(db: &Database) {
    setup(db).await;
    let current = now().div_euclid(3600) * 3600;
    for days in [3, 10, 40] {
        seed(db, current - days * DAY).await;
    }
    compact(db, current).await;
    for days in [3, 10, 40] {
        let base = current - days * DAY;
        let metrics = sqlx::query(db.sql(
            "SELECT * FROM metric_history WHERE timestamp>=? AND timestamp<? ORDER BY timestamp",
        ))
        .bind(base)
        .bind(base + 3600)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(metrics.len(), if days == 3 { 2 } else { 1 });
        assert_eq!(
            metrics
                .iter()
                .map(|r| r.get::<i64, _>("sample_count"))
                .sum::<i64>(),
            4
        );
        let last = metrics.last().unwrap();
        assert_eq!(
            last.get::<i64, _>("net_rx_total"),
            3,
            "traffic reset must use the latest counter"
        );
        assert_eq!(last.get::<i64, _>("uptime"), 3);
        assert_eq!(last.get::<i64, _>("last_timestamp"), base + 61);
        if days != 3 {
            let row = &metrics[0];
            assert_eq!(row.get::<f64, _>("cpu"), 45.0);
            assert_eq!(row.get::<f64, _>("cpu_min"), 10.0);
            assert_eq!(row.get::<f64, _>("cpu_max"), 80.0);
            assert_eq!(row.get::<i64, _>("mem_used"), 450);
            assert_eq!(row.get::<i64, _>("mem_used_max"), 800);
            assert_eq!(row.get::<f64, _>("memory_avg"), 45.0);
            assert_eq!(row.get::<f64, _>("memory_min"), 10.0);
            assert_eq!(row.get::<f64, _>("net_in"), 80.0);
            assert_eq!(row.get::<f64, _>("net_in_avg"), 45.0);
            assert_eq!(row.get::<i64, _>("first_timestamp"), base + 1);
        }
    }
    // Repeated passes and promotion of minute/five-minute summaries to hours
    // must retain original sample weights, including failed latency attempts.
    compact(db, current).await;
    compact(db, current + 40 * DAY).await;
    compact(db, current + 40 * DAY).await;
    let metrics = sqlx::query("SELECT * FROM metric_history ORDER BY timestamp")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert_eq!(metrics.len(), 3);
    for row in metrics {
        assert_eq!(row.get::<i64, _>("timestamp") % 3600, 0);
        assert_eq!(row.get::<i64, _>("sample_count"), 4);
        assert_eq!(row.get::<f64, _>("cpu"), 45.0);
    }
    let latency = sqlx::query("SELECT * FROM latency_results ORDER BY timestamp")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert_eq!(latency.len(), 3);
    for row in latency {
        assert_eq!(row.get::<i64, _>("timestamp") % 3600, 0);
        assert_eq!(row.get::<i64, _>("sample_count"), 4);
        assert_eq!(row.get::<i64, _>("latency_sample_count"), 3);
        assert!((row.get::<f64, _>("latency_ms") - 70.0).abs() < 1e-9);
        assert_eq!(row.get::<f64, _>("packet_loss"), 25.0);
    }
    let points = history(db, "node", 90 * 24).await.unwrap();
    assert_eq!(points.iter().map(|p| p.sample_count).sum::<i64>(), 12);
    assert!(points.iter().all(|p| p.cpu == 45.0 && p.cpu_max == 80.0));
    let (_, points) = latency_history(db, "node", 90 * 24).await.unwrap();
    assert_eq!(points.len(), 3);
    assert!(
        points
            .iter()
            .all(|p| (p.latency_ms - 70.0).abs() < 1e-9 && p.packet_loss == 25.0)
    );
}

#[tokio::test]
async fn sqlite_history_tiers_preserve_weights_peaks_and_counters() {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    assert_tiers(&db).await;
}

#[tokio::test]
async fn tier_boundaries_and_late_samples_keep_their_finer_resolution() {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    setup(&db).await;
    let current = now().div_euclid(3600) * 3600;
    for base in [current - 3600, current - 7 * DAY, current - 30 * DAY] {
        seed(&db, base).await;
    }
    compact(&db, current).await;
    for (base, metric_count, latency_count) in [
        (current - 3600, 3, 4),
        (current - 7 * DAY, 2, 2),
        (current - 30 * DAY, 1, 1),
    ] {
        let metrics: i64 = sqlx::query_scalar(
            db.sql("SELECT COUNT(*) FROM metric_history WHERE timestamp>=? AND timestamp<?"),
        )
        .bind(base)
        .bind(base + 3600)
        .fetch_one(db.pool())
        .await
        .unwrap();
        let latency: i64 = sqlx::query_scalar(
            db.sql("SELECT COUNT(*) FROM latency_results WHERE timestamp>=? AND timestamp<?"),
        )
        .bind(base)
        .bind(base + 3600)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!((metrics, latency), (metric_count, latency_count));
    }
    let base = current - 30 * DAY;
    let latency: (i64, i64) = sqlx::query_as(
        db.sql("SELECT timestamp,last_timestamp FROM latency_results WHERE timestamp=?"),
    )
    .bind(base)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(latency, (base, base + 61));
}

#[tokio::test]
async fn compaction_rolls_back_the_summary_when_source_deletion_fails() {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    setup(&db).await;
    let current = now().div_euclid(3600) * 3600;
    seed(&db, current - 40 * DAY).await;
    sqlx::query("CREATE TRIGGER reject_compaction BEFORE DELETE ON metric_history BEGIN SELECT RAISE(ABORT,'injected deletion failure'); END")
        .execute(db.pool()).await.unwrap();
    assert!(
        compact_history_at(&db, 365, current, Duration::from_secs(10))
            .await
            .is_err()
    );
    let stats: (i64, i64) =
        sqlx::query_as("SELECT COUNT(*),CAST(SUM(sample_count) AS BIGINT) FROM metric_history")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stats, (3, 4));
    sqlx::query("DROP TRIGGER reject_compaction")
        .execute(db.pool())
        .await
        .unwrap();
    compact(&db, current).await;
    let stats: (i64, i64) =
        sqlx::query_as("SELECT COUNT(*),CAST(SUM(sample_count) AS BIGINT) FROM metric_history")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stats, (1, 4));
}

#[tokio::test]
async fn compaction_is_bounded_and_concurrent_passes_do_not_duplicate_samples() {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    setup(&db).await;
    let current = now().div_euclid(3600) * 3600;
    for i in 0..300 {
        let timestamp = current - 40 * DAY + i * 60 + 1;
        sqlx::query(db.sql("INSERT INTO metric_history(server_id,timestamp,first_timestamp,last_timestamp,cpu) VALUES ('node',?,?,?,25.0)"))
            .bind(timestamp).bind(timestamp).bind(timestamp).execute(db.pool()).await.unwrap();
    }
    compact_history_at(&db, 365, current, Duration::ZERO)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metric_history")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 300);
    for _ in 0..3 {
        tokio::join!(compact(&db, current), compact(&db, current));
    }
    let stats: (i64, i64) =
        sqlx::query_as("SELECT COUNT(*),CAST(SUM(sample_count) AS BIGINT) FROM metric_history")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stats, (5, 300));
}

#[tokio::test]
#[ignore = "requires NODEFLARE_TEST_POSTGRES_URL with CREATE DATABASE permission"]
async fn postgres_history_tiers_preserve_weights_peaks_and_counters() {
    let url = std::env::var("NODEFLARE_TEST_POSTGRES_URL").unwrap();
    let control = crate::db::connect(&url).await.unwrap();
    let name = format!("nodeflare_rollup_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(control.pool())
        .await
        .unwrap();
    let mut url = reqwest::Url::parse(&url).unwrap();
    url.set_path(&name);
    let db = crate::db::connect(url.as_str()).await.unwrap();
    let test_db = db.clone();
    let result = tokio::spawn(async move {
        assert_tiers(&test_db).await;
    })
    .await;
    db.pool().close().await;
    sqlx::query(AssertSqlSafe(format!("DROP DATABASE {name}")))
        .execute(control.pool())
        .await
        .unwrap();
    control.pool().close().await;
    result.unwrap();
}

#[tokio::test]
async fn latency_queries_weight_summaries_and_keep_retention_boundary_buckets() {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    setup(&db).await;
    let current = now();
    let base = (current - 40 * DAY).div_euclid(DAY) * DAY;
    seed(&db, base).await;
    sqlx::query(db.sql("INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss) VALUES ('task','node',?,200.0,0.0)"))
        .bind(base+3601).execute(db.pool()).await.unwrap();
    compact(&db, current).await;
    let (_, points) = latency_history(&db, "node", 90 * 24).await.unwrap();
    assert_eq!(points.len(), 1);
    assert!((points[0].latency_ms - 102.5).abs() < 1e-9);
    assert_eq!(points[0].packet_loss, 20.0);
    let cutoff = current - DAY;
    let bucket = cutoff.div_euclid(3600) * 3600;
    sqlx::query(db.sql("INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss,sample_count,latency_sample_count,last_timestamp) VALUES ('task','node',?,12.0,0.0,2,2,?)"))
        .bind(bucket).bind(cutoff+60).execute(db.pool()).await.unwrap();
    super::super::cleanup_database(&db, 1).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM latency_results")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let (_, points) = latency_history(&db, "node", 24).await.unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].latency_ms, 12.0);
}

#[tokio::test]
async fn latency_rollup_keeps_failures_and_respects_reassignment() {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    setup(&db).await;
    let current = now().div_euclid(3600) * 3600;
    let base = current - 10 * DAY;
    sqlx::query(db.sql("UPDATE latency_task_servers SET assigned_at=?"))
        .bind(base + 20)
        .execute(db.pool())
        .await
        .unwrap();
    for (offset, latency, loss) in [(1, 10.0, 0.0), (31, -1.0, 100.0), (61, -1.0, 100.0)] {
        sqlx::query(db.sql("INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss) VALUES ('task','node',?,?,?)"))
            .bind(base + offset).bind(latency).bind(loss).execute(db.pool()).await.unwrap();
    }
    compact(&db, current).await;
    let counts: (i64, i64) =
        sqlx::query_as("SELECT sample_count,latency_sample_count FROM latency_results")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(counts, (2, 0));
    let (_, points) = latency_history(&db, "node", 30 * 24).await.unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].latency_ms, -1.0);
    assert_eq!(points[0].packet_loss, 100.0);
    let latest = super::super::list_servers(&db, true).await.unwrap();
    assert_eq!(latest[0].latency[0].timestamp, base + 61);
}
