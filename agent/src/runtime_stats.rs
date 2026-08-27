use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct RuntimeStats {
    latency_queue_rejected: AtomicU64,
    live_queue_dropped: AtomicU64,
    live_connect_failures: AtomicU64,
    live_persistence_failures: AtomicU64,
    live_batches: AtomicU64,
    live_samples: AtomicU64,
    persisted_samples_pruned: AtomicU64,
    collection_count: AtomicU64,
    collection_total_micros: AtomicU64,
    collection_max_micros: AtomicU64,
}

impl RuntimeStats {
    pub(crate) fn latency_queue_rejected(&self) {
        self.latency_queue_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn live_queue_dropped(&self, count: usize) {
        self.live_queue_dropped
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    pub(crate) fn live_connect_failed(&self) {
        self.live_connect_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn live_persistence_failed(&self) {
        self.live_persistence_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn live_batch_sent(&self, count: usize) {
        self.live_batches.fetch_add(1, Ordering::Relaxed);
        self.live_samples.fetch_add(count as u64, Ordering::Relaxed);
    }

    pub(crate) fn persisted_samples_pruned(&self, count: usize) {
        self.persisted_samples_pruned
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    pub(crate) fn collection_finished(&self, elapsed: Duration) {
        let micros = elapsed.as_micros().min(u64::MAX as u128) as u64;
        self.collection_count.fetch_add(1, Ordering::Relaxed);
        self.collection_total_micros
            .fetch_add(micros, Ordering::Relaxed);
        self.collection_max_micros
            .fetch_max(micros, Ordering::Relaxed);
    }

    pub(crate) fn log_and_reset(&self) {
        let collections = self.collection_count.swap(0, Ordering::Relaxed);
        let total_micros = self.collection_total_micros.swap(0, Ordering::Relaxed);
        let max_micros = self.collection_max_micros.swap(0, Ordering::Relaxed);
        let average_ms = if collections == 0 {
            0.0
        } else {
            total_micros as f64 / collections as f64 / 1000.0
        };
        eprintln!(
            "{}",
            serde_json::json!({
                "level": "info",
                "message": "agent runtime stats",
                "collections": collections,
                "collectionAverageMs": average_ms,
                "collectionMaxMs": max_micros as f64 / 1000.0,
                "latencyQueueRejected": self.latency_queue_rejected.swap(0, Ordering::Relaxed),
                "liveQueueDropped": self.live_queue_dropped.swap(0, Ordering::Relaxed),
                "liveConnectFailures": self.live_connect_failures.swap(0, Ordering::Relaxed),
                "livePersistenceFailures": self.live_persistence_failures.swap(0, Ordering::Relaxed),
                "liveBatches": self.live_batches.swap(0, Ordering::Relaxed),
                "liveSamples": self.live_samples.swap(0, Ordering::Relaxed),
                "persistedSamplesPruned": self.persisted_samples_pruned.swap(0, Ordering::Relaxed),
            })
        );
    }
}
