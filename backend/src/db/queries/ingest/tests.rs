use super::*;

async fn assert_metadata_updates_preserve_persistence(db: &Database) {
    db.migrate().await.unwrap();
    sqlx::query("INSERT INTO servers(id,name,token_hash,created_at,updated_at) VALUES ('node','Node','hash',1,1)")
        .execute(db.pool()).await.unwrap();
    if !db.is_postgres() {
        // Count actual UPDATEs, including no-op ones, rather than relying only
        // on second-resolution timestamps to detect redundant writes.
        sqlx::query("CREATE TABLE metadata_writes(server_id TEXT NOT NULL)")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER count_metadata_writes AFTER UPDATE ON servers BEGIN INSERT INTO metadata_writes VALUES (NEW.id); END")
            .execute(db.pool()).await.unwrap();
    }
    let identity = AgentIdentity {
        server_id: "node".into(),
        hidden: false,
        report_interval: 60,
        reset_day: 1,
        reset_timezone: chrono_tz::UTC,
        rx_correction: 0,
        tx_correction: 0,
    };
    // All samples are in one minute and therefore one traffic reset cycle.
    let base = now().div_euclid(60) * 60 - 60;
    let cases = [
        ("", "", "", "", "", false),
        (
            "192.0.2.1",
            "2001:db8::1",
            "198.51.100.1",
            "192.0.2.1",
            "2001:db8::1",
            true,
        ),
        (
            "192.0.2.1",
            "2001:db8::1",
            "198.51.100.1",
            "192.0.2.1",
            "2001:db8::1",
            false,
        ),
        ("", "", "198.51.100.1", "192.0.2.1", "2001:db8::1", false),
        (
            "192.0.2.2",
            "",
            "198.51.100.1",
            "192.0.2.2",
            "2001:db8::1",
            true,
        ),
        (
            "",
            "2001:db8::2",
            "198.51.100.1",
            "192.0.2.2",
            "2001:db8::2",
            true,
        ),
        ("", "", "198.51.100.2", "192.0.2.2", "2001:db8::2", true),
        ("", "", "", "192.0.2.2", "2001:db8::2", true),
        ("", "", "", "192.0.2.2", "2001:db8::2", false),
    ];
    for (index, (ip_v4, ip_v6, remote_ip, expected_v4, expected_v6, changed)) in
        cases.into_iter().enumerate()
    {
        // Also prove unchanged metadata retains its timestamp on PostgreSQL.
        sqlx::query("UPDATE servers SET updated_at=1 WHERE id='node'")
            .execute(db.pool())
            .await
            .unwrap();
        let writes_before = metadata_writes(db).await;
        let timestamp = base + index as i64;
        let report = AgentReport {
            timestamp,
            cpu: index as f64,
            ip_v4: ip_v4.into(),
            ip_v6: ip_v6.into(),
            net_rx_total: 100 + index as i64,
            net_tx_total: 200 + index as i64 * 2,
            ..AgentReport::default()
        };
        let result = save_agent_batch(
            db,
            &identity,
            &format!("batch-{index}"),
            &[report],
            remote_ip,
        )
        .await
        .unwrap();
        assert!(result.persisted);
        assert_eq!(result.persisted_through, timestamp);
        assert_eq!(result.reports.len(), 1);
        assert_eq!(result.next_persist_after_ms, 60_000);
        let metadata: (String, String, String, i64) =
            sqlx::query_as("SELECT last_ip,ip_v4,ip_v6,updated_at FROM servers WHERE id='node'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(
            (&*metadata.0, &*metadata.1, &*metadata.2),
            (remote_ip, expected_v4, expected_v6)
        );
        assert_eq!(metadata.3 != 1, changed);
        if let Some(before) = writes_before {
            assert_eq!(
                metadata_writes(db).await.unwrap() - before,
                i64::from(changed)
            );
        }
        let latest: (i64, String, String) = sqlx::query_as(
            "SELECT latest_timestamp,latest_json,last_batch_id FROM server_latest_state WHERE server_id='node'",
        ).fetch_one(db.pool()).await.unwrap();
        assert_eq!(latest.0, timestamp);
        assert_eq!(latest.2, format!("batch-{index}"));
        let persisted: AgentReport = serde_json::from_str(&latest.1).unwrap();
        assert_eq!(persisted.timestamp, timestamp);
        assert_eq!(persisted.cpu, index as f64);
        assert_eq!(persisted.net_rx_total, 100 + index as i64);
        let traffic: (i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT timestamp,raw_rx,raw_tx,used_rx,used_tx FROM server_traffic_state WHERE server_id='node'",
        ).fetch_one(db.pool()).await.unwrap();
        assert_eq!(
            traffic,
            (
                timestamp,
                100 + index as i64,
                200 + index as i64 * 2,
                100 + index as i64,
                200 + index as i64 * 2
            )
        );
        let samples: i64 = sqlx::query_scalar(
            "SELECT CAST(SUM(sample_count) AS BIGINT) FROM metric_history WHERE server_id='node'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(samples, index as i64 + 1);
    }

    // Neither a repeated batch ID nor an already persisted timestamp may
    // overwrite metadata or double count history/traffic during reconnects.
    let writes_before = metadata_writes(db).await;
    for (batch_id, timestamp) in [("batch-8", base + 9), ("old-sample", base)] {
        let retry = AgentReport {
            timestamp,
            ip_v4: "203.0.113.1".into(),
            ip_v6: "2001:db8::99".into(),
            net_rx_total: 999,
            net_tx_total: 999,
            ..AgentReport::default()
        };
        let result = save_agent_batch(db, &identity, batch_id, &[retry], "203.0.113.2")
            .await
            .unwrap();
        assert!(result.persisted);
        assert!(result.reports.is_empty());
        assert_eq!(result.persisted_through, base + 8);
    }
    assert_eq!(metadata_writes(db).await, writes_before);
    let metadata: (String, String, String, i64) =
        sqlx::query_as("SELECT last_ip,ip_v4,ip_v6,updated_at FROM servers WHERE id='node'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        metadata,
        (String::new(), "192.0.2.2".into(), "2001:db8::2".into(), 1)
    );
    let saved = super::super::servers::list_servers(db, true)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(saved.timestamp, Some(base + 8));
    assert_eq!(saved.cpu, Some(8.0));
    assert_eq!(saved.net_rx_total, Some(108));
    let samples: i64 = sqlx::query_scalar(
        "SELECT CAST(SUM(sample_count) AS BIGINT) FROM metric_history WHERE server_id='node'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(samples, 9);
}

async fn metadata_writes(db: &Database) -> Option<i64> {
    if db.is_postgres() {
        None
    } else {
        Some(
            sqlx::query_scalar("SELECT COUNT(*) FROM metadata_writes")
                .fetch_one(db.pool())
                .await
                .unwrap(),
        )
    }
}

#[tokio::test]
async fn unchanged_metadata_skips_writes_without_skipping_agent_persistence() {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    assert_metadata_updates_preserve_persistence(&db).await;
}

#[tokio::test]
async fn cancelled_commit_guard_rewinds_history_compaction() {
    let db = crate::db::connect("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();
    sqlx::query("INSERT INTO servers(id,name,token_hash,created_at,updated_at) VALUES ('node','Node','hash',1,1)")
        .execute(db.pool()).await.unwrap();
    let current = now().div_euclid(3600) * 3600;
    let timestamp = current - 40 * 86_400 + 1;
    let compact = || {
        super::super::rollup::compact_history_at(
            &db,
            60,
            current,
            std::time::Duration::from_secs(10),
        )
    };
    // Advance cursors past an empty range, then emulate a commit that reached
    // the database just before its waiting caller was cancelled.
    compact().await.unwrap();
    sqlx::query("INSERT INTO metric_history(server_id,timestamp) VALUES ('node',?)")
        .bind(timestamp)
        .execute(db.pool())
        .await
        .unwrap();
    let committed = db.clone();
    let (ready, receive) = tokio::sync::oneshot::channel();
    let pending = tokio::spawn(async move {
        let _invalidate = InvalidateHistory {
            db: &committed,
            since: timestamp,
        };
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    receive.await.unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    compact().await.unwrap();
    let stored: i64 = sqlx::query_scalar("SELECT timestamp FROM metric_history")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(stored, timestamp.div_euclid(3600) * 3600);
}

#[tokio::test]
#[ignore = "requires NODEFLARE_TEST_POSTGRES_URL with CREATE DATABASE permission"]
async fn postgres_conditional_metadata_updates_preserve_agent_persistence() {
    let url = std::env::var("NODEFLARE_TEST_POSTGRES_URL").unwrap();
    let control = crate::db::connect(&url).await.unwrap();
    let name = format!("nodeflare_ingest_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(control.pool())
        .await
        .unwrap();
    let mut url = reqwest::Url::parse(&url).unwrap();
    url.set_path(&name);
    let db = crate::db::connect(url.as_str()).await.unwrap();
    let test_db = db.clone();
    let result = tokio::spawn(async move {
        assert_metadata_updates_preserve_persistence(&test_db).await;
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
