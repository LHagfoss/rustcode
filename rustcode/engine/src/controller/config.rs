//! Config read/mutate contract for frontends (issue #1440).
//!
//! File format, validation, and merge rules stay in `crate::config`
//! untouched. This module declares which config shapes and operations are
//! frontend-visible so settings flows, theme setup, and the future `serve`
//! transport go through one seam instead of each reaching into
//! `crate::config` directly.
//!
//! The CLI entry (`rustcode/tui/src/run.rs`) intentionally keeps its direct
//! `crate::config` uses: it boots process-level config (workspace load,
//! project init, legacy migration, sync) before any controller session
//! exists, so there is no session to route through.

use std::path::PathBuf;

pub use crate::config::{
    AgentMode, ApiProtocol, AppConfig, ModelProfile, MonthlyUsage, SandboxMode, SessionMeta,
    ToolProtocol,
};

/// The effective shell permissions for a mode, as shown in the status line,
/// the welcome banner, and `/status`. Frontends render this string; they must
/// not match on mode variants, which would pin them to engine internals.
pub fn sandbox_effective_description(mode: SandboxMode) -> &'static str {
    mode.effective_description()
}

/// Persist `config` to disk, preserving project overrides like the
/// settings and MCP flows expect. No-op for invalid configs.
pub fn save_config(config: &AppConfig) {
    crate::config::save_entire_config(config);
}

/// Directory holding `config.toml`, themes, and history. Used for
/// startup/path needs (theme setup) before a session is active.
pub fn config_dir() -> Option<PathBuf> {
    crate::config::get_config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_matches_the_engine_source() {
        assert_eq!(config_dir(), crate::config::get_config_dir());
        assert!(config_dir().is_some());
    }

    #[test]
    fn save_config_round_trips_through_the_contract() {
        // `#[cfg(test)]` isolates the config dir per thread, so this never
        // touches the real user config.
        let mut config = AppConfig::default();
        config.agent_mode = AgentMode::Plan;
        save_config(&config);

        let dir = config_dir().expect("test config dir");
        assert!(dir.join("config.toml").exists());
        let (_, _, reloaded) =
            crate::config::load_config_for_workspace(&std::env::current_dir().unwrap());
        assert_eq!(reloaded.agent_mode, AgentMode::Plan);
    }
}
