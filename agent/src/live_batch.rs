use std::collections::VecDeque;

use super::Report;

pub(crate) const MAX_LIVE_BATCH_BYTES: usize = 768 * 1024;

pub(crate) fn batch_len(queue: &VecDeque<Report>) -> usize {
    let mut encoded_bytes = 192_usize;
    let mut count = 0_usize;
    for report in queue {
        let Ok(report_bytes) = serde_json::to_vec(report) else {
            break;
        };
        let next_bytes = encoded_bytes
            .saturating_add(report_bytes.len())
            .saturating_add(usize::from(count > 0));
        if next_bytes > MAX_LIVE_BATCH_BYTES {
            break;
        }
        encoded_bytes = next_bytes;
        count += 1;
        if encoded_bytes >= MAX_LIVE_BATCH_BYTES {
            break;
        }
    }
    count.min(super::LIVE_QUEUE_CAPACITY)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{MAX_LIVE_BATCH_BYTES, batch_len};
    use crate::Report;

    #[test]
    fn bounds_batches_by_serialized_size() {
        let report = Report {
            cpu_model: "x".repeat(128 * 1024),
            ..Report::default()
        };
        let queue = std::iter::repeat_n(report, 10).collect::<VecDeque<_>>();
        let count = batch_len(&queue);
        assert!((1..10).contains(&count));
        let payload = crate::live_update_payload(
            &queue.iter().take(count).cloned().collect::<Vec<_>>(),
            Some(true),
        )
        .unwrap();
        assert!(payload.len() <= MAX_LIVE_BATCH_BYTES);
    }

    #[test]
    fn persistence_batch_keeps_the_oldest_unconfirmed_samples() {
        let queue = (0..10)
            .map(|timestamp| Report {
                timestamp,
                cpu_model: "x".repeat(128 * 1024),
                ..Report::default()
            })
            .collect::<VecDeque<_>>();
        let reports = crate::live_persistence_batch(&queue, -1);
        assert!(!reports.is_empty());
        assert!(reports.len() < queue.len());
        assert_eq!(reports.first().unwrap().timestamp, 0);
        assert!(
            reports
                .windows(2)
                .all(|pair| pair[0].timestamp < pair[1].timestamp)
        );
        let mut persisted = -1;
        let mut received = Vec::new();
        loop {
            let batch = crate::live_persistence_batch(&queue, persisted);
            if batch.is_empty() {
                break;
            }
            let message = crate::live_update_payload(&batch, Some(true)).unwrap();
            assert!(message.len() <= MAX_LIVE_BATCH_BYTES);
            persisted = batch.last().unwrap().timestamp;
            received.extend(batch.into_iter().map(|report| report.timestamp));
        }
        assert_eq!(received, (0..10).collect::<Vec<_>>());
        let legacy = crate::legacy_persistence_batch(&queue, -1);
        assert_eq!(legacy.last().unwrap().timestamp, 9);
        assert!(legacy.len() < queue.len());
    }

    #[test]
    fn caps_batch_count_and_preserves_legacy_payload_shape() {
        let queue = (0..1000)
            .map(|timestamp| Report {
                timestamp,
                ..Report::default()
            })
            .collect::<VecDeque<_>>();
        assert!(batch_len(&queue) <= crate::LIVE_QUEUE_CAPACITY);
        let reports = crate::live_persistence_batch(&queue, 100);
        assert_eq!(reports.first().unwrap().timestamp, 101);
        let message = crate::live_update_payload(&reports, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&message).unwrap();
        assert!(value.get("persist").is_none());
        assert_eq!(value.as_object().unwrap().len(), 3);
    }
}
