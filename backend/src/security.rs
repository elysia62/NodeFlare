use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct AttemptState {
    failures: u32,
    window_started: Instant,
    blocked_until: Option<Instant>,
    last_seen: Instant,
}

pub struct AttemptLimiter {
    entries: Mutex<HashMap<String, AttemptState>>,
    maximum_failures: u32,
    window: Duration,
    block_for: Duration,
    retention: Duration,
}

pub struct IntervalLimiter {
    entries: Mutex<HashMap<String, Instant>>,
    minimum_interval: Duration,
    retention: Duration,
}

impl IntervalLimiter {
    pub fn new(minimum_interval: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            minimum_interval,
            retention: minimum_interval.saturating_mul(3_600),
        }
    }

    pub fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        let Ok(mut entries) = self.entries.lock() else {
            return false;
        };
        entries.retain(|_, seen| now.duration_since(*seen) <= self.retention);
        if entries
            .get(key)
            .is_some_and(|seen| now.duration_since(*seen) < self.minimum_interval)
        {
            return false;
        }
        entries.insert(key.to_string(), now);
        true
    }
}

impl AttemptLimiter {
    pub fn new(maximum_failures: u32, window: Duration, block_for: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            maximum_failures: maximum_failures.max(1),
            window,
            block_for,
            retention: window.max(block_for).saturating_mul(4),
        }
    }

    pub fn retry_after(&self, key: &str) -> Option<u64> {
        let now = Instant::now();
        let mut entries = self.entries.lock().ok()?;
        entries.retain(|_, state| now.duration_since(state.last_seen) <= self.retention);
        let state = entries.get_mut(key)?;
        state.last_seen = now;
        match state.blocked_until {
            Some(until) if until > now => Some(until.duration_since(now).as_secs().max(1)),
            Some(_) => {
                entries.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn record_failure(&self, key: &str) {
        let now = Instant::now();
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let state = entries.entry(key.to_string()).or_insert(AttemptState {
            failures: 0,
            window_started: now,
            blocked_until: None,
            last_seen: now,
        });
        if now.duration_since(state.window_started) > self.window {
            state.failures = 0;
            state.window_started = now;
            state.blocked_until = None;
        }
        state.failures = state.failures.saturating_add(1);
        state.last_seen = now;
        if state.failures >= self.maximum_failures {
            state.blocked_until = Some(now + self.block_for);
        }
    }

    pub fn clear(&self, key: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_the_configured_number_of_failures() {
        let limiter = AttemptLimiter::new(3, Duration::from_secs(60), Duration::from_secs(60));
        assert_eq!(limiter.retry_after("client"), None);
        limiter.record_failure("client");
        limiter.record_failure("client");
        assert_eq!(limiter.retry_after("client"), None);
        limiter.record_failure("client");
        assert!(limiter.retry_after("client").is_some());
        limiter.clear("client");
        assert_eq!(limiter.retry_after("client"), None);
    }

    #[test]
    fn interval_limiter_coalesces_repeated_requests() {
        let limiter = IntervalLimiter::new(Duration::from_secs(60));
        assert!(limiter.allow("client"));
        assert!(!limiter.allow("client"));
        assert!(limiter.allow("other"));
    }
}
