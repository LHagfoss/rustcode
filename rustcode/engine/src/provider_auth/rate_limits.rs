//! Subscription rate-limit windows reported by a provider alongside a
//! response. They describe account quota, not RustCode's local token totals.

use serde_json::Value;

/// One quota window: how much of it is used and when it starts over.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    /// Unix seconds at which the window resets.
    pub resets_at: Option<i64>,
}

/// The short and long quota windows of a subscription account.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProviderRateLimits {
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
}

/// Response headers worth recording when diagnosing which quota data a
/// provider sends. Values are counters and timestamps, never credentials.
pub(crate) fn rate_limit_header_pairs(headers: &reqwest::header::HeaderMap) -> Vec<String> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            ["ratelimit", "rate-limit", "quota", "x-codex-"]
                .iter()
                .any(|needle| name.contains(needle))
        })
        .filter_map(|(name, value)| Some(format!("{}={}", name.as_str(), value.to_str().ok()?)))
        .collect()
}

impl ProviderRateLimits {
    fn non_empty(self) -> Option<Self> {
        (self.primary.is_some() || self.secondary.is_some()).then_some(self)
    }

    /// Read the `x-codex-{primary,secondary}-*` response headers. `now` is the
    /// current Unix time, used when a reset is reported as a relative delay.
    pub(crate) fn from_headers(headers: &reqwest::header::HeaderMap, now: i64) -> Option<Self> {
        let number = |name: String| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite())
        };
        let window = |slot: &str| {
            let used_percent = number(format!("x-codex-{slot}-used-percent"))?;
            let resets_at = number(format!("x-codex-{slot}-reset-at"))
                .or_else(|| number(format!("x-codex-{slot}-resets-at")))
                .map(|at| at as i64)
                .or_else(|| {
                    number(format!("x-codex-{slot}-reset-after-seconds"))
                        .map(|delay| now.saturating_add(delay as i64))
                });
            Some(RateLimitWindow {
                used_percent: used_percent.clamp(0.0, 100.0),
                window_minutes: number(format!("x-codex-{slot}-window-minutes"))
                    .map(|minutes| minutes as u64),
                resets_at,
            })
        };
        Self {
            primary: window("primary"),
            secondary: window("secondary"),
        }
        .non_empty()
    }

    /// Read a `rate_limits` object (`primary`/`secondary`, or the
    /// `primary_window`/`secondary_window` spelling of the usage response).
    pub(crate) fn from_json(rate_limits: &Value, now: i64) -> Option<Self> {
        let window = |keys: [&str; 2]| {
            let window = keys.iter().find_map(|key| rate_limits.get(key))?;
            let used_percent = window.get("used_percent").and_then(Value::as_f64)?;
            let window_minutes = window
                .get("window_minutes")
                .and_then(Value::as_u64)
                .or_else(|| {
                    window
                        .get("limit_window_seconds")
                        .and_then(Value::as_u64)
                        .map(|seconds| seconds / 60)
                });
            let resets_at = ["resets_at", "reset_at"]
                .iter()
                .find_map(|key| window.get(key).and_then(Value::as_i64))
                .or_else(|| {
                    ["reset_after_seconds", "resets_in_seconds"]
                        .iter()
                        .find_map(|key| window.get(key).and_then(Value::as_i64))
                        .map(|delay| now.saturating_add(delay))
                });
            Some(RateLimitWindow {
                used_percent: used_percent.clamp(0.0, 100.0),
                window_minutes,
                resets_at,
            })
        };
        Self {
            primary: window(["primary", "primary_window"]),
            secondary: window(["secondary", "secondary_window"]),
        }
        .non_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_yield_both_windows_and_resolve_relative_resets() {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in [
            ("x-codex-primary-used-percent", "9"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-after-seconds", "120"),
            ("x-codex-secondary-used-percent", "33.5"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-at", "1800000000"),
        ] {
            headers.insert(name, value.parse().unwrap());
        }
        let limits = ProviderRateLimits::from_headers(&headers, 1_000).expect("limits");
        assert_eq!(
            limits.primary,
            Some(RateLimitWindow {
                used_percent: 9.0,
                window_minutes: Some(300),
                resets_at: Some(1_120),
            })
        );
        assert_eq!(
            limits.secondary,
            Some(RateLimitWindow {
                used_percent: 33.5,
                window_minutes: Some(10_080),
                resets_at: Some(1_800_000_000),
            })
        );
        assert_eq!(rate_limit_header_pairs(&headers).len(), 6);
    }

    #[test]
    fn responses_without_quota_data_yield_nothing() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("content-type", "text/event-stream".parse().unwrap());
        assert_eq!(ProviderRateLimits::from_headers(&headers, 0), None);
        assert!(rate_limit_header_pairs(&headers).is_empty());
        assert_eq!(
            ProviderRateLimits::from_json(&serde_json::json!({}), 0),
            None
        );
    }

    #[test]
    fn json_accepts_both_window_spellings() {
        let limits = ProviderRateLimits::from_json(
            &serde_json::json!({
                "primary": {"used_percent": 20.0, "window_minutes": 300, "resets_at": 1_700_000_000_i64},
                "secondary_window": {"used_percent": 150.0, "limit_window_seconds": 604_800, "reset_after_seconds": 60}
            }),
            1_000,
        )
        .expect("limits");
        assert_eq!(limits.primary.unwrap().resets_at, Some(1_700_000_000));
        let secondary = limits.secondary.unwrap();
        assert_eq!(secondary.used_percent, 100.0);
        assert_eq!(secondary.window_minutes, Some(10_080));
        assert_eq!(secondary.resets_at, Some(1_060));
    }
}
