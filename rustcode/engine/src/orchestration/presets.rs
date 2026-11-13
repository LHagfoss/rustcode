//! Delegate presets (#3391): named sets of `start_session` / `fan_out`
//! arguments the user keeps in Settings › Harnesses › Delegate presets, stored
//! as the `delegate-presets` server setting (`acp/settings.rs`).
//!
//! A call names one with `preset`; the engine expands it here, before the call
//! is forwarded to the app or answered with no app attached, so both paths and
//! every client version see ordinary arguments. An argument the call passes
//! itself wins over the preset's; the preset's `instructions` go before the
//! prompt, with a blank line between. In `fan_out` an entry's `preset` gives
//! that entry's harness, model, effort and account, and a top-level `preset`
//! gives the shared prompt's instructions and the access (each `fan_out` thread
//! has its own worktree anyway).

use serde::Deserialize;
use serde_json::{Map, Value};

/// The setting the presets live in.
pub const SETTING: &str = "delegate-presets";
/// The argument a call names a preset with.
pub const ARG: &str = "preset";

/// One preset as Settings saves it. Every field but the name is optional.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Preset {
    pub name: String,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    /// `ask`, `edits` or `full`: what the thread starts at, never more than the
    /// caller's own (the app lowers it, `state/host.ts`).
    #[serde(default)]
    pub access: Option<String>,
    #[serde(default)]
    pub worktree: Option<bool>,
    #[serde(default)]
    pub instructions: Option<String>,
}

/// The fields a preset can give one thread's harness choice.
const HARNESS_KEYS: [&str; 4] = ["harness", "model", "effort", "account"];

/// The presets this machine has saved. An entry without a name, or one that
/// does not parse, is skipped: a file edited by hand must not break a call.
pub fn saved() -> Vec<Preset> {
    let Some(Value::Array(entries)) = crate::acp::settings::value(SETTING) else {
        return Vec::new();
    };
    parse(&entries)
}

fn parse(entries: &[Value]) -> Vec<Preset> {
    entries
        .iter()
        .filter_map(|entry| serde_json::from_value::<Preset>(entry.clone()).ok())
        .filter(|preset| !preset.name.trim().is_empty())
        .collect()
}

/// `args` with any `preset` replaced by what it names, or `None` when the call
/// names none. An unknown name is an error that lists the saved ones.
pub fn expand(name: &str, args: &Value) -> Result<Option<Value>, String> {
    if !names_a_preset(name, args) {
        return Ok(None);
    }
    expand_with(&saved(), name, args).map(Some)
}

fn names_a_preset(name: &str, args: &Value) -> bool {
    match name {
        super::START_SESSION => args.get(ARG).is_some(),
        super::FAN_OUT => {
            args.get(ARG).is_some()
                || args
                    .get("harnesses")
                    .and_then(Value::as_array)
                    .is_some_and(|entries| entries.iter().any(|entry| entry.get(ARG).is_some()))
        }
        _ => false,
    }
}

pub(crate) fn expand_with(presets: &[Preset], name: &str, args: &Value) -> Result<Value, String> {
    let mut out = args.clone();
    let Some(object) = out.as_object_mut() else {
        return Ok(out);
    };
    let top = take_preset(presets, object)?;
    if let Some(preset) = top {
        if name == super::START_SESSION {
            fill_harness(object, preset);
            if let Some(worktree) = preset.worktree
                && matches!(object.get("worktree"), None | Some(Value::Null))
            {
                object.insert("worktree".to_owned(), Value::Bool(worktree));
            }
        }
        if let Some(access) = preset
            .access
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            && unset(object.get("access"))
        {
            object.insert("access".to_owned(), Value::String(access.to_owned()));
        }
        if let Some(instructions) = preset
            .instructions
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            let prompt = object.get("prompt").and_then(Value::as_str).unwrap_or("");
            object.insert(
                "prompt".to_owned(),
                Value::String(format!("{instructions}\n\n{prompt}")),
            );
        }
    }
    if name == super::FAN_OUT
        && let Some(Value::Array(entries)) = object.get_mut("harnesses")
    {
        for entry in entries.iter_mut() {
            if let Some(entry) = entry.as_object_mut()
                && let Some(preset) = take_preset(presets, entry)?
            {
                fill_harness(entry, preset);
            }
        }
    }
    Ok(out)
}

/// Remove `preset` from `object` and find what it names.
fn take_preset<'a>(
    presets: &'a [Preset],
    object: &mut Map<String, Value>,
) -> Result<Option<&'a Preset>, String> {
    let Some(asked) = object.remove(ARG) else {
        return Ok(None);
    };
    let asked = match &asked {
        Value::Null => return Ok(None),
        Value::String(asked) => asked.trim(),
        _ => return Err("preset must be a preset's name".to_owned()),
    };
    if asked.is_empty() {
        return Ok(None);
    }
    presets
        .iter()
        .find(|preset| preset.name.trim().eq_ignore_ascii_case(asked))
        .map(Some)
        .ok_or_else(|| unknown(presets, asked))
}

fn unknown(presets: &[Preset], asked: &str) -> String {
    if presets.is_empty() {
        return format!(
            "there is no preset named {asked}: no delegate presets are saved (Settings › Harnesses › Delegate presets)"
        );
    }
    format!(
        "there is no preset named {asked}; the saved presets are {}",
        names(presets).join(", ")
    )
}

/// Give `object` the preset's harness fields it does not set itself.
fn fill_harness(object: &mut Map<String, Value>, preset: &Preset) {
    for key in HARNESS_KEYS {
        let value = match key {
            "harness" => &preset.harness,
            "model" => &preset.model,
            "effort" => &preset.effort,
            _ => &preset.account,
        };
        let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        if unset(object.get(key)) {
            object.insert(key.to_owned(), Value::String(value.to_owned()));
        }
    }
}

/// Whether the call left an argument out: missing, null or blank.
fn unset(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(own)) => own.trim().is_empty(),
        Some(_) => false,
    }
}

pub fn names(presets: &[Preset]) -> Vec<String> {
    presets.iter().map(|preset| preset.name.clone()).collect()
}

/// The note `list_harnesses` ends with, so an agent learns the names.
pub fn listing(presets: &[Preset]) -> Option<String> {
    if presets.is_empty() {
        return None;
    }
    let lines: Vec<String> = presets
        .iter()
        .map(|preset| {
            let mut parts: Vec<String> = Vec::new();
            for (label, value) in [
                ("harness", &preset.harness),
                ("model", &preset.model),
                ("effort", &preset.effort),
                ("account", &preset.account),
                ("access", &preset.access),
            ] {
                if let Some(value) = value.as_deref().filter(|v| !v.trim().is_empty()) {
                    parts.push(format!("{label} {value}"));
                }
            }
            if let Some(worktree) = preset.worktree {
                parts.push(format!("worktree {worktree}"));
            }
            if preset
                .instructions
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
            {
                parts.push("with instructions".to_owned());
            }
            if parts.is_empty() {
                format!("- {}", preset.name)
            } else {
                format!("- {}: {}", preset.name, parts.join(", "))
            }
        })
        .collect();
    Some(format!(
        "Delegate presets (pass `preset` to start_session, or on a fan_out entry; arguments you pass override it):\n{}",
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn presets() -> Vec<Preset> {
        parse(&[
            json!({
                "name": "Pi flash",
                "harness": "pi",
                "model": "deepseek/deepseek-flash",
                "effort": "high",
                "access": "edits",
                "worktree": true,
                "instructions": "Read AGENTS.md first."
            }),
            json!({ "name": "codex", "harness": "codex", "account": "work" }),
            json!({ "name": "  ", "harness": "nameless" }),
            json!("not a preset"),
        ])
    }

    #[test]
    fn a_preset_fills_what_the_call_leaves_out_and_prepends_its_instructions() {
        let args = json!({ "prompt": "fix the login", "preset": "pi flash" });
        let out = expand_with(&presets(), super::super::START_SESSION, &args).unwrap();
        assert_eq!(
            out,
            json!({
                "prompt": "Read AGENTS.md first.\n\nfix the login",
                "harness": "pi",
                "model": "deepseek/deepseek-flash",
                "effort": "high",
                "access": "edits",
                "worktree": true,
            })
        );
    }

    #[test]
    fn explicit_arguments_override_the_preset() {
        let args = json!({
            "prompt": "fix it",
            "preset": "Pi flash",
            "model": "deepseek/deepseek-pro",
            "worktree": false,
            "effort": "",
        });
        let out = expand_with(&presets(), super::super::START_SESSION, &args).unwrap();
        assert_eq!(out["harness"], "pi");
        assert_eq!(out["model"], "deepseek/deepseek-pro");
        assert_eq!(out["worktree"], false);
        assert_eq!(out["effort"], "high", "an empty argument is not a choice");
        assert!(out.get("preset").is_none());
    }

    #[test]
    fn an_unknown_preset_is_an_error_that_names_the_saved_ones() {
        let args = json!({ "prompt": "x", "preset": "opus" });
        let error = expand_with(&presets(), super::super::START_SESSION, &args).unwrap_err();
        assert!(error.contains("opus"), "{error}");
        assert!(error.contains("Pi flash, codex"), "{error}");
        let none = expand_with(&[], super::super::START_SESSION, &args).unwrap_err();
        assert!(none.contains("no delegate presets are saved"), "{none}");
        let wrong = json!({ "prompt": "x", "preset": 3 });
        assert!(expand_with(&presets(), super::super::START_SESSION, &wrong).is_err());
    }

    #[test]
    fn a_fan_out_entry_takes_its_own_preset_and_the_top_level_one_gives_instructions() {
        let args = json!({
            "prompt": "compare",
            "preset": "Pi flash",
            "harnesses": [
                { "preset": "codex" },
                { "preset": "codex", "account": "home" },
                { "harness": "claude-code" },
            ],
        });
        let out = expand_with(&presets(), super::super::FAN_OUT, &args).unwrap();
        assert_eq!(out["prompt"], "Read AGENTS.md first.\n\ncompare");
        assert_eq!(out["access"], "edits");
        assert!(
            out.get("worktree").is_none(),
            "every fan_out thread has a worktree"
        );
        assert!(
            out.get("harness").is_none(),
            "fan_out has no top-level harness"
        );
        assert_eq!(
            out["harnesses"],
            json!([
                { "harness": "codex", "account": "work" },
                { "harness": "codex", "account": "home" },
                { "harness": "claude-code" },
            ])
        );
        let bad =
            json!({ "prompt": "x", "harnesses": [{ "preset": "nope" }, { "harness": "pi" }] });
        assert!(expand_with(&presets(), super::super::FAN_OUT, &bad).is_err());
    }

    #[test]
    fn only_a_call_that_names_a_preset_is_expanded() {
        let plain = json!({ "prompt": "x", "harness": "pi" });
        assert_eq!(expand(super::super::START_SESSION, &plain), Ok(None));
        assert_eq!(
            expand(super::super::MESSAGE_SESSION, &json!({ "preset": "x" })),
            Ok(None)
        );
    }

    #[test]
    fn the_listing_names_each_preset_and_what_it_sets() {
        let text = listing(&presets()).unwrap();
        assert!(text.contains("- Pi flash: harness pi, model deepseek/deepseek-flash, effort high, access edits, worktree true, with instructions"), "{text}");
        assert!(
            text.contains("- codex: harness codex, account work"),
            "{text}"
        );
        assert!(!text.contains("nameless"));
        assert_eq!(listing(&[]), None);
    }
}
