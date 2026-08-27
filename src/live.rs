use std::cell::RefCell;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use worker::*;

use crate::db::{
    AgentLiveContext, AlertMetricRow, HistoryMetricAggregate, HISTORY_WRITE_INTERVAL_SECONDS,
};
use crate::latency::LatencyMetricAggregates;
use crate::models::{AgentConfigView, AgentReport, AlertRuleView, HistoryPoint, ServerView};

const MAX_LIVE_SAMPLES: usize = 720;
const MAX_LIVE_LATENCY_RESULTS: usize = 4096;
const AGENT_MESSAGE_WINDOW_SECONDS: i64 = 60;
const MAX_AGENT_MESSAGES_PER_WINDOW: u32 = 120;
const MAX_AGENT_SAMPLES_PER_WINDOW: u32 = 1440;
const MAX_AGENT_BYTES_PER_WINDOW: u32 = 4 * 1024 * 1024;
const LIVE_REPORT_DIVISOR: i64 = 15;
const MAX_SERIALIZED_ATTACHMENT_BYTES: usize = 15 * 1024;
const MAX_CACHED_LIVE_SAMPLES: usize = 32;
const MAX_CACHED_LIVE_BYTES: usize = 12 * 1024;
const MAX_CACHED_REPLAY_UPDATES: usize = 64;
const MAX_CACHED_REPLAY_BYTES: usize = 256 * 1024;
const CACHED_LIVE_TTL_SECONDS: i64 = 5 * 60;
const ALERT_WINDOW_SECONDS: i64 = 24 * 60 * 60;
const MAX_ALERT_WINDOW_SAMPLES: usize = 24 * 60 + 1;
const ALERT_STORAGE_PREFIX: &str = "resource-alert:";
const DASHBOARD_HUB_NAME: &str = "dashboard";

fn server_hub_name(server_id: &str) -> String {
    format!("server:{server_id}")
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct AlertWindowSample(i64, u32, f64, f64, f64, f64, f64);

impl AlertWindowSample {
    fn from_history(point: &HistoryPoint) -> Self {
        let memory = if point.mem_total > 0 {
            point.mem_used as f64 * 100.0 / point.mem_total as f64
        } else {
            0.0
        };
        let disk = if point.disk_total > 0 {
            point.disk_used as f64 * 100.0 / point.disk_total as f64
        } else {
            0.0
        };
        Self(
            point.timestamp / 60 * 60,
            1,
            point.cpu,
            memory,
            disk,
            point.net_in / 1_048_576.0,
            point.net_out / 1_048_576.0,
        )
    }

    fn merge(&mut self, other: &Self) {
        self.1 = self.1.saturating_add(other.1);
        self.2 += other.2;
        self.3 += other.3;
        self.4 += other.4;
        self.5 = self.5.max(other.5);
        self.6 = self.6.max(other.6);
    }

    fn metric(&self, metric: &str) -> Option<f64> {
        let count = f64::from(self.1.max(1));
        match metric {
            "cpu" => Some(self.2 / count),
            "memory" => Some(self.3 / count),
            "disk" => Some(self.4 / count),
            "net_in" => Some(self.5),
            "net_out" => Some(self.6),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct AlertRecordRequest {
    server_id: String,
    point: HistoryPoint,
}

#[derive(Debug, Deserialize, Serialize)]
struct AlertEvaluationServer {
    id: String,
    name: String,
    report_interval: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct AlertEvaluationRequest {
    current_time: i64,
    rules: Vec<AlertRuleView>,
    servers: Vec<AlertEvaluationServer>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AlertEvaluationValue {
    pub rule_id: String,
    pub row: AlertMetricRow,
}

fn alert_storage_key(server_id: &str) -> String {
    format!("{ALERT_STORAGE_PREFIX}{server_id}")
}

fn update_alert_window(samples: &mut Vec<AlertWindowSample>, point: &HistoryPoint) {
    let sample = AlertWindowSample::from_history(point);
    let newest = samples
        .last()
        .map(|value| value.0)
        .unwrap_or(sample.0)
        .max(sample.0);
    let cutoff = newest - ALERT_WINDOW_SECONDS;
    if sample.0 < cutoff {
        return;
    }
    match samples.binary_search_by_key(&sample.0, |value| value.0) {
        Ok(index) => samples[index].merge(&sample),
        Err(index) => samples.insert(index, sample),
    }
    samples.retain(|value| value.0 >= cutoff);
    if samples.len() > MAX_ALERT_WINDOW_SAMPLES {
        let overflow = samples.len() - MAX_ALERT_WINDOW_SAMPLES;
        samples.drain(..overflow);
    }
}

fn evaluate_alert_window(
    samples: &[AlertWindowSample],
    rule: &AlertRuleView,
    server: &AlertEvaluationServer,
    current_time: i64,
) -> Option<AlertMetricRow> {
    let since = current_time - rule.duration_minutes.clamp(1, 1440) * 60;
    let mut values = samples
        .iter()
        .filter(|sample| sample.0 >= since && sample.0 <= current_time)
        .filter_map(|sample| sample.metric(&rule.metric).map(|value| (sample.0, value)));
    let (first_timestamp, first_value) = values.next()?;
    let mut count = 1_i64;
    let mut last_timestamp = first_timestamp;
    let mut total = first_value;
    let mut minimum = first_value;
    for (timestamp, value) in values {
        count += 1;
        last_timestamp = timestamp;
        total += value;
        minimum = minimum.min(value);
    }
    Some(AlertMetricRow {
        server_id: server.id.clone(),
        name: server.name.clone(),
        value: if rule.aggregation == "continuous" {
            minimum
        } else {
            total / count as f64
        },
        sample_count: count,
        first_timestamp,
        last_timestamp,
        report_interval: server.report_interval,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentLiveBatch {
    #[serde(rename = "type")]
    message_type: String,
    #[serde(rename = "batchId")]
    batch_id: String,
    samples: Vec<AgentReport>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TrafficAttachment {
    #[serde(rename = "d")]
    reset_day: i64,
    #[serde(rename = "sd")]
    state_reset_day: i64,
    #[serde(rename = "st")]
    timestamp: i64,
    #[serde(rename = "c")]
    cycle_key: i64,
    #[serde(rename = "rr")]
    raw_rx: i64,
    #[serde(rename = "rt")]
    raw_tx: i64,
    #[serde(rename = "ur")]
    used_rx: i64,
    #[serde(rename = "ut")]
    used_tx: i64,
    #[serde(rename = "rc")]
    rx_correction: i64,
    #[serde(rename = "tc")]
    tx_correction: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CachedLiveSample {
    ts: i64,
    data: serde_json::Value,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct AgentMessageBudget {
    #[serde(rename = "w")]
    window_started_at: i64,
    #[serde(rename = "m")]
    messages: u32,
    #[serde(rename = "s")]
    samples: u32,
    #[serde(rename = "b")]
    bytes: u32,
}

fn consume_agent_message_budget(
    budget: &mut AgentMessageBudget,
    now: i64,
    message_bytes: usize,
    sample_count: usize,
) -> bool {
    if budget.window_started_at <= 0
        || now < budget.window_started_at
        || now.saturating_sub(budget.window_started_at) >= AGENT_MESSAGE_WINDOW_SECONDS
    {
        *budget = AgentMessageBudget {
            window_started_at: now,
            ..AgentMessageBudget::default()
        };
    }
    budget.messages = budget.messages.saturating_add(1);
    budget.samples = budget
        .samples
        .saturating_add(u32::try_from(sample_count).unwrap_or(u32::MAX));
    budget.bytes = budget
        .bytes
        .saturating_add(u32::try_from(message_bytes).unwrap_or(u32::MAX));
    budget.messages <= MAX_AGENT_MESSAGES_PER_WINDOW
        && budget.samples <= MAX_AGENT_SAMPLES_PER_WINDOW
        && budget.bytes <= MAX_AGENT_BYTES_PER_WINDOW
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SocketAttachment {
    #[serde(rename = "r")]
    role: String,
    #[serde(rename = "s")]
    server_id: Option<String>,
    #[serde(rename = "h")]
    hidden: bool,
    #[serde(rename = "ri")]
    report_interval: i64,
    #[serde(rename = "ci")]
    collect_interval: i64,
    #[serde(rename = "dw")]
    last_d1_write_at: i64,
    #[serde(rename = "pw")]
    persisted_through: i64,
    #[serde(rename = "df")]
    d1_flush: Option<D1FlushSnapshot>,
    #[serde(rename = "tr")]
    traffic: Option<TrafficAttachment>,
    #[serde(rename = "mh")]
    history_aggregate: HistoryMetricAggregate,
    #[serde(rename = "lh")]
    latency_aggregates: LatencyMetricAggregates,
    #[serde(rename = "ls")]
    latest_samples: Vec<CachedLiveSample>,
    #[serde(rename = "lr")]
    latest_received_at: i64,
    #[serde(rename = "mb")]
    message_budget: AgentMessageBudget,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct D1FlushSnapshot {
    #[serde(rename = "h")]
    history: HistoryMetricAggregate,
    #[serde(rename = "l")]
    latency: LatencyMetricAggregates,
}

fn begin_d1_flush(attachment: &mut SocketAttachment) -> Option<D1FlushSnapshot> {
    if let Some(snapshot) = attachment.d1_flush.clone() {
        return Some(snapshot);
    }
    attachment.history_aggregate.point()?;
    let snapshot = D1FlushSnapshot {
        history: std::mem::take(&mut attachment.history_aggregate),
        latency: std::mem::take(&mut attachment.latency_aggregates),
    };
    attachment.d1_flush = Some(snapshot.clone());
    Some(snapshot)
}

fn finish_d1_flush(
    attachment: &mut SocketAttachment,
    succeeded: bool,
    persisted_at: i64,
    persisted_through: i64,
) {
    let Some(snapshot) = attachment.d1_flush.take() else {
        return;
    };
    if succeeded {
        attachment.last_d1_write_at = persisted_at;
        attachment.persisted_through = attachment.persisted_through.max(persisted_through);
    } else {
        attachment.history_aggregate.merge(snapshot.history);
        attachment.latency_aggregates.merge(snapshot.latency);
    }
}

fn agent_wss_interval_ms(attachment: &SocketAttachment, realtime_active: bool) -> i64 {
    let collect = attachment.collect_interval.clamp(1, 60);
    let report_interval = attachment.report_interval.clamp(15, 3600);
    let realtime_seconds =
        report_interval.saturating_add(LIVE_REPORT_DIVISOR - 1) / LIVE_REPORT_DIVISOR;
    let realtime = realtime_seconds.clamp(1, 60).max(collect) * 1000;
    if realtime_active {
        return realtime;
    }
    report_interval * 1000
}

fn next_d1_write_ms(attachment: &SocketAttachment, now: i64) -> i64 {
    if attachment.d1_flush.is_some() {
        return 0;
    }
    let interval = HISTORY_WRITE_INTERVAL_SECONDS;
    if attachment.last_d1_write_at <= 0 {
        return interval * 1000;
    }
    attachment
        .last_d1_write_at
        .saturating_add(interval)
        .saturating_sub(now)
        .max(0)
        .saturating_mul(1000)
}

fn live_ack_payload(
    timestamp: i64,
    persisted: bool,
    persistence_error: bool,
    persisted_through: i64,
    next_d1_write_after_ms: i64,
    next_wss_report_after_ms: i64,
    realtime_hint: bool,
) -> Result<String> {
    Ok(serde_json::to_string(&serde_json::json!({
        "type": "ack",
        "ts": timestamp,
        "persisted": persisted,
        "persistenceError": persistence_error,
        "persistedThroughTs": persisted_through,
        "nextD1WriteAfterMs": next_d1_write_after_ms,
        "nextWssReportAfterMs": next_wss_report_after_ms,
        "realtimeHint": realtime_hint
    }))?)
}

fn agent_config_payload(timestamp: i64, config: &AgentConfigView) -> Result<String> {
    Ok(serde_json::to_string(&serde_json::json!({
        "type": "config",
        "ts": timestamp,
        "config": config
    }))?)
}

fn apply_live_traffic(report: &mut AgentReport, traffic: &mut TrafficAttachment) {
    if report.timestamp <= traffic.timestamp {
        report.net_rx_total = traffic.used_rx.saturating_add(traffic.rx_correction).max(0);
        report.net_tx_total = traffic.used_tx.saturating_add(traffic.tx_correction).max(0);
        return;
    }
    let cycle_key = crate::db::traffic_cycle_key(report.timestamp, traffic.reset_day);
    if traffic.timestamp <= 0 {
        traffic.used_rx = report.net_rx_total;
        traffic.used_tx = report.net_tx_total;
    } else if cycle_key != traffic.cycle_key || traffic.reset_day != traffic.state_reset_day {
        traffic.used_rx = 0;
        traffic.used_tx = 0;
    } else {
        let rx_delta = if report.net_rx_total >= traffic.raw_rx {
            report.net_rx_total - traffic.raw_rx
        } else {
            report.net_rx_total
        };
        let tx_delta = if report.net_tx_total >= traffic.raw_tx {
            report.net_tx_total - traffic.raw_tx
        } else {
            report.net_tx_total
        };
        traffic.used_rx = traffic.used_rx.saturating_add(rx_delta);
        traffic.used_tx = traffic.used_tx.saturating_add(tx_delta);
    }
    traffic.cycle_key = cycle_key;
    traffic.state_reset_day = traffic.reset_day;
    traffic.timestamp = report.timestamp;
    traffic.raw_rx = report.net_rx_total;
    traffic.raw_tx = report.net_tx_total;
    report.net_rx_total = traffic.used_rx.saturating_add(traffic.rx_correction).max(0);
    report.net_tx_total = traffic.used_tx.saturating_add(traffic.tx_correction).max(0);
}

fn batch_update_parts_at(
    server_id: &str,
    reports: &[AgentReport],
    traffic: &mut TrafficAttachment,
    envelope_timestamp: i64,
) -> Result<(String, Vec<CachedLiveSample>)> {
    let mut reports = reports.to_vec();
    reports.sort_by_key(|report| report.timestamp);
    let mut samples = Vec::with_capacity(reports.len());
    for mut report in reports {
        apply_live_traffic(&mut report, traffic);
        let timestamp = report.timestamp;
        let mut data = serde_json::to_value(report)?;
        if let Some(data) = data.as_object_mut() {
            data.remove("timestamp");
        }
        samples.push(CachedLiveSample {
            ts: timestamp,
            data,
        });
    }
    let payload = serde_json::to_string(&serde_json::json!({
        "type": "batchUpdate",
        "ts": envelope_timestamp,
        "updates": [{
            "serverId": server_id,
            "samples": &samples
        }]
    }))?;
    Ok((payload, samples))
}

fn trim_cached_live_samples(samples: &mut Vec<CachedLiveSample>) {
    if samples.len() > MAX_CACHED_LIVE_SAMPLES {
        let overflow = samples.len() - MAX_CACHED_LIVE_SAMPLES;
        samples.drain(..overflow);
    }
    while samples.len() > 1
        && serde_json::to_vec(samples)
            .map(|encoded| encoded.len() > MAX_CACHED_LIVE_BYTES)
            .unwrap_or(true)
    {
        samples.remove(0);
    }
    if samples.len() == 1
        && serde_json::to_vec(samples)
            .map(|encoded| encoded.len() > MAX_CACHED_LIVE_BYTES)
            .unwrap_or(true)
    {
        samples.clear();
    }
}

fn trim_socket_attachment(attachment: &mut SocketAttachment) -> bool {
    while !attachment.latest_samples.is_empty()
        && serde_json::to_vec(attachment)
            .map(|encoded| encoded.len() > MAX_SERIALIZED_ATTACHMENT_BYTES)
            .unwrap_or(true)
    {
        attachment.latest_samples.remove(0);
    }
    serde_json::to_vec(attachment)
        .map(|encoded| encoded.len() <= MAX_SERIALIZED_ATTACHMENT_BYTES)
        .unwrap_or(false)
}

fn cached_update_value(
    attachment: &SocketAttachment,
    now: i64,
    server_id: Option<&str>,
) -> Option<serde_json::Value> {
    let agent_server_id = attachment.server_id.as_deref()?;
    if attachment.role != "agent"
        || attachment.hidden
        || server_id.is_some_and(|requested| requested != agent_server_id)
        || attachment.latest_received_at <= 0
        || attachment.latest_samples.is_empty()
    {
        return None;
    }
    let age = now.saturating_sub(attachment.latest_received_at);
    if !(0..=CACHED_LIVE_TTL_SECONDS).contains(&age) {
        return None;
    }
    Some(serde_json::json!({
        "serverId": agent_server_id,
        "reportAgeMs": age.saturating_mul(1000),
        "samples": &attachment.latest_samples
    }))
}

fn agent_attachment(
    server_id: String,
    hidden: bool,
    context: AgentLiveContext,
) -> SocketAttachment {
    SocketAttachment {
        role: "agent".to_string(),
        server_id: Some(server_id),
        hidden,
        report_interval: context.report_interval.clamp(15, 3600),
        collect_interval: context.collect_interval.clamp(1, 60),
        last_d1_write_at: context.last_persisted_at,
        persisted_through: context.last_persisted_at,
        d1_flush: None,
        traffic: Some(TrafficAttachment {
            reset_day: context.reset_day.clamp(1, 31),
            state_reset_day: context.traffic_reset_day.clamp(1, 31),
            timestamp: context.traffic_timestamp,
            cycle_key: context.cycle_key,
            raw_rx: context.raw_rx,
            raw_tx: context.raw_tx,
            used_rx: context.used_rx,
            used_tx: context.used_tx,
            rx_correction: context.rx_correction,
            tx_correction: context.tx_correction,
        }),
        history_aggregate: HistoryMetricAggregate::default(),
        latency_aggregates: LatencyMetricAggregates::default(),
        latest_samples: Vec::new(),
        latest_received_at: 0,
        message_budget: AgentMessageBudget::default(),
    }
}

fn required_i64_header(req: &Request, name: &str) -> Result<i64> {
    req.headers()
        .get(name)?
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| Error::RustError(format!("missing or invalid {name}")))
}

fn live_context_from_headers(req: &Request) -> Result<AgentLiveContext> {
    Ok(AgentLiveContext {
        report_interval: required_i64_header(req, "X-Live-Report-Interval")?,
        collect_interval: required_i64_header(req, "X-Live-Collect-Interval")?,
        reset_day: required_i64_header(req, "X-Live-Reset-Day")?,
        cycle_key: required_i64_header(req, "X-Live-Cycle-Key")?,
        traffic_reset_day: required_i64_header(req, "X-Live-Traffic-Reset-Day")?,
        traffic_timestamp: required_i64_header(req, "X-Live-Traffic-Timestamp")?,
        raw_rx: required_i64_header(req, "X-Live-Raw-Rx")?,
        raw_tx: required_i64_header(req, "X-Live-Raw-Tx")?,
        used_rx: required_i64_header(req, "X-Live-Used-Rx")?,
        used_tx: required_i64_header(req, "X-Live-Used-Tx")?,
        rx_correction: required_i64_header(req, "X-Live-Rx-Correction")?,
        tx_correction: required_i64_header(req, "X-Live-Tx-Correction")?,
        last_persisted_at: required_i64_header(req, "X-Live-Last-Persisted-At")?,
    })
}

fn dashboard_attachment() -> SocketAttachment {
    SocketAttachment {
        role: "dashboard".to_string(),
        server_id: None,
        hidden: false,
        report_interval: 0,
        collect_interval: 0,
        last_d1_write_at: 0,
        persisted_through: 0,
        d1_flush: None,
        traffic: None,
        history_aggregate: HistoryMetricAggregate::default(),
        latency_aggregates: LatencyMetricAggregates::default(),
        latest_samples: Vec::new(),
        latest_received_at: 0,
        message_budget: AgentMessageBudget::default(),
    }
}

#[durable_object(websocket)]
pub struct LiveHub {
    state: State,
    env: Env,
    active_d1_flushes: RefCell<HashSet<String>>,
}

impl LiveHub {
    async fn broadcast_to_global_dashboard(&self, server_id: &str, payload: &str) -> bool {
        let result = async {
            let namespace = self.env.durable_object("LIVE_HUB")?;
            let stub = namespace.id_from_name(DASHBOARD_HUB_NAME)?.get_stub()?;
            let headers = Headers::new();
            headers.set("X-Server-ID", server_id)?;
            let mut init = RequestInit::new();
            init.with_method(Method::Post)
                .with_headers(headers)
                .with_body(Some(worker::wasm_bindgen::JsValue::from_str(payload)));
            let req = Request::new_with_init("https://live.internal/push", &init)?;
            let mut response = stub.fetch_with_request(req).await?;
            ensure_live_success(&response, "global-push")?;
            response.json::<bool>().await
        }
        .await;
        match result {
            Ok(active) => active,
            Err(error) => {
                console_warn!("global live broadcast failed for {server_id}: {error:?}");
                false
            }
        }
    }

    async fn record_global_alert_point(&self, server_id: &str, point: &HistoryPoint) {
        let result = async {
            let response = post_live_action(
                &self.env,
                "record-alert-sample",
                "record-alert-sample",
                &serde_json::to_string(&AlertRecordRequest {
                    server_id: server_id.to_string(),
                    point: point.clone(),
                })?,
            )
            .await?;
            ensure_live_success(&response, "record-alert-sample")
        }
        .await;
        if let Err(error) = result {
            console_warn!("global alert window write failed for {server_id}: {error:?}");
        }
    }

    fn dashboard_active_for(&self, server_id: &str) -> bool {
        !self.state.get_websockets_with_tag("all").is_empty()
            || !self
                .state
                .get_websockets_with_tag(&format!("server:{server_id}"))
                .is_empty()
    }

    fn hint_agents(&self, server_id: Option<&str>) {
        let sockets = server_id.map_or_else(
            || self.state.get_websockets_with_tag("agents"),
            |server_id| {
                self.state
                    .get_websockets_with_tag(&format!("agent:{server_id}"))
            },
        );
        let now = crate::now();
        for socket in sockets {
            let Ok(Some(attachment)) = socket.deserialize_attachment::<SocketAttachment>() else {
                continue;
            };
            let Some(agent_server_id) = attachment.server_id.as_deref() else {
                continue;
            };
            let realtime_active = !attachment.hidden && self.dashboard_active_for(agent_server_id);
            let Ok(payload) = live_ack_payload(
                now,
                false,
                false,
                attachment.persisted_through,
                next_d1_write_ms(&attachment, now),
                agent_wss_interval_ms(&attachment, realtime_active),
                true,
            ) else {
                continue;
            };
            if socket.send_with_str(&payload).is_err() {
                let _ = socket.close(Some(1011), Some("send failed"));
            }
        }
    }

    fn send_cached_updates(dashboard: &WebSocket, now: i64, updates: &[serde_json::Value]) -> bool {
        let Ok(payload) = serde_json::to_string(&serde_json::json!({
            "type": "batchUpdate",
            "ts": now,
            "cached": true,
            "updates": updates
        })) else {
            return false;
        };
        if dashboard.send_with_str(&payload).is_ok() {
            return true;
        }
        let _ = dashboard.close(Some(1011), Some("send failed"));
        false
    }

    fn replay_latest(&self, dashboard: &WebSocket, server_id: Option<&str>) {
        let sockets = server_id.map_or_else(
            || self.state.get_websockets_with_tag("agents"),
            |server_id| {
                self.state
                    .get_websockets_with_tag(&format!("agent:{server_id}"))
            },
        );
        let now = crate::now();
        let mut updates = Vec::new();
        let mut encoded_bytes = 0_usize;
        for socket in sockets {
            let Some(update) = socket
                .deserialize_attachment::<SocketAttachment>()
                .ok()
                .flatten()
                .and_then(|attachment| cached_update_value(&attachment, now, server_id))
            else {
                continue;
            };
            let Ok(update_bytes) = serde_json::to_vec(&update).map(|encoded| encoded.len()) else {
                continue;
            };
            if !updates.is_empty()
                && (updates.len() >= MAX_CACHED_REPLAY_UPDATES
                    || encoded_bytes.saturating_add(update_bytes) > MAX_CACHED_REPLAY_BYTES)
            {
                if !Self::send_cached_updates(dashboard, now, &updates) {
                    return;
                }
                updates.clear();
                encoded_bytes = 0;
            }
            encoded_bytes = encoded_bytes.saturating_add(update_bytes);
            updates.push(update);
        }
        if !updates.is_empty() {
            Self::send_cached_updates(dashboard, now, &updates);
        }
    }

    async fn record_alert_point(&self, server_id: &str, point: &HistoryPoint) -> Result<()> {
        if server_id.is_empty() || server_id.len() > 80 || server_id.contains('/') {
            return Err(Error::RustError(
                "invalid alert server identity".to_string(),
            ));
        }
        let key = alert_storage_key(server_id);
        let storage = self.state.storage();
        let mut samples = storage
            .get::<Vec<AlertWindowSample>>(&key)
            .await?
            .unwrap_or_default();
        update_alert_window(&mut samples, point);
        storage.put(&key, &samples).await
    }

    async fn evaluate_alerts(
        &self,
        request: &AlertEvaluationRequest,
    ) -> Result<Vec<AlertEvaluationValue>> {
        let enabled = request.rules.iter().any(|rule| rule.enabled);
        if !enabled {
            return Ok(Vec::new());
        }
        let storage = self.state.storage();
        let mut values = Vec::new();
        for server in &request.servers {
            let applicable_rules = request
                .rules
                .iter()
                .filter(|rule| {
                    rule.enabled
                        && (rule.server_ids.is_empty()
                            || rule.server_ids.iter().any(|id| id == &server.id))
                })
                .collect::<Vec<_>>();
            if applicable_rules.is_empty() {
                continue;
            }
            let samples = storage
                .get::<Vec<AlertWindowSample>>(&alert_storage_key(&server.id))
                .await?
                .unwrap_or_default();
            if samples.is_empty() {
                continue;
            }
            for rule in applicable_rules {
                if let Some(row) =
                    evaluate_alert_window(&samples, rule, server, request.current_time)
                {
                    values.push(AlertEvaluationValue {
                        rule_id: rule.id.clone(),
                        row,
                    });
                }
            }
        }
        Ok(values)
    }
}

impl DurableObject for LiveHub {
    fn new(state: State, env: Env) -> Self {
        if let Ok(pair) = WebSocketRequestResponsePair::new("ping", "pong") {
            state.set_websocket_auto_response(&pair);
        }
        Self {
            state,
            env,
            active_d1_flushes: RefCell::new(HashSet::new()),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if req.method() == Method::Post {
            match req.headers().get("X-Live-Action")?.as_deref() {
                Some("disconnect-agents") => {
                    let server_ids = req.json::<Vec<String>>().await?;
                    for server_id in server_ids {
                        if server_id.is_empty() || server_id.len() > 80 || server_id.contains('/') {
                            continue;
                        }
                        for socket in self
                            .state
                            .get_websockets_with_tag(&format!("agent:{server_id}"))
                        {
                            let _ = socket.close(Some(1008), Some("server configuration changed"));
                        }
                    }
                    return Response::empty();
                }
                Some("record-alert-sample") => {
                    let record = req.json::<AlertRecordRequest>().await?;
                    self.record_alert_point(&record.server_id, &record.point)
                        .await?;
                    return Response::empty();
                }
                Some("evaluate-alerts") => {
                    let request = req.json::<AlertEvaluationRequest>().await?;
                    return Response::from_json(&self.evaluate_alerts(&request).await?);
                }
                Some("clear-alert-windows") => {
                    self.state.storage().delete_all().await?;
                    return Response::empty();
                }
                Some("remove-alert-samples") => {
                    let server_ids = req.json::<Vec<String>>().await?;
                    let keys = server_ids
                        .into_iter()
                        .filter(|id| !id.is_empty() && id.len() <= 80 && !id.contains('/'))
                        .map(|id| alert_storage_key(&id))
                        .collect::<Vec<_>>();
                    if !keys.is_empty() {
                        self.state.storage().delete_multiple(keys).await?;
                    }
                    return Response::empty();
                }
                _ => {}
            }
            let payload = req.text().await?;
            if payload.len() > 1024 * 1024 {
                return Response::error("live broadcast payload too large", 413);
            }
            let server_id = req.headers().get("X-Server-ID")?.unwrap_or_default();
            let mut sockets = self.state.get_websockets_with_tag("all");
            if !server_id.is_empty() {
                sockets.extend(
                    self.state
                        .get_websockets_with_tag(&format!("server:{server_id}")),
                );
            }
            let dashboard_active = !sockets.is_empty();
            for socket in sockets {
                if socket.send_with_str(&payload).is_err() {
                    let _ = socket.close(Some(1011), Some("send failed"));
                }
            }
            return Response::from_json(&dashboard_active);
        }

        let is_upgrade = req
            .headers()
            .get("Upgrade")?
            .map(|value| value.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false);
        if !is_upgrade {
            return Response::error("WebSocket upgrade required", 426);
        }

        let pair = WebSocketPair::new()?;
        if let Some(server_id) = req.headers().get("X-Live-Agent-ID")? {
            if server_id.is_empty() || server_id.len() > 80 || server_id.contains('/') {
                return Response::error("Invalid live Agent identity", 400);
            }
            let hidden = req
                .headers()
                .get("X-Live-Agent-Hidden")?
                .is_some_and(|value| value == "1");
            let context = match live_context_from_headers(&req) {
                Ok(context) => context,
                Err(_) => return Response::error("Invalid live Agent context", 400),
            };
            let database = self.env.d1("DB")?;
            let Some(config) = crate::db::agent_config(&database, &server_id).await? else {
                return Response::error("Agent server not found", 404);
            };
            let config_payload = agent_config_payload(crate::now(), &config)?;
            let agent_tag = format!("agent:{server_id}");
            for existing in self.state.get_websockets_with_tag(&agent_tag) {
                let _ = existing.close(Some(1008), Some("replaced by a newer Agent connection"));
            }
            pair.server.serialize_attachment(agent_attachment(
                server_id.clone(),
                hidden,
                context,
            ))?;
            self.state
                .accept_websocket_with_tags(&pair.server, &["agents", agent_tag.as_str()]);
            pair.server.send_with_str(&config_payload)?;
            return Response::from_websocket(pair.client);
        }
        let server_id = req
            .url()?
            .query_pairs()
            .find_map(|(key, value)| (key == "server_id").then(|| value.to_string()))
            .filter(|value| !value.is_empty() && value.len() <= 80 && !value.contains('/'));
        let tag = server_id
            .as_ref()
            .map_or_else(|| "all".to_string(), |id| format!("server:{id}"));
        pair.server.serialize_attachment(dashboard_attachment())?;
        self.state.accept_websocket_with_tags(&pair.server, &[&tag]);
        self.replay_latest(&pair.server, server_id.as_deref());
        self.hint_agents(server_id.as_deref());
        Response::from_websocket(pair.client)
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let Some(mut attachment) = ws.deserialize_attachment::<SocketAttachment>()? else {
            return Ok(());
        };
        if attachment.role != "agent" {
            return ws.close(Some(1008), Some("dashboard messages are not accepted"));
        }
        let WebSocketIncomingMessage::String(message) = message else {
            return ws.close(
                Some(1003),
                Some("binary live metric payload is not accepted"),
            );
        };
        if message.len() > 1024 * 1024 {
            return ws.close(Some(1009), Some("live metric payload too large"));
        }
        let Ok(batch) = serde_json::from_str::<AgentLiveBatch>(&message) else {
            return ws.close(Some(1007), Some("invalid live metric payload"));
        };
        if batch.message_type != "update"
            || batch.samples.is_empty()
            || batch.samples.len() > MAX_LIVE_SAMPLES
            || !crate::db::valid_report_batch_id(&batch.batch_id)
        {
            return ws.close(Some(1007), Some("invalid live metric batch"));
        }
        let received_at = crate::now();
        if !consume_agent_message_budget(
            &mut attachment.message_budget,
            received_at,
            message.len(),
            batch.samples.len(),
        ) {
            return ws.close(Some(1008), Some("live metric message rate exceeded"));
        }
        let mut latency_result_count = 0usize;
        for report in &batch.samples {
            if report.timestamp <= 0
                || (report.timestamp - received_at).abs() > 7200
                || crate::validate_report(report).is_some()
            {
                return ws.close(Some(1007), Some("invalid live metric sample"));
            }
            latency_result_count =
                latency_result_count.saturating_add(report.latency_results.len());
        }
        if latency_result_count > MAX_LIVE_LATENCY_RESULTS {
            return ws.close(Some(1009), Some("too many live latency results"));
        }
        let Some(server_id) = attachment.server_id.clone() else {
            return ws.close(Some(1008), Some("missing server identity"));
        };
        let unpersisted_reports =
            crate::db::reports_after(&batch.samples, attachment.persisted_through);
        let mut traffic = attachment
            .traffic
            .take()
            .ok_or_else(|| Error::RustError("missing live traffic state".to_string()))?;
        let previous_traffic_timestamp = traffic.timestamp;
        let fresh_reports = crate::db::reports_after(&batch.samples, previous_traffic_timestamp);
        let latency_results = fresh_reports
            .iter()
            .flat_map(|report| report.latency_results.iter().cloned())
            .collect::<Vec<_>>();
        let (payload, mut cached_samples) =
            batch_update_parts_at(&server_id, &fresh_reports, &mut traffic, received_at)?;
        trim_cached_live_samples(&mut cached_samples);
        let traffic_state = crate::db::TrafficCounterState {
            cycle_key: traffic.cycle_key,
            reset_day: traffic.state_reset_day,
            timestamp: traffic.timestamp,
            raw_rx: traffic.raw_rx,
            raw_tx: traffic.raw_tx,
            used_rx: traffic.used_rx,
            used_tx: traffic.used_tx,
        };
        attachment.traffic = Some(traffic);
        if !fresh_reports.is_empty() {
            attachment.latest_samples = cached_samples;
            attachment.latest_received_at = received_at;
            attachment.history_aggregate.extend(&fresh_reports);
            attachment
                .latency_aggregates
                .extend(&latency_results, received_at);
        }

        let due_for_d1 = received_at.saturating_sub(attachment.last_d1_write_at)
            >= HISTORY_WRITE_INTERVAL_SECONDS;
        let flush_active = self.active_d1_flushes.borrow().contains(&server_id);
        let mut flush = if !unpersisted_reports.is_empty()
            && (due_for_d1 || attachment.d1_flush.is_some())
            && !flush_active
        {
            let database = self.env.d1("DB")?;
            begin_d1_flush(&mut attachment).map(|snapshot| (database, snapshot))
        } else {
            None
        };
        if flush.is_some() {
            if !trim_socket_attachment(&mut attachment) {
                return ws.close(Some(1011), Some("live state exceeds attachment limit"));
            }
            ws.serialize_attachment(&attachment)?;
        }

        let mut global_dashboard_active = false;
        if !fresh_reports.is_empty() && !attachment.hidden {
            let mut sockets = self.state.get_websockets_with_tag("all");
            sockets.extend(
                self.state
                    .get_websockets_with_tag(&format!("server:{server_id}")),
            );
            for dashboard in sockets {
                if dashboard.send_with_str(&payload).is_err() {
                    let _ = dashboard.close(Some(1011), Some("send failed"));
                }
            }
            global_dashboard_active = self
                .broadcast_to_global_dashboard(&server_id, &payload)
                .await;
        }

        let mut persisted = false;
        let mut persistence_error = false;
        let mut attachment_serialized = false;
        if let Some((database, snapshot)) = flush.take() {
            self.active_d1_flushes
                .borrow_mut()
                .insert(server_id.clone());
            let history_point = snapshot
                .history
                .point()
                .ok_or_else(|| Error::RustError("missing live history aggregate".to_string()))?;
            let sample_count = snapshot.history.sample_count();
            let metrics_result = crate::db::save_reports_with_history(
                &database,
                &server_id,
                crate::db::ReportBatchRef {
                    reports: &unpersisted_reports,
                    id: &batch.batch_id,
                },
                &history_point,
                sample_count,
                &traffic_state,
                &snapshot.latency,
            )
            .await;
            let (write_succeeded, newly_written) = match metrics_result {
                Err(error) => {
                    console_warn!("live D1 write failed for {server_id}: {error:?}");
                    persistence_error = true;
                    (false, false)
                }
                Ok(inserted) => (true, inserted),
            };
            self.active_d1_flushes.borrow_mut().remove(&server_id);

            let Some(mut current_attachment) = ws.deserialize_attachment::<SocketAttachment>()?
            else {
                return ws.close(Some(1011), Some("missing live state after D1 write"));
            };
            if current_attachment.role != "agent"
                || current_attachment.server_id.as_deref() != Some(server_id.as_str())
            {
                return ws.close(Some(1011), Some("invalid live state after D1 write"));
            }
            finish_d1_flush(
                &mut current_attachment,
                write_succeeded,
                received_at,
                traffic_state.timestamp,
            );
            if !trim_socket_attachment(&mut current_attachment) {
                return ws.close(Some(1011), Some("live state exceeds attachment limit"));
            }
            ws.serialize_attachment(&current_attachment)?;
            attachment = current_attachment;
            attachment_serialized = true;
            persisted = write_succeeded;

            if newly_written {
                match crate::db::has_enabled_alert_rules(&database).await {
                    Ok(true) => {
                        self.record_global_alert_point(&server_id, &history_point)
                            .await;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        console_warn!("enabled alert rule check failed for {server_id}: {error:?}");
                    }
                }
            }
        }
        let realtime_active = !attachment.hidden
            && (global_dashboard_active || self.dashboard_active_for(&server_id));
        let next_wss_ms = agent_wss_interval_ms(&attachment, realtime_active);
        let next_d1_ms = if persisted {
            HISTORY_WRITE_INTERVAL_SECONDS * 1000
        } else {
            next_d1_write_ms(&attachment, received_at)
        };
        let ack = live_ack_payload(
            received_at,
            persisted,
            persistence_error,
            attachment.persisted_through,
            next_d1_ms,
            next_wss_ms,
            false,
        )?;
        if !attachment_serialized {
            if !trim_socket_attachment(&mut attachment) {
                return ws.close(Some(1011), Some("live state exceeds attachment limit"));
            }
            ws.serialize_attachment(attachment)?;
        }
        ws.send_with_str(&ack)?;
        Ok(())
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        code: usize,
        reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        match outbound_close_code(code) {
            Some(code) => ws.close(Some(code), Some(reason)),
            None => ws.close(None, None::<&str>),
        }
    }

    async fn websocket_error(&self, ws: WebSocket, _error: Error) -> Result<()> {
        ws.close(Some(1011), Some("socket error"))
    }
}

fn outbound_close_code(code: usize) -> Option<u16> {
    let code = u16::try_from(code).ok()?;
    ((1000..=4999).contains(&code) && !matches!(code, 1004..=1006 | 1015)).then_some(code)
}

async fn post_live_action(env: &Env, path: &str, action: &str, body: &str) -> Result<Response> {
    let namespace = env.durable_object("LIVE_HUB")?;
    let stub = namespace.id_from_name(DASHBOARD_HUB_NAME)?.get_stub()?;
    let headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    headers.set("X-Live-Action", action)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(body)));
    let req = Request::new_with_init(&format!("https://live.internal/{path}"), &init)?;
    stub.fetch_with_request(req).await
}

fn ensure_live_success(response: &Response, action: &str) -> Result<()> {
    let status = response.status_code();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(Error::RustError(format!(
            "live action {action} returned HTTP {status}"
        )))
    }
}

pub async fn evaluate_resource_alerts(
    env: &Env,
    rules: &[AlertRuleView],
    servers: &[ServerView],
    current_time: i64,
) -> Result<Vec<AlertEvaluationValue>> {
    let request = AlertEvaluationRequest {
        current_time,
        rules: rules.to_vec(),
        servers: servers
            .iter()
            .filter(|server| server.hidden == 0)
            .map(|server| AlertEvaluationServer {
                id: server.id.clone(),
                name: server.name.clone(),
                report_interval: server.report_interval,
            })
            .collect(),
    };
    let mut response = post_live_action(
        env,
        "evaluate-alerts",
        "evaluate-alerts",
        &serde_json::to_string(&request)?,
    )
    .await?;
    ensure_live_success(&response, "evaluate-alerts")?;
    response.json::<Vec<AlertEvaluationValue>>().await
}

pub async fn clear_alert_windows(env: &Env) -> Result<()> {
    let response =
        post_live_action(env, "clear-alert-windows", "clear-alert-windows", "{}").await?;
    ensure_live_success(&response, "clear-alert-windows")?;
    Ok(())
}

pub async fn remove_alert_samples(env: &Env, server_ids: &[String]) -> Result<()> {
    if server_ids.is_empty() {
        return Ok(());
    }
    let response = post_live_action(
        env,
        "remove-alert-samples",
        "remove-alert-samples",
        &serde_json::to_string(server_ids)?,
    )
    .await?;
    ensure_live_success(&response, "remove-alert-samples")?;
    Ok(())
}

fn copied_request_headers(req: &Request) -> Result<Headers> {
    let headers = Headers::new();
    for (name, value) in req.headers().entries() {
        headers.append(&name, &value)?;
    }
    Ok(headers)
}

fn request_with_headers(req: &Request, headers: Headers) -> Result<Request> {
    let mut init = RequestInit::new();
    init.with_method(req.method()).with_headers(headers);
    Request::new_with_init(req.url()?.as_str(), &init)
}

pub async fn disconnect_agents(env: &Env, server_ids: &[String]) -> Result<()> {
    if server_ids.is_empty() {
        return Ok(());
    }
    let namespace = env.durable_object("LIVE_HUB")?;
    for server_id in server_ids {
        let stub = namespace
            .id_from_name(&server_hub_name(server_id))?
            .get_stub()?;
        let headers = Headers::new();
        headers.set("Content-Type", "application/json")?;
        headers.set("X-Live-Action", "disconnect-agents")?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(worker::wasm_bindgen::JsValue::from_str(
                &serde_json::to_string(std::slice::from_ref(server_id))?,
            )));
        let req = Request::new_with_init("https://live.internal/disconnect-agents", &init)?;
        let response = stub.fetch_with_request(req).await?;
        ensure_live_success(&response, "disconnect-agents")?;
    }
    Ok(())
}

pub async fn upgrade(req: Request, env: &Env) -> Result<Response> {
    let namespace = env.durable_object("LIVE_HUB")?;
    let server_id = req
        .url()?
        .query_pairs()
        .find_map(|(key, value)| (key == "server_id").then(|| value.to_string()))
        .filter(|value| !value.is_empty() && value.len() <= 80 && !value.contains('/'));
    let hub_name = server_id
        .as_deref()
        .map(server_hub_name)
        .unwrap_or_else(|| DASHBOARD_HUB_NAME.to_string());
    let stub = namespace.id_from_name(&hub_name)?.get_stub()?;
    let headers = copied_request_headers(&req)?;
    headers.delete("X-Live-Agent-ID")?;
    headers.delete("X-Live-Agent-Hidden")?;
    for header in [
        "X-Live-Report-Interval",
        "X-Live-Collect-Interval",
        "X-Live-Reset-Day",
        "X-Live-Cycle-Key",
        "X-Live-Traffic-Reset-Day",
        "X-Live-Traffic-Timestamp",
        "X-Live-Raw-Rx",
        "X-Live-Raw-Tx",
        "X-Live-Used-Rx",
        "X-Live-Used-Tx",
        "X-Live-Rx-Correction",
        "X-Live-Tx-Correction",
        "X-Live-Last-Persisted-At",
    ] {
        headers.delete(header)?;
    }
    let req = request_with_headers(&req, headers)?;
    stub.fetch_with_request(req).await
}

pub async fn upgrade_agent(
    req: Request,
    env: &Env,
    server_id: &str,
    hidden: bool,
    context: AgentLiveContext,
) -> Result<Response> {
    let namespace = env.durable_object("LIVE_HUB")?;
    let stub = namespace
        .id_from_name(&server_hub_name(server_id))?
        .get_stub()?;
    let headers = copied_request_headers(&req)?;
    headers.set("X-Live-Agent-ID", server_id)?;
    headers.set("X-Live-Agent-Hidden", if hidden { "1" } else { "0" })?;
    headers.set(
        "X-Live-Report-Interval",
        &context.report_interval.to_string(),
    )?;
    headers.set(
        "X-Live-Collect-Interval",
        &context.collect_interval.to_string(),
    )?;
    headers.set("X-Live-Reset-Day", &context.reset_day.to_string())?;
    headers.set("X-Live-Cycle-Key", &context.cycle_key.to_string())?;
    headers.set(
        "X-Live-Traffic-Reset-Day",
        &context.traffic_reset_day.to_string(),
    )?;
    headers.set(
        "X-Live-Traffic-Timestamp",
        &context.traffic_timestamp.to_string(),
    )?;
    headers.set("X-Live-Raw-Rx", &context.raw_rx.to_string())?;
    headers.set("X-Live-Raw-Tx", &context.raw_tx.to_string())?;
    headers.set("X-Live-Used-Rx", &context.used_rx.to_string())?;
    headers.set("X-Live-Used-Tx", &context.used_tx.to_string())?;
    headers.set("X-Live-Rx-Correction", &context.rx_correction.to_string())?;
    headers.set("X-Live-Tx-Correction", &context.tx_correction.to_string())?;
    headers.set(
        "X-Live-Last-Persisted-At",
        &context.last_persisted_at.to_string(),
    )?;
    headers.delete("Authorization")?;
    let req = request_with_headers(&req, headers)?;
    stub.fetch_with_request(req).await
}

#[cfg(test)]
mod tests {
    use super::{
        agent_wss_interval_ms, batch_update_parts_at, begin_d1_flush, cached_update_value,
        consume_agent_message_budget, finish_d1_flush, live_ack_payload, next_d1_write_ms,
        outbound_close_code, trim_cached_live_samples, trim_socket_attachment, update_alert_window,
        AgentMessageBudget, AlertEvaluationServer, AlertWindowSample, CachedLiveSample,
        SocketAttachment, TrafficAttachment, ALERT_WINDOW_SECONDS, CACHED_LIVE_TTL_SECONDS,
        MAX_AGENT_BYTES_PER_WINDOW, MAX_AGENT_MESSAGES_PER_WINDOW, MAX_AGENT_SAMPLES_PER_WINDOW,
        MAX_CACHED_LIVE_BYTES, MAX_CACHED_LIVE_SAMPLES, MAX_SERIALIZED_ATTACHMENT_BYTES,
    };
    use crate::db::HistoryMetricAggregate;
    use crate::latency::LatencyMetricAggregates;
    use crate::models::{AgentLatencyResult, AgentReport, AlertRuleView, HistoryPoint};

    fn alert_point(timestamp: i64, cpu: f64, net_in: f64) -> HistoryPoint {
        HistoryPoint {
            timestamp,
            cpu,
            mem_used: 50,
            mem_total: 100,
            disk_used: 25,
            disk_total: 100,
            net_in,
            net_out: net_in / 2.0,
            ..HistoryPoint::default()
        }
    }

    #[test]
    fn filters_reserved_and_invalid_websocket_close_codes() {
        assert_eq!(outbound_close_code(1000), Some(1000));
        assert_eq!(outbound_close_code(1011), Some(1011));
        assert_eq!(outbound_close_code(4000), Some(4000));
        assert_eq!(outbound_close_code(999), None);
        assert_eq!(outbound_close_code(1004), None);
        assert_eq!(outbound_close_code(1005), None);
        assert_eq!(outbound_close_code(1006), None);
        assert_eq!(outbound_close_code(1015), None);
        assert_eq!(outbound_close_code(5000), None);
    }

    #[test]
    fn bounds_agent_messages_per_connection_window() {
        let mut budget = AgentMessageBudget::default();
        assert!(consume_agent_message_budget(
            &mut budget,
            100,
            MAX_AGENT_BYTES_PER_WINDOW as usize,
            MAX_AGENT_SAMPLES_PER_WINDOW as usize,
        ));
        assert!(!consume_agent_message_budget(&mut budget, 100, 1, 0));

        let mut message_budget = AgentMessageBudget::default();
        for _ in 0..MAX_AGENT_MESSAGES_PER_WINDOW {
            assert!(consume_agent_message_budget(&mut message_budget, 100, 1, 1));
        }
        assert!(!consume_agent_message_budget(
            &mut message_budget,
            100,
            1,
            1
        ));
        assert!(consume_agent_message_budget(&mut message_budget, 160, 1, 1));
    }

    #[test]
    fn live_payload_contains_cycle_adjusted_traffic_and_latency() {
        let report = AgentReport {
            timestamp: 1_234,
            cpu: 12.5,
            load1: 0.5,
            load5: 0.4,
            load15: 0.3,
            mem_used: 1,
            mem_total: 2,
            swap_used: 0,
            swap_total: 0,
            disk_used: 3,
            disk_total: 4,
            net_in: 100.0,
            net_out: 200.0,
            net_rx_total: 300,
            net_tx_total: 400,
            uptime: 500,
            processes: 10,
            tcp_connections: 2,
            udp_connections: 1,
            cpu_cores: 2,
            cpu_model: "CPU".to_string(),
            os: "Linux".to_string(),
            kernel: "6.0".to_string(),
            arch: "x86_64".to_string(),
            virtualization: "kvm".to_string(),
            gpu_usage: 0.0,
            gpu_model: String::new(),
            agent_version: "test".to_string(),
            disk_read_bps: 0.0,
            disk_write_bps: 0.0,
            disk_read_iops: 0.0,
            disk_write_iops: 0.0,
            disk_await_ms: 0.0,
            disk_utilization: 0.0,
            disks: vec![],
            gpus: vec![],
            latency_results: vec![],
        };
        let mut traffic = TrafficAttachment {
            reset_day: 1,
            state_reset_day: 1,
            timestamp: 1_230,
            cycle_key: crate::db::traffic_cycle_key(1_234, 1),
            raw_rx: 100,
            raw_tx: 100,
            used_rx: 500,
            used_tx: 700,
            rx_correction: 10,
            tx_correction: 20,
        };
        let payload =
            batch_update_parts_at("node-a", std::slice::from_ref(&report), &mut traffic, 1_235)
                .unwrap()
                .0;
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["type"], "batchUpdate");
        assert_eq!(payload["updates"][0]["serverId"], "node-a");
        assert_eq!(payload["updates"][0]["samples"][0]["ts"], 1_234);
        assert_eq!(payload["updates"][0]["samples"][0]["data"]["cpu"], 12.5);
        assert_eq!(
            payload["updates"][0]["samples"][0]["data"]["net_rx_total"],
            710
        );
        assert!(payload["updates"][0]["samples"][0]["data"]
            .get("timestamp")
            .is_none());
        let attachment = SocketAttachment {
            role: "agent".to_string(),
            server_id: Some("node-a".to_string()),
            hidden: false,
            report_interval: 60,
            collect_interval: 1,
            last_d1_write_at: 1_200,
            persisted_through: 1_190,
            d1_flush: None,
            traffic: None,
            history_aggregate: HistoryMetricAggregate::default(),
            latency_aggregates: LatencyMetricAggregates::default(),
            latest_samples: Vec::new(),
            latest_received_at: 0,
            message_budget: AgentMessageBudget::default(),
        };
        assert_eq!(agent_wss_interval_ms(&attachment, true), 4_000);
        assert_eq!(agent_wss_interval_ms(&attachment, false), 60_000);
        assert_eq!(next_d1_write_ms(&attachment, 1_235), 85_000);
        let hint: serde_json::Value = serde_json::from_str(
            &live_ack_payload(1_235, false, false, 1_190, 25_000, 4_000, true).unwrap(),
        )
        .unwrap();
        assert_eq!(hint["realtimeHint"], true);
        assert_eq!(hint["persistedThroughTs"], 1_190);
        assert_eq!(hint["nextWssReportAfterMs"], 4_000);

        let mut corrected_down = traffic;
        corrected_down.rx_correction = -800;
        let mut corrected_report = report.clone();
        corrected_report.timestamp = 1_235;
        corrected_report.net_rx_total = 301;
        let payload =
            batch_update_parts_at("node-a", &[corrected_report], &mut corrected_down, 1_236)
                .unwrap()
                .0;
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            payload["updates"][0]["samples"][0]["data"]["net_rx_total"],
            0
        );
    }

    #[test]
    fn bounds_and_expires_cached_live_samples() {
        let mut samples = (0..(MAX_CACHED_LIVE_SAMPLES + 8))
            .map(|index| CachedLiveSample {
                ts: index as i64,
                data: serde_json::json!({ "cpu": index }),
            })
            .collect::<Vec<_>>();
        trim_cached_live_samples(&mut samples);
        assert_eq!(samples.len(), MAX_CACHED_LIVE_SAMPLES);
        assert_eq!(samples[0].ts, 8);

        let mut attachment = SocketAttachment {
            role: "agent".to_string(),
            server_id: Some("node-a".to_string()),
            hidden: false,
            report_interval: 60,
            collect_interval: 1,
            last_d1_write_at: 0,
            persisted_through: 0,
            d1_flush: None,
            traffic: None,
            history_aggregate: HistoryMetricAggregate::default(),
            latency_aggregates: LatencyMetricAggregates::default(),
            latest_samples: samples,
            latest_received_at: 100,
            message_budget: AgentMessageBudget::default(),
        };
        let update = cached_update_value(&attachment, 102, Some("node-a")).unwrap();
        assert_eq!(update["serverId"], "node-a");
        assert_eq!(update["reportAgeMs"], 2_000);
        assert_eq!(
            update["samples"].as_array().unwrap().len(),
            MAX_CACHED_LIVE_SAMPLES
        );
        assert!(cached_update_value(&attachment, 102, Some("node-b")).is_none());
        assert!(cached_update_value(&attachment, 101 + CACHED_LIVE_TTL_SECONDS, None).is_none());

        let mut oversized = vec![CachedLiveSample {
            ts: 1,
            data: serde_json::json!({ "value": "x".repeat(MAX_CACHED_LIVE_BYTES) }),
        }];
        trim_cached_live_samples(&mut oversized);
        assert!(oversized.is_empty());

        let latency_results = (0..128)
            .map(|index| AgentLatencyResult {
                task_id: format!("00000000-0000-0000-0000-{index:012}"),
                timestamp: 100,
                latency_ms: index as f64,
                packet_loss: 0.0,
            })
            .collect::<Vec<_>>();
        attachment.latency_aggregates.extend(&latency_results, 100);
        assert!(trim_socket_attachment(&mut attachment));
        assert!(serde_json::to_vec(&attachment).unwrap().len() <= MAX_SERIALIZED_ATTACHMENT_BYTES);
    }

    #[test]
    fn keeps_samples_received_during_d1_flush_for_next_write() {
        let first = AgentReport {
            timestamp: 120,
            cpu: 10.0,
            net_in: 100.0,
            latency_results: vec![AgentLatencyResult {
                task_id: "task-a".to_string(),
                timestamp: 120,
                latency_ms: 10.0,
                packet_loss: 0.0,
            }],
            ..AgentReport::default()
        };
        let second = AgentReport {
            timestamp: 121,
            cpu: 20.0,
            net_in: 500.0,
            latency_results: vec![AgentLatencyResult {
                task_id: "task-a".to_string(),
                timestamp: 121,
                latency_ms: 20.0,
                packet_loss: 10.0,
            }],
            ..AgentReport::default()
        };
        let mut attachment = SocketAttachment {
            role: "agent".to_string(),
            server_id: Some("node-a".to_string()),
            hidden: false,
            report_interval: 60,
            collect_interval: 1,
            last_d1_write_at: 60,
            persisted_through: 60,
            d1_flush: None,
            traffic: None,
            history_aggregate: HistoryMetricAggregate::default(),
            latency_aggregates: LatencyMetricAggregates::default(),
            latest_samples: Vec::new(),
            latest_received_at: 0,
            message_budget: AgentMessageBudget::default(),
        };
        attachment
            .history_aggregate
            .extend(std::slice::from_ref(&first));
        attachment
            .latency_aggregates
            .extend(&first.latency_results, first.timestamp);

        let mut successful = attachment.clone();
        let successful_snapshot = begin_d1_flush(&mut successful).unwrap();
        assert!(successful.d1_flush.is_some());
        assert!(successful.history_aggregate.point().is_none());
        assert!(begin_d1_flush(&mut successful).is_some());
        let serialized = serde_json::to_vec(&successful).unwrap();
        successful = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(
            begin_d1_flush(&mut successful)
                .unwrap()
                .history
                .point()
                .unwrap()
                .cpu,
            10.0
        );
        successful
            .history_aggregate
            .extend(std::slice::from_ref(&second));
        successful
            .latency_aggregates
            .extend(&second.latency_results, second.timestamp);
        assert_eq!(successful_snapshot.history.point().unwrap().cpu, 10.0);
        finish_d1_flush(&mut successful, true, 180, 120);

        let pending = successful.history_aggregate.point().unwrap();
        assert_eq!(pending.cpu, 20.0);
        assert_eq!(pending.net_in, 500.0);
        assert_eq!(successful.last_d1_write_at, 180);
        assert_eq!(successful.persisted_through, 120);
        assert!(successful.d1_flush.is_none());
        let latency = serde_json::to_value(&successful.latency_aggregates).unwrap();
        assert_eq!(latency["v"]["task-a"][0].as_u64(), Some(1));
        assert_eq!(latency["v"]["task-a"][4].as_i64(), Some(121));

        let failed_snapshot = begin_d1_flush(&mut attachment).unwrap();
        attachment
            .history_aggregate
            .extend(std::slice::from_ref(&second));
        attachment
            .latency_aggregates
            .extend(&second.latency_results, second.timestamp);
        assert_eq!(failed_snapshot.history.point().unwrap().cpu, 10.0);
        finish_d1_flush(&mut attachment, false, 180, 120);

        let retried = attachment.history_aggregate.point().unwrap();
        assert_eq!(retried.cpu, 15.0);
        assert_eq!(retried.net_in, 500.0);
        assert_eq!(attachment.last_d1_write_at, 60);
        assert_eq!(attachment.persisted_through, 60);
        assert!(attachment.d1_flush.is_none());
        let latency = serde_json::to_value(&attachment.latency_aggregates).unwrap();
        assert_eq!(latency["v"]["task-a"][0].as_u64(), Some(2));
        assert_eq!(latency["v"]["task-a"][4].as_i64(), Some(121));
    }

    #[test]
    fn merges_and_evaluates_persistent_alert_windows() {
        let mut samples = Vec::<AlertWindowSample>::new();
        update_alert_window(&mut samples, &alert_point(121, 10.0, 1_048_576.0));
        update_alert_window(&mut samples, &alert_point(149, 30.0, 3_145_728.0));
        update_alert_window(&mut samples, &alert_point(181, 40.0, 2_097_152.0));
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].metric("cpu"), Some(20.0));
        assert_eq!(samples[0].metric("memory"), Some(50.0));
        assert_eq!(samples[0].metric("disk"), Some(25.0));
        assert_eq!(samples[0].metric("net_in"), Some(3.0));

        let server = AlertEvaluationServer {
            id: "node-a".to_string(),
            name: "Node A".to_string(),
            report_interval: 60,
        };
        let mut rule = AlertRuleView {
            id: "rule-a".to_string(),
            name: "CPU".to_string(),
            metric: "cpu".to_string(),
            threshold: 25.0,
            duration_minutes: 2,
            aggregation: "average".to_string(),
            enabled: true,
            server_ids: Vec::new(),
        };
        let average = super::evaluate_alert_window(&samples, &rule, &server, 240).unwrap();
        assert_eq!(average.value, 30.0);
        assert_eq!(average.sample_count, 2);
        assert_eq!(average.first_timestamp, 120);
        assert_eq!(average.last_timestamp, 180);

        rule.aggregation = "continuous".to_string();
        let continuous = super::evaluate_alert_window(&samples, &rule, &server, 240).unwrap();
        assert_eq!(continuous.value, 20.0);

        for minute in 0..1_500 {
            update_alert_window(&mut samples, &alert_point(minute * 60, minute as f64, 0.0));
        }
        assert!(samples.len() <= super::MAX_ALERT_WINDOW_SAMPLES);
        assert!(samples.last().unwrap().0 - samples.first().unwrap().0 <= ALERT_WINDOW_SECONDS);
    }
}
