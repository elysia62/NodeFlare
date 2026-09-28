-- Keep the weights of latency summaries when merging them into coarser tiers.
-- NULL metadata denotes a legacy/raw sample and is resolved by readers.
ALTER TABLE latency_results ADD COLUMN sample_count INTEGER NOT NULL DEFAULT 1 CHECK(sample_count > 0);
ALTER TABLE latency_results ADD COLUMN latency_sample_count INTEGER CHECK(latency_sample_count >= 0 AND latency_sample_count <= sample_count);
ALTER TABLE latency_results ADD COLUMN last_timestamp INTEGER;
