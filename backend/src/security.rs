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

#[derive(Clone, Copy)]
struct BucketState {
    tokens: f64,
    last_refill: Instant,
    last_seen: Instant,
}

/// Token bucket per caller and resource for expensive public endpoints.
///
/// Unlike [`AttemptLimiter`], which only reacts to failures, this counts every
/// request, so it bounds how much work a single caller can demand. Tokens refill
/// continuously rather than resetting on a window boundary, so a burst that
/// exhausts the bucket recovers gradually instead of all at once.
pub struct RateLimiter {
    entries: Mutex<HashMap<(String, String), BucketState>>,
    capacity: f64,
    tokens_per_second: f64,
    retention: Duration,
}

impl RateLimiter {
    /// Allows `capacity` requests per `window`, refilling that quota evenly
    /// across the window.
    pub fn new(capacity: u32, window: Duration) -> Self {
        let capacity = f64::from(capacity.max(1));
        let window_seconds = window.as_secs_f64();
        // A zero or degenerate window would divide by zero; falling back to
        // `capacity` per second keeps the limiter usable rather than unbounded.
        let tokens_per_second = if window_seconds.is_finite() && window_seconds > 0.0 {
            capacity / window_seconds
        } else {
            capacity
        };
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
            tokens_per_second,
            retention: window.saturating_mul(4),
        }
    }

    /// Consumes one token, returning how long to wait when none are left.
    ///
    /// A poisoned lock denies the request rather than allowing it: this guards
    /// public endpoints, so failing closed is the safer default.
    pub fn check(&self, caller: &str, resource: &str) -> Result<(), u64> {
        self.check_at(caller, resource, Instant::now())
    }

    fn check_at(&self, caller: &str, resource: &str, now: Instant) -> Result<(), u64> {
        let Ok(mut entries) = self.entries.lock() else {
            return Err(1);
        };
        entries.retain(|_, state| now.duration_since(state.last_seen) <= self.retention);
        let state = entries
            .entry((caller.to_string(), resource.to_string()))
            .or_insert(BucketState {
                tokens: self.capacity,
                last_refill: now,
                last_seen: now,
            });
        let elapsed = now.duration_since(state.last_refill).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.tokens_per_second).min(self.capacity);
        state.last_refill = now;
        state.last_seen = now;
        if state.tokens < 1.0 {
            let missing = 1.0 - state.tokens;
            return Err((missing / self.tokens_per_second).ceil().max(1.0) as u64);
        }
        state.tokens -= 1.0;
        Ok(())
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
    fn rate_limiter_allows_a_burst_then_throttles_per_key() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        let now = Instant::now();
        for _ in 0..3 {
            assert!(limiter.check_at("client", "node", now).is_ok());
        }
        assert_eq!(limiter.check_at("client", "node", now), Err(20));

        // Other callers have their own bucket.
        assert!(limiter.check_at("other", "node", now).is_ok());
    }

    #[test]
    fn loading_many_nodes_does_not_exhaust_one_nodes_history_budget() {
        let limiter = RateLimiter::new(120, Duration::from_secs(60));
        let now = Instant::now();
        for node in 0..150 {
            assert!(
                limiter
                    .check_at("client", &format!("node-{node}"), now)
                    .is_ok()
            );
        }
        // A detail request for an already loaded card remains available.
        assert!(limiter.check_at("client", "node-0", now).is_ok());
        for _ in 0..118 {
            assert!(limiter.check_at("client", "node-0", now).is_ok());
        }
        assert_eq!(limiter.check_at("client", "node-0", now), Err(1));
        assert!(limiter.check_at("client", "node-1", now).is_ok());
    }

    #[test]
    fn rate_limiter_refills_over_time() {
        let limiter = RateLimiter::new(2, Duration::from_secs(2));
        let now = Instant::now();
        assert!(limiter.check_at("client", "node", now).is_ok());
        assert!(limiter.check_at("client", "node", now).is_ok());
        assert_eq!(limiter.check_at("client", "node", now), Err(1));
        let later = now + Duration::from_secs(1);
        assert!(limiter.check_at("client", "node", later).is_ok());
        assert_eq!(limiter.check_at("client", "node", later), Err(1));
    }

    #[test]
    fn rate_limiter_never_accumulates_beyond_capacity() {
        let limiter = RateLimiter::new(2, Duration::from_secs(1));
        let now = Instant::now();
        assert!(limiter.check_at("client", "node", now).is_ok());
        // Let the existing bucket refill without expiring it.
        let later = now + Duration::from_secs(3);
        assert!(limiter.check_at("client", "node", later).is_ok());
        assert!(limiter.check_at("client", "node", later).is_ok());
        assert_eq!(limiter.check_at("client", "node", later), Err(1));
    }
}
