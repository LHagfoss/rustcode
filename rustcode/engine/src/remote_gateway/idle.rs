//! Idle expiry of pairings (#4404). A paired device that has not connected
//! for `[remote] idle_expiry_days` (90 by default, `0` turns it off) is
//! refused at authentication like an expired static pairing, and listed as
//! expired until it is removed or pairs again. Each authentication moves
//! `last_seen_at`, so a device in use never reaches the limit.

use chrono::{DateTime, Duration, Utc};

use super::devices::DeviceRecord;

/// The idle limit when `[remote] idle_expiry_days` is unset.
pub const DEFAULT_IDLE_DAYS: u32 = 90;

/// The limit `idle_expiry_days` sets; `None` when it is `0`.
pub fn limit_of(days: Option<u32>) -> Option<Duration> {
    match days.unwrap_or(DEFAULT_IDLE_DAYS) {
        0 => None,
        days => Some(Duration::days(i64::from(days))),
    }
}

/// The limit in the user's config.
pub fn limit() -> Option<Duration> {
    let (_, _, config) = crate::config::load_config();
    limit_of(config.remote.idle_expiry_days)
}

/// What the gateway answers an idle device, the same refusal an expired
/// static pairing gets.
pub const REFUSAL: &str = "unknown device or token; pair this device again";

impl DeviceRecord {
    /// When the device last connected, or paired when that was never kept.
    pub fn idle_since(&self) -> DateTime<Utc> {
        self.last_seen_at.unwrap_or(self.paired_at)
    }

    /// Whether the device has gone unused for `limit` at `now`.
    pub fn is_idle(&self, now: DateTime<Utc>, limit: Option<Duration>) -> bool {
        limit.is_some_and(|limit| now - self.idle_since() >= limit)
    }

    /// [`Self::is_idle`] under the configured limit.
    pub fn is_idle_now(&self) -> bool {
        self.is_idle(Utc::now(), limit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_gateway::devices::{DeviceProfile, Devices};

    fn rotating() -> DeviceRecord {
        Devices::default()
            .enroll_rotating("iPad", None, &DeviceProfile::default())
            .unwrap()
            .record
    }

    #[test]
    fn the_limit_defaults_to_ninety_days_and_zero_turns_it_off() {
        assert_eq!(limit_of(None), Some(Duration::days(90)));
        assert_eq!(limit_of(Some(7)), Some(Duration::days(7)));
        assert_eq!(limit_of(Some(0)), None);
    }

    #[test]
    fn a_rotating_record_idle_past_the_limit_is_idle() {
        let record = rotating();
        assert_eq!(record.expires_at(), None);
        let seen = record.last_seen_at.unwrap();
        let limit = limit_of(None);
        assert!(!record.is_idle(seen + Duration::days(89), limit));
        assert!(record.is_idle(seen + Duration::days(90), limit));
        assert!(!record.is_idle(seen + Duration::days(900), None));
    }

    #[test]
    fn a_record_used_recently_is_not_idle_and_use_extends_it() {
        let mut devices = Devices::default();
        let id = devices
            .enroll_rotating("iPad", None, &DeviceProfile::default())
            .unwrap()
            .record
            .id;
        let limit = limit_of(None);
        let later = devices.list()[0].last_seen_at.unwrap() + Duration::days(80);
        assert!(devices.mark_seen(&id, later));
        let record = &devices.list()[0];
        assert!(!record.is_idle(later + Duration::days(80), limit));
        assert!(record.is_idle(later + Duration::days(90), limit));
    }

    #[test]
    fn a_record_without_last_seen_counts_from_pairing() {
        let mut record = rotating();
        record.last_seen_at = None;
        assert_eq!(record.idle_since(), record.paired_at);
        assert!(record.is_idle(record.paired_at + Duration::days(91), limit_of(None)));
    }
}
