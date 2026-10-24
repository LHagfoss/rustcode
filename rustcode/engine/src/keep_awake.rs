//! Keeps the Mac from idle-sleeping while a turn runs. A turn left to work
//! while the reader is away stopped when the machine slept: the provider
//! connection dropped and the turn ended (session 01a16b37). Closing the lid
//! still sleeps the machine; the transport retry picks the turn up after.
//!
//! One `caffeinate -i` runs while any turn holds a [`KeepAwake`], subagents'
//! turns included. It is started with `-w` on this process, so it exits with
//! the engine even if the engine never gets to drop the guard.

use std::sync::Mutex;

/// Held for the length of a turn; the Mac may idle-sleep again once the last
/// one is dropped.
pub(crate) struct KeepAwake(&'static Mutex<Holders>);

struct Holders {
    count: usize,
    child: Option<std::process::Child>,
}

static HOLDERS: Mutex<Holders> = Mutex::new(Holders {
    count: 0,
    child: None,
});

impl KeepAwake {
    pub(crate) fn acquire() -> Self {
        Self::acquire_from(&HOLDERS, start)
    }

    fn acquire_from(
        holders: &'static Mutex<Holders>,
        start: fn() -> Option<std::process::Child>,
    ) -> Self {
        let mut held = holders.lock().unwrap_or_else(|e| e.into_inner());
        held.count += 1;
        if held.child.is_none() {
            held.child = start();
        }
        KeepAwake(holders)
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        held.count = held.count.saturating_sub(1);
        if held.count == 0
            && let Some(mut child) = held.child.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn start() -> Option<std::process::Child> {
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| crate::dbg_log!("keep awake: caffeinate did not start: {error}"))
        .ok()
}

#[cfg(any(not(target_os = "macos"), test))]
fn start() -> Option<std::process::Child> {
    None
}

#[cfg(all(unix, test))]
mod tests {
    use super::*;

    static TEST_HOLDERS: Mutex<Holders> = Mutex::new(Holders {
        count: 0,
        child: None,
    });

    /// A sleeping child stands in for `caffeinate`, so the test checks the
    /// holding without changing how the machine sleeps.
    fn sleeper() -> Option<std::process::Child> {
        std::process::Command::new("sleep").arg("30").spawn().ok()
    }

    fn child_id() -> Option<u32> {
        let mut held = TEST_HOLDERS.lock().unwrap();
        let child = held.child.as_mut()?;
        child
            .try_wait()
            .ok()
            .flatten()
            .is_none()
            .then(|| child.id())
    }

    #[test]
    fn one_child_runs_while_any_turn_holds_it() {
        let first = KeepAwake::acquire_from(&TEST_HOLDERS, sleeper);
        let pid = child_id();
        assert!(pid.is_some());
        let second = KeepAwake::acquire_from(&TEST_HOLDERS, sleeper);
        assert_eq!(child_id(), pid, "a second turn shares the child");
        drop(first);
        assert_eq!(child_id(), pid, "the second turn still holds it");
        drop(second);
        assert!(TEST_HOLDERS.lock().unwrap().child.is_none());
    }
}
