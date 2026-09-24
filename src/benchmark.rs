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
