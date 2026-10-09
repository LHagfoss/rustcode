//! Minimal benchmark scoring for agent turns (P2).
//!
//! `score_turn` converts raw turn stats (rounds, tool calls, recoveries,
//! completion) into a 0–100 score so `--evolve`-style loops and humans can
//! compare runs without re-reading transcripts.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnStats {
    pub rounds: usize,
    pub tool_calls: usize,
    pub recoveries: usize,
    pub completed: bool,
}

pub fn score_turn(stats: TurnStats) -> u8 {
    let mut score: i32 = 100;
    score -= (stats.rounds.min(20) as i32) * 2;
    score -= (stats.recoveries.min(3) as i32) * 10;
    if stats.tool_calls == 0 && !stats.completed {
        score -= 20;
    }
    if stats.completed {
        score += 10;
    }
    score.clamp(0, 100) as u8
}

pub fn grade(score: u8) -> &'static str {
    match score {
        90..=100 => "A",
        75..=89 => "B",
        55..=74 => "C",
        _ => "D",
    }
}

pub fn format_report(stats: TurnStats) -> String {
    let score = score_turn(stats);
    format!(
        "benchmark score: {}/100 (grade {}) — rounds={} calls={} recoveries={} completed={}",
        score,
        grade(score),
        stats.rounds,
        stats.tool_calls,
        stats.recoveries,
        stats.completed
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_fast_completion_scores_high() {
        let score = score_turn(TurnStats {
            rounds: 2,
            tool_calls: 3,
            recoveries: 0,
            completed: true,
        });
        assert!(score >= 90, "score was {score}");
        assert_eq!(grade(score), "A");
    }

    #[test]
    fn recoveries_and_incomplete_penalize() {
        let bad = score_turn(TurnStats {
            rounds: 20,
            tool_calls: 0,
            recoveries: 3,
            completed: false,
        });
        let good = score_turn(TurnStats {
            rounds: 3,
            tool_calls: 5,
            recoveries: 0,
            completed: true,
        });
        assert!(bad < good);
        assert_eq!(grade(100), "A");
        assert_eq!(grade(80), "B");
        assert_eq!(grade(60), "C");
        assert_eq!(grade(10), "D");
    }

    #[test]
    fn report_mentions_key_stats() {
        let report = format_report(TurnStats {
            rounds: 4,
            tool_calls: 6,
            recoveries: 1,
            completed: true,
        });
        assert!(report.contains("rounds=4"));
        assert!(report.contains("completed=true"));
    }
}

/// Provider-neutral timings in microseconds. Tool work can overlap; only the
/// elapsed batch wall time is subtracted when deriving harness overhead. Time
/// a question or approval waited on the user is `user_wait_us`: it is part of
/// `wall_us` and of no other field.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TurnPerformance {
    pub wall_us: u64,
    pub context_us: u64,
    pub schema_us: u64,
    pub serialization_us: u64,
    pub model_us: u64,
    pub ttft_us: Option<u64>,
    pub tool_wall_us: u64,
    pub tool_work_us: u64,
    pub user_wait_us: u64,
    pub persistence_enqueue_us: u64,
    pub context_bytes: usize,
    pub requests: usize,
    pub rounds: usize,
    pub tool_calls: usize,
    pub recoveries: usize,
    pub replayed_reads: usize,
    pub parallel_groups: usize,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub completed: bool,
}

impl TurnPerformance {
    pub fn harness_us(&self) -> u64 {
        self.wall_us
            .saturating_sub(self.model_us)
            .saturating_sub(self.tool_wall_us)
            .saturating_sub(self.user_wait_us)
    }
    /// Record one executed tool batch: `batch_us` is its elapsed wall time,
    /// `work_us` the summed per-call execution time, and `user_wait_us` the
    /// part of the batch spent waiting on the user (an `ask_question` answer
    /// or a per-call approval), which is not tool time (#1891).
    pub(crate) fn record_tool_batch(&mut self, batch_us: u64, work_us: u64, user_wait_us: u64) {
        self.tool_wall_us += batch_us.saturating_sub(user_wait_us);
        self.tool_work_us += work_us;
        self.user_wait_us += user_wait_us;
    }
    pub fn report(&self) -> String {
        let metric = |value: Option<u64>| {
            value
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unavailable".into())
        };
        format!(
            "Turn: {:.2}s; completed={}\nModel: {:.2}ms; TTFT: {} us\nTools: {:.2}ms wall / {:.2}ms work\nUser wait: {:.2}ms\nHarness: {:.2}ms\n  Context: {:.2}ms; schema: {:.2}ms; serialization: {:.2}ms\n  Persistence enqueue: {:.2}ms\nInput: {}; output: {}; cached input: {}\nRequests: {}; rounds: {}; calls: {}; recoveries: {}\nRead replays: {}; parallel groups: {}; context: {} bytes",
            self.wall_us as f64 / 1e6,
            self.completed,
            self.model_us as f64 / 1000.,
            metric(self.ttft_us),
            self.tool_wall_us as f64 / 1000.,
            self.tool_work_us as f64 / 1000.,
            self.user_wait_us as f64 / 1000.,
            self.harness_us() as f64 / 1000.,
            self.context_us as f64 / 1000.,
            self.schema_us as f64 / 1000.,
            self.serialization_us as f64 / 1000.,
            self.persistence_enqueue_us as f64 / 1000.,
            metric(self.input_tokens),
            metric(self.output_tokens),
            metric(self.cached_input_tokens),
            self.requests,
            self.rounds,
            self.tool_calls,
            self.recoveries,
            self.replayed_reads,
            self.parallel_groups,
            self.context_bytes
        )
    }
}

pub(crate) fn elapsed_us(start: std::time::Instant) -> u64 {
    duration_us(start.elapsed())
}

pub(crate) fn duration_us(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    #[test]
    fn overlapping_tool_work_does_not_inflate_harness_time() {
        let p = TurnPerformance {
            wall_us: 1000,
            model_us: 600,
            tool_wall_us: 200,
            tool_work_us: 500,
            ..Default::default()
        };
        assert_eq!(p.harness_us(), 200);
        let value = serde_json::to_value(&p).unwrap();
        assert!(value["ttft_us"].is_null());
        assert!(p.report().contains("Harness"));
    }
    #[test]
    fn user_wait_is_reported_apart_from_tool_and_harness_time() {
        // Session 01a11ffc: a 1,014 s question inside a batch of sub-second
        // tools was reported as 1,014 s of tool time (#1891).
        let mut p = TurnPerformance {
            wall_us: 1_020_000_000,
            model_us: 4_000_000,
            ..Default::default()
        };
        p.record_tool_batch(1_014_500_000, 400_000, 1_014_000_000);
        p.record_tool_batch(300_000, 300_000, 0);
        assert_eq!(p.user_wait_us, 1_014_000_000);
        assert_eq!(p.tool_wall_us, 800_000);
        assert_eq!(p.tool_work_us, 700_000);
        assert_eq!(p.harness_us(), 1_200_000);
        assert!(p.report().contains("User wait: 1014000.00ms"));
        let value = serde_json::to_value(&p).unwrap();
        assert_eq!(value["user_wait_us"], 1_014_000_000u64);
        // Reports written before the field existed still load.
        let old: TurnPerformance =
            serde_json::from_str(r#"{"wall_us":5,"tool_wall_us":2}"#).unwrap();
        assert_eq!(old.user_wait_us, 0);
    }
    #[test]
    fn unavailable_cache_usage_stays_unknown() {
        let p = TurnPerformance::default();
        assert!(p.report().contains("unavailable"));
    }
}

/// Compare actual telemetry files without using the legacy synthetic grade.
pub fn compare_reports(before: &TurnPerformance, after: &TurnPerformance) -> String {
    let change = |a: u64, b: u64| {
        if a == 0 {
            "unavailable".into()
        } else {
            format!("{:+.1}%", (b as f64 / a as f64 - 1.) * 100.)
        }
    };
    format!(
        "Wall: {}\nModel: {}\nHarness: {}\nTool wall: {}\nInput tokens: {}\nCalls: {} → {}\nSuccess: {} → {}",
        change(before.wall_us, after.wall_us),
        change(before.model_us, after.model_us),
        change(before.harness_us(), after.harness_us()),
        change(before.tool_wall_us, after.tool_wall_us),
        before
            .input_tokens
            .zip(after.input_tokens)
            .map(|(a, b)| change(a, b))
            .unwrap_or_else(|| "unavailable".into()),
        before.tool_calls,
        after.tool_calls,
        before.completed,
        after.completed
    )
}
