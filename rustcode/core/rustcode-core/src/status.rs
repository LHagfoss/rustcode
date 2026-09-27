//! Compact status formatting shared by frontends.
//!
//! Pure math over elapsed seconds and token budgets. Live status assembly
//! stays in the engine; these values do not.

pub fn format_elapsed_compact(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        format!("{elapsed_secs}s")
    } else if elapsed_secs < 3600 {
        format!("{}m {:02}s", elapsed_secs / 60, elapsed_secs % 60)
    } else {
        format!(
            "{}h {:02}m {:02}s",
            elapsed_secs / 3600,
            (elapsed_secs % 3600) / 60,
            elapsed_secs % 60
        )
    }
}

pub fn context_remaining_percent(used_tokens: u32, context_window: u32) -> u32 {
    if context_window == 0 {
        return 0;
    }
    100u32.saturating_sub(
        ((used_tokens as f64 / context_window as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_stays_compact_for_footer_and_live_work() {
        assert_eq!(format_elapsed_compact(0), "0s");
        assert_eq!(format_elapsed_compact(61), "1m 01s");
        assert_eq!(format_elapsed_compact(3_723), "1h 02m 03s");
    }

    #[test]
    fn remaining_percent_rounds_and_clamps() {
        assert_eq!(context_remaining_percent(25, 100), 75);
        assert_eq!(context_remaining_percent(200, 100), 0);
    }
}
