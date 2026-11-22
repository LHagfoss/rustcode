//! The RustCode server hosts the scheduler (`docs/daemon.md` › In the server).
//!
//! Scheduled jobs used to run only while someone had started `rc daemon
//! start` since the last login. The server already runs at login when it is
//! installed, so it runs the scheduler too, in its own process: one engine
//! per machine, and a job fires with no terminal or app open.
//!
//! The daemon's owner lock keeps one scheduler per config directory. A
//! scheduler already running (`rc daemon start` from a terminal) is left
//! alone; the server looks again a minute later and takes over once it is
//! gone. `rc daemon stop` stops the server's scheduler for the rest of the
//! server's life.

use super::lifecycle::DaemonLifecycle;
use std::time::Duration;

/// How long the server waits before looking again for a scheduler to host.
const RETRY: Duration = Duration::from_secs(60);

/// Run the scheduler as a task of `rc server run` for as long as the server
/// lives.
pub fn spawn() {
    // A server with no runtime (a test, a CLI path that never serves) must
    // not panic here.
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async {
        // Said once per reason, not every minute.
        let mut said = String::new();
        loop {
            if let Some(config_directory) = crate::config::get_config_dir() {
                match DaemonLifecycle::new(&config_directory).run().await {
                    Ok(()) => {
                        crate::dbg_log!("the scheduler was stopped; the server leaves it stopped");
                        return;
                    }
                    Err(error) => {
                        let why = format!("{error:#}");
                        if why != said {
                            crate::dbg_log!("the server is not running the scheduler: {why}");
                            said = why;
                        }
                    }
                }
            }
            tokio::time::sleep(RETRY).await;
        }
    });
}
