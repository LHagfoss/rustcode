//! Whether the machine is short of memory, which is when the idle reaper
//! (`session.rs`) may stop a harness process that has not been idle for long.
//!
//! One signal for every platform: the memory the OS says is available
//! (`sysinfo`: `vm_stat` and `host_statistics` on macOS, `MemAvailable` in
//! `/proc/meminfo` on Linux, `GlobalMemoryStatusEx` on Windows). Below
//! [`LOW_AVAILABLE_PERCENT`] of the total, the machine is under pressure. When
//! the OS reports no total, the pressure is unknown and the reaper keeps the
//! plain cap it had before.

/// Under pressure when less than this share of memory is available.
pub(crate) const LOW_AVAILABLE_PERCENT: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pressure {
    /// Enough memory is free: nothing needs stopping for the cap's sake.
    Low,
    /// Memory is short: the least recently used processes may go to the cap.
    High,
    /// The machine did not say: the cap applies as it always has.
    Unknown,
}

impl Pressure {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
            Self::Unknown => "unknown",
        }
    }
}

/// The pressure `available` of `total` bytes means.
pub(crate) fn pressure_of(available: u64, total: u64) -> Pressure {
    if total == 0 {
        return Pressure::Unknown;
    }
    // Compared without a division, so a small total loses nothing to rounding.
    if u128::from(available) * 100 < u128::from(total) * u128::from(LOW_AVAILABLE_PERCENT) {
        Pressure::High
    } else {
        Pressure::Low
    }
}

/// The machine's pressure right now. Reads the OS's own memory counters, so it
/// is called only when a pass has a cap to enforce.
pub(crate) fn current() -> Pressure {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    pressure_of(system.available_memory(), system.total_memory())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn plenty_of_free_memory_is_low_pressure() {
        assert_eq!(pressure_of(8 * GIB, 16 * GIB), Pressure::Low);
    }

    #[test]
    fn under_a_tenth_available_is_high_pressure() {
        assert_eq!(pressure_of(GIB, 16 * GIB), Pressure::High);
    }

    #[test]
    fn the_threshold_itself_is_not_pressure() {
        assert_eq!(pressure_of(10, 100), Pressure::Low);
        assert_eq!(pressure_of(9, 100), Pressure::High);
    }

    #[test]
    fn a_machine_that_reports_no_total_is_unknown() {
        assert_eq!(pressure_of(0, 0), Pressure::Unknown);
        assert_eq!(pressure_of(5 * GIB, 0), Pressure::Unknown);
    }
}
