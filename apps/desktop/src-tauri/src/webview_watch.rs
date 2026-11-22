//! The window's web process, in `webview.log`. On macOS WebKit draws the page
//! in a process of its own (`com.apple.WebKit.WebContent`, which Activity
//! Monitor names after the page, `tauri://localhost`). launchd starts it, not
//! the app, so it is not listed under RustCode, and killing it blanks the
//! window until WebKit loads the page again, which restarts the engine. Nothing
//! recorded any of that: this samples the process once a minute and writes a
//! line when it appears, is replaced, crosses a memory step or stays busy, plus
//! an hourly summary, so a hot or bloated window leaves evidence behind.

use std::time::Duration;

/// How often the process is sampled.
const INTERVAL: Duration = Duration::from_secs(60);
/// A summary line this often (in samples), even when nothing stood out.
const SUMMARY_EVERY: u32 = 60;
/// Each memory step is recorded once per process, on the way up.
const MEMORY_STEPS_MB: &[u64] = &[1024, 2048, 4096, 8192];
/// CPU over a whole interval above this, in percent of one core, is "busy".
const BUSY_PERCENT: f64 = 50.0;

/// One reading of the web process.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub pid: i32,
    /// Physical footprint (what Activity Monitor shows as Memory), in bytes.
    pub footprint: u64,
    /// User plus system CPU time so far, in nanoseconds.
    pub cpu_ns: u64,
}

/// What the watcher remembers between samples, and the lines it decides on.
#[derive(Default)]
struct Watch {
    last: Option<Sample>,
    steps_hit: usize,
    samples: u32,
    peak: u64,
    busy_streak: u32,
}

impl Watch {
    /// The log lines (`event`, `detail`) for a new reading; `None` means the
    /// web process could not be read (no window, or an older WebKit).
    fn observe(
        &mut self,
        sample: Option<Sample>,
        interval: Duration,
    ) -> Vec<(&'static str, String)> {
        let mut lines = Vec::new();
        let Some(now) = sample else {
            return lines;
        };
        match self.last {
            None => lines.push(("webview.process", describe(&now))),
            Some(last) if last.pid != now.pid => {
                lines.push((
                    "webview.process_replaced",
                    format!(
                        "pid {} -> {}; the old one last used {} (peak {}); now {}",
                        last.pid,
                        now.pid,
                        mb(last.footprint),
                        mb(self.peak),
                        mb(now.footprint)
                    ),
                ));
                self.steps_hit = 0;
                self.peak = 0;
                self.busy_streak = 0;
            }
            Some(last) => {
                let percent = cpu_percent(last.cpu_ns, now.cpu_ns, interval);
                if percent >= BUSY_PERCENT {
                    self.busy_streak += 1;
                    // The first busy minute, then every ten while it lasts.
                    if self.busy_streak % 10 == 1 {
                        lines.push((
                            "webview.busy",
                            format!(
                                "{percent:.0}% CPU for {} min; {}",
                                self.busy_streak,
                                describe(&now)
                            ),
                        ));
                    }
                } else {
                    self.busy_streak = 0;
                }
            }
        }
        self.peak = self.peak.max(now.footprint);
        while let Some(step) = MEMORY_STEPS_MB.get(self.steps_hit) {
            if now.footprint < step * 1024 * 1024 {
                break;
            }
            self.steps_hit += 1;
            lines.push((
                "webview.memory",
                format!("over {} MB; {}", step, describe(&now)),
            ));
        }
        self.samples += 1;
        if self.samples.is_multiple_of(SUMMARY_EVERY) {
            let percent = self
                .last
                .filter(|last| last.pid == now.pid)
                .map(|last| cpu_percent(last.cpu_ns, now.cpu_ns, interval))
                .unwrap_or(0.0);
            lines.push((
                "webview.sample",
                format!(
                    "{}; peak {}; {percent:.0}% CPU",
                    describe(&now),
                    mb(self.peak)
                ),
            ));
        }
        self.last = Some(now);
        lines
    }
}

fn describe(sample: &Sample) -> String {
    format!(
        "pid {} uses {}, {:.0} s CPU so far",
        sample.pid,
        mb(sample.footprint),
        sample.cpu_ns as f64 / 1e9
    )
}

fn mb(bytes: u64) -> String {
    format!("{} MB", bytes / (1024 * 1024))
}

/// CPU time spent between two readings, in percent of one core.
fn cpu_percent(before_ns: u64, after_ns: u64, interval: Duration) -> f64 {
    let wall = interval.as_nanos() as f64;
    if wall == 0.0 {
        return 0.0;
    }
    after_ns.saturating_sub(before_ns) as f64 / wall * 100.0
}

/// Samples the main window's web process for the life of the app (macOS only).
pub fn spawn(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    std::thread::spawn(move || {
        let mut watch = Watch::default();
        loop {
            let sample = macos::web_process_pid(&app).and_then(macos::sample);
            for (event, detail) in watch.observe(sample, INTERVAL) {
                crate::update_log::record_webview(event, &detail);
            }
            std::thread::sleep(INTERVAL);
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Sample;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, sel};
    use tauri::Manager;

    /// `rusage_info_v0` from `<sys/resource.h>`.
    #[repr(C)]
    #[derive(Default)]
    struct RusageInfoV0 {
        uuid: [u8; 16],
        user_time: u64,
        system_time: u64,
        pkg_idle_wkups: u64,
        interrupt_wkups: u64,
        pageins: u64,
        wired_size: u64,
        resident_size: u64,
        phys_footprint: u64,
        proc_start_abstime: u64,
        proc_exit_abstime: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }

    const RUSAGE_INFO_V0: i32 = 0;

    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    /// The main window's web process id (`_webProcessIdentifier`, which WebKit
    /// has had for years but does not document), read on the main thread.
    pub fn web_process_pid(app: &tauri::AppHandle) -> Option<i32> {
        let window = app.get_webview_window("main")?;
        let (tx, rx) = std::sync::mpsc::channel();
        window
            .with_webview(move |webview| {
                let view: *mut AnyObject = webview.inner().cast();
                // SAFETY: a live WKWebView on the main thread; the selector is checked first.
                let pid = unsafe {
                    let known: bool = !view.is_null()
                        && msg_send![view, respondsToSelector: sel!(_webProcessIdentifier)];
                    known.then(|| {
                        let pid: i32 = msg_send![view, _webProcessIdentifier];
                        pid
                    })
                };
                let _ = tx.send(pid);
            })
            .ok()?;
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .ok()
            .flatten()
            .filter(|pid| *pid > 0)
    }

    /// Memory and CPU time of `pid`, which belongs to the same user.
    pub fn sample(pid: i32) -> Option<Sample> {
        let mut info = RusageInfoV0::default();
        let mut timebase = Timebase::default();
        // SAFETY: both buffers are the C layouts these calls fill.
        unsafe {
            if proc_pid_rusage(pid, RUSAGE_INFO_V0, &mut info) != 0 {
                return None;
            }
            mach_timebase_info(&mut timebase);
        }
        let ticks = info.user_time.saturating_add(info.system_time) as u128;
        // CPU times are in Mach ticks (nanoseconds on Intel, 125/3 ns on Apple silicon).
        let cpu_ns = if timebase.denom == 0 {
            ticks
        } else {
            ticks * timebase.numer as u128 / timebase.denom as u128
        };
        Some(Sample {
            pid,
            footprint: info.phys_footprint,
            cpu_ns: cpu_ns.min(u64::MAX as u128) as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    fn at(pid: i32, footprint_mb: u64, cpu_s: u64) -> Option<Sample> {
        Some(Sample {
            pid,
            footprint: footprint_mb * MB,
            cpu_ns: cpu_s * 1_000_000_000,
        })
    }

    fn events(lines: &[(&'static str, String)]) -> Vec<&'static str> {
        lines.iter().map(|(event, _)| *event).collect()
    }

    #[test]
    fn records_the_first_process_and_its_replacement() {
        let mut watch = Watch::default();
        assert_eq!(
            events(&watch.observe(at(10, 200, 0), INTERVAL)),
            ["webview.process"]
        );
        assert!(watch.observe(at(10, 300, 1), INTERVAL).is_empty());
        let lines = watch.observe(at(11, 250, 0), INTERVAL);
        assert_eq!(events(&lines), ["webview.process_replaced"]);
        assert!(lines[0].1.contains("pid 10 -> 11"), "{}", lines[0].1);
        assert!(lines[0].1.contains("peak 300 MB"), "{}", lines[0].1);
    }

    #[test]
    fn each_memory_step_is_recorded_once() {
        let mut watch = Watch::default();
        watch.observe(at(10, 200, 0), INTERVAL);
        assert_eq!(
            events(&watch.observe(at(10, 1100, 0), INTERVAL)),
            ["webview.memory"]
        );
        assert!(watch.observe(at(10, 1200, 0), INTERVAL).is_empty());
        assert_eq!(
            events(&watch.observe(at(10, 4200, 0), INTERVAL)),
            ["webview.memory", "webview.memory"]
        );
    }

    #[test]
    fn a_busy_stretch_is_recorded_at_its_start_and_every_ten_minutes() {
        let mut watch = Watch::default();
        watch.observe(at(10, 200, 0), INTERVAL);
        let mut busy = 0;
        for minute in 1..=12 {
            // 45 s of CPU a minute: 75%.
            busy += events(&watch.observe(at(10, 200, minute * 45), INTERVAL))
                .iter()
                .filter(|e| **e == "webview.busy")
                .count();
        }
        assert_eq!(busy, 2);
        assert!(watch.observe(at(10, 200, 12 * 45 + 1), INTERVAL).is_empty());
    }

    #[test]
    fn an_unreadable_process_writes_nothing() {
        let mut watch = Watch::default();
        assert!(watch.observe(None, INTERVAL).is_empty());
    }
}
