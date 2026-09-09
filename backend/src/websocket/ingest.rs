use crate::db::{
    Database, now,
    queries::{self, AgentIdentity, PersistResult, TrafficState},
};
use crate::models::AgentReport;
use anyhow::{Result, ensure};
use nodeflare_telemetry::{MAX_BATCH_BYTES, MAX_SAMPLES};

pub struct AgentBuffer {
    pending: Vec<AgentReport>,
    pending_bytes: usize,
    received_through: i64,
    persisted_through: i64,
    traffic: TrafficState,
}

pub struct IngestResult {
    pub latest: Option<AgentReport>,
    pub samples: Vec<AgentReport>,
    pub acknowledgement: Option<PersistResult>,
}

impl AgentBuffer {
    pub async fn new(db: &Database, server_id: &str) -> Result<Self> {
        let traffic = queries::agent_traffic_state(db, server_id).await?;
        Ok(Self {
            pending: Vec::new(),
            pending_bytes: 0,
            received_through: traffic.timestamp,
            persisted_through: traffic.timestamp,
            traffic,
        })
    }

    pub async fn receive(
        &mut self,
        db: &Database,
        identity: &AgentIdentity,
        remote_ip: &str,
        mut reports: Vec<AgentReport>,
        persist: bool,
    ) -> Result<IngestResult> {
        let current = now();
        ensure!(reports.len() <= MAX_SAMPLES, "too many telemetry samples");
        ensure!(
            reports.iter().all(|report| report.timestamp > 0
                && report.timestamp >= current - 7200
                && report.timestamp <= current + 300
                && queries::valid_agent_report(report, current)),
            "invalid telemetry sample"
        );
        reports.sort_by_key(|report| report.timestamp);
        reports.dedup_by_key(|report| report.timestamp);
        reports.retain(|report| report.timestamp > self.received_through);
        let bytes = reports.iter().try_fold(0_usize, |size, report| {
            serde_json::to_vec(report).map(|encoded| size.saturating_add(encoded.len()))
        })?;
        ensure!(bytes <= MAX_BATCH_BYTES, "telemetry batch is too large");
        let mut acknowledgement = None;
        if !self.pending.is_empty()
            && (self.pending.len() + reports.len() > MAX_SAMPLES
                || self.pending_bytes.saturating_add(bytes) > MAX_BATCH_BYTES)
        {
            acknowledgement = Some(self.flush(db, identity, remote_ip).await?);
        }
        let mut samples = Vec::with_capacity(reports.len());
        let mut latency = Vec::new();
        for report in &reports {
            let mut live = report.clone();
            queries::apply_traffic(&mut live, &mut self.traffic, identity);
            self.received_through = live.timestamp;
            latency.extend(live.latency_results.iter().cloned());
            samples.push(live);
        }
        let mut latest = samples.last().cloned();
        if let Some(latest) = &mut latest {
            latest.latency_results = latency;
        }
        self.pending.extend(reports);
        self.pending_bytes = self.pending_bytes.saturating_add(bytes);
        if persist || self.pending_bytes >= MAX_BATCH_BYTES || self.pending.len() >= MAX_SAMPLES {
            acknowledgement = Some(self.flush(db, identity, remote_ip).await?);
        }
        Ok(IngestResult {
            latest,
            samples,
            acknowledgement,
        })
    }

    async fn flush(
        &mut self,
        db: &Database,
        identity: &AgentIdentity,
        remote_ip: &str,
    ) -> Result<PersistResult> {
        let Some(first) = self.pending.first() else {
            return Ok(PersistResult {
                reports: Vec::new(),
                persisted: true,
                persisted_through: self.persisted_through,
                next_persist_after_ms: identity.report_interval.clamp(15, 3600) as u64 * 1000,
            });
        };
        let batch_id = format!("{}:{}", first.timestamp, self.received_through);
        let result =
            queries::save_agent_batch(db, identity, &batch_id, &self.pending, remote_ip).await?;
        self.persisted_through = result.persisted_through;
        self.pending.clear();
        self.pending_bytes = 0;
        Ok(result)
    }
}
