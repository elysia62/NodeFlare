use std::collections::VecDeque;

use super::Report;

pub(crate) const MAX_LIVE_BATCH_BYTES: usize = 768 * 1024;

pub(crate) fn batch_len(queue: &VecDeque<Report>) -> usize {
    // Allow room for the JSON envelope, commas, and future small protocol fields.
    let mut encoded_bytes = 64_usize;
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

    use super::{batch_len, MAX_LIVE_BATCH_BYTES};
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
        let payload =
            crate::live_update_payload(&queue.iter().take(count).cloned().collect::<Vec<_>>())
                .unwrap();
        assert!(payload.len() <= MAX_LIVE_BATCH_BYTES);
    }
}
