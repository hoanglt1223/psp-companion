//! Token bucket that keeps all PSP traffic (both directions) under the Wi-Fi budget.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const HARD_CAP_KBPS: u32 = 300;
pub const BUCKET_CAPACITY: f64 = 16.0 * 1024.0;
pub const TCP_OVERHEAD: usize = 40;
pub const UDP_OVERHEAD: usize = 28;

#[derive(Debug)]
pub struct TokenBucket {
    rate: f64,
    capacity: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(kbps: u32, now: Instant) -> Self {
        let rate = f64::from(kbps.min(HARD_CAP_KBPS)) * 1000.0 / 8.0;
        Self { rate, capacity: BUCKET_CAPACITY, tokens: BUCKET_CAPACITY, last: now }
    }

    pub fn bytes_per_second(&self) -> f64 {
        self.rate
    }

    fn refill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + dt * self.rate).min(self.capacity);
        self.last = now;
    }

    /// Non-blocking send: true if `n` bytes may go now.
    pub fn try_take(&mut self, n: usize, now: Instant) -> bool {
        self.refill(now);
        if self.tokens >= self.need(n) {
            self.tokens -= n as f64;
            true
        } else {
            false
        }
    }

    /// Received bytes are already on the air, so they are always charged and may push the bucket into debt.
    pub fn charge(&mut self, n: usize, now: Instant) {
        self.refill(now);
        self.tokens -= n as f64;
    }

    /// How long until `try_take(n)` would succeed.
    pub fn wait_time(&mut self, n: usize, now: Instant) -> Duration {
        self.refill(now);
        let missing = self.need(n) - self.tokens;
        if missing <= 0.0 { Duration::ZERO } else { Duration::from_secs_f64(missing / self.rate) }
    }

    // A frame larger than the bucket may go once the bucket is full; it then leaves a debt.
    fn need(&self, n: usize) -> f64 {
        (n as f64).min(self.capacity)
    }
}

/// Tokio's clock, so paused-time tests drive the bucket too.
fn now() -> Instant {
    tokio::time::Instant::now().into_std()
}

#[derive(Clone, Debug)]
pub struct RateLimiter {
    inner: Arc<Mutex<TokenBucket>>,
}

impl RateLimiter {
    pub fn new(kbps: u32) -> Self {
        Self { inner: Arc::new(Mutex::new(TokenBucket::new(kbps, now()))) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TokenBucket> {
        // The bucket holds plain numbers; a panic mid-update cannot leave it unusable.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits until `n` wire bytes fit in the budget, then takes them.
    pub async fn acquire(&self, n: usize) {
        loop {
            let wait = {
                let mut b = self.lock();
                let now = now();
                if b.try_take(n, now) {
                    return;
                }
                b.wait_time(n, now)
            };
            tokio::time::sleep(wait.max(Duration::from_millis(1))).await;
        }
    }

    pub fn try_acquire(&self, n: usize) -> bool {
        self.lock().try_take(n, now())
    }

    pub fn charge_incoming(&self, n: usize) {
        self.lock().charge(n, now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_to_hard_cap() {
        let b = TokenBucket::new(10_000, Instant::now());
        assert_eq!(b.bytes_per_second(), 37_500.0);
    }

    #[test]
    fn sustained_rate_never_exceeds_budget() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(300, t0);
        let mut sent = 0usize;
        // Offer 1000-byte packets every millisecond (8 Mbps) for 10 simulated seconds.
        for ms in 0..10_000u64 {
            if b.try_take(1000, t0 + Duration::from_millis(ms)) {
                sent += 1000;
            }
        }
        let max = 37_500.0 * 10.0 + BUCKET_CAPACITY;
        assert!(sent as f64 <= max, "sent {sent} > {max}");
        assert!(sent as f64 >= 37_500.0 * 9.5, "limiter too strict: {sent}");
    }

    #[test]
    fn incoming_traffic_reduces_outgoing_budget() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(300, t0);
        b.charge(BUCKET_CAPACITY as usize + 37_500, t0);
        assert!(!b.try_take(1, t0));
        let wait = b.wait_time(1000, t0);
        assert!(wait > Duration::from_millis(1000) && wait < Duration::from_millis(1100), "{wait:?}");
        assert!(b.try_take(1000, t0 + wait + Duration::from_millis(1)));
    }

    #[test]
    fn oversized_frame_goes_when_full_and_leaves_debt() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(300, t0);
        assert!(b.try_take(20_000, t0));
        assert!(!b.try_take(1, t0));
        assert_eq!(b.wait_time(20_000, t0 + Duration::from_secs(60)), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_waits_for_tokens() {
        let rl = RateLimiter::new(300);
        assert!(rl.try_acquire(BUCKET_CAPACITY as usize));
        let start = tokio::time::Instant::now();
        rl.acquire(3750).await;
        assert!(start.elapsed() >= Duration::from_millis(99), "{:?}", start.elapsed());
    }
}
