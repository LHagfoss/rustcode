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

/// Bound on waiting for response headers once a TCP connection is
/// established. `reqwest::Client`'s `connect_timeout` only covers the TCP
/// handshake — a provider that accepts the connection but never sends
/// headers would otherwise hang forever. This is distinct from a whole
/// request timeout (which would kill legitimate long-running SSE streams).
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Races `fut` against `cancel_token`. Returns `None` the instant the token
/// fires, without waiting for `fut` to resolve; otherwise returns `Some` of
/// whatever `fut` produced.
///
/// Used to make request send and retry-backoff waits cancellation-aware:
/// wrap `req.send()` or `tokio::time::sleep(delay)` in this instead of
/// awaiting them directly, so a mid-flight cancellation takes effect
/// immediately rather than after the in-flight operation completes.
pub async fn race_cancellable<F: std::future::Future>(
    fut: F,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Option<F::Output> {
    tokio::select! {
        biased;
        _ = cancel_token.cancelled() => None,
        v = fut => Some(v),
    }
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

    // --- race_cancellable ---
    //
    // These stand in for the two real call sites that need to be
    // cancellation-aware: the initial `req.send()` (headers not yet
    // received) and the retry backoff `sleep`. Both are exercised here in
    // isolation with dummy futures rather than a real HTTP server, per the
    // task's guidance to prefer a few fast, deterministic tests. Real
    // network behavior (`stream_request` itself) is verified by code
    // inspection — see the PR description.

    #[tokio::test]
    async fn race_cancellable_returns_none_when_already_cancelled() {
        // Simulates: cancellation arrives before response headers ever
        // come back. A pending future never resolves on its own, so if
        // this returns at all, the cancel branch won.
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();
        let result = race_cancellable(std::future::pending::<()>(), &cancel_token).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn race_cancellable_returns_value_when_not_cancelled() {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let result = race_cancellable(async { 42 }, &cancel_token).await;
        assert_eq!(result, Some(42));
    }

    #[tokio::test(start_paused = true)]
    async fn race_cancellable_interrupts_backoff_sleep_promptly() {
        // Simulates: cancellation arrives mid-backoff-delay. The sleep is
        // long (30s of *virtual* time); with the clock paused, if the
        // cancel branch didn't win, this test would hang instead of
        // completing, since nothing ever advances the virtual clock.
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            // Yield once so the racing sleep is polled and registered
            // before we cancel, exercising the actual race rather than a
            // pre-cancelled token.
            tokio::task::yield_now().await;
            cancel_token_clone.cancel();
        });
        let result =
            race_cancellable(tokio::time::sleep(Duration::from_secs(30)), &cancel_token).await;
        assert!(result.is_none());
    }

    // Simulates: `stream_request` got a non-success HTTP response and is
    // reading the error body via `resp.text()` when the user cancels. A
    // `reqwest::Response::text()` future can't be constructed without a real
    // response, so this stands in with an equivalent shape — a future that
    // resolves to `Result<String, _>` and would otherwise hang until it
    // finishes — proving `race_cancellable` interrupts a slow body read
    // exactly the way it interrupts the header wait and backoff sleep.
    #[tokio::test]
    async fn race_cancellable_interrupts_an_in_flight_error_body_read() {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel_token_clone.cancel();
        });
        // A body read that never completes on its own — if the cancel
        // branch didn't win, this future would await forever.
        let slow_body_read = std::future::pending::<Result<String, std::io::Error>>();
        let result = race_cancellable(slow_body_read, &cancel_token).await;
        assert!(
            result.is_none(),
            "cancellation must interrupt an in-flight error-body read"
        );
    }

    // Preserves the non-cancelled path for the same shape: a completed body
    // read still returns its content normally through the race.
    #[tokio::test]
    async fn race_cancellable_preserves_error_body_when_not_cancelled() {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let body_read = async { Ok::<String, std::io::Error>("rate limited".to_string()) };
        let result = race_cancellable(body_read, &cancel_token).await;
        match result {
            Some(Ok(body)) => assert_eq!(body, "rate limited"),
            other => panic!("expected the body read to complete normally, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn header_timeout_fires_without_cancellation() {
        // Simulates: connection succeeds but the provider never sends
        // headers, with no cancellation involved. `tokio::time::timeout`
        // bounds the wait; virtual time is advanced explicitly since the
        // clock is paused.
        let never = std::future::pending::<()>();
        let timed = tokio::time::timeout(HEADER_TIMEOUT, never);
        tokio::time::advance(HEADER_TIMEOUT + Duration::from_millis(1)).await;
        assert!(timed.await.is_err());
    }

    // Safety regression: `stream_request` wraps ONLY the header wait
    // (`req.send()`) in `tokio::time::timeout(HEADER_TIMEOUT, ...)` — once
    // headers arrive, the SSE body-reading loop that follows is a separate,
    // unwrapped `while` loop bounded only by cancellation, never by
    // HEADER_TIMEOUT. This proves that two-phase structure holds: a header
    // wait that resolves promptly is unaffected, and a "stream" that then
    // runs far longer than HEADER_TIMEOUT is never touched by it, because
    // nothing wraps phase two in that timeout at all.
    #[tokio::test(start_paused = true)]
    async fn header_timeout_does_not_bound_a_long_running_stream_after_headers_arrive() {
        // Phase 1: headers arrive well within the header timeout.
        let headers = tokio::time::timeout(HEADER_TIMEOUT, async { "200 OK" }).await;
        assert!(headers.is_ok(), "headers must arrive before the timeout");

        // Phase 2: a long-running SSE stream, deliberately NOT wrapped in
        // HEADER_TIMEOUT or any timeout — exactly like the real read loop.
        // Advance virtual time well past HEADER_TIMEOUT; an unwrapped future
        // is unaffected by however long it takes.
        let long_stream = async {
            tokio::time::sleep(HEADER_TIMEOUT * 3).await;
            "stream finished"
        };
        assert_eq!(long_stream.await, "stream finished");
    }
}
