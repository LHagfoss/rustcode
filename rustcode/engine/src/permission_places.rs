//! Where the user changes each permission, in the words a refusal uses.
//!
//! A refusal at any level (thread access, a delegate account, a management
//! write, an outside agent, the command guard, a harness's own check) says
//! what was refused, which layer refused it, and the one place that allows
//! it (`docs/permissions.md`). The places are named here once, so a renamed
//! Settings page is renamed in every refusal, and a test pins each to the
//! label the app shows.

/// The thread's own access level: the composer menu, and the default.
pub const THREAD_ACCESS: &str = "the thread's Permissions menu under its composer in the RustCode app (or Settings › Permissions › Default access for new threads)";

/// The switch that lets delegates spend named accounts without a card.
pub const TRUST_ACCOUNTS: &str =
    "Settings › Delegation › “Let delegates use my named accounts without asking”";

/// Where every pending approval is answered.
pub const NEEDS_YOU: &str = "“Needs you” at the top of the RustCode sidebar";

/// Remembered "Allow for This Thread" answers, to review or revoke.
pub const REMEMBERED: &str = "Settings › Permissions › Remembered approvals";

/// An outside agent's own grant.
pub const OUTSIDE_AGENTS: &str = "Settings › Outside agents";

/// A refusal in the one shape: what, which layer, and where to change it.
/// `what` and `layer` are sentences without a final period; `change` names a
/// place from this module (or a command) and what to do there.
pub fn refusal(what: &str, layer: &str, change: &str) -> String {
    format!("{what}. Refused by {layer}. To allow it: {change}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_names_what_the_layer_and_the_place() {
        let text = refusal(
            "delegate account “work” on codex was not started",
            "the delegate account check",
            TRUST_ACCOUNTS,
        );
        assert_eq!(
            text,
            "delegate account “work” on codex was not started. Refused by the delegate account check. To allow it: Settings › Delegation › “Let delegates use my named accounts without asking”."
        );
    }

    #[test]
    fn each_place_matches_the_label_the_app_shows() {
        let registry = include_str!("../../../apps/desktop/src/state/settingsRegistry.ts");
        assert!(registry.contains("\"Let delegates use my named accounts without asking\""));
        let index = include_str!("../../../apps/desktop/src/ui/settingsIndex.ts");
        for title in ["Default access", "Remembered approvals"] {
            assert!(index.contains(&format!("title: \"{title}\"")), "{title}");
        }
        let settings = include_str!("../../../apps/desktop/src/ui/Settings.tsx");
        for label in ["\"Orchestration\"", "\"Permissions\"", "\"Outside agents\""] {
            assert!(settings.contains(label), "{label}");
        }
        let inbox = include_str!("../../../apps/desktop/src/ui/Inbox.tsx");
        assert!(inbox.contains(">Needs you<"));
        let composer = include_str!("../../../apps/desktop/src/ui/ComposerFooter.tsx");
        assert!(composer.contains("aria-label=\"Permissions\""));
    }
}
