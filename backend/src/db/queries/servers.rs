use super::super::{Database, now};
use super::AgentIdentity;
use super::latency::latest_latency_map;
use crate::auth;
use crate::models::{AgentReport, ServerInput, ServerView};
use anyhow::Result;
use sqlx::Row;
use std::collections::{HashMap, HashSet};

pub async fn list_servers(db: &Database, include_hidden: bool) -> Result<Vec<ServerView>> {
    list_servers_with_live(db, include_hidden, &HashMap::new()).await
}

pub async fn list_servers_with_live(
    db: &Database,
    include_hidden: bool,
    live: &HashMap<String, AgentReport>,
) -> Result<Vec<ServerView>> {
    let statement = if include_hidden {
        "SELECT s.id, s.name, s.region, s.group_name, s.tags, s.hidden, s.expires_at, \
         s.traffic_limit, s.traffic_limit_type, s.price, s.billing_cycle, s.currency, \
         s.auto_renewal, s.last_ip, s.ip_v4, s.ip_v6, s.network_interface, s.reset_day, \
         s.report_interval, s.collect_interval, s.rx_correction, s.tx_correction, \
         s.agent_mirror, s.offline_notify_disabled, s.auto_update, \
         l.latest_json \
         FROM servers s LEFT JOIN server_latest_state l ON l.server_id=s.id \
         ORDER BY s.sort_order, s.created_at"
    } else {
        "SELECT s.id, s.name, s.region, s.group_name, s.tags, s.hidden, s.expires_at, \
         s.traffic_limit, s.traffic_limit_type, s.price, s.billing_cycle, s.currency, \
         s.auto_renewal, s.last_ip, s.ip_v4, s.ip_v6, s.network_interface, s.reset_day, \
         s.report_interval, s.collect_interval, s.rx_correction, s.tx_correction, \
         s.agent_mirror, s.offline_notify_disabled, s.auto_update, \
         l.latest_json \
         FROM servers s LEFT JOIN server_latest_state l ON l.server_id=s.id \
         WHERE s.hidden=0 ORDER BY s.sort_order, s.created_at"
    };
    let rows = sqlx::query(statement).fetch_all(db.pool()).await?;
    let latency = latest_latency_map(db, live).await?;
    rows.into_iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            let latest_json: Option<String> = row.try_get("latest_json")?;
            let persisted_report = latest_json
                .as_deref()
                .filter(|value| !value.is_empty() && *value != "{}")
                .and_then(|value| serde_json::from_str::<AgentReport>(value).ok());
            let report = live
                .get(&id)
                .filter(|current| {
                    current.timestamp
                        >= persisted_report
                            .as_ref()
                            .map_or(0, |report| report.timestamp)
                })
                .or(persisted_report.as_ref());
            let value = |read: fn(&AgentReport) -> f64| report.map(read);
            let integer = |read: fn(&AgentReport) -> i64| report.map(read);
            let text = |read: fn(&AgentReport) -> &String| report.as_ref().map(|r| read(r).clone());
            Ok(ServerView {
                id: id.clone(),
                name: row.try_get("name")?,
                region: row.try_get("region")?,
                group_name: row.try_get("group_name")?,
                tags: row.try_get("tags")?,
                hidden: row.try_get::<i64, _>("hidden")? != 0,
                expires_at: row.try_get("expires_at")?,
                traffic_limit: row.try_get("traffic_limit")?,
                traffic_limit_type: row.try_get("traffic_limit_type")?,
                price: row.try_get("price")?,
                billing_cycle: row.try_get("billing_cycle")?,
                currency: row.try_get("currency")?,
                auto_renewal: row.try_get::<i64, _>("auto_renewal")? != 0,
                last_ip: row.try_get("last_ip")?,
                ip_v4: row.try_get("ip_v4")?,
                ip_v6: row.try_get("ip_v6")?,
                network_interface: row.try_get("network_interface")?,
                reset_day: row.try_get("reset_day")?,
                report_interval: row.try_get("report_interval")?,
                collect_interval: row
                    .try_get::<i64, _>("collect_interval")?
                    .max(nodeflare_telemetry::MIN_UPLOAD_INTERVAL as i64),
                rx_correction: row.try_get("rx_correction")?,
                tx_correction: row.try_get("tx_correction")?,
                agent_mirror: row.try_get("agent_mirror")?,
                offline_notify_disabled: row.try_get::<i64, _>("offline_notify_disabled")? != 0,
                auto_update: row.try_get::<i64, _>("auto_update")? != 0,
                timestamp: report.as_ref().map(|report| report.timestamp),
                cpu: value(|r| r.cpu),
                load1: value(|r| r.load1),
                load5: value(|r| r.load5),
                load15: value(|r| r.load15),
                mem_used: integer(|r| r.mem_used),
                mem_total: integer(|r| r.mem_total),
                swap_used: integer(|r| r.swap_used),
                swap_total: integer(|r| r.swap_total),
                disk_used: integer(|r| r.disk_used),
                disk_total: integer(|r| r.disk_total),
                net_in: value(|r| r.net_in),
                net_out: value(|r| r.net_out),
                net_rx_total: integer(|r| r.net_rx_total),
                net_tx_total: integer(|r| r.net_tx_total),
                uptime: integer(|r| r.uptime),
                processes: integer(|r| r.processes),
                tcp_connections: integer(|r| r.tcp_connections),
                udp_connections: integer(|r| r.udp_connections),
                cpu_cores: integer(|r| r.cpu_cores),
                cpu_model: text(|r| &r.cpu_model),
                os: text(|r| &r.os),
                kernel: text(|r| &r.kernel),
                arch: text(|r| &r.arch),
                virtualization: text(|r| &r.virtualization),
                gpu_usage: value(|r| r.gpu_usage),
                gpu_model: text(|r| &r.gpu_model),
                agent_version: text(|r| &r.agent_version),
                disk_read_bps: value(|r| r.disk_read_bps),
                disk_write_bps: value(|r| r.disk_write_bps),
                disk_read_iops: value(|r| r.disk_read_iops),
                disk_write_iops: value(|r| r.disk_write_iops),
                disk_await_ms: value(|r| r.disk_await_ms),
                disk_utilization: value(|r| r.disk_utilization),
                disks: report.as_ref().map(|r| r.disks.clone()).unwrap_or_default(),
                gpus: report.as_ref().map(|r| r.gpus.clone()).unwrap_or_default(),
                latency: latency.get(&id).cloned().unwrap_or_default(),
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

pub async fn all_server_ids(db: &Database) -> Result<HashSet<String>> {
    let rows = sqlx::query("SELECT id FROM servers")
        .fetch_all(db.pool())
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("id").ok())
        .collect())
}

pub async fn create_server(db: &Database, input: &ServerInput) -> Result<(String, String)> {
    let id = uuid::Uuid::new_v4().to_string();
    let token = auth::random_token(32);
    let token_hash = auth::token_hash(&token);
    let timestamp = now();
    let mut transaction = db.pool().begin().await?;
    sqlx::query(db.sql(
        "INSERT INTO servers( \
         id, token_hash, name, region, group_name, tags, hidden, expires_at, traffic_limit, \
         traffic_limit_type, price, billing_cycle, currency, auto_renewal, last_ip, ip_v4, ip_v6, \
         network_interface, reset_day, report_interval, collect_interval, rx_correction, \
         tx_correction, agent_mirror, offline_notify_disabled, auto_update, created_at, updated_at, \
         sort_order) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '', '', '', ?, ?, ?, ?, ?, \
         ?, ?, ?, ?, ?, ?, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM servers))",
    ))
    .bind(&id)
    .bind(&token_hash)
    .bind(input.name.trim())
    .bind(input.region.trim())
    .bind(input.group_name.trim())
    .bind(input.tags.trim())
    .bind(i64::from(input.hidden))
    .bind(input.expires_at)
    .bind(input.traffic_limit)
    .bind(&input.traffic_limit_type)
    .bind(input.price)
    .bind(input.billing_cycle)
    .bind(input.currency.to_ascii_uppercase())
    .bind(i64::from(input.auto_renewal))
    .bind(input.network_interface.trim())
    .bind(input.reset_day)
    .bind(input.report_interval)
    .bind(input.collect_interval)
    .bind(input.rx_correction)
    .bind(input.tx_correction)
    .bind(input.agent_mirror.trim().trim_end_matches('/'))
    .bind(i64::from(input.offline_notify_disabled))
    .bind(i64::from(input.auto_update))
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(db.sql(
        "INSERT INTO latency_task_servers(task_id, server_id, assigned_at) \
         SELECT id, ?, ? FROM latency_tasks WHERE default_enabled=1 \
         ON CONFLICT(task_id, server_id) DO NOTHING",
    ))
    .bind(&id)
    .bind(timestamp)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((id, token))
}

pub async fn update_server(db: &Database, id: &str, input: &ServerInput) -> Result<bool> {
    let result = sqlx::query(db.sql(
        "UPDATE servers SET name=?, region=?, group_name=?, tags=?, hidden=?, expires_at=?, \
         traffic_limit=?, traffic_limit_type=?, price=?, billing_cycle=?, currency=?, auto_renewal=?, \
         network_interface=?, reset_day=?, report_interval=?, collect_interval=?, rx_correction=?, \
         tx_correction=?, agent_mirror=?, offline_notify_disabled=?, auto_update=?, updated_at=? \
         WHERE id=?",
    ))
    .bind(input.name.trim())
    .bind(input.region.trim())
    .bind(input.group_name.trim())
    .bind(input.tags.trim())
    .bind(i64::from(input.hidden))
    .bind(input.expires_at)
    .bind(input.traffic_limit)
    .bind(&input.traffic_limit_type)
    .bind(input.price)
    .bind(input.billing_cycle)
    .bind(input.currency.to_ascii_uppercase())
    .bind(i64::from(input.auto_renewal))
    .bind(input.network_interface.trim())
    .bind(input.reset_day)
    .bind(input.report_interval)
    .bind(input.collect_interval)
    .bind(input.rx_correction)
    .bind(input.tx_correction)
    .bind(input.agent_mirror.trim().trim_end_matches('/'))
    .bind(i64::from(input.offline_notify_disabled))
    .bind(i64::from(input.auto_update))
    .bind(now())
    .bind(id)
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_server(db: &Database, id: &str) -> Result<bool> {
    let result = sqlx::query(db.sql("DELETE FROM servers WHERE id=?"))
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_servers(db: &Database, ids: &[String]) -> Result<()> {
    let mut transaction = db.pool().begin().await?;
    for id in ids {
        sqlx::query(db.sql("DELETE FROM servers WHERE id=?"))
            .bind(id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn reorder_servers(db: &Database, ids: &[String]) -> Result<()> {
    let mut transaction = db.pool().begin().await?;
    for (index, id) in ids.iter().enumerate() {
        sqlx::query(db.sql("UPDATE servers SET sort_order=?, updated_at=? WHERE id=?"))
            .bind(index as i64)
            .bind(now())
            .bind(id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn agent_install_token(db: &Database, id: &str) -> Result<Option<String>> {
    let token = auth::random_token(32);
    // The existence check and the insert share one transaction so a server
    // deleted in between cannot turn a missing node into a foreign-key 500.
    let mut transaction = if db.is_postgres() {
        db.pool().begin().await?
    } else {
        db.pool().begin_with("BEGIN IMMEDIATE").await?
    };
    let exists = sqlx::query_scalar::<_, i64>(db.sql("SELECT COUNT(*) FROM servers WHERE id=?"))
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;
    if exists == 0 {
        transaction.rollback().await?;
        return Ok(None);
    }
    sqlx::query(db.sql(
        "INSERT INTO server_install_tokens(token_hash, server_id, created_at) VALUES (?, ?, ?)",
    ))
    .bind(auth::token_hash(&token))
    .bind(id)
    .bind(now())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(token))
}

pub async fn server_name(db: &Database, id: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, String>(db.sql("SELECT name FROM servers WHERE id=?"))
            .bind(id)
            .fetch_optional(db.pool())
            .await?,
    )
}

pub async fn agent_identity(db: &Database, token: &str) -> Result<Option<AgentIdentity>> {
    let token_hash = auth::token_hash(token);
    let row = sqlx::query(db.sql(
        "SELECT id, hidden, report_interval, reset_day, rx_correction, \
         tx_correction FROM servers WHERE token_hash=? OR EXISTS (\
         SELECT 1 FROM server_install_tokens WHERE server_id=servers.id AND token_hash=?\
         )",
    ))
    .bind(&token_hash)
    .bind(&token_hash)
    .fetch_optional(db.pool())
    .await?;
    row.map(|row| {
        Ok::<AgentIdentity, sqlx::Error>(AgentIdentity {
            server_id: row.try_get("id")?,
            hidden: row.try_get::<i64, _>("hidden")? != 0,
            report_interval: row.try_get("report_interval")?,
            reset_day: row.try_get("reset_day")?,
            rx_correction: row.try_get("rx_correction")?,
            tx_correction: row.try_get("tx_correction")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn agent_config(db: &Database, id: &str) -> Result<Option<serde_json::Value>> {
    let row = sqlx::query(db.sql(
        "SELECT report_interval, collect_interval, network_interface, agent_mirror, auto_update \
         FROM servers WHERE id=?",
    ))
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let tasks = super::latency::tasks_for_server(db, id).await?;
    Ok(Some(serde_json::json!({
        "report_interval": row.try_get::<i64, _>("report_interval")?,
        "collect_interval": row.try_get::<i64, _>("collect_interval")?.max(nodeflare_telemetry::MIN_UPLOAD_INTERVAL as i64),
        "network_interface": row.try_get::<String, _>("network_interface")?,
        "agent_mirror": row.try_get::<String, _>("agent_mirror")?,
        "auto_update": row.try_get::<i64, _>("auto_update")? != 0,
        "latency_tasks": tasks,
    })))
}

pub async fn public_server_exists(db: &Database, server_id: &str) -> Result<bool> {
    let count = sqlx::query_scalar::<_, i64>(
        db.sql("SELECT COUNT(*) FROM servers WHERE id=? AND hidden=0"),
    )
    .bind(server_id)
    .fetch_one(db.pool())
    .await?;
    Ok(count > 0)
}
