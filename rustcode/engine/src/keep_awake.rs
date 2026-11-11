//! Keeps Windows and the Mac from idle-sleeping while a turn runs. A turn left to work
//! while the reader is away stopped when the machine slept: the provider
//! connection dropped and the turn ended (session 01a16b37). Closing the lid
//! still sleeps the machine; the transport retry picks the turn up after.
//!
//! While any turn holds a [`KeepAwake`], including subagents' turns, macOS
//! runs one `caffeinate -i -w <pid>` child and Windows holds a system execution
//! request on a dedicated thread. Both end with the engine process even if
//! it never gets to drop the guard. Other platforms remain a no-op.

use std::sync::Mutex;

/// Held for the length of a turn; the system may idle-sleep again once the last
/// one is dropped.
pub(crate) struct KeepAwake(&'static Mutex<Holders>);

struct Holders {
    count: usize,
    child: Option<Backend>,
}

static HOLDERS: Mutex<Holders> = Mutex::new(Holders {
    count: 0,
    child: None,
});

impl KeepAwake {
    pub(crate) fn acquire() -> Self {
        Self::acquire_from(&HOLDERS, start)
    }

    fn acquire_from(holders: &'static Mutex<Holders>, start: fn() -> Option<Backend>) -> Self {
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
            && let Some(child) = held.child.take()
        {
            child.stop();
        }
    }
}

// Windows execution state belongs to the calling thread, so keep that thread
// alive until the last turn releases it. Tests inject callbacks instead of OS calls.
enum Backend {
    #[cfg(any(target_os = "macos", all(unix, test)))]
    Child(std::process::Child),
    #[cfg(any(windows, test))]
    Thread {
        stop: std::sync::mpsc::Sender<()>,
        join: std::thread::JoinHandle<()>,
    },
}

impl Backend {
    fn stop(self) {
        match self {
            #[cfg(any(target_os = "macos", all(unix, test)))]
            Self::Child(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(any(windows, test))]
            Self::Thread { stop, join } => {
                let _ = stop.send(());
                let _ = join.join();
            }
        }
    }

    #[cfg(any(windows, test))]
    fn thread(acquire: fn() -> bool, release: fn()) -> Option<Self> {
        let (stop, stopped) = std::sync::mpsc::channel();
        let (ready, started) = std::sync::mpsc::sync_channel(0);
        let join = std::thread::Builder::new()
            .name("keep-awake".into())
            .spawn(move || {
                let acquired = acquire();
                let _ = ready.send(acquired);
                if acquired {
                    let _ = stopped.recv();
                    release();
                }
            })
            .map_err(|error| crate::dbg_log!("keep awake: thread did not start: {error}"))
            .ok()?;
        if started.recv().unwrap_or(false) {
            Some(Self::Thread { stop, join })
        } else {
            let _ = join.join();
            None
        }
    }
}

#[cfg(all(windows, not(test)))]
fn start() -> Option<Backend> {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    Backend::thread(
        || {
            // SAFETY: this API takes only flags; the worker owns this state.
            let success =
                unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) != 0 };
            if !success {
                crate::dbg_log!("keep awake: SetThreadExecutionState failed");
            }
            success
        },
        || {
            // SAFETY: clear the request on the same thread that acquired it.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        },
    )
}

#[cfg(all(target_os = "macos", not(test)))]
fn start() -> Option<Backend> {
    crate::platform::command("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| crate::dbg_log!("keep awake: caffeinate did not start: {error}"))
        .ok()
        .map(Backend::Child)
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
fn start() -> Option<Backend> {
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
    fn sleeper() -> Option<Backend> {
        crate::platform::command("sleep")
            .arg("30")
            .spawn()
            .ok()
            .map(Backend::Child)
    }

    fn child_id() -> Option<u32> {
        let mut held = TEST_HOLDERS.lock().unwrap();
        let Backend::Child(child) = held.child.as_mut()? else {
            return None;
        };
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

#[cfg(test)]
mod thread_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static WORKER: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);
    static ACQUIRED: AtomicUsize = AtomicUsize::new(0);
    static RELEASED: AtomicUsize = AtomicUsize::new(0);

    fn fake() -> Option<Backend> {
        Backend::thread(
            || {
                *WORKER.lock().unwrap() = Some(std::thread::current().id());
                ACQUIRED.fetch_add(1, Ordering::SeqCst);
                true
            },
            || {
                assert_eq!(
                    WORKER.lock().unwrap().take(),
                    Some(std::thread::current().id())
                );
                RELEASED.fetch_add(1, Ordering::SeqCst);
            },
        )
    }

    #[test]
    fn worker_lives_until_last_guard_and_can_restart() {
        let holders = Box::leak(Box::new(Mutex::new(Holders {
            count: 0,
            child: None,
        })));
        let first = KeepAwake::acquire_from(holders, fake);
        let second = KeepAwake::acquire_from(holders, fake);
        assert_eq!(ACQUIRED.load(Ordering::SeqCst), 1);
        drop(first);
        assert_eq!(RELEASED.load(Ordering::SeqCst), 0);
        drop(second);
        assert_eq!(RELEASED.load(Ordering::SeqCst), 1);
        assert_eq!(holders.lock().unwrap().count, 0);
        drop(KeepAwake::acquire_from(holders, fake));
        assert_eq!(ACQUIRED.load(Ordering::SeqCst), 2);
        assert_eq!(RELEASED.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failed_acquisition_leaves_no_worker() {
        assert!(Backend::thread(|| false, || panic!("nothing to release")).is_none());
    }
}
