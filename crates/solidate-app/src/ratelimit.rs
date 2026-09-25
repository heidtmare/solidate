//! Per-token request rate limiting: an in-process token bucket per API token.
//!
//! Limits apply per server process; running several replicas multiplies the
//! effective limit by the replica count.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use solidate_db::TokenId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimit {
    /// Sustained requests per minute.
    pub per_minute: u32,
    /// Requests allowed at once from a full bucket.
    pub burst: u32,
}

impl RateLimit {
    fn per_sec(&self) -> f64 {
        f64::from(self.per_minute) / 60.0
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl Bucket {
    fn full(limit: &RateLimit, now: Instant) -> Self {
        Self {
            tokens: f64::from(limit.burst),
            updated: now,
        }
    }

    fn refill(&mut self, limit: &RateLimit, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * limit.per_sec()).min(f64::from(limit.burst));
        self.updated = now;
    }

    /// Takes one request. `Err` carries the wait until one is available.
    fn take(&mut self, limit: &RateLimit, now: Instant) -> Result<(), Duration> {
        self.refill(limit, now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - self.tokens) / limit.per_sec()))
        }
    }
}

/// Map size above which full (idle) buckets are dropped on insert.
const PRUNE_AT: usize = 4096;

pub(crate) struct RateLimiter {
    limit: Option<RateLimit>,
    buckets: Mutex<HashMap<TokenId, Bucket>>,
}

impl RateLimiter {
    pub(crate) fn new(limit: Option<RateLimit>) -> Self {
        let limit = limit.filter(|l| l.per_minute > 0 && l.burst > 0);
        Self {
            limit,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Counts one request for `token`. `Err` carries the wait until the next
    /// request is allowed.
    pub(crate) fn check(&self, token: TokenId) -> Result<(), Duration> {
        self.check_at(token, Instant::now())
    }

    fn check_at(&self, token: TokenId, now: Instant) -> Result<(), Duration> {
        let Some(limit) = &self.limit else { return Ok(()) };
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        if buckets.len() >= PRUNE_AT && !buckets.contains_key(&token) {
            buckets.retain(|_, b| {
                b.refill(limit, now);
                b.tokens < f64::from(limit.burst)
            });
        }
        buckets
            .entry(token)
            .or_insert_with(|| Bucket::full(limit, now))
            .take(limit, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: RateLimit = RateLimit {
        per_minute: 60,
        burst: 3,
    };

    #[test]
    fn burst_then_refill() {
        let rl = RateLimiter::new(Some(LIMIT));
        let (t, t0) = (TokenId::new(), Instant::now());
        for _ in 0..3 {
            assert!(rl.check_at(t, t0).is_ok());
        }
        let wait = rl.check_at(t, t0).unwrap_err();
        assert!(
            wait > Duration::from_millis(900) && wait <= Duration::from_secs(1),
            "{wait:?}"
        );
        assert!(rl.check_at(t, t0 + Duration::from_millis(500)).is_err());
        assert!(rl.check_at(t, t0 + Duration::from_millis(1100)).is_ok());
        // Refill is capped at the burst size.
        let later = t0 + Duration::from_secs(3600);
        for _ in 0..3 {
            assert!(rl.check_at(t, later).is_ok());
        }
        assert!(rl.check_at(t, later).is_err());
    }

    #[test]
    fn tokens_are_independent() {
        let rl = RateLimiter::new(Some(LIMIT));
        let (a, b, now) = (TokenId::new(), TokenId::new(), Instant::now());
        for _ in 0..3 {
            rl.check_at(a, now).unwrap();
        }
        assert!(rl.check_at(a, now).is_err());
        assert!(rl.check_at(b, now).is_ok());
    }

    #[test]
    fn disabled_limits_never_reject() {
        for limit in [
            None,
            Some(RateLimit {
                per_minute: 0,
                burst: 3,
            }),
        ] {
            let rl = RateLimiter::new(limit);
            let (t, now) = (TokenId::new(), Instant::now());
            for _ in 0..1000 {
                assert!(rl.check_at(t, now).is_ok());
            }
        }
    }

    #[test]
    fn prune_drops_idle_buckets() {
        let rl = RateLimiter::new(Some(LIMIT));
        let now = Instant::now();
        let busy = TokenId::new();
        for _ in 0..3 {
            rl.check_at(busy, now).unwrap();
        }
        for _ in 0..PRUNE_AT {
            rl.check_at(TokenId::new(), now).unwrap();
        }
        // After 2 s the other buckets (2 of 3 left) are full again and dropped; the
        // drained one (2 of 3 refilled) is kept.
        rl.check_at(TokenId::new(), now + Duration::from_secs(2)).unwrap();
        let buckets = rl.buckets.lock().unwrap();
        assert!(buckets.len() < 10, "{}", buckets.len());
        assert!(buckets.contains_key(&busy));
    }
}
