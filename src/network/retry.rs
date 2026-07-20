//! Transient-failure retry policy for LLM requests.
//!
//! `stream_request` uses this to survive rate limits (429), server errors
//! (5xx), and network blips (timeout/connect/request) that happen *before*
//! the SSE stream starts. Once bytes are flowing we do not retry — that would
//! duplicate partial output.
//!
//! Rate limits (429) get patient Fibonacci-like delays; other transient
//! errors get exponential backoff. Both add jitter to avoid thundering herds.

use std::time::Duration;

/// Max retry attempts for a single request.
pub const MAX_RETRIES: usize = 5;

/// Fibonacci-like delays for rate limits (1, 2, 3, 5, 8, 13, 21, 30 s).
/// More patient than exponential — avoids hammering a throttled endpoint.
const FIBO_DELAYS_MS: &[u64] = &[1000, 2000, 3000, 5000, 8000, 13000, 21000, 30000];
const BASE_DELAY_MS: u64 = 500;
const MAX_DELAY_MS: u64 = 30_000;

/// True when an HTTP status is worth retrying. `0` means "no status"
/// (e.g. a network error surfaced without a response).
pub fn is_retryable_status(status: u16) -> bool {
    status == 0 || status >= 500 || status == 408 || status == 429
}

/// True when a reqwest transport error is transient (timeout / connect /
/// incomplete request) rather than a permanent client-side problem.
pub fn is_retryable_transport(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

/// Delay before retry attempt `attempt` (0-based). `status` steers the
/// strategy: 429 → Fibonacci, everything else → exponential backoff.
pub fn delay_for_attempt(attempt: usize, status: u16) -> Duration {
    if status == 429 {
        let ms = FIBO_DELAYS_MS.get(attempt).copied().unwrap_or(MAX_DELAY_MS);
        return Duration::from_millis(ms + jitter(ms, 0.15));
    }
    // Exponential backoff, capped. Shift is bounded so it can't overflow.
    let ms = (BASE_DELAY_MS.saturating_mul(1u64 << attempt.min(16))).min(MAX_DELAY_MS);
    Duration::from_millis(ms + jitter(ms, 0.10))
}

/// Add up to `frac` of `ms` as pseudo-random jitter (dependency-free).
fn jitter(ms: u64, frac: f64) -> u64 {
    (ms as f64 * frac * fastrand()) as u64
}

/// Cheap pseudo-random 0.0–1.0 from the system clock's sub-second nanos.
fn fastrand() -> f64 {
    use std::time::SystemTime;
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ((t.subsec_nanos() as f64) / 1_000_000_000.0).fract()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable_status(0)); // network error, no status
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(404));
    }

    #[test]
    fn rate_limit_uses_fibonacci() {
        // 429 attempt 0 ~= 1000ms (+ <=15% jitter)
        let d0 = delay_for_attempt(0, 429);
        assert!(d0.as_millis() >= 1000 && d0.as_millis() <= 1150);
        let d2 = delay_for_attempt(2, 429);
        assert!(d2.as_millis() >= 3000 && d2.as_millis() <= 3450);
    }

    #[test]
    fn server_error_uses_exponential() {
        // 500 → base 500ms, doubling: 500, 1000, 2000 (+ <=10% jitter)
        let d0 = delay_for_attempt(0, 500);
        assert!(d0.as_millis() >= 500 && d0.as_millis() <= 550);
        let d2 = delay_for_attempt(2, 500);
        assert!(d2.as_millis() >= 2000 && d2.as_millis() <= 2200);
    }

    #[test]
    fn delay_is_capped() {
        let d = delay_for_attempt(20, 500);
        assert!(d.as_millis() <= (MAX_DELAY_MS as f64 * 1.1) as u128 + 1);
    }
}
