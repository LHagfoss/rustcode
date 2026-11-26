//! A run's spend limits: the tokens its workers' harnesses reported and the
//! USD the ones that report cost said (`data_store::run_usage`). Settings
//! `workflow-token-budget` and `workflow-cost-budget-usd` (0 or unset is no
//! limit); a call can lower them, never raise them. A budget that is reached
//! stops new `agent()` calls; workers already running go on to their end.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::data_store::run_usage::ThreadsUsage;

/// The most a token budget may be set to.
pub(crate) const MAX_TOKEN_BUDGET: u64 = 10_000_000_000;
/// The most a cost budget may be set to, in USD.
pub(crate) const MAX_COST_BUDGET_USD: f64 = 100_000.0;

/// What a run may spend; `None` is no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Budget {
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

impl Budget {
    fn is_unlimited(self) -> bool {
        self.tokens.is_none() && self.cost_usd.is_none()
    }
}

/// The user's budgets, read through `read`.
pub(super) fn configured(read: impl Fn(&str) -> Option<Value>) -> Budget {
    Budget {
        tokens: read("workflow-token-budget")
            .and_then(|value| value.as_u64())
            .filter(|tokens| *tokens > 0)
            .map(|tokens| tokens.min(MAX_TOKEN_BUDGET)),
        cost_usd: read("workflow-cost-budget-usd")
            .and_then(|value| value.as_f64())
            .filter(|cost| cost.is_finite() && *cost > 0.0)
            .map(|cost| cost.min(MAX_COST_BUDGET_USD)),
    }
}

/// `configured` lowered by what the call asked for.
pub(super) fn lowered(configured: Budget, args: &Value) -> Budget {
    let tokens = args
        .get("token_budget")
        .and_then(Value::as_u64)
        .filter(|tokens| *tokens > 0);
    let cost = args
        .get("cost_budget_usd")
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost > 0.0);
    Budget {
        tokens: match (configured.tokens, tokens) {
            (Some(set), Some(asked)) => Some(set.min(asked)),
            (set, asked) => set.or(asked),
        },
        cost_usd: match (configured.cost_usd, cost) {
            (Some(set), Some(asked)) => Some(set.min(asked)),
            (set, asked) => set.or(asked),
        },
    }
}

/// Check `token_budget` and `cost_budget_usd` in a `run_workflow` call.
pub(super) fn validate(args: &serde_json::Map<String, Value>) -> Result<(), String> {
    match args.get("token_budget") {
        None | Some(Value::Null) => {}
        Some(value) => match value.as_u64() {
            Some(tokens) if (1..=MAX_TOKEN_BUDGET).contains(&tokens) => {}
            _ => {
                return Err(format!(
                    "token_budget must be a whole number from 1 to {MAX_TOKEN_BUDGET}"
                ));
            }
        },
    }
    match args.get("cost_budget_usd") {
        None | Some(Value::Null) => Ok(()),
        Some(value) => match value.as_f64() {
            Some(cost) if cost > 0.0 && cost <= MAX_COST_BUDGET_USD => Ok(()),
            _ => Err(format!(
                "cost_budget_usd must be a number above 0, at most {MAX_COST_BUDGET_USD}"
            )),
        },
    }
}

/// The two arguments as `run_workflow`'s schema lists them.
pub(super) fn schema_properties() -> [(&'static str, Value); 2] {
    [
        (
            "token_budget",
            json!({"type": "integer", "minimum": 1, "maximum": MAX_TOKEN_BUDGET, "description": "Start no agent() call once workers used this many tokens; at most the user's setting"}),
        ),
        (
            "cost_budget_usd",
            json!({"type": "number", "exclusiveMinimum": 0, "maximum": MAX_COST_BUDGET_USD, "description": "The same for USD the harnesses reported"}),
        ),
    ]
}

/// Which budget `usage` reached, if any.
fn reached(budget: Budget, usage: &ThreadsUsage) -> Option<&'static str> {
    if budget.tokens.is_some_and(|tokens| usage.tokens >= tokens) {
        return Some("tokens");
    }
    if budget.cost_usd.is_some_and(|cost| usage.cost_usd >= cost) {
        return Some("cost");
    }
    None
}

/// What the run's workers spent so far; nothing when the store cannot be read.
pub(super) fn spent(workers: &[String]) -> ThreadsUsage {
    crate::data_store::run_usage::threads_usage(workers).unwrap_or_else(|error| {
        crate::dbg_log!("workflow usage: {error}");
        ThreadsUsage {
            threads: workers.len(),
            ..ThreadsUsage::default()
        }
    })
}

/// Why no further `agent()` call may start, if a budget is reached. Reads the
/// store only when the run has a budget.
pub(super) fn refusal(budget: Budget, workers: impl FnOnce() -> Vec<String>) -> Option<String> {
    if budget.is_unlimited() {
        return None;
    }
    let usage = spent(&workers());
    Some(match reached(budget, &usage)? {
        "tokens" => format!(
            "the run reached its token budget of {} tokens ({} used); no further agent() call starts, running workers finish",
            budget.tokens.unwrap_or_default(),
            usage.tokens
        ),
        _ => format!(
            "the run reached its cost budget of ${:.2} reported (${:.2} so far); no further agent() call starts, running workers finish",
            budget.cost_usd.unwrap_or_default(),
            usage.cost_usd
        ),
    })
}

/// The `budget` and `usage` of a `workflow_status` answer.
pub(super) fn report(budget: Budget, workers: &[String]) -> (Value, Value) {
    let usage = spent(workers);
    let budget_value = json!({
        "tokens": budget.tokens,
        "cost_usd": budget.cost_usd,
        "reached": reached(budget, &usage),
    });
    let usage_value = json!({
        "tokens": usage.tokens,
        "cost_usd": (usage.threads_with_cost > 0).then_some(usage.cost_usd),
        "cost_reported_by": format!("{} of {} workers", usage.threads_with_cost, usage.threads),
    });
    (budget_value, usage_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_set_the_budgets_and_a_call_can_only_lower_them() {
        let set = configured(|key| match key {
            "workflow-token-budget" => Some(json!(1_000)),
            "workflow-cost-budget-usd" => Some(json!(0)),
            _ => None,
        });
        assert_eq!(
            set,
            Budget {
                tokens: Some(1_000),
                cost_usd: None
            }
        );
        let asked = lowered(set, &json!({"token_budget": 5_000, "cost_budget_usd": 2.5}));
        assert_eq!(asked.tokens, Some(1_000));
        assert_eq!(asked.cost_usd, Some(2.5));
        let lower = lowered(set, &json!({"token_budget": 10}));
        assert_eq!(lower.tokens, Some(10));
    }

    #[test]
    fn a_reached_budget_names_itself_and_an_unlimited_run_never_reads_the_store() {
        let usage = ThreadsUsage {
            tokens: 500,
            cost_usd: 1.0,
            threads_with_cost: 1,
            threads: 2,
        };
        let tokens = Budget {
            tokens: Some(500),
            cost_usd: None,
        };
        assert_eq!(reached(tokens, &usage), Some("tokens"));
        let cost = Budget {
            tokens: Some(10_000),
            cost_usd: Some(0.5),
        };
        assert_eq!(reached(cost, &usage), Some("cost"));
        assert_eq!(reached(Budget::default(), &usage), None);
        assert!(refusal(Budget::default(), || unreachable!()).is_none());
    }

    #[test]
    fn validation_refuses_a_budget_out_of_range() {
        let check = |args: Value| validate(args.as_object().unwrap());
        assert!(check(json!({"token_budget": 100, "cost_budget_usd": 1.5})).is_ok());
        assert!(check(json!({"token_budget": 0})).is_err());
        assert!(check(json!({"cost_budget_usd": -1})).is_err());
        assert!(check(json!({"cost_budget_usd": "five"})).is_err());
    }
}
