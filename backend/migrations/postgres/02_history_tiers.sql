ALTER TABLE latency_results ADD COLUMN sample_count BIGINT NOT NULL DEFAULT 1 CHECK(sample_count > 0);
ALTER TABLE latency_results ADD COLUMN latency_sample_count BIGINT CHECK(latency_sample_count >= 0 AND latency_sample_count <= sample_count);
ALTER TABLE latency_results ADD COLUMN last_timestamp BIGINT;
