//! An opt-in cost estimate for `rc usage --pull-requests --prices`
//! (#3934). RustCode ships no prices: the user's file names its source and
//! date, and the estimate is printed apart from the cost the harness
//! reported, which it never replaces.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// A price file: where the prices come from, when they were read, and USD
/// per million tokens per model.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PriceTable {
    pub source: String,
    pub as_of: String,
    pub models: BTreeMap<String, ModelPrice>,
}

/// USD per million tokens. `cached_input` defaults to `input`, and
/// `cache_write` to `input`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cached_input: Option<f64>,
    pub cache_write: Option<f64>,
}

impl PriceTable {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let table: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        if table.source.trim().is_empty() {
            return Err("the price file must name its `source`".to_owned());
        }
        if chrono::NaiveDate::parse_from_str(table.as_of.trim(), "%Y-%m-%d").is_err() {
            return Err("`as_of` must be the day the prices were read, YYYY-MM-DD".to_owned());
        }
        for (model, price) in &table.models {
            let rates = [
                Some(price.input),
                Some(price.output),
                price.cached_input,
                price.cache_write,
            ];
            if rates
                .into_iter()
                .flatten()
                .any(|rate| !rate.is_finite() || rate < 0.0)
            {
                return Err(format!("`{model}` has a negative or invalid price"));
            }
        }
        Ok(table)
    }

    /// The estimated USD for one `pr_outcomes_by_model` row's summed tokens,
    /// or `None` when its model has no price or it has no token counts.
    /// Codex's `input_tokens` include the cached ones; the other harnesses
    /// count cache reads and writes apart from input.
    pub(crate) fn estimate(&self, row: &serde_json::Value) -> Option<f64> {
        let model = row.get("model")?.as_str()?;
        let price = self.models.get(model)?;
        let tokens = |key: &str| row.get(key).and_then(serde_json::Value::as_f64);
        let input = tokens("input_tokens");
        let output = tokens("output_tokens");
        if input.is_none() && output.is_none() {
            return None;
        }
        let input = input.unwrap_or(0.0);
        let cached = tokens("cached_tokens").unwrap_or(0.0);
        let written = tokens("cache_write_tokens").unwrap_or(0.0);
        let codex = row.get("harness").and_then(serde_json::Value::as_str) == Some("codex");
        let (uncached, written) = if codex {
            ((input - cached).max(0.0), 0.0)
        } else {
            (input, written)
        };
        let per_token = |rate: f64| rate / 1_000_000.0;
        Some(
            uncached * per_token(price.input)
                + cached * per_token(price.cached_input.unwrap_or(price.input))
                + written * per_token(price.cache_write.unwrap_or(price.input))
                + output.unwrap_or(0.0) * per_token(price.output),
        )
    }

    /// Adds `estimated_cost_usd` and `estimated_cost_per_merged_pr` to each
    /// row it can price, and returns how many it priced.
    pub(crate) fn annotate(&self, rows: &mut [serde_json::Value]) -> usize {
        let mut priced = 0;
        for row in rows {
            let Some(cost) = self.estimate(row) else {
                continue;
            };
            priced += 1;
            let merged = row
                .get("merged")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if let Some(object) = row.as_object_mut() {
                object.insert("estimated_cost_usd".to_owned(), cost.into());
                if merged > 0 {
                    object.insert(
                        "estimated_cost_per_merged_pr".to_owned(),
                        (cost / merged as f64).into(),
                    );
                }
            }
        }
        priced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FILE: &str = r#"
source = "https://openai.com/api/pricing"
as_of = "2026-11-17"

[models."gpt-5-codex"]
input = 1.25
cached_input = 0.125
output = 10.0

[models."claude-opus"]
input = 5.0
cached_input = 0.5
cache_write = 6.25
output = 25.0
"#;

    #[test]
    fn a_price_file_must_name_its_source_and_date() {
        assert!(PriceTable::parse(FILE).is_ok());
        assert!(PriceTable::parse(&FILE.replace("https://openai.com/api/pricing", " ")).is_err());
        assert!(PriceTable::parse(&FILE.replace("2026-11-17", "last week")).is_err());
        assert!(PriceTable::parse("source = \"x\"\n[models]\n").is_err());
        assert!(PriceTable::parse(&FILE.replace("input = 1.25", "input = -1.0")).is_err());
    }

    #[test]
    fn codex_input_includes_cached_tokens_and_unpriced_models_stay_blank() {
        let table = PriceTable::parse(FILE).unwrap();
        let mut rows = vec![
            json!({"harness": "codex", "model": "gpt-5-codex", "merged": 2,
                   "input_tokens": 2_000_000, "cached_tokens": 1_000_000,
                   "output_tokens": 100_000, "cost_usd": null}),
            json!({"harness": "claude-code", "model": "claude-opus", "merged": 0,
                   "input_tokens": 1_000_000, "cached_tokens": 1_000_000,
                   "cache_write_tokens": 1_000_000, "output_tokens": 0,
                   "cost_usd": 3.0}),
            json!({"harness": "codex", "model": "unpriced", "merged": 1,
                   "input_tokens": 5, "output_tokens": 5}),
        ];
        assert_eq!(table.annotate(&mut rows), 2);
        let uncached_cached_output = 1.0 * 1.25 + 1.0 * 0.125 + 0.1 * 10.0;
        let codex = rows[0]["estimated_cost_usd"].as_f64().unwrap();
        assert!((codex - uncached_cached_output).abs() < 1e-9);
        let per_merged = rows[0]["estimated_cost_per_merged_pr"].as_f64().unwrap();
        assert!((per_merged - 1.1875).abs() < 1e-9);
        let claude = rows[1]["estimated_cost_usd"].as_f64().unwrap();
        assert!((claude - 11.75).abs() < 1e-9);
        assert!(rows[1].get("estimated_cost_per_merged_pr").is_none());
        assert_eq!(rows[1]["cost_usd"], 3.0, "the reported cost is untouched");
        assert!(rows[2].get("estimated_cost_usd").is_none());
    }
}
