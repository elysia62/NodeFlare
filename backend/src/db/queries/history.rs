use super::super::{Database, now};
use crate::models::{AgentReport, HistoryPoint};
use anyhow::Result;
use sqlx::Row;
use std::collections::BTreeMap;

pub const HISTORY_AGGREGATE_COLUMNS: [&str; 14] = [
    "sample_count",
    "first_timestamp",
    "last_timestamp",
    "cpu_min",
    "cpu_max",
    "mem_used_max",
    "memory_avg",
    "memory_min",
    "disk_avg",
    "disk_min",
    "net_in_avg",
    "net_in_min",
    "net_out_avg",
    "net_out_min",
];

#[derive(Debug, Clone)]
pub(crate) struct HistoryAggregate {
    timestamp: i64,
    first_timestamp: i64,
    latest: AgentReport,
    count: f64,
    cpu_sum: f64,
    cpu_min: f64,
    cpu_max: f64,
    load1_sum: f64,
    load5_sum: f64,
    load15_sum: f64,
    mem_sum: f64,
    mem_max: i64,
    swap_sum: f64,
    gpu_sum: f64,
    net_in_max: f64,
    net_out_max: f64,
    disk_read_bps_max: f64,
    disk_write_bps_max: f64,
    disk_read_iops_max: f64,
    disk_write_iops_max: f64,
    disk_await_max: f64,
    disk_utilization_max: f64,
    disk_used_max: i64,
    processes_max: i64,
    tcp_connections_max: i64,
    udp_connections_max: i64,
    memory_sum: f64,
    memory_min: f64,
    disk_sum: f64,
    disk_min: f64,
    net_in_sum: f64,
    net_in_min: f64,
    net_out_sum: f64,
    net_out_min: f64,
}

impl HistoryAggregate {
    fn new(report: &AgentReport, timestamp: i64) -> Self {
        Self {
            timestamp,
            first_timestamp: report.timestamp,
            latest: report.clone(),
            count: 0.0,
            cpu_sum: 0.0,
            cpu_min: f64::INFINITY,
            cpu_max: f64::NEG_INFINITY,
            load1_sum: 0.0,
            load5_sum: 0.0,
            load15_sum: 0.0,
            mem_sum: 0.0,
            mem_max: 0,
            swap_sum: 0.0,
            gpu_sum: 0.0,
            net_in_max: 0.0,
            net_out_max: 0.0,
            disk_read_bps_max: 0.0,
            disk_write_bps_max: 0.0,
            disk_read_iops_max: 0.0,
            disk_write_iops_max: 0.0,
            disk_await_max: 0.0,
            disk_utilization_max: 0.0,
            disk_used_max: 0,
            processes_max: 0,
            tcp_connections_max: 0,
            udp_connections_max: 0,
            memory_sum: 0.0,
            memory_min: f64::INFINITY,
            disk_sum: 0.0,
            disk_min: f64::INFINITY,
            net_in_sum: 0.0,
            net_in_min: f64::INFINITY,
            net_out_sum: 0.0,
            net_out_min: f64::INFINITY,
        }
    }

    fn add(&mut self, report: &AgentReport) {
        let mem_total = self.latest.mem_total.max(report.mem_total);
        let swap_total = self.latest.swap_total.max(report.swap_total);
        let disk_total = self.latest.disk_total.max(report.disk_total);
        self.latest = report.clone();
        self.latest.mem_total = mem_total;
        self.latest.swap_total = swap_total;
        self.latest.disk_total = disk_total;
        self.count += 1.0;
        self.cpu_sum += report.cpu;
        self.cpu_min = self.cpu_min.min(report.cpu);
        self.cpu_max = self.cpu_max.max(report.cpu);
        self.load1_sum += report.load1;
        self.load5_sum += report.load5;
        self.load15_sum += report.load15;
        self.mem_sum += report.mem_used as f64;
        self.mem_max = self.mem_max.max(report.mem_used);
        self.swap_sum += report.swap_used as f64;
        self.gpu_sum += report.gpu_usage;
        self.net_in_max = self.net_in_max.max(report.net_in);
        self.net_out_max = self.net_out_max.max(report.net_out);
        self.disk_read_bps_max = self.disk_read_bps_max.max(report.disk_read_bps);
        self.disk_write_bps_max = self.disk_write_bps_max.max(report.disk_write_bps);
        self.disk_read_iops_max = self.disk_read_iops_max.max(report.disk_read_iops);
        self.disk_write_iops_max = self.disk_write_iops_max.max(report.disk_write_iops);
        self.disk_await_max = self.disk_await_max.max(report.disk_await_ms);
        self.disk_utilization_max = self.disk_utilization_max.max(report.disk_utilization);
        self.disk_used_max = self.disk_used_max.max(report.disk_used);
        self.processes_max = self.processes_max.max(report.processes);
        self.tcp_connections_max = self.tcp_connections_max.max(report.tcp_connections);
        self.udp_connections_max = self.udp_connections_max.max(report.udp_connections);
        let memory = capacity_percentage(report.mem_used, report.mem_total);
        let disk = capacity_percentage(report.disk_used, report.disk_total);
        self.memory_sum += memory;
        self.memory_min = self.memory_min.min(memory);
        self.disk_sum += disk;
        self.disk_min = self.disk_min.min(disk);
        self.net_in_sum += report.net_in;
        self.net_in_min = self.net_in_min.min(report.net_in);
        self.net_out_sum += report.net_out;
        self.net_out_min = self.net_out_min.min(report.net_out);
    }

    fn finish(mut self) -> HistoryRow {
        let count = self.count.max(1.0);
        let last_timestamp = self.latest.timestamp;
        self.latest.timestamp = self.timestamp;
        self.latest.cpu = self.cpu_sum / count;
        self.latest.load1 = self.load1_sum / count;
        self.latest.load5 = self.load5_sum / count;
        self.latest.load15 = self.load15_sum / count;
        self.latest.mem_used = (self.mem_sum / count).round() as i64;
        self.latest.swap_used = (self.swap_sum / count).round() as i64;
        self.latest.net_in = self.net_in_max;
        self.latest.net_out = self.net_out_max;
        self.latest.gpu_usage = self.gpu_sum / count;
        self.latest.disk_read_bps = self.disk_read_bps_max;
        self.latest.disk_write_bps = self.disk_write_bps_max;
        self.latest.disk_read_iops = self.disk_read_iops_max;
        self.latest.disk_write_iops = self.disk_write_iops_max;
        self.latest.disk_await_ms = self.disk_await_max;
        self.latest.disk_utilization = self.disk_utilization_max;
        self.latest.disk_used = self.disk_used_max;
        self.latest.processes = self.processes_max;
        self.latest.tcp_connections = self.tcp_connections_max;
        self.latest.udp_connections = self.udp_connections_max;
        HistoryRow {
            report: self.latest,
            sample_count: self.count as i64,
            first_timestamp: self.first_timestamp,
            last_timestamp,
            cpu_min: self.cpu_min,
            cpu_max: self.cpu_max,
            mem_used_max: self.mem_max,
            memory_avg: self.memory_sum / count,
            memory_min: self.memory_min,
            disk_avg: self.disk_sum / count,
            disk_min: self.disk_min,
            net_in_avg: self.net_in_sum / count,
            net_in_min: self.net_in_min,
            net_out_avg: self.net_out_sum / count,
            net_out_min: self.net_out_min,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryRow {
    pub(crate) report: AgentReport,
    pub(crate) sample_count: i64,
    pub(crate) first_timestamp: i64,
    pub(crate) last_timestamp: i64,
    pub(crate) cpu_min: f64,
    pub(crate) cpu_max: f64,
    pub(crate) mem_used_max: i64,
    pub(crate) memory_avg: f64,
    pub(crate) memory_min: f64,
    pub(crate) disk_avg: f64,
    pub(crate) disk_min: f64,
    pub(crate) net_in_avg: f64,
    pub(crate) net_in_min: f64,
    pub(crate) net_out_avg: f64,
    pub(crate) net_out_min: f64,
}
pub(crate) fn capacity_percentage(used: i64, total: i64) -> f64 {
    if total > 0 {
        used as f64 * 100.0 / total as f64
    } else {
        0.0
    }
}
pub(crate) fn aggregate_history(reports: &[AgentReport], interval: i64) -> Vec<HistoryRow> {
    let interval = interval.clamp(15, 3600);
    let mut buckets = BTreeMap::<i64, HistoryAggregate>::new();
    for report in reports {
        let timestamp = report.timestamp.div_euclid(interval) * interval;
        buckets
            .entry(timestamp)
            .or_insert_with(|| HistoryAggregate::new(report, timestamp))
            .add(report);
    }
    buckets
        .into_values()
        .map(HistoryAggregate::finish)
        .collect()
}

pub async fn history(db: &Database, server_id: &str, hours: i64) -> Result<Vec<HistoryPoint>> {
    let hours = hours.clamp(1, 24 * 365);
    let bucket = (hours * 3600 / 720).max(1);
    let since = now() - hours * 3600;
    let rows = sqlx::query(db.sql(
        "SELECT bucket_timestamp, SUM(cpu * sample_count) / SUM(sample_count) AS cpu, \
         MIN(cpu_min) AS cpu_min, MAX(cpu_max) AS cpu_max, \
         CAST(SUM(sample_count) AS BIGINT) AS sample_count, \
         SUM(load1 * sample_count) / SUM(sample_count) AS load1, \
         SUM(load5 * sample_count) / SUM(sample_count) AS load5, \
         SUM(load15 * sample_count) / SUM(sample_count) AS load15, \
         SUM(CAST(mem_used AS DOUBLE PRECISION) * sample_count) / SUM(sample_count) AS mem_used, \
         MAX(mem_used_max) AS mem_used_max, \
         MAX(mem_total) AS mem_total, \
         SUM(CAST(swap_used AS DOUBLE PRECISION) * sample_count) / SUM(sample_count) AS swap_used, \
         MAX(swap_total) AS swap_total, \
         MAX(disk_used) AS disk_used, MAX(disk_total) AS disk_total, MAX(net_in) AS net_in, \
         MAX(net_out) AS net_out, MAX(net_rx_total) AS net_rx_total, \
         MAX(net_tx_total) AS net_tx_total, MAX(processes) AS processes, \
         MAX(tcp_connections) AS tcp_connections, MAX(udp_connections) AS udp_connections, \
         SUM(gpu_usage * sample_count) / SUM(sample_count) AS gpu_usage, MAX(disk_read_bps) AS disk_read_bps, \
         MAX(disk_write_bps) AS disk_write_bps, MAX(disk_read_iops) AS disk_read_iops, \
         MAX(disk_write_iops) AS disk_write_iops, MAX(disk_await_ms) AS disk_await_ms, \
         MAX(disk_utilization) AS disk_utilization FROM ( \
           SELECT (timestamp / ?) * ? AS bucket_timestamp, metric_history.* \
           FROM metric_history WHERE server_id=? \
             AND timestamp>=? AND last_timestamp>=? \
         ) samples GROUP BY bucket_timestamp ORDER BY bucket_timestamp",
    ))
    .bind(bucket)
    .bind(bucket)
    .bind(server_id)
    .bind(since.saturating_sub(3600))
    .bind(since)
    .fetch_all(db.pool())
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(HistoryPoint {
                timestamp: row.try_get("bucket_timestamp")?,
                cpu: row.try_get::<f64, _>("cpu")?,
                cpu_min: row.try_get("cpu_min")?,
                cpu_max: row.try_get("cpu_max")?,
                sample_count: row.try_get("sample_count")?,
                load1: row.try_get::<f64, _>("load1")?,
                load5: row.try_get::<f64, _>("load5")?,
                load15: row.try_get::<f64, _>("load15")?,
                mem_used: row.try_get::<f64, _>("mem_used")?.round() as i64,
                mem_used_max: row.try_get("mem_used_max")?,
                mem_total: row.try_get("mem_total")?,
                swap_used: row.try_get::<f64, _>("swap_used")?.round() as i64,
                swap_total: row.try_get("swap_total")?,
                disk_used: row.try_get("disk_used")?,
                disk_total: row.try_get("disk_total")?,
                net_in: row.try_get("net_in")?,
                net_out: row.try_get("net_out")?,
                net_rx_total: row.try_get("net_rx_total")?,
                net_tx_total: row.try_get("net_tx_total")?,
                processes: row.try_get("processes")?,
                tcp_connections: row.try_get("tcp_connections")?,
                udp_connections: row.try_get("udp_connections")?,
                gpu_usage: row.try_get("gpu_usage")?,
                disk_read_bps: row.try_get("disk_read_bps")?,
                disk_write_bps: row.try_get("disk_write_bps")?,
                disk_read_iops: row.try_get("disk_read_iops")?,
                disk_write_iops: row.try_get("disk_write_iops")?,
                disk_await_ms: row.try_get("disk_await_ms")?,
                disk_utilization: row.try_get("disk_utilization")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}
