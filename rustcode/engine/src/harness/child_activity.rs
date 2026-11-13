//! What the processes a harness started are doing while a turn runs. A turn
//! that sits in `cargo test` sends the app nothing for minutes, yet the
//! compiler is busy: only the process tree shows it. The app's stall notice
//! (`apps/desktop/src/state/orchestration.ts`) reads this so that a child that
//! still uses CPU counts as progress, and a notice names what runs and for how
//! long it has used CPU.
//!
//! A turn samples the harness's process tree every [`INTERVAL`]. A sample
//! becomes a `_rustcode/session/childActivity` notification
//! (`acp/harnesses.rs`) while the tree has children, and once more when the
//! last one ends.

use super::session::{Output, OutputSender};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How often a running turn's process tree is read.
pub(crate) const INTERVAL: Duration = Duration::from_secs(30);

/// CPU a tree must add between two samples to count as working. Waiting on a
/// lock or a sleep adds next to nothing; a compiler adds seconds.
pub(crate) const ACTIVE_CPU_MS: u64 = 500;

/// One process, as a sample reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proc {
    pub pid: u32,
    pub parent: Option<u32>,
    /// CPU time it has used since it started.
    pub cpu_ms: u64,
    pub name: String,
}

/// What the harness's children did, as of one sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildActivity {
    /// Processes below the harness.
    pub processes: usize,
    /// CPU time they have used so far, in total.
    pub cpu_ms: u64,
    /// The one that used the most CPU (`rustc`, `ld`).
    pub busiest: Option<String>,
    /// How long ago the tree last added [`ACTIVE_CPU_MS`] of CPU between two
    /// samples; `None` when it has not since the turn began.
    pub active_ago_ms: Option<u64>,
}

/// Reads successive samples of one harness's tree.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    previous: Option<HashMap<u32, u64>>,
    /// The time, in the caller's clock, of the last sample that was active.
    active_at: Option<u64>,
}

/// The processes below `root`, not `root` itself.
fn descendants(root: u32, procs: &[Proc]) -> Vec<&Proc> {
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for process in procs {
        if let Some(parent) = process.parent {
            children.entry(parent).or_default().push(process);
        }
    }
    let mut found = Vec::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            // A pid that loops back would never end otherwise.
            if child.pid != root && !found.iter().any(|seen: &&Proc| seen.pid == child.pid) {
                found.push(*child);
                queue.push(child.pid);
            }
        }
    }
    found
}

impl Tracker {
    /// Read a sample taken at `now_ms` (any monotonic clock, in milliseconds).
    /// The first one is only a baseline: it cannot say what changed.
    pub(crate) fn observe(&mut self, now_ms: u64, root: u32, procs: &[Proc]) -> ChildActivity {
        let tree = descendants(root, procs);
        let current: HashMap<u32, u64> = tree
            .iter()
            .map(|process| (process.pid, process.cpu_ms))
            .collect();
        if let Some(previous) = &self.previous {
            // A process that ended took its CPU out of the total, which says
            // nothing about the others; one that is new counts from zero.
            let added: u64 = current
                .iter()
                .map(|(pid, cpu)| cpu.saturating_sub(previous.get(pid).copied().unwrap_or(0)))
                .sum();
            if added >= ACTIVE_CPU_MS {
                self.active_at = Some(now_ms);
            }
        }
        self.previous = Some(current);
        let busiest = tree
            .iter()
            .max_by_key(|process| process.cpu_ms)
            .map(|process| process.name.clone());
        ChildActivity {
            processes: tree.len(),
            cpu_ms: tree.iter().map(|process| process.cpu_ms).sum(),
            busiest,
            active_ago_ms: self.active_at.map(|at| now_ms.saturating_sub(at)),
        }
    }
}

/// Every process on this machine, with its CPU time.
fn snapshot(system: &mut sysinfo::System) -> Vec<Proc> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
    // Threads are processes of their own on Linux and would count twice.
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().without_tasks(),
    );
    system
        .processes()
        .iter()
        .map(|(pid, process)| Proc {
            pid: pid.as_u32(),
            parent: process.parent().map(sysinfo::Pid::as_u32),
            cpu_ms: process.accumulated_cpu_time(),
            name: process.name().to_string_lossy().into_owned(),
        })
        .collect()
}

/// A sampler that stops when dropped: the turn it watched is over.
pub(crate) struct Watch(tokio::task::JoinHandle<()>);

impl Drop for Watch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Sample the tree below `root` every [`INTERVAL`] and send what it finds to
/// `out` while there are children, and once when the last one is gone.
pub(crate) fn watch(root: u32, out: OutputSender) -> Watch {
    Watch(tokio::spawn(async move {
        let started = Instant::now();
        let mut tracker = Tracker::default();
        let mut system = Some(sysinfo::System::new());
        let mut had_children = false;
        loop {
            let Some(mut taken) = system.take() else {
                return;
            };
            let Ok((returned, procs)) = tokio::task::spawn_blocking(move || {
                let procs = snapshot(&mut taken);
                (taken, procs)
            })
            .await
            else {
                return;
            };
            system = Some(returned);
            let now_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let activity = tracker.observe(now_ms, root, &procs);
            if (activity.processes > 0 || had_children)
                && out.send(Output::ChildActivity(activity.clone())).is_err()
            {
                return;
            }
            had_children = activity.processes > 0;
            tokio::time::sleep(INTERVAL).await;
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: u32, cpu_ms: u64, name: &str) -> Proc {
        Proc {
            pid,
            parent: Some(parent),
            cpu_ms,
            name: name.to_owned(),
        }
    }

    #[test]
    fn only_the_harnesss_own_tree_counts() {
        let procs = [
            proc(10, 1, 9_000, "harness"),
            proc(11, 10, 100, "zsh"),
            proc(12, 11, 2_000, "cargo"),
            proc(13, 12, 5_000, "rustc"),
            proc(99, 1, 70_000, "other-delegate-rustc"),
        ];
        let mut tracker = Tracker::default();
        let sample = tracker.observe(0, 10, &procs);
        assert_eq!(sample.processes, 3);
        assert_eq!(sample.cpu_ms, 7_100);
        assert_eq!(sample.busiest.as_deref(), Some("rustc"));
        assert_eq!(sample.active_ago_ms, None, "the first sample is a baseline");
    }

    #[test]
    fn a_child_that_adds_cpu_is_active_and_one_that_waits_is_not() {
        let mut tracker = Tracker::default();
        let waiting = |cpu| [proc(11, 10, cpu, "zsh"), proc(12, 11, cpu, "cargo")];
        tracker.observe(0, 10, &waiting(1_000));
        // Compiling: seconds of CPU since the last sample.
        let busy = tracker.observe(30_000, 10, &waiting(4_000));
        assert_eq!(busy.active_ago_ms, Some(0));
        // Blocked on a lock: a few milliseconds.
        let blocked = tracker.observe(60_000, 10, &waiting(4_010));
        assert_eq!(blocked.active_ago_ms, Some(30_000));
        let later = tracker.observe(90_000, 10, &waiting(4_020));
        assert_eq!(later.active_ago_ms, Some(60_000));
        // And busy again.
        let again = tracker.observe(120_000, 10, &waiting(9_000));
        assert_eq!(again.active_ago_ms, Some(0));
    }

    #[test]
    fn a_process_that_ends_does_not_look_like_activity_and_a_new_one_counts_from_zero() {
        let mut tracker = Tracker::default();
        tracker.observe(
            0,
            10,
            &[proc(12, 10, 8_000, "rustc"), proc(13, 10, 300, "cc")],
        );
        // rustc ended: the total fell, nothing was added.
        let ended = tracker.observe(30_000, 10, &[proc(13, 10, 320, "cc")]);
        assert_eq!(ended.active_ago_ms, None);
        // A new compiler that already used a second.
        let started = tracker.observe(
            60_000,
            10,
            &[proc(13, 10, 320, "cc"), proc(14, 10, 1_000, "rustc")],
        );
        assert_eq!(started.active_ago_ms, Some(0));
    }

    #[test]
    fn a_tree_with_no_children_reports_none() {
        let mut tracker = Tracker::default();
        let sample = tracker.observe(0, 10, &[proc(5, 1, 100, "unrelated")]);
        assert_eq!(sample.processes, 0);
        assert_eq!(sample.busiest, None);
    }

    #[test]
    fn a_parent_loop_ends() {
        let procs = [
            proc(11, 12, 0, "a"),
            proc(12, 11, 0, "b"),
            proc(13, 10, 0, "c"),
        ];
        let mut tracker = Tracker::default();
        assert_eq!(tracker.observe(0, 10, &procs).processes, 1);
    }
}
