use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashSet;
use std::io::{self, Read};
pub mod probe;

pub const AGENT_PROTOCOL_VERSION: &str = "1";
pub const AGENT_PROTOCOL_HEADER: &str = "x-nodeflare-agent-protocol";
pub const AGENT_CAPABILITIES_HEADER: &str = "x-nodeflare-agent-capabilities";
pub const REQUIRED_AGENT_CAPABILITIES: [&str; 3] = ["metrics-v1", "config-v1", "task-ack-v1"];
pub const REMOTE_EXEC_CAPABILITY: &str = "remote-exec-v1";
pub const MIN_UPLOAD_INTERVAL: u64 = 3;
pub const MAX_BATCH_BYTES: usize = 768 * 1024;
pub const MAX_DECODED_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SAMPLES: usize = 720;
pub const AGENT_REPORT_MAX_AGE_SECONDS: i64 = 2 * 60 * 60;
const AGENT_REPORT_MAX_BYTES: usize = 256 * 1024;
const AGENT_REPORT_MAX_DISKS: usize = 128;
const AGENT_REPORT_MAX_GPUS: usize = 32;
pub const AGENT_REPORT_MAX_LATENCY_RESULTS: usize = 2048;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Report {
    pub timestamp: i64,
    #[serde(deserialize_with = "metric_float")]
    pub cpu: f64,
    #[serde(deserialize_with = "metric_float")]
    pub load1: f64,
    #[serde(deserialize_with = "metric_float")]
    pub load5: f64,
    #[serde(deserialize_with = "metric_float")]
    pub load15: f64,
    pub mem_used: i64,
    pub mem_total: i64,
    pub swap_used: i64,
    pub swap_total: i64,
    pub disk_used: i64,
    pub disk_total: i64,
    #[serde(deserialize_with = "metric_float")]
    pub net_in: f64,
    #[serde(deserialize_with = "metric_float")]
    pub net_out: f64,
    pub net_rx_total: i64,
    pub net_tx_total: i64,
    pub uptime: i64,
    pub processes: i64,
    pub tcp_connections: i64,
    pub udp_connections: i64,
    pub cpu_cores: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cpu_model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub os: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kernel: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub arch: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub virtualization: String,
    #[serde(deserialize_with = "metric_float")]
    pub gpu_usage: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gpu_model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent_version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ip_v4: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ip_v6: String,
    #[serde(deserialize_with = "metric_float")]
    pub disk_read_bps: f64,
    #[serde(deserialize_with = "metric_float")]
    pub disk_write_bps: f64,
    #[serde(deserialize_with = "metric_float")]
    pub disk_read_iops: f64,
    #[serde(deserialize_with = "metric_float")]
    pub disk_write_iops: f64,
    #[serde(deserialize_with = "metric_float")]
    pub disk_await_ms: f64,
    #[serde(deserialize_with = "metric_float")]
    pub disk_utilization: f64,
    pub disks: Vec<DiskMetric>,
    pub gpus: Vec<GpuMetric>,
    pub latency_results: Vec<LatencyResult>,
}

impl Report {
    fn round_for_upload(&mut self) {
        for value in [
            &mut self.net_in,
            &mut self.net_out,
            &mut self.disk_read_bps,
            &mut self.disk_write_bps,
        ] {
            *value = value.round();
        }
        for value in [
            &mut self.cpu,
            &mut self.load1,
            &mut self.load5,
            &mut self.load15,
            &mut self.gpu_usage,
            &mut self.disk_read_iops,
            &mut self.disk_write_iops,
            &mut self.disk_await_ms,
            &mut self.disk_utilization,
        ] {
            *value = (*value * 100.0).round() / 100.0;
        }
        for disk in &mut self.disks {
            disk.read_bps = disk.read_bps.round();
            disk.write_bps = disk.write_bps.round();
            for value in [
                &mut disk.read_iops,
                &mut disk.write_iops,
                &mut disk.await_ms,
                &mut disk.utilization,
            ] {
                *value = (*value * 100.0).round() / 100.0;
            }
        }
        for gpu in &mut self.gpus {
            gpu.usage = gpu.usage.map(|value| (value * 100.0).round() / 100.0);
        }
        for latency in &mut self.latency_results {
            latency.latency_ms = (latency.latency_ms * 100.0).round() / 100.0;
            latency.packet_loss = (latency.packet_loss * 100.0).round() / 100.0;
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiskMetric {
    pub name: String,
    pub mount_point: String,
    pub used: i64,
    pub total: i64,
    #[serde(deserialize_with = "metric_float")]
    pub read_bps: f64,
    #[serde(deserialize_with = "metric_float")]
    pub write_bps: f64,
    #[serde(deserialize_with = "metric_float")]
    pub read_iops: f64,
    #[serde(deserialize_with = "metric_float")]
    pub write_iops: f64,
    #[serde(deserialize_with = "metric_float")]
    pub await_ms: f64,
    #[serde(deserialize_with = "metric_float")]
    pub utilization: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GpuMetric {
    pub model: String,
    pub usage: Option<f64>,
    pub memory_used: i64,
    pub memory_total: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LatencyResult {
    pub task_id: String,
    pub timestamp: i64,
    #[serde(deserialize_with = "metric_float")]
    pub latency_ms: f64,
    #[serde(deserialize_with = "metric_float")]
    pub packet_loss: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Info {
    cpu_model: String,
    os: String,
    kernel: String,
    arch: String,
    virtualization: String,
    gpu_model: String,
    agent_version: String,
    ip_v4: String,
    ip_v6: String,
}

impl Info {
    fn take(report: &mut Report) -> Self {
        Self {
            cpu_model: std::mem::take(&mut report.cpu_model),
            os: std::mem::take(&mut report.os),
            kernel: std::mem::take(&mut report.kernel),
            arch: std::mem::take(&mut report.arch),
            virtualization: std::mem::take(&mut report.virtualization),
            gpu_model: std::mem::take(&mut report.gpu_model),
            agent_version: std::mem::take(&mut report.agent_version),
            ip_v4: std::mem::take(&mut report.ip_v4),
            ip_v6: std::mem::take(&mut report.ip_v6),
        }
    }

    fn apply(&self, report: &mut Report) {
        report.cpu_model.clone_from(&self.cpu_model);
        report.os.clone_from(&self.os);
        report.kernel.clone_from(&self.kernel);
        report.arch.clone_from(&self.arch);
        report.virtualization.clone_from(&self.virtualization);
        report.gpu_model.clone_from(&self.gpu_model);
        report.agent_version.clone_from(&self.agent_version);
        report.ip_v4.clone_from(&self.ip_v4);
        report.ip_v6.clone_from(&self.ip_v6);
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub metrics: Report,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<Info>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub samples: Vec<Sample>,
    pub persist: bool,
}

impl Update {
    pub fn from_reports(reports: Vec<Report>, persist: bool, info: &mut Option<Info>) -> Self {
        let samples = reports
            .into_iter()
            .map(|mut metrics| {
                let current = Info::take(&mut metrics);
                metrics.round_for_upload();
                let changed = info.as_ref() != Some(&current);
                if changed {
                    *info = Some(current.clone());
                }
                Sample {
                    metrics,
                    info: changed.then_some(current),
                }
            })
            .collect();
        Self { samples, persist }
    }

    pub fn into_reports(self, info: &mut Option<Info>) -> io::Result<Vec<Report>> {
        if self.samples.len() > MAX_SAMPLES {
            return Err(io::Error::other("too many telemetry samples"));
        }
        self.samples
            .into_iter()
            .map(|mut sample| {
                if let Some(current) = sample.info {
                    *info = Some(current);
                }
                info.as_ref()
                    .ok_or_else(|| io::Error::other("missing telemetry info"))?
                    .apply(&mut sample.metrics);
                Ok(sample.metrics)
            })
            .collect()
    }
}

pub fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    serde_json::to_writer(&mut encoder, value)?;
    encoder.finish()
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.len() > MAX_DECODED_BYTES {
        return Err(io::Error::other("telemetry frame is too large"));
    }
    let mut decoded = Vec::new();
    GzDecoder::new(bytes)
        .take(MAX_DECODED_BYTES as u64 + 1)
        .read_to_end(&mut decoded)?;
    if decoded.len() > MAX_DECODED_BYTES {
        return Err(io::Error::other("expanded telemetry frame is too large"));
    }
    serde_json::from_slice(&decoded).map_err(io::Error::other)
}

// JSON encodes non-finite metrics as null. Keep them invalid at the sample
// boundary rather than failing the entire telemetry envelope.
// A missing field still uses the struct's serde(default) value (0.0); an explicit
// null must remain NaN so validation rejects that sample instead of inventing 0.
fn metric_float<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
}

pub fn valid_report(report: &Report, current: i64) -> bool {
    fn finite_between(value: f64, minimum: f64, maximum: f64) -> bool {
        value.is_finite() && (minimum..=maximum).contains(&value)
    }

    fn valid_capacity(used: i64, total: i64) -> bool {
        used >= 0 && total >= 0 && (total == 0 || used <= total)
    }

    if report.timestamp <= 0
        || report.timestamp < current.saturating_sub(AGENT_REPORT_MAX_AGE_SECONDS)
        || report.timestamp > current.saturating_add(300)
        || !finite_between(report.cpu, 0.0, 100.0)
        || !finite_between(report.load1, 0.0, 1_000_000.0)
        || !finite_between(report.load5, 0.0, 1_000_000.0)
        || !finite_between(report.load15, 0.0, 1_000_000.0)
        || !valid_capacity(report.mem_used, report.mem_total)
        || !valid_capacity(report.swap_used, report.swap_total)
        || !valid_capacity(report.disk_used, report.disk_total)
        || !finite_between(report.net_in, 0.0, 1.0e18)
        || !finite_between(report.net_out, 0.0, 1.0e18)
        || report.net_rx_total < 0
        || report.net_tx_total < 0
        || report.uptime < 0
        || report.processes < 0
        || report.tcp_connections < 0
        || report.udp_connections < 0
        || !(0..=1_000_000).contains(&report.cpu_cores)
        || !finite_between(report.gpu_usage, 0.0, 100.0)
        || !finite_between(report.disk_read_bps, 0.0, 1.0e18)
        || !finite_between(report.disk_write_bps, 0.0, 1.0e18)
        || !finite_between(report.disk_read_iops, 0.0, 1.0e12)
        || !finite_between(report.disk_write_iops, 0.0, 1.0e12)
        || !finite_between(report.disk_await_ms, 0.0, 86_400_000.0)
        || !finite_between(report.disk_utilization, 0.0, 100.0)
        || report.cpu_model.len() > 512
        || report.os.len() > 128
        || report.kernel.len() > 256
        || report.arch.len() > 64
        || report.virtualization.len() > 128
        || report.gpu_model.len() > 512
        || report.agent_version.len() > 64
        || report
            .ip_v4
            .parse::<std::net::Ipv4Addr>()
            .is_err_and(|_| !report.ip_v4.is_empty())
        || report
            .ip_v6
            .parse::<std::net::Ipv6Addr>()
            .is_err_and(|_| !report.ip_v6.is_empty())
        || report.disks.len() > AGENT_REPORT_MAX_DISKS
        || report.gpus.len() > AGENT_REPORT_MAX_GPUS
        || report.latency_results.len() > AGENT_REPORT_MAX_LATENCY_RESULTS
    {
        return false;
    }

    if report.disks.iter().any(|disk| {
        disk.name.len() > 128
            || disk.mount_point.len() > 512
            || !valid_capacity(disk.used, disk.total)
            || !finite_between(disk.read_bps, 0.0, 1.0e18)
            || !finite_between(disk.write_bps, 0.0, 1.0e18)
            || !finite_between(disk.read_iops, 0.0, 1.0e12)
            || !finite_between(disk.write_iops, 0.0, 1.0e12)
            || !finite_between(disk.await_ms, 0.0, 86_400_000.0)
            || !finite_between(disk.utilization, 0.0, 100.0)
    }) || report.gpus.iter().any(|gpu| {
        gpu.model.len() > 512
            || gpu
                .usage
                .is_some_and(|usage| !finite_between(usage, 0.0, 100.0))
            || !valid_capacity(gpu.memory_used, gpu.memory_total)
    }) {
        return false;
    }

    let mut latency_ids = HashSet::with_capacity(report.latency_results.len());
    if report.latency_results.iter().any(|latency| {
        latency.task_id.is_empty()
            || latency.task_id.len() > 80
            || !latency_ids.insert(&latency.task_id)
            || latency.timestamp < current.saturating_sub(AGENT_REPORT_MAX_AGE_SECONDS)
            || latency.timestamp > current.saturating_add(300)
            || !finite_between(latency.latency_ms, -1.0, 86_400_000.0)
            || !finite_between(latency.packet_loss, 0.0, 100.0)
    }) {
        return false;
    }

    serde_json::to_vec(report).is_ok_and(|encoded| encoded.len() <= AGENT_REPORT_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonfinite_fields_decode_for_individual_rejection() {
        for field in ["cpu", "load1", "disk_read_bps"] {
            let mut value = serde_json::to_value(Report {
                timestamp: 100,
                ..Report::default()
            })
            .unwrap();
            value[field] = serde_json::Value::Null;
            let report: Report = serde_json::from_value(value).unwrap();
            assert!(!valid_report(&report, 100), "{field}");
        }
        assert!(valid_report(
            &Report {
                timestamp: 100,
                ..Report::default()
            },
            100
        ));
        assert!(!valid_report(
            &Report {
                timestamp: 0,
                ..Report::default()
            },
            100
        ));
    }

    #[test]
    fn trims_float_noise_without_changing_byte_counters() {
        let report = Report {
            cpu: 100.0 / 3.0,
            net_in: 12345.6789,
            net_rx_total: i64::MAX,
            disks: vec![DiskMetric {
                read_bps: 125.6789,
                await_ms: 1.234567,
                ..DiskMetric::default()
            }],
            latency_results: vec![LatencyResult {
                latency_ms: 28.4444,
                packet_loss: 100.0 / 3.0,
                ..LatencyResult::default()
            }],
            ..Report::default()
        };
        let encoded = encode(&Update::from_reports(vec![report], false, &mut None)).unwrap();
        let reports = decode::<Update>(&encoded)
            .unwrap()
            .into_reports(&mut None)
            .unwrap();
        assert_eq!(reports[0].cpu, 33.33);
        assert_eq!(reports[0].net_in, 12346.0);
        assert_eq!(reports[0].net_rx_total, i64::MAX);
        assert_eq!(reports[0].disks[0].read_bps, 126.0);
        assert_eq!(reports[0].disks[0].await_ms, 1.23);
        assert_eq!(reports[0].latency_results[0].latency_ms, 28.44);
    }

    #[test]
    fn info_is_sent_on_connect_and_changes_including_during_replay() {
        let mut encoder = None;
        let mut decoder = None;
        for (name, changed) in [
            ("first", true),
            ("first", false),
            ("second", true),
            ("first", true),
        ] {
            let update = Update::from_reports(
                vec![Report {
                    cpu_model: name.into(),
                    cpu: 23.5,
                    ..Report::default()
                }],
                false,
                &mut encoder,
            );
            assert_eq!(update.samples[0].info.is_some(), changed);
            assert!(update.samples[0].metrics.cpu_model.is_empty());
            let bytes = encode(&update).unwrap();
            let decoded: Update = decode(&bytes).unwrap();
            let reports = decoded.into_reports(&mut decoder).unwrap();
            assert_eq!(reports[0].cpu_model, name);
            assert_eq!(reports[0].cpu, 23.5);
        }
        let update = Update::from_reports(vec![Report::default()], true, &mut None);
        assert!(update.samples[0].info.is_some());
    }

    #[test]
    fn rejects_missing_info_corruption_and_decompression_bombs() {
        let update = Update {
            samples: vec![Sample {
                metrics: Report::default(),
                info: None,
            }],
            persist: false,
        };
        assert!(update.into_reports(&mut None).is_err());
        assert!(decode::<Update>(b"not gzip").is_err());
        let bytes = encode(&"x".repeat(MAX_DECODED_BYTES + 1)).unwrap();
        assert!(decode::<String>(&bytes).is_err());
    }

    #[test]
    fn compresses_batches_and_supports_empty_commit_messages() {
        let update = Update::from_reports(vec![Report::default(); 20], false, &mut None);
        assert!(encode(&update).unwrap().len() < serde_json::to_vec(&update).unwrap().len() / 4);
        let update = Update::from_reports(Vec::new(), true, &mut None);
        let decoded: Update = decode(&encode(&update).unwrap()).unwrap();
        assert!(decoded.persist);
        assert!(decoded.samples.is_empty());
    }
}
