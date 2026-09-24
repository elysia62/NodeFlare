/// Shared by the alert-rule and latency-task assignment writers.
pub(crate) const ASSIGNMENT_INSERT_BATCH_ROWS: usize = 200;
use crate::models::AgentReport;

#[derive(Debug, Clone)]
pub struct AgentIdentity {
    pub server_id: String,
    pub hidden: bool,
    pub report_interval: i64,
    pub reset_day: i64,
    pub rx_correction: i64,
    pub tx_correction: i64,
}

#[derive(Debug, Clone)]
pub struct PersistResult {
    pub reports: Vec<AgentReport>,
    pub persisted: bool,
    pub persisted_through: i64,
    pub next_persist_after_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TrafficState {
    cycle_key: i64,
    reset_day: i64,
    pub timestamp: i64,
    raw_rx: i64,
    raw_tx: i64,
    used_rx: i64,
    used_tx: i64,
}

mod alerts;
mod exchange;
mod history;
mod ingest;
mod latency;
mod maintenance;
mod remote;
mod servers;
mod telegram;
mod themes;
mod totp;

pub use alerts::{
    AlertNotification, AlertObservation, PendingAlertNotification, complete_notification,
    create_alert_rule, delete_alert_rule, evaluate_resource_rules, list_alert_rules,
    next_notification, record_alert, retry_notification, update_alert_rule,
};
pub use exchange::{
    ExchangeSnapshot, exchange_snapshot, mark_exchange_attempt, upsert_exchange_snapshot,
};
#[cfg(test)]
pub use history::HISTORY_AGGREGATE_COLUMNS;
pub use history::history;
pub use ingest::save_agent_batch;
pub(crate) use ingest::{agent_traffic_state, apply_traffic, valid_agent_report};
pub use latency::{
    create_latency_task, delete_latency_task, latency_history, latency_task_server_ids,
    list_latency_tasks, update_latency_task,
};
pub use maintenance::cleanup_database;
pub use remote::{
    create_remote_task, mark_remote_task_sent, remote_task, update_remote_task_result,
};
pub use servers::{
    agent_config, agent_identity, agent_install_token, all_server_ids, create_server,
    delete_server, delete_servers, list_servers, list_servers_with_live, public_server_exists,
    reorder_servers, server_name, update_server,
};
pub use telegram::{raw_telegram_settings, save_telegram_settings, telegram_settings};
pub use themes::{
    cleanup_theme_previews, create_theme, create_theme_preview, delete_theme, list_themes,
    set_active_theme, theme_asset, theme_exists, theme_preview_url, theme_resolved_url,
};
pub use totp::{get_totp_secret, save_totp_secret, set_totp_enabled};

#[cfg(test)]
mod tests {
    use super::super::{Database, now};
    use super::*;
    use crate::db::SECRET_MASK;
    use crate::models::{
        AgentLatencyResult, AlertRuleInput, LatencyTaskInput, ServerInput, TelegramSettingsInput,
    };
    use history::aggregate_history;
    use ingest::AGENT_REPORT_MAX_LATENCY_RESULTS;
    use latency::latency_history_bucket_seconds;
    use maintenance::cleanup_database_with_budget;
    use sqlx::AssertSqlSafe;
    use std::collections::{HashMap, HashSet};

    #[tokio::test]
    async fn telegram_settings_mask_and_preserve_saved_credentials() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        assert!(telegram_settings(&db).await.unwrap().is_none());
        let mut input = TelegramSettingsInput {
            bot_token: "123456789:test-token".into(),
            chat_id: "-1001234567890".into(),
            message_thread_id: None,
            template: "{{message}}".into(),
        };
        save_telegram_settings(&db, &input).await.unwrap();
        let view = telegram_settings(&db).await.unwrap().unwrap();
        assert_eq!(view.bot_token, SECRET_MASK);
        assert_eq!(view.chat_id, SECRET_MASK);

        input.bot_token = view.bot_token;
        input.chat_id = view.chat_id;
        input.template = "{{title}}: {{message}}".into();
        input.message_thread_id = Some(123);
        save_telegram_settings(&db, &input).await.unwrap();
        let raw = raw_telegram_settings(&db).await.unwrap().unwrap();
        assert_eq!(raw.bot_token, "123456789:test-token");
        assert_eq!(raw.chat_id, "-1001234567890");
        assert_eq!(raw.template, input.template);
        assert_eq!(raw.message_thread_id, Some(123));

        input.chat_id = " -1009876543210 ".into();
        save_telegram_settings(&db, &input).await.unwrap();
        let raw = raw_telegram_settings(&db).await.unwrap().unwrap();
        assert_eq!(raw.chat_id, "-1009876543210");
        assert_eq!(raw.bot_token, "123456789:test-token");

        input.chat_id = SECRET_MASK.into();
        input.bot_token = "987654321:replacement-token".into();
        save_telegram_settings(&db, &input).await.unwrap();
        let raw = raw_telegram_settings(&db).await.unwrap().unwrap();
        assert_eq!(raw.chat_id, "-1009876543210");
        assert_eq!(raw.bot_token, input.bot_token);
        let view = telegram_settings(&db).await.unwrap().unwrap();
        assert_eq!(view.bot_token, SECRET_MASK);
        assert_eq!(view.chat_id, SECRET_MASK);
    }

    fn server_input(traffic_limit: i64) -> ServerInput {
        ServerInput {
            name: "Large traffic node".to_string(),
            region: "CN".to_string(),
            group_name: "Test".to_string(),
            tags: String::new(),
            hidden: false,
            expires_at: None,
            traffic_limit,
            traffic_limit_type: "sum".to_string(),
            price: 1.0,
            billing_cycle: 30,
            currency: "CNY".to_string(),
            auto_renewal: false,
            network_interface: String::new(),
            reset_day: 1,
            report_interval: 60,
            collect_interval: 5,
            rx_correction: 0,
            tx_correction: 0,
            agent_mirror: String::new(),
            offline_notify_disabled: false,
            auto_update: true,
        }
    }

    async fn test_server() -> (Database, AgentIdentity) {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (_, token) = create_server(&db, &server_input(0)).await.unwrap();
        let identity = agent_identity(&db, &token).await.unwrap().unwrap();
        (db, identity)
    }

    fn alert_input(server_ids: Vec<String>, all_servers: bool) -> AlertRuleInput {
        AlertRuleInput {
            name: "CPU".to_string(),
            metric: "cpu".to_string(),
            threshold: 80.0,
            duration_minutes: 1,
            aggregation: "average".to_string(),
            all_servers,
            enabled: true,
            server_ids,
        }
    }

    #[tokio::test]
    async fn deleting_selected_servers_never_expands_alert_scope() {
        let (db, identity) = test_server().await;
        let a = identity.server_id;
        let (b, _) = create_server(&db, &server_input(0)).await.unwrap();
        let (c, _) = create_server(&db, &server_input(0)).await.unwrap();
        let single = create_alert_rule(&db, &alert_input(vec![a.clone()], false))
            .await
            .unwrap();
        let multiple = create_alert_rule(&db, &alert_input(vec![a.clone(), b.clone()], false))
            .await
            .unwrap();
        let global = create_alert_rule(&db, &alert_input(vec![], true))
            .await
            .unwrap();
        delete_server(&db, &a).await.unwrap();
        let rules = list_alert_rules(&db).await.unwrap();
        let single_rule = rules.iter().find(|rule| rule.id == single).unwrap();
        assert!(!single_rule.all_servers);
        assert!(single_rule.server_ids.is_empty());
        assert_eq!(
            rules
                .iter()
                .find(|rule| rule.id == multiple)
                .unwrap()
                .server_ids,
            std::slice::from_ref(&b)
        );
        for (server_id, expected) in [(&b, vec![multiple, global.clone()]), (&c, vec![global])] {
            let actual: HashSet<_> = evaluate_resource_rules(&db, server_id)
                .await
                .unwrap()
                .into_iter()
                .map(|evaluation| evaluation.rule.id)
                .collect();
            assert_eq!(actual, expected.into_iter().collect());
        }
    }

    fn observation<'a>(
        key: &'a str,
        server_id: &'a str,
        rule_id: Option<&'a str>,
        active: bool,
    ) -> AlertObservation<'a> {
        AlertObservation {
            key,
            server_id,
            rule_id,
            active,
            details: serde_json::json!({"active": active}),
            notification: Some(AlertNotification {
                title: "CPU",
                server_name: "Node",
                message: "Threshold changed",
            }),
        }
    }

    #[tokio::test]
    async fn alert_observations_are_atomic_and_the_queue_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", directory.path().join("alerts.db").display());
        let db = crate::db::connect(&url).await.unwrap();
        db.migrate().await.unwrap();
        let (id, _) = create_server(&db, &server_input(0)).await.unwrap();
        let (first, repeated) = tokio::join!(
            record_alert(&db, observation("offline", &id, None, true)),
            record_alert(&db, observation("offline", &id, None, true)),
        );
        first.unwrap();
        repeated.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_outbox")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        for index in 1..64 {
            record_alert(&db, observation("offline", &id, None, index % 2 == 0))
                .await
                .unwrap();
        }
        assert!(
            record_alert(&db, observation("offline", &id, None, true))
                .await
                .is_err()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT active FROM alert_states")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        db.pool().close().await;
        let reopened = crate::db::connect(&url).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_outbox")
                .fetch_one(reopened.pool())
                .await
                .unwrap(),
            64
        );
        assert!(next_notification(&reopened).await.unwrap().is_some());
        delete_server(&reopened, &id).await.unwrap();
        assert!(next_notification(&reopened).await.unwrap().is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alert_states")
                .fetch_one(reopened.pool())
                .await
                .unwrap(),
            0
        );
        reopened.pool().close().await;
    }

    #[tokio::test]
    async fn deleting_a_rule_cascades_only_its_states_and_notifications() {
        let (db, identity) = test_server().await;
        let id = &identity.server_id;
        let rule = create_alert_rule(&db, &alert_input(vec![id.clone()], false))
            .await
            .unwrap();
        record_alert(&db, observation("resource", id, Some(&rule), true))
            .await
            .unwrap();
        record_alert(&db, observation("offline", id, None, true))
            .await
            .unwrap();
        delete_alert_rule(&db, &rule).await.unwrap();
        for table in ["alert_states", "notification_outbox"] {
            assert_eq!(
                sqlx::query_scalar::<_, String>(AssertSqlSafe(format!(
                    "SELECT state_key FROM {table}"
                )))
                .fetch_all(db.pool())
                .await
                .unwrap(),
                ["offline"]
            );
        }
    }

    #[tokio::test]
    async fn agent_install_tokens_do_not_replace_the_primary_token() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (id, primary_token) = create_server(&db, &server_input(0)).await.unwrap();
        let first_install_token = agent_install_token(&db, &id).await.unwrap().unwrap();
        let second_install_token = agent_install_token(&db, &id).await.unwrap().unwrap();
        assert!(agent_identity(&db, &primary_token).await.unwrap().is_some());
        assert!(
            agent_identity(&db, &first_install_token)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            agent_identity(&db, &second_install_token)
                .await
                .unwrap()
                .is_some()
        );
        assert_ne!(first_install_token, second_install_token);
    }

    fn sample(timestamp: i64, cpu: f64) -> AgentReport {
        AgentReport {
            timestamp,
            cpu,
            load1: cpu / 10.0,
            mem_used: cpu as i64 * 10,
            mem_total: 1000,
            disk_used: cpu as i64 * 10,
            disk_total: 1000,
            net_in: cpu * 1_048_576.0,
            net_out: cpu * 2_097_152.0,
            disk_read_bps: cpu * 100.0,
            gpu_usage: cpu,
            net_rx_total: timestamp * 10,
            net_tx_total: timestamp * 20,
            ..AgentReport::default()
        }
    }

    #[tokio::test]
    async fn aggregate_batches_merge_weighted_values_and_ignore_retries() {
        let (db, identity) = test_server().await;
        let start = now().div_euclid(60) * 60 - 120;
        let reports = [
            sample(start, 10.0),
            sample(start + 1, 90.0),
            sample(start + 2, 20.0),
        ];
        let first = save_agent_batch(&db, &identity, "first", &reports[..2], "")
            .await
            .unwrap();
        assert_eq!(first.persisted_through, start + 1);
        // Reconnect with an overlapping batch: only the unacknowledged sample is added.
        let second = save_agent_batch(&db, &identity, "overlap", &reports[1..], "")
            .await
            .unwrap();
        assert_eq!(second.persisted_through, start + 2);
        for id in ["overlap", "another-retry"] {
            let retry = save_agent_batch(&db, &identity, id, &reports, "")
                .await
                .unwrap();
            assert!(retry.reports.is_empty());
            assert_eq!(retry.persisted_through, start + 2);
        }
        let points = history(&db, &identity.server_id, 1).await.unwrap();
        assert_eq!(points.len(), 1);
        let point = &points[0];
        assert_eq!(point.sample_count, 3);
        assert_eq!(point.cpu, 40.0);
        assert_eq!(point.cpu_min, 10.0);
        assert_eq!(point.cpu_max, 90.0);
        assert_eq!(point.mem_used, 400);
        assert_eq!(point.mem_used_max, 900);
        assert_eq!(point.net_in, 90.0 * 1_048_576.0);
        assert_eq!(point.disk_read_bps, 9000.0);
        assert_eq!(point.net_rx_total, reports[2].net_rx_total);
        let latest = list_servers(&db, true).await.unwrap().remove(0);
        assert_eq!(latest.timestamp, Some(start + 2));
        assert_eq!(latest.cpu, Some(20.0));
    }

    #[tokio::test]
    async fn realtime_batches_cannot_acknowledge_unpersisted_history() {
        let (db, identity) = test_server().await;
        let start = now() - 180;
        let reports = (0..121)
            .map(|offset| sample(start + offset, 25.0))
            .collect::<Vec<_>>();
        save_agent_batch(&db, &identity, "initial", &reports[..1], "")
            .await
            .unwrap();
        let mut buffer = crate::websocket::ingest::AgentBuffer::new(&db, &identity.server_id)
            .await
            .unwrap();
        let live = buffer
            .receive(&db, &identity, "", reports[120..].to_vec(), false)
            .await
            .unwrap();
        assert!(live.acknowledgement.is_none());
        assert_eq!(live.latest.unwrap().timestamp, start + 120);
        assert_eq!(
            list_servers(&db, true).await.unwrap()[0].timestamp,
            Some(start)
        );
        // Even a payload-size-limited batch shorter than report_interval must commit.
        for (index, batch) in reports[1..].chunks(3).enumerate() {
            let result = save_agent_batch(&db, &identity, &format!("batch-{index}"), batch, "")
                .await
                .unwrap();
            assert!(result.persisted);
            assert_eq!(result.persisted_through, batch.last().unwrap().timestamp);
        }
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT CAST(SUM(sample_count) AS BIGINT) FROM metric_history",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 121);
    }

    #[tokio::test]
    async fn latest_latency_cache_respects_probe_assignment_boundaries() {
        let (db, identity) = test_server().await;
        let current = now();
        let task_id = create_latency_task(
            &db,
            &LatencyTaskInput {
                name: "Cache test".into(),
                task_type: "tcp".into(),
                target: "example.com".into(),
                port: Some(443),
                interval_seconds: 60,
                default_enabled: false,
                server_ids: vec![identity.server_id.clone()],
            },
        )
        .await
        .unwrap();
        let live = HashMap::from([(
            identity.server_id.clone(),
            AgentReport {
                timestamp: current,
                latency_results: vec![AgentLatencyResult {
                    task_id: task_id.clone(),
                    timestamp: current,
                    latency_ms: 28.4,
                    packet_loss: 0.0,
                }],
                ..AgentReport::default()
            },
        )]);
        assert_eq!(
            list_servers_with_live(&db, true, &live).await.unwrap()[0].latency[0].latency_ms,
            28.4
        );
        sqlx::query("UPDATE latency_task_servers SET assigned_at=? WHERE task_id=?")
            .bind(current + 1)
            .bind(&task_id)
            .execute(db.pool())
            .await
            .unwrap();
        let latest = list_servers_with_live(&db, true, &live)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(latest.latency[0].timestamp, 0);
        assert_eq!(latest.latency[0].latency_ms, -1.0);
    }

    #[tokio::test]
    async fn live_buffer_preserves_each_one_second_sample_for_dashboard_playback() {
        use crate::websocket::ingest::AgentBuffer;
        let (db, identity) = test_server().await;
        let start = now() - 10;
        let mut buffer = AgentBuffer::new(&db, &identity.server_id).await.unwrap();
        let reports = (0..3)
            .map(|index| sample(start + index, 10.0 + index as f64))
            .collect::<Vec<_>>();
        let result = buffer
            .receive(&db, &identity, "", reports.clone(), false)
            .await
            .unwrap();
        assert_eq!(
            result
                .samples
                .iter()
                .map(|report| (report.timestamp, report.cpu))
                .collect::<Vec<_>>(),
            vec![(start, 10.0), (start + 1, 11.0), (start + 2, 12.0)]
        );
        assert_eq!(result.latest.unwrap().timestamp, start + 2);
        assert!(result.acknowledgement.is_none());
        let duplicate = buffer
            .receive(&db, &identity, "", reports, false)
            .await
            .unwrap();
        assert!(duplicate.samples.is_empty());
    }

    #[tokio::test]
    async fn live_buffer_commits_without_reupload_and_replays_only_unconfirmed_samples() {
        use crate::websocket::ingest::AgentBuffer;
        let (db, identity) = test_server().await;
        let start = now() - 120;
        let first = sample(start, 10.0);
        let second = sample(start + 3, 20.0);
        let third = sample(start + 6, 30.0);
        let mut buffer = AgentBuffer::new(&db, &identity.server_id).await.unwrap();
        let initial = buffer
            .receive(&db, &identity, "", vec![first.clone()], true)
            .await
            .unwrap();
        assert_eq!(initial.acknowledgement.unwrap().persisted_through, start);

        let result = buffer
            .receive(&db, &identity, "", vec![second.clone()], false)
            .await
            .unwrap();
        assert!(result.acknowledgement.is_none());
        let latest = result.latest.unwrap();
        let snapshot = HashMap::from([(identity.server_id.clone(), latest)]);
        assert_eq!(
            list_servers_with_live(&db, true, &snapshot).await.unwrap()[0].cpu,
            Some(20.0)
        );
        assert_eq!(list_servers(&db, true).await.unwrap()[0].cpu, Some(10.0));
        let duplicate = buffer
            .receive(&db, &identity, "", vec![second.clone()], false)
            .await
            .unwrap();
        assert!(duplicate.latest.is_none());
        let committed = buffer
            .receive(&db, &identity, "", Vec::new(), true)
            .await
            .unwrap();
        assert_eq!(
            committed.acknowledgement.unwrap().persisted_through,
            start + 3
        );
        assert!(committed.latest.is_none());

        buffer
            .receive(&db, &identity, "", vec![third.clone()], false)
            .await
            .unwrap();
        drop(buffer);
        let mut reconnected = AgentBuffer::new(&db, &identity.server_id).await.unwrap();
        let replay = reconnected
            .receive(&db, &identity, "", vec![first, second, third], true)
            .await
            .unwrap();
        let ack = replay.acknowledgement.unwrap();
        assert_eq!(ack.persisted_through, start + 6);
        assert_eq!(ack.reports.len(), 1);
        assert_eq!(
            history(&db, &identity.server_id, 1)
                .await
                .unwrap()
                .iter()
                .map(|point| point.sample_count)
                .sum::<i64>(),
            3
        );
        assert_eq!(
            ack.reports[0].net_rx_total,
            replay.latest.unwrap().net_rx_total
        );
    }

    #[tokio::test]
    async fn live_buffer_bounds_pending_samples_and_retains_all_latency_results() {
        use crate::websocket::ingest::AgentBuffer;
        let (db, identity) = test_server().await;
        let start = now() - 2400;
        let reports: Vec<_> = (0..721)
            .map(|index| AgentReport {
                timestamp: start + index * 3,
                latency_results: vec![AgentLatencyResult {
                    task_id: "test-task".into(),
                    timestamp: start + index * 3,
                    latency_ms: 20.0,
                    packet_loss: 0.0,
                }],
                ..AgentReport::default()
            })
            .collect();
        let mut buffer = AgentBuffer::new(&db, &identity.server_id).await.unwrap();
        let buffered = buffer
            .receive(&db, &identity, "", reports[..719].to_vec(), false)
            .await
            .unwrap();
        assert!(buffered.acknowledgement.is_none());
        assert_eq!(buffered.latest.unwrap().latency_results.len(), 719);
        let bounded = buffer
            .receive(&db, &identity, "", reports[719..].to_vec(), false)
            .await
            .unwrap();
        assert_eq!(
            bounded.acknowledgement.unwrap().persisted_through,
            reports[718].timestamp
        );
        assert_eq!(bounded.latest.unwrap().latency_results.len(), 2);
        let committed = buffer
            .receive(&db, &identity, "", Vec::new(), true)
            .await
            .unwrap();
        assert_eq!(
            committed.acknowledgement.unwrap().persisted_through,
            reports[720].timestamp
        );
        assert_eq!(
            history(&db, &identity.server_id, 1)
                .await
                .unwrap()
                .iter()
                .map(|point| point.sample_count)
                .sum::<i64>(),
            721
        );
    }

    #[tokio::test]
    async fn failed_persistence_rolls_back_history_and_watermark() {
        let (db, identity) = test_server().await;
        let start = now() - 60;
        save_agent_batch(&db, &identity, "initial", &[sample(start, 10.0)], "")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_latest BEFORE UPDATE ON server_latest_state \
            BEGIN SELECT RAISE(ABORT, 'test write failure'); END",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let next = sample(start + 1, 90.0);
        let mut buffer = crate::websocket::ingest::AgentBuffer::new(&db, &identity.server_id)
            .await
            .unwrap();
        assert!(
            buffer
                .receive(&db, &identity, "", vec![next], true)
                .await
                .is_err()
        );
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT CAST(SUM(sample_count) AS BIGINT) FROM metric_history",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
        let timestamp =
            sqlx::query_scalar::<_, i64>("SELECT latest_timestamp FROM server_latest_state")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(timestamp, start);
        sqlx::query("DROP TRIGGER reject_latest")
            .execute(db.pool())
            .await
            .unwrap();
        let result = buffer
            .receive(&db, &identity, "", Vec::new(), true)
            .await
            .unwrap();
        assert_eq!(result.acknowledgement.unwrap().persisted_through, start + 1);
    }

    #[tokio::test]
    async fn history_query_weights_different_windows() {
        let (db, identity) = test_server().await;
        let start = (now() - 1200).div_euclid(120) * 120;
        let reports = [
            sample(start, 90.0),
            sample(start + 1, 90.0),
            sample(start + 2, 90.0),
            sample(start + 60, 10.0),
        ];
        save_agent_batch(&db, &identity, "weighted", &reports, "")
            .await
            .unwrap();
        let points = history(&db, &identity.server_id, 24).await.unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].cpu, 70.0);
        assert_eq!(points[0].cpu_min, 10.0);
        assert_eq!(points[0].cpu_max, 90.0);
        assert_eq!(points[0].sample_count, 4);
    }

    #[tokio::test]
    async fn history_includes_a_bucket_crossing_the_query_start() {
        let (db, identity) = test_server().await;
        let current = now();
        let since = current - 3600;
        sqlx::query(
            "INSERT INTO metric_history(server_id,timestamp,cpu,cpu_min,cpu_max,mem_used_max,first_timestamp,last_timestamp) \
             VALUES (?,?,?,42.0,42.0,0,?,?)",
        )
        .bind(&identity.server_id)
        .bind(since - 4)
        .bind(42.0)
        .bind(since - 4)
        .bind(since + 10)
        .execute(db.pool())
        .await
        .unwrap();

        let points = history(&db, &identity.server_id, 1).await.unwrap();
        assert!(points.iter().any(|point| point.cpu == 42.0));
    }

    #[tokio::test]
    async fn aggregate_alerts_use_average_and_minimum_not_bucket_peaks() {
        let (db, identity) = test_server().await;
        let start = now() - 120;
        let reports = (0..=120)
            .map(|offset| sample(start + offset, if offset == 60 { 10.0 } else { 90.0 }))
            .collect::<Vec<_>>();
        save_agent_batch(&db, &identity, "alert-data", &reports, "")
            .await
            .unwrap();
        for metric in ["cpu", "memory", "disk", "net_in", "net_out"] {
            for aggregation in ["average", "continuous"] {
                create_alert_rule(
                    &db,
                    &AlertRuleInput {
                        name: format!("{metric}-{aggregation}"),
                        metric: metric.to_string(),
                        threshold: 50.0,
                        duration_minutes: 2,
                        aggregation: aggregation.to_string(),
                        all_servers: false,
                        enabled: true,
                        server_ids: vec![identity.server_id.clone()],
                    },
                )
                .await
                .unwrap();
            }
        }
        let evaluations = evaluate_resource_rules(&db, &identity.server_id)
            .await
            .unwrap();
        assert_eq!(evaluations.len(), 10);
        for evaluation in evaluations {
            assert_eq!(
                evaluation.triggered,
                evaluation.rule.aggregation == "average",
                "{}: {}",
                evaluation.rule.name,
                evaluation.value
            );
        }
    }

    #[tokio::test]
    async fn concurrent_sqlite_retries_are_counted_once() {
        let directory = tempfile::tempdir().unwrap();
        let db = crate::db::connect(&format!(
            "sqlite://{}",
            directory.path().join("concurrent.db").display()
        ))
        .await
        .unwrap();
        db.migrate().await.unwrap();
        let (_, token) = create_server(&db, &server_input(0)).await.unwrap();
        let identity = agent_identity(&db, &token).await.unwrap().unwrap();
        let reports = vec![sample(now() - 60, 20.0)];
        let mut handles = Vec::new();
        for index in 0..8 {
            let db = db.clone();
            let identity = identity.clone();
            let reports = reports.clone();
            handles.push(tokio::spawn(async move {
                save_agent_batch(&db, &identity, &format!("retry-{index}"), &reports, "").await
            }));
        }
        for handle in handles {
            handle.await.unwrap().unwrap();
        }
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT CAST(SUM(sample_count) AS BIGINT) FROM metric_history",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
        db.pool().close().await;
    }

    #[test]
    fn aggregation_uses_the_configured_window_and_preserves_counter_resets() {
        let reports = [sample(3600, 10.0), sample(3614, 30.0), sample(3615, 90.0)];
        let rows = aggregate_history(&reports, 15);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].report.cpu, 20.0);
        assert_eq!(rows[0].sample_count, 2);
        assert_eq!(aggregate_history(&reports, 3600).len(), 1);
        let mut reports = reports;
        reports[2].net_rx_total = 5;
        assert_eq!(aggregate_history(&reports, 60)[0].report.net_rx_total, 5);
    }

    #[test]
    fn validates_agent_report_bounds() {
        let current = now();
        let mut report = AgentReport {
            timestamp: current,
            ..AgentReport::default()
        };
        assert!(valid_agent_report(&report, current));

        report.cpu = f64::NAN;
        assert!(!valid_agent_report(&report, current));
        report.cpu = 10.0;
        report.cpu_model = "x".repeat(513);
        assert!(!valid_agent_report(&report, current));
        report.cpu_model.clear();
        report.latency_results = vec![AgentLatencyResult {
            task_id: "failed-probe".to_string(),
            timestamp: current,
            latency_ms: -1.0,
            packet_loss: 100.0,
        }];
        assert!(valid_agent_report(&report, current));
        report.latency_results[0].packet_loss = -1.0;
        assert!(!valid_agent_report(&report, current));
        report.latency_results = (0..=AGENT_REPORT_MAX_LATENCY_RESULTS)
            .map(|index| AgentLatencyResult {
                task_id: format!("task-{index}"),
                timestamp: current,
                latency_ms: 10.0,
                packet_loss: 0.0,
            })
            .collect();
        assert!(!valid_agent_report(&report, current));
    }

    #[tokio::test]
    async fn sqlite_preserves_large_traffic_limits() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let expected = 100_i64 * 1024 * 1024 * 1024;
        let (id, _) = create_server(&db, &server_input(expected)).await.unwrap();
        assert_eq!(
            server_name(&db, &id).await.unwrap().as_deref(),
            Some("Large traffic node")
        );
        let raw =
            sqlx::query_scalar::<_, i64>(db.sql("SELECT traffic_limit FROM servers WHERE id=?"))
                .bind(&id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(raw, expected);
        let server = list_servers(&db, true)
            .await
            .unwrap()
            .into_iter()
            .find(|server| server.id == id)
            .unwrap();
        assert_eq!(server.traffic_limit, expected);
    }

    #[tokio::test]
    async fn sqlite_persists_history_at_report_interval_and_all_latency_results() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (id, token) = create_server(&db, &server_input(0)).await.unwrap();
        let latency_task_id = create_latency_task(
            &db,
            &LatencyTaskInput {
                name: "Batch latency".to_string(),
                task_type: "tcp".to_string(),
                target: "example.com".to_string(),
                port: Some(443),
                interval_seconds: 60,
                default_enabled: false,
                server_ids: vec![id.clone()],
            },
        )
        .await
        .unwrap();
        let identity = agent_identity(&db, &token).await.unwrap().unwrap();
        let start = now().div_euclid(60) * 60 - 600;
        let reports = (0..600)
            .map(|offset| AgentReport {
                timestamp: start + offset,
                cpu: offset as f64 / 10.0,
                latency_results: vec![AgentLatencyResult {
                    task_id: latency_task_id.clone(),
                    timestamp: start + offset,
                    latency_ms: if offset == 100 {
                        -1.0
                    } else {
                        10.0 + offset as f64 / 100.0
                    },
                    packet_loss: if offset == 100 { 100.0 } else { 0.0 },
                }],
                ..AgentReport::default()
            })
            .collect::<Vec<_>>();

        let result = save_agent_batch(&db, &identity, "large-batch", &reports, "127.0.0.1")
            .await
            .unwrap();
        let rows = sqlx::query_scalar::<_, i64>(
            db.sql("SELECT COUNT(*) FROM metric_history WHERE server_id=?"),
        )
        .bind(&id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        let latency_rows = sqlx::query_scalar::<_, i64>(
            db.sql("SELECT COUNT(*) FROM latency_results WHERE server_id=?"),
        )
        .bind(&id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(result.persisted);
        assert_eq!(result.reports.len(), 600);
        assert_eq!(rows, 10);
        assert_eq!(latency_rows, 600);

        let persisted_at = start + 599;
        let mut buffer = crate::websocket::ingest::AgentBuffer::new(&db, &identity.server_id)
            .await
            .unwrap();
        let skipped = buffer
            .receive(
                &db,
                &identity,
                "127.0.0.1",
                vec![AgentReport {
                    timestamp: persisted_at + 1,
                    ..AgentReport::default()
                }],
                false,
            )
            .await
            .unwrap();
        assert!(skipped.acknowledgement.is_none());
        assert_eq!(skipped.latest.unwrap().timestamp, persisted_at + 1);

        let due = save_agent_batch(
            &db,
            &identity,
            "report-due",
            &[AgentReport {
                timestamp: persisted_at + identity.report_interval,
                ..AgentReport::default()
            }],
            "127.0.0.1",
        )
        .await
        .unwrap();
        assert!(due.persisted);
        let rows = sqlx::query_scalar::<_, i64>(
            db.sql("SELECT COUNT(*) FROM metric_history WHERE server_id=?"),
        )
        .bind(&id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(rows, 11);
    }

    #[tokio::test]
    async fn sqlite_reads_a_bucket_containing_only_failed_latency() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (server_id, token) = create_server(&db, &server_input(0)).await.unwrap();
        let task_id = create_latency_task(
            &db,
            &LatencyTaskInput {
                name: "Failed latency".to_string(),
                task_type: "tcp".to_string(),
                target: "does-not-exist.invalid".to_string(),
                port: Some(443),
                interval_seconds: 60,
                default_enabled: false,
                server_ids: vec![server_id.clone()],
            },
        )
        .await
        .unwrap();
        let timestamp = now();
        let identity = agent_identity(&db, &token).await.unwrap().unwrap();
        save_agent_batch(
            &db,
            &identity,
            "failed-latency",
            &[AgentReport {
                timestamp,
                latency_results: vec![AgentLatencyResult {
                    task_id: task_id.clone(),
                    timestamp,
                    latency_ms: -1.0,
                    packet_loss: 100.0,
                }],
                ..AgentReport::default()
            }],
            "127.0.0.1",
        )
        .await
        .unwrap();

        let (_, points) = latency_history(&db, &server_id, 1).await.unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].task_id, task_id);
        assert_eq!(points[0].latency_ms, -1.0);
        assert_eq!(points[0].packet_loss, 100.0);
    }

    #[tokio::test]
    async fn latency_history_survives_metadata_edits_but_not_probe_or_assignment_changes() {
        for change in ["metadata", "target", "port", "type", "reassign"] {
            let db = super::super::connect("sqlite::memory:").await.unwrap();
            db.migrate().await.unwrap();
            let (server_id, _) = create_server(&db, &server_input(0)).await.unwrap();
            let mut input = LatencyTaskInput {
                name: "Persistent latency".to_string(),
                task_type: "tcp".to_string(),
                target: "example.com".to_string(),
                port: Some(443),
                interval_seconds: 60,
                default_enabled: false,
                server_ids: vec![server_id.clone()],
            };
            let task_id = create_latency_task(&db, &input).await.unwrap();
            let assigned_at = now() - 300;
            sqlx::query(db.sql(
                "UPDATE latency_task_servers SET assigned_at=? WHERE task_id=? AND server_id=?",
            ))
            .bind(assigned_at)
            .bind(&task_id)
            .bind(&server_id)
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(db.sql(
                "INSERT INTO latency_results(task_id, server_id, timestamp, latency_ms, packet_loss) \
                 VALUES (?, ?, ?, 12.0, 0.0)",
            ))
            .bind(&task_id)
            .bind(&server_id)
            .bind(assigned_at + 60)
            .execute(db.pool())
            .await
            .unwrap();
            assert_eq!(
                latency_history(&db, &server_id, 1).await.unwrap().1.len(),
                1
            );

            match change {
                "metadata" => {
                    input.name = "Renamed latency".to_string();
                    input.interval_seconds = 120;
                    input.default_enabled = true;
                    let (additional, _) = create_server(&db, &server_input(0)).await.unwrap();
                    input.server_ids.push(additional);
                }
                "target" => input.target = "example.org".to_string(),
                "port" => input.port = Some(80),
                "type" => {
                    input.task_type = "icmp".to_string();
                    input.port = None;
                }
                "reassign" => {
                    input.server_ids.clear();
                    update_latency_task(&db, &task_id, &input).await.unwrap();
                    assert!(
                        latency_history(&db, &server_id, 1)
                            .await
                            .unwrap()
                            .1
                            .is_empty()
                    );
                    input.server_ids.push(server_id.clone());
                }
                _ => unreachable!(),
            }
            update_latency_task(&db, &task_id, &input).await.unwrap();
            let (_, points) = latency_history(&db, &server_id, 1).await.unwrap();
            assert_eq!(
                points.len(),
                if change == "metadata" { 1 } else { 0 },
                "{change}"
            );
        }
    }

    #[tokio::test]
    async fn database_cleanup_expires_tasks_and_removes_retained_data() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (server_id, _) = create_server(&db, &server_input(0)).await.unwrap();
        let old = now() - 3 * 86_400;
        sqlx::query(
            "INSERT INTO remote_tasks(\
             id, server_id, command, status, requested_by, requested_at, completed_at, result\
             ) VALUES \
             ('pending-old', ?, 'uptime', 'pending', 'admin', ?, NULL, ''), \
             ('success-old', ?, 'uptime', 'success', 'admin', ?, ?, 'done')",
        )
        .bind(&server_id)
        .bind(old)
        .bind(&server_id)
        .bind(old)
        .bind(old)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO alert_states(state_key, server_id, active, updated_at, details_json) VALUES \
             ('inactive-old', ?, 0, ?, '{}'), ('active-old', ?, 1, ?, '{}')",
        )
        .bind(&server_id)
        .bind(old)
        .bind(&server_id)
        .bind(old)
        .execute(db.pool())
        .await
        .unwrap();

        cleanup_database(&db, 1).await.unwrap();

        let pending_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM remote_tasks WHERE id='pending-old'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let completed_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM remote_tasks WHERE id='success-old'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let inactive_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM alert_states WHERE state_key='inactive-old'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let active_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM alert_states WHERE state_key='active-old'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(pending_status, "failed");
        assert_eq!(completed_count, 0);
        assert_eq!(inactive_count, 0);
        assert_eq!(active_count, 1);
    }

    #[tokio::test]
    async fn cleanup_is_bounded_and_keeps_current_data_and_server_state() {
        let (db, identity) = test_server().await;
        let current = now();
        save_agent_batch(&db, &identity, "latest", &[sample(current, 40.0)], "")
            .await
            .unwrap();
        let task_id = create_latency_task(
            &db,
            &LatencyTaskInput {
                name: "Cleanup".to_string(),
                task_type: "tcp".to_string(),
                target: "example.com".to_string(),
                port: Some(443),
                interval_seconds: 60,
                default_enabled: false,
                server_ids: vec![identity.server_id.clone()],
            },
        )
        .await
        .unwrap();
        let old = current - 3 * 86_400;
        sqlx::query(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<2100) \
            INSERT INTO metric_history(server_id,timestamp,last_timestamp) SELECT ?,?+n,?+n FROM seq",
        )
        .bind(&identity.server_id)
        .bind(old)
        .bind(old)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO latency_results(task_id,server_id,timestamp,latency_ms,packet_loss) \
            SELECT ?,server_id,timestamp,10.0,0.0 FROM metric_history",
        )
        .bind(&task_id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO alert_states(state_key,server_id,active,updated_at) VALUES ('active',?,1,?),('expired',?,0,?)")
            .bind(&identity.server_id).bind(old).bind(&identity.server_id).bind(old).execute(db.pool()).await.unwrap();
        cleanup_database_with_budget(&db, 1, 1, std::time::Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM metric_history")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            2101
        );
        cleanup_database_with_budget(&db, 1, 1, std::time::Duration::from_secs(60))
            .await
            .unwrap();
        for table in ["metric_history", "latency_results"] {
            let count = sqlx::query_scalar::<_, i64>(AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table}"
            )))
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(count, 1101, "{table}");
        }
        // Later tables still get a turn while metrics have a backlog.
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alert_states WHERE active=0")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        cleanup_database(&db, 1).await.unwrap();
        for table in [
            "metric_history",
            "latency_results",
            "servers",
            "server_latest_state",
            "server_traffic_state",
            "latency_task_servers",
            "alert_states",
        ] {
            let count = sqlx::query_scalar::<_, i64>(AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table}"
            )))
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(count, 1, "{table}");
        }
        delete_server(&db, &identity.server_id).await.unwrap();
        for table in [
            "metric_history",
            "latency_results",
            "servers",
            "server_latest_state",
            "server_traffic_state",
            "latency_task_servers",
        ] {
            let count = sqlx::query_scalar::<_, i64>(AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table}"
            )))
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }

    #[tokio::test]
    async fn cleanup_bounds_expired_task_updates_and_keeps_live_tasks() {
        let (db, identity) = test_server().await;
        let old = now() - 3 * 86_400;
        sqlx::query(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<1100) \
            INSERT INTO remote_tasks(id,server_id,command,requested_by,requested_at) \
            SELECT CAST(n AS TEXT),?,'uptime','admin',? FROM seq",
        )
        .bind(&identity.server_id)
        .bind(old)
        .execute(db.pool())
        .await
        .unwrap();
        let live = create_remote_task(&db, &identity.server_id, "uptime", "admin")
            .await
            .unwrap();
        cleanup_database_with_budget(&db, 1, 1, std::time::Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM remote_tasks WHERE status='failed'")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1000
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM remote_tasks WHERE status='pending'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            101
        );
        assert_eq!(
            remote_task(&db, &live.id).await.unwrap().unwrap().status,
            "pending"
        );
    }

    #[tokio::test]
    async fn remote_task_delivery_and_results_are_idempotent() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let (server_id, _) = create_server(&db, &server_input(0)).await.unwrap();
        let task = create_remote_task(&db, &server_id, "uptime", "admin")
            .await
            .unwrap();

        mark_remote_task_sent(&db, &task.id, &server_id)
            .await
            .unwrap();
        mark_remote_task_sent(&db, &task.id, &server_id)
            .await
            .unwrap();
        assert_eq!(
            remote_task(&db, &task.id).await.unwrap().unwrap().status,
            "sent"
        );

        assert!(
            update_remote_task_result(&db, &server_id, &task.id, "success", "done", Some(0))
                .await
                .unwrap()
        );
        assert!(
            update_remote_task_result(&db, &server_id, &task.id, "success", "done", Some(0))
                .await
                .unwrap()
        );
        let completed = remote_task(&db, &task.id).await.unwrap().unwrap();
        assert_eq!(completed.status, "success");
        assert_eq!(completed.result, "done");
        assert_eq!(completed.exit_code, Some(0));
    }

    #[tokio::test]
    async fn sqlite_migration_contains_optimized_indexes() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let indexes = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type='index' AND name IN (\
             'metric_history_time',\
             'latency_results_time',\
             'remote_tasks_server',\
             'servers_public_sort')",
        )
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .collect::<HashSet<_>>();
        assert_eq!(indexes.len(), 4);
    }

    #[tokio::test]
    async fn sqlite_migration_rejects_invalid_server_configuration() {
        let db = super::super::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let result = sqlx::query(
            "INSERT INTO servers(\
             id, name, hidden, traffic_limit_type, currency, reset_day, report_interval, \
             collect_interval, token_hash, created_at, updated_at\
             ) VALUES ('invalid', 'Invalid', 2, 'sum', 'CNY', 1, 60, 5, 'token', 1, 1)",
        )
        .execute(db.pool())
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn latency_history_bucket_scales_beyond_128_tasks() {
        assert!(latency_history_bucket_seconds(1, 256) > latency_history_bucket_seconds(1, 128));
    }
}
