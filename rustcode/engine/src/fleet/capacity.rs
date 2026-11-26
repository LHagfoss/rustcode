//! How busy a computer is, sent with its own fleet entry so an agent placing
//! work (`list_machines`) can prefer an idle machine on mains power. Whole
//! numbers only, each clamped, so a hostile or broken peer cannot store
//! negative, NaN or huge values; an older peer ignores the field.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The most cores kept.
const MAX_CORES: u32 = 4096;
/// The highest one-minute load kept, in hundredths.
const MAX_LOAD_HUNDREDTHS: u32 = 100_000;
/// The most free memory kept, in MiB (64 TiB).
const MAX_MEMORY_MB: u64 = 64 * 1024 * 1024;
/// The most running turns kept.
const MAX_RUNNING_TURNS: u32 = 10_000;
/// A report older than this is stale: shown with its age, not as current.
pub(crate) const FRESH_SECS: i64 = 180;

/// One computer's load when it last reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Capacity {
    #[serde(default)]
    pub cores: u32,
    /// The one-minute load average times 100; 0 where the system has none.
    #[serde(default)]
    pub load1_hundredths: u32,
    #[serde(default)]
    pub memory_free_mb: u64,
    /// `None` when it cannot tell (no battery, or no way to read it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_battery: Option<bool>,
    #[serde(default)]
    pub running_turns: u32,
    pub at: DateTime<Utc>,
}

impl Capacity {
    /// Clamped to what any real computer reports, and timed no later than
    /// the clock skew allows.
    pub(super) fn cleaned(self, now: DateTime<Utc>) -> Self {
        Self {
            cores: self.cores.min(MAX_CORES),
            load1_hundredths: self.load1_hundredths.min(MAX_LOAD_HUNDREDTHS),
            memory_free_mb: self.memory_free_mb.min(MAX_MEMORY_MB),
            on_battery: self.on_battery,
            running_turns: self.running_turns.min(MAX_RUNNING_TURNS),
            at: super::bounded_stamp(self.at, now),
        }
    }

    /// This computer now.
    pub(crate) fn measure(now: DateTime<Utc>) -> Self {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        let load = sysinfo::System::load_average().one;
        Self {
            cores: std::thread::available_parallelism().map_or(1, |cores| cores.get() as u32),
            load1_hundredths: if load.is_finite() && load > 0.0 {
                (load * 100.0).round() as u32
            } else {
                0
            },
            memory_free_mb: system.available_memory() / (1024 * 1024),
            on_battery: on_battery(),
            running_turns: crate::thread_status::running_count() as u32,
            at: now,
        }
        .cleaned(now)
    }

    /// As `list_machines` shows it, with its age and whether it is stale.
    pub(crate) fn row(&self, now: DateTime<Utc>) -> Value {
        let age = (now - self.at).num_seconds().max(0);
        json!({
            "cores": self.cores,
            "load1": f64::from(self.load1_hundredths) / 100.0,
            "memory_free_mb": self.memory_free_mb,
            "on_battery": self.on_battery,
            "running_turns": self.running_turns,
            "at": self.at,
            "age_secs": age,
            "stale": age > FRESH_SECS,
        })
    }
}

/// Whether this computer runs on battery: Linux's power supply class; `None`
/// elsewhere or when nothing says.
fn on_battery() -> Option<bool> {
    let supplies = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let mut mains = None;
    for supply in supplies.flatten() {
        let path = supply.path();
        let kind = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        if kind.trim() == "Mains" {
            let online = std::fs::read_to_string(path.join("online")).unwrap_or_default();
            mains = Some(mains.unwrap_or(false) || online.trim() == "1");
        }
    }
    mains.map(|online| !online)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleaning_bounds_every_value_and_an_old_report_reads_as_stale() {
        let now = Utc::now();
        let wild = Capacity {
            cores: u32::MAX,
            load1_hundredths: u32::MAX,
            memory_free_mb: u64::MAX,
            on_battery: Some(true),
            running_turns: u32::MAX,
            at: now + chrono::Duration::days(365),
        }
        .cleaned(now);
        assert_eq!(wild.cores, MAX_CORES);
        assert_eq!(wild.load1_hundredths, MAX_LOAD_HUNDREDTHS);
        assert_eq!(wild.memory_free_mb, MAX_MEMORY_MB);
        assert_eq!(wild.running_turns, MAX_RUNNING_TURNS);
        assert!(wild.at <= now + chrono::Duration::minutes(super::super::MAX_CLOCK_SKEW_MINUTES));

        let old = Capacity {
            at: now - chrono::Duration::minutes(10),
            ..wild
        };
        let row = old.row(now);
        assert_eq!(row["stale"], true);
        assert_eq!(row["age_secs"], 600);
        assert_eq!(Capacity { at: now, ..wild }.row(now)["stale"], false);
    }

    #[test]
    fn this_computer_measures_at_least_one_core() {
        let now = Utc::now();
        let measured = Capacity::measure(now);
        assert!(measured.cores >= 1);
        assert_eq!(measured.at, now);
    }
}
