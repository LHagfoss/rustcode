//! A server that keeps itself up to date (`docs/server.md` › Staying up to
//! date).
//!
//! A machine nobody sits at, a PC running `rustcode server` as a background
//! service, used to stay on the version it was installed with: one ran 1.31.0
//! while the Mac ran 1.31.3, and a bug fixed in 1.31.1 kept biting there. So a
//! server that runs as a service looks for a newer stable release about every
//! half hour. When it finds one it waits until no turn is running (a turn that
//! asks the user something is a running turn, so nothing waits on a person
//! either), installs it exactly as `rustcode --update` does (the release
//! archive is checked against `SHA256SUMS`; Homebrew installs go through
//! `brew`), and restarts onto it through `rustcode server restart --when-idle`,
//! which is safe to start from inside the server. The remote gateway comes
//! back on the new version by itself: the restarted server replaces an older
//! gateway (`remote_gateway::host`).
//!
//! It never installs a prerelease, never runs from a development build or
//! for a prerelease build, and `[server] auto_update = false` turns it off
//! without a restart. A failure is logged and retried after a longer pause
//! each time, never in a loop.

use crate::service::server_service as service;
use crate::update::{current_version, latest_available_version, run_update};
use rustcode_core::update::{Version, format_version};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the server looks for a release.
pub(crate) const PERIOD: Duration = Duration::from_secs(30 * 60);
/// The longest pause after failures.
const MAX_BACKOFF: Duration = Duration::from_secs(6 * 60 * 60);
/// The first look comes a little after start-up, not with it.
const FIRST_LOOK: Duration = Duration::from_secs(2 * 60);
/// How often a found release waits to see whether the server is idle.
const IDLE_POLL: Duration = Duration::from_secs(10);

/// What the server knows when it looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Facts {
    /// `[server] auto_update`.
    pub enabled: bool,
    /// This binary is a development build or a prerelease: neither updates
    /// itself.
    pub build: Build,
    /// The version running now.
    pub current: Version,
    /// The newest stable release, once looked up. A prerelease is never one.
    pub latest: Option<Version>,
    /// A version this server already wrote to disk and has not restarted
    /// onto.
    pub installed: Option<Version>,
    /// A restart that waits for no turn to run is already under way.
    pub restart_pending: bool,
    /// Threads with a turn running; `None` when the server cannot be asked,
    /// which counts as busy.
    pub running_turns: Option<usize>,
}

/// What kind of binary runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Build {
    Release,
    /// Cargo output, or built with debug assertions.
    Development,
    /// `1.31.4-beta.1`: updated by the beta channel, never by this.
    Prerelease,
}

/// What to do on this look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing, for this reason.
    Skip(Skip),
    /// A newer stable release exists and a turn runs: wait for idle.
    WaitForIdle(Version),
    /// A newer stable release exists and no turn runs: install it.
    Install(Version),
    /// It is installed; the restart onto it is due.
    Restart(Version),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Skip {
    Off,
    Development,
    Prerelease,
    UpToDate,
    RestartPending,
}

/// Whether the look needs to ask GitHub: not when it is off, not for a build
/// that never updates, and not when the release is already on disk.
pub(crate) fn needs_lookup(facts: &Facts) -> bool {
    facts.enabled && facts.build == Build::Release && facts.installed.is_none()
}

/// The decision, from what is known. Pure, so every case is a test.
pub(crate) fn decide(facts: &Facts) -> Step {
    if !facts.enabled {
        return Step::Skip(Skip::Off);
    }
    match facts.build {
        Build::Development => return Step::Skip(Skip::Development),
        Build::Prerelease => return Step::Skip(Skip::Prerelease),
        Build::Release => {}
    }
    if let Some(installed) = facts.installed
        && installed > facts.current
    {
        return if facts.restart_pending {
            Step::Skip(Skip::RestartPending)
        } else {
            Step::Restart(installed)
        };
    }
    match facts.latest {
        Some(latest) if latest > facts.current => match facts.running_turns {
            Some(0) => Step::Install(latest),
            _ => Step::WaitForIdle(latest),
        },
        _ => Step::Skip(Skip::UpToDate),
    }
}

/// The pause before the next look: the period, doubled for each failure in a
/// row up to [`MAX_BACKOFF`], spread ±10% by `unit` (0.0 to 1.0) so that
/// machines paired together do not all ask GitHub at the same second.
pub(crate) fn next_delay(failures: u32, unit: f64) -> Duration {
    let base = PERIOD
        .saturating_mul(1u32.checked_shl(failures.min(8)).unwrap_or(u32::MAX))
        .min(MAX_BACKOFF);
    base.mul_f64(0.9 + 0.2 * unit.clamp(0.0, 1.0))
}

fn jitter() -> f64 {
    rand::random::<u64>() as f64 / u64::MAX as f64
}

/// How this binary was built.
fn build_kind(program: &Path) -> Build {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from);
    if cfg!(debug_assertions) || crate::service::is_dev_binary(program, target_dir.as_deref()) {
        Build::Development
    } else if crate::update::current_is_prerelease() {
        Build::Prerelease
    } else {
        Build::Release
    }
}

/// How one look ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Nothing went wrong, whether or not there was anything to do.
    Fine,
    /// A step failed: the next look comes after a longer pause.
    Failed,
}

/// Start looking for releases for as long as the server lives. Called by a
/// server that a service manager started; a server an app or a terminal
/// started is that app's or terminal's to update.
pub fn spawn() {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    // The path now: after an update, a running binary that was replaced
    // reports a path the restart cannot start (Linux adds " (deleted)").
    let Ok(program) = crate::service::executable_path() else {
        return;
    };
    let build = build_kind(&program);
    if build != Build::Release {
        crate::dbg_log!(
            "RustCode server auto-update: off for {} build",
            if build == Build::Development {
                "a development"
            } else {
                "a prerelease"
            }
        );
        return;
    }
    runtime.spawn(async move {
        let mut installed = None;
        let mut failures = 0u32;
        tokio::time::sleep(FIRST_LOOK.mul_f64(0.5 + jitter())).await;
        loop {
            match look(&program, build, &mut installed).await {
                Outcome::Fine => failures = 0,
                Outcome::Failed => failures = failures.saturating_add(1),
            }
            let wait = next_delay(failures, jitter());
            if failures > 0 {
                crate::dbg_log!(
                    "RustCode server auto-update: trying again in {} minutes",
                    wait.as_secs() / 60
                );
            }
            tokio::time::sleep(wait).await;
        }
    });
}

fn enabled() -> bool {
    crate::config::load_config().2.server.auto_update
}

/// One look at the machine and whatever it leads to.
async fn look(program: &Path, build: Build, installed: &mut Option<Version>) -> Outcome {
    let mut facts = Facts {
        enabled: enabled(),
        build,
        current: current_version(),
        latest: None,
        installed: *installed,
        restart_pending: service::restart_waiting(),
        running_turns: None,
    };
    if needs_lookup(&facts) {
        let client = match reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(10 * 60))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                crate::dbg_log!("RustCode server auto-update: could not start: {error}");
                return Outcome::Failed;
            }
        };
        let Some(latest) = latest_available_version(&client).await else {
            crate::dbg_log!("RustCode server auto-update: could not read the latest release");
            return Outcome::Failed;
        };
        facts.latest = Some(latest);
        facts.running_turns = service::running_turns().await;
        return act(&client, program, facts, installed).await;
    }
    match decide(&facts) {
        Step::Restart(version) => restart(program, version),
        Step::Skip(_) | Step::WaitForIdle(_) | Step::Install(_) => Outcome::Fine,
    }
}

async fn act(
    client: &reqwest::Client,
    program: &Path,
    facts: Facts,
    installed: &mut Option<Version>,
) -> Outcome {
    match decide(&facts) {
        Step::Skip(_) | Step::Restart(_) => Outcome::Fine,
        Step::WaitForIdle(latest) => {
            crate::dbg_log!(
                "RustCode server auto-update: found {}, running {}; waiting for no turn to run",
                format_version(latest),
                format_version(facts.current)
            );
            let outcome =
                service::wait_until_idle(service::running_turns, IDLE_POLL, 2, None, |running| {
                    crate::dbg_log!(
                        "RustCode server auto-update: {running} running {}",
                        if running == 1 { "turn" } else { "turns" }
                    );
                })
                .await;
            if outcome != service::Idle::Idle {
                // The server cannot ask itself: look again next time.
                return Outcome::Failed;
            }
            if !enabled() {
                crate::dbg_log!("RustCode server auto-update: turned off while waiting");
                return Outcome::Fine;
            }
            install(client, program, latest, installed).await
        }
        Step::Install(latest) => {
            crate::dbg_log!(
                "RustCode server auto-update: found {}, running {}; no turn is running",
                format_version(latest),
                format_version(facts.current)
            );
            install(client, program, latest, installed).await
        }
    }
}

async fn install(
    client: &reqwest::Client,
    program: &Path,
    latest: Version,
    installed: &mut Option<Version>,
) -> Outcome {
    crate::dbg_log!(
        "RustCode server auto-update: installing {}",
        format_version(latest)
    );
    if let Err(error) = run_update(client, latest).await {
        crate::dbg_log!(
            "RustCode server auto-update: installing {} failed: {error}",
            format_version(latest)
        );
        return Outcome::Failed;
    }
    crate::dbg_log!(
        "RustCode server auto-update: installed {}",
        format_version(latest)
    );
    // Remembered, so a restart that fails is tried again without downloading
    // the release a second time.
    *installed = Some(latest);
    restart(program, latest)
}

fn restart(program: &Path, version: Version) -> Outcome {
    crate::dbg_log!(
        "RustCode server auto-update: restarting onto {} once no turn is running",
        format_version(version)
    );
    match service::spawn_detached_restart_from(program, true) {
        Ok(()) => Outcome::Fine,
        Err(error) => {
            crate::dbg_log!("RustCode server auto-update: could not restart: {error:#}");
            Outcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            enabled: true,
            build: Build::Release,
            current: (1, 31, 0),
            latest: Some((1, 31, 3)),
            installed: None,
            restart_pending: false,
            running_turns: Some(0),
        }
    }

    #[test]
    fn a_newer_stable_release_installs_when_no_turn_runs() {
        assert_eq!(decide(&facts()), Step::Install((1, 31, 3)));
    }

    #[test]
    fn a_busy_server_waits_for_idle_and_one_that_cannot_be_asked_counts_as_busy() {
        let busy = Facts {
            running_turns: Some(2),
            ..facts()
        };
        assert_eq!(decide(&busy), Step::WaitForIdle((1, 31, 3)));
        let unknown = Facts {
            running_turns: None,
            ..facts()
        };
        assert_eq!(decide(&unknown), Step::WaitForIdle((1, 31, 3)));
    }

    #[test]
    fn the_current_or_an_older_release_is_left_alone() {
        for latest in [Some((1, 31, 0)), Some((1, 30, 9)), None] {
            let same = Facts { latest, ..facts() };
            assert_eq!(decide(&same), Step::Skip(Skip::UpToDate));
        }
    }

    #[test]
    fn a_prerelease_is_never_the_release_to_install() {
        // The lookup keeps only stable tags, so a beta is no `latest` at all.
        assert_eq!(crate::update::stable_version("v1.32.0-beta.1"), None);
        let none = Facts {
            latest: crate::update::stable_version("v1.32.0-beta.1"),
            ..facts()
        };
        assert_eq!(decide(&none), Step::Skip(Skip::UpToDate));
        // And a prerelease or development build never updates itself.
        let beta = Facts {
            build: Build::Prerelease,
            ..facts()
        };
        assert_eq!(decide(&beta), Step::Skip(Skip::Prerelease));
        assert!(!needs_lookup(&beta));
        let dev = Facts {
            build: Build::Development,
            ..facts()
        };
        assert_eq!(decide(&dev), Step::Skip(Skip::Development));
        assert!(!needs_lookup(&dev));
    }

    #[test]
    fn the_setting_off_skips_everything_and_asks_nobody() {
        let off = Facts {
            enabled: false,
            ..facts()
        };
        assert_eq!(decide(&off), Step::Skip(Skip::Off));
        assert!(!needs_lookup(&off));
    }

    #[test]
    fn an_installed_release_only_restarts_and_does_not_download_again() {
        let installed = Facts {
            installed: Some((1, 31, 3)),
            ..facts()
        };
        assert!(!needs_lookup(&installed));
        assert_eq!(decide(&installed), Step::Restart((1, 31, 3)));
        let waiting = Facts {
            restart_pending: true,
            ..installed
        };
        assert_eq!(decide(&waiting), Step::Skip(Skip::RestartPending));
    }

    #[test]
    fn a_failure_backs_off_and_the_pause_stays_bounded() {
        let at = |failures| next_delay(failures, 0.5);
        let spread = |base: Duration| base.mul_f64(0.9 + 0.2 * 0.5);
        assert_eq!(at(0), spread(PERIOD));
        assert_eq!(at(1), spread(PERIOD * 2));
        assert_eq!(at(2), spread(PERIOD * 4));
        assert_eq!(at(3), spread(PERIOD * 8));
        assert_eq!(at(4), spread(MAX_BACKOFF));
        assert_eq!(at(50), spread(MAX_BACKOFF));
        assert_eq!(at(u32::MAX), spread(MAX_BACKOFF));
    }

    #[test]
    fn the_pause_is_spread_by_ten_percent() {
        assert_eq!(next_delay(0, 0.0), PERIOD.mul_f64(0.9));
        assert_eq!(next_delay(0, 1.0), PERIOD.mul_f64(1.1));
        assert_eq!(next_delay(0, 7.0), PERIOD.mul_f64(1.1), "clamped");
    }

    #[test]
    fn cargo_output_is_a_development_build() {
        assert_eq!(
            build_kind(Path::new("/work/rustcode/target/release/rustcode")),
            Build::Development
        );
    }
}
