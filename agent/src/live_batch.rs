use super::Report;

pub(crate) const MAX_LIVE_BATCH_BYTES: usize = nodeflare_telemetry::MAX_BATCH_BYTES;

/// Collects the longest prefix of `queue` that fits one live batch, cloning only
/// the reports that are actually selected and serializing each candidate once
/// (the size check needs the encoded length).
pub(crate) fn batch_from<'a>(queue: impl Iterator<Item = &'a Report>) -> Vec<Report> {
    let mut encoded_bytes = 192_usize;
    let mut batch = Vec::new();
    for report in queue {
        if batch.len() >= super::LIVE_QUEUE_CAPACITY {
            break;
        }
        let Ok(report_bytes) = serde_json::to_vec(report) else {
            break;
        };
        let next_bytes = encoded_bytes
            .saturating_add(report_bytes.len())
            .saturating_add(256);
        if next_bytes > MAX_LIVE_BATCH_BYTES {
            break;
        }
        encoded_bytes = next_bytes;
        batch.push(report.clone());
        if encoded_bytes >= MAX_LIVE_BATCH_BYTES {
            break;
        }
    }
    batch
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{MAX_LIVE_BATCH_BYTES, batch_from};
    use crate::Report;

    #[test]
    fn bounds_batches_by_serialized_size() {
        let report = Report {
            cpu_model: "x".repeat(128 * 1024),
            ..Report::default()
        };
        let queue = std::iter::repeat_n(report, 10).collect::<VecDeque<_>>();
        let count = batch_from(queue.iter()).len();
        assert!((1..10).contains(&count));
        let payload = crate::live_update_payload(
            queue.iter().take(count).cloned().collect::<Vec<_>>(),
            true,
            &mut None,
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
        let reports = crate::live_batch_after(&queue, -1);
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
            let batch = crate::live_batch_after(&queue, persisted);
            if batch.is_empty() {
                break;
            }
            let message = crate::live_update_payload(batch.clone(), true, &mut None).unwrap();
            assert!(message.len() <= MAX_LIVE_BATCH_BYTES);
            persisted = batch.last().unwrap().timestamp;
            received.extend(batch.into_iter().map(|report| report.timestamp));
        }
        assert_eq!(received, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn caps_batch_count_and_always_sends_the_persistence_flag() {
        let queue = (0..1000)
            .map(|timestamp| Report {
                timestamp,
                ..Report::default()
            })
            .collect::<VecDeque<_>>();
        assert!(batch_from(queue.iter()).len() <= crate::LIVE_QUEUE_CAPACITY);
        let reports = crate::live_batch_after(&queue, 100);
        assert_eq!(reports.first().unwrap().timestamp, 101);
        for persist in [true, false] {
            let message = crate::live_update_payload(reports.clone(), persist, &mut None).unwrap();
            let value: serde_json::Value = nodeflare_telemetry::decode(&message).unwrap();
            assert_eq!(value["persist"], persist);
            assert_eq!(value.as_object().unwrap().len(), 2);
        }
    }
}
