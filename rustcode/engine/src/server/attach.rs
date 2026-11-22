//! `rc server connect --or-standalone`: how the desktop app reaches an engine.
//!
//! The app's engine is the user's one RustCode server when it can be
//! (`docs/server.md` › The app's engine): the app relays to it, so a thread
//! the app started keeps running after the app closes, and a terminal or
//! a paired device sees the same threads. When joining it would mean talking
//! to a different build, the app gets the engine it always had instead, a
//! standalone `--acp` engine that ends with it.

use crate::service::server_service::{self, LiveStatus};
use std::path::Path;

/// What the app's engine process does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachPlan {
    /// Relay to the server that runs: it is this build.
    Attach,
    /// No server runs: start one from this build, then relay to it.
    Start,
    /// A server of another version runs that nobody uses and no service
    /// manager keeps: stop it, then start one from this build.
    Replace { pid: u32 },
    /// Run a standalone engine, for the reason given.
    Standalone(String),
}

/// Decide from this build's version, the running server (if any) and
/// whether a service manager runs it. A `development` (debug) build carries
/// the release's version number but not its code, so it never joins or
/// replaces the user's real server.
pub fn plan_attach(
    own_version: &str,
    development: bool,
    live: Option<&LiveStatus>,
    installed: bool,
) -> AttachPlan {
    if development {
        return AttachPlan::Standalone("this is a development build".to_owned());
    }
    let Some(live) = live else {
        return AttachPlan::Start;
    };
    if live.version == own_version {
        return AttachPlan::Attach;
    }
    if !installed && live.clients == 0 && live.running_turns == 0 {
        return AttachPlan::Replace { pid: live.pid };
    }
    AttachPlan::Standalone(format!(
        "the RustCode server runs version {}, this is {own_version}",
        live.version
    ))
}

/// Relay stdin and stdout to the server when the plan allows; `Ok(false)`
/// when the caller should run a standalone engine instead. Nothing has been
/// relayed by then, so the standalone engine starts from a clean stream.
pub async fn connect_or_standalone(program: &Path) -> anyhow::Result<bool> {
    let live = server_service::live_status().await;
    let plan = plan_attach(
        env!("CARGO_PKG_VERSION"),
        cfg!(debug_assertions),
        live.as_ref(),
        server_service::installed(),
    );
    let stream = match plan {
        AttachPlan::Attach => super::connect_running().await.map_err(anyhow::Error::from),
        AttachPlan::Start => super::start_and_connect(program).await,
        AttachPlan::Replace { pid } => {
            eprintln!(
                "rc: replacing the idle RustCode server (pid {pid}, version {}) with {}",
                live.as_ref().map_or("?", |live| live.version.as_str()),
                env!("CARGO_PKG_VERSION")
            );
            match server_service::stop_server(pid).await {
                Ok(()) => super::start_and_connect(program).await,
                Err(error) => Err(error),
            }
        }
        AttachPlan::Standalone(reason) => {
            eprintln!("rc: running a standalone engine: {reason}");
            return Ok(false);
        }
    };
    match stream {
        Ok(stream) => {
            eprintln!("rc: relaying to the RustCode server");
            super::bridge(stream, tokio::io::stdin(), tokio::io::stdout()).await?;
            Ok(true)
        }
        Err(error) => {
            eprintln!("rc: running a standalone engine: could not reach the server: {error:#}");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(version: &str, clients: usize, running_turns: usize) -> LiveStatus {
        LiveStatus {
            version: version.to_owned(),
            pid: 42,
            clients,
            running_turns,
        }
    }

    #[test]
    fn a_development_build_stays_standalone() {
        assert!(matches!(
            plan_attach("1.0.0", true, Some(&live("1.0.0", 0, 0)), false),
            AttachPlan::Standalone(_)
        ));
    }

    #[test]
    fn no_server_starts_one() {
        assert_eq!(plan_attach("1.0.0", false, None, false), AttachPlan::Start);
    }

    #[test]
    fn the_same_version_attaches() {
        assert_eq!(
            plan_attach("1.0.0", false, Some(&live("1.0.0", 3, 1)), true),
            AttachPlan::Attach
        );
    }

    #[test]
    fn an_idle_unmanaged_server_of_another_version_is_replaced() {
        assert_eq!(
            plan_attach("1.1.0", false, Some(&live("1.0.0", 0, 0)), false),
            AttachPlan::Replace { pid: 42 }
        );
    }

    #[test]
    fn a_busy_or_managed_server_of_another_version_is_left_alone() {
        for (clients, turns, installed) in [(1, 0, false), (0, 1, false), (0, 0, true)] {
            assert!(matches!(
                plan_attach(
                    "1.1.0",
                    false,
                    Some(&live("1.0.0", clients, turns)),
                    installed
                ),
                AttachPlan::Standalone(_)
            ));
        }
    }
}
