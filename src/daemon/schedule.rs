use super::model::ScheduleSpec;
use super::{DaemonError, Result};
use chrono::{DateTime, Duration, LocalResult, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use std::str::FromStr;

fn cron_parts(spec: &ScheduleSpec) -> Result<(Schedule, Tz)> {
    let (expression, timezone) = match spec {
        ScheduleSpec::Cron {
            expression,
            timezone,
            ..
        } => (expression.clone(), timezone.as_str()),
        ScheduleSpec::Daily {
            hour,
            minute,
            timezone,
            ..
        } => (format!("{minute} {hour} * * *"), timezone.as_str()),
        ScheduleSpec::Monthly {
            day,
            hour,
            minute,
            timezone,
            ..
        } => (format!("{minute} {hour} {day} * *"), timezone.as_str()),
        ScheduleSpec::Once { .. } => {
            return Err(DaemonError::InvalidSchedule(
                "one-shot schedules do not contain cron fields".into(),
            ));
        }
    };
    let field_count = expression.split_whitespace().count();
    if field_count != 5 {
        return Err(DaemonError::InvalidSchedule(
            "cron expressions must contain exactly five fields".into(),
        ));
    }
    let cron = Schedule::from_str(&format!("0 {expression}"))
        .map_err(|error| DaemonError::InvalidSchedule(format!("cron expression: {error}")))?;
    let timezone = timezone
        .parse::<Tz>()
        .map_err(|_| DaemonError::InvalidSchedule(format!("unknown timezone `{timezone}`")))?;
    Ok((cron, timezone))
}

pub fn validate(spec: &ScheduleSpec) -> Result<()> {
    match spec {
        ScheduleSpec::Daily { hour, minute, .. } => {
            if *hour > 23 || *minute > 59 {
                return Err(DaemonError::InvalidSchedule(
                    "daily hour/minute is outside its valid range".into(),
                ));
            }
            cron_parts(spec).map(|_| ())
        }
        ScheduleSpec::Monthly {
            day, hour, minute, ..
        } => {
            if !(1..=31).contains(day) || *hour > 23 || *minute > 59 {
                return Err(DaemonError::InvalidSchedule(
                    "monthly day/hour/minute is outside its valid range".into(),
                ));
            }
            cron_parts(spec).map(|_| ())
        }
        ScheduleSpec::Cron { .. } => cron_parts(spec).map(|_| ()),
        ScheduleSpec::Once { .. } => Ok(()),
    }
}

pub fn next_after(spec: &ScheduleSpec, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    if let ScheduleSpec::Once { at } = spec {
        return (*at > after)
            .then_some(*at)
            .ok_or_else(|| DaemonError::InvalidSchedule("one-shot schedule has elapsed".into()));
    }

    let (schedule, timezone) = cron_parts(spec)?;
    let local_after = after.with_timezone(&timezone).naive_local();
    let civil_cursor = Utc.from_utc_datetime(&local_after);

    for candidate in schedule.after(&civil_cursor).take(10_000) {
        let mut civil = candidate.naive_utc();
        for _ in 0..=(26 * 60) {
            let resolved = match timezone.from_local_datetime(&civil) {
                LocalResult::Single(value) => Some(value.with_timezone(&Utc)),
                LocalResult::Ambiguous(first, second) => {
                    Some(first.min(second).with_timezone(&Utc))
                }
                LocalResult::None => None,
            };
            if let Some(instant) = resolved {
                if instant > after {
                    return Ok(instant);
                }
                break;
            }
            civil += Duration::minutes(1);
            civil = civil
                .with_second(0)
                .and_then(|value| value.with_nanosecond(0))
                .unwrap_or(civil);
        }
    }

    Err(DaemonError::InvalidSchedule(
        "cron expression has no future occurrence".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::model::{MisfirePolicy, ScheduleSpec};
    use chrono::{TimeZone, Utc};

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn daily_schedule_uses_iana_timezone() {
        let schedule = ScheduleSpec::daily(8, 0, "Europe/Oslo", MisfirePolicy::SkipMissed)
            .expect("valid daily schedule");
        assert_eq!(
            schedule.next_after(utc(2026, 1, 15, 6, 59)).unwrap(),
            utc(2026, 1, 15, 7, 0)
        );
    }

    #[test]
    fn monthly_schedule_advances_to_next_valid_month() {
        let schedule = ScheduleSpec::monthly(31, 9, 15, "Europe/Oslo", MisfirePolicy::RunOnce)
            .expect("valid monthly schedule");
        assert_eq!(
            schedule.next_after(utc(2026, 4, 1, 0, 0)).unwrap(),
            utc(2026, 5, 31, 7, 15)
        );
    }

    #[test]
    fn invalid_cron_expression_is_rejected() {
        let error =
            ScheduleSpec::cron("not a cron", "Europe/Oslo", MisfirePolicy::SkipMissed).unwrap_err();
        assert!(error.to_string().contains("cron"));
    }

    #[test]
    fn spring_forward_moves_nonexistent_time_to_next_valid_instant() {
        let schedule = ScheduleSpec::daily(2, 30, "Europe/Oslo", MisfirePolicy::SkipMissed)
            .expect("valid daily schedule");
        assert_eq!(
            schedule.next_after(utc(2026, 3, 28, 23, 0)).unwrap(),
            utc(2026, 3, 29, 1, 0)
        );
    }

    #[test]
    fn fall_back_ambiguous_time_fires_once_at_first_instant() {
        let schedule = ScheduleSpec::daily(2, 30, "Europe/Oslo", MisfirePolicy::SkipMissed)
            .expect("valid daily schedule");
        let first = schedule.next_after(utc(2026, 10, 24, 23, 0)).unwrap();
        assert_eq!(first, utc(2026, 10, 25, 0, 30));
        assert_eq!(
            schedule.next_after(first).unwrap(),
            utc(2026, 10, 26, 1, 30)
        );
    }

    #[test]
    fn recurring_schedules_accept_both_misfire_policies() {
        for policy in [MisfirePolicy::SkipMissed, MisfirePolicy::RunOnce] {
            let schedule = ScheduleSpec::cron("0 8 * * *", "UTC", policy).unwrap();
            assert_eq!(schedule.misfire_policy(), policy);
            schedule.validate().unwrap();
        }
    }
}
