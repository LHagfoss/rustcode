//! The days `_rustcode/data/usage` groups and starts from, at the caller's
//! offset from UTC, so a day on the Statistics page ends at the user's own
//! midnight rather than UTC's.

/// The widest offsets any time zone uses, UTC-12 to UTC+14, in minutes.
const MAX_OFFSET_MINUTES: i32 = 14 * 60;

/// An offset in minutes east of UTC, held to the ones a clock can be at.
pub(crate) fn clamp(utc_offset_minutes: i32) -> i32 {
    utc_offset_minutes.clamp(-MAX_OFFSET_MINUTES, MAX_OFFSET_MINUTES)
}

/// The `day` key column: `started_at`'s calendar day at the offset.
pub(crate) fn day_key(offset: i32) -> String {
    if offset == 0 {
        "date(t.started_at) AS day".to_owned()
    } else {
        format!("date(t.started_at, '{offset:+} minutes') AS day")
    }
}

/// The filter for `since` (`YYYY-MM-DD` or `YYYY-MM`). `started_at` is RFC
/// 3339 in UTC and sorts as a string, so at offset zero a day or a month is
/// its own prefix and the column's index still applies; at any other offset
/// the bound is that day's local midnight written the same way.
pub(crate) fn since_filter(since: &str, offset: i32) -> String {
    let since = since.replace('\'', "");
    if offset == 0 {
        return format!("t.started_at >= '{since}'");
    }
    let start = if since.len() == 7 {
        format!("{since}-01")
    } else {
        since
    };
    format!(
        "t.started_at >= strftime('%Y-%m-%dT%H:%M:%SZ', '{start}', '{:+} minutes')",
        -offset
    )
}
