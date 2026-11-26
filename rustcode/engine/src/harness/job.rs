//! A Windows job object per harness, so stopping a harness ends every process
//! it started (an `npx` launcher, the `node` adapter, a build it ran), and an
//! engine that dies without cleaning up takes them with it: the job is set to
//! kill its processes when its last handle closes, and the engine holds the
//! only one.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

/// An open job handle, kept as an integer so the owner stays `Send` and `Sync`.
pub(crate) struct Job(isize);

impl Job {
    /// A job holding the process `process`, or `None` when Windows refuses
    /// (the engine then kills only the harness's own process, as before).
    pub(crate) fn holding(process: HANDLE) -> Option<Self> {
        // SAFETY: plain Win32 calls on handles this function owns or was given;
        // every failure path closes the job it created.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0;
            if !configured || AssignProcessToJobObject(job, process) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Self(job as isize))
        }
    }

    /// End every process in the job now.
    pub(crate) fn terminate(&self) {
        // SAFETY: the handle is open until `drop`.
        unsafe {
            TerminateJobObject(self.0 as HANDLE, 1);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: closing the handle this value owns; the job's processes end with it.
        unsafe {
            CloseHandle(self.0 as HANDLE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::time::{Duration, Instant};

    #[test]
    fn terminating_the_job_ends_the_grandchild_too() {
        let marker = tempfile::tempdir().unwrap();
        let pid_file = marker.path().join("grandchild.pid");
        let script = format!(
            "$p = Start-Process -PassThru -WindowStyle Hidden powershell -ArgumentList '-NoProfile','-Command','Start-Sleep 60'; \
             Set-Content -Path '{}' -Value $p.Id; Start-Sleep 60",
            pid_file.display()
        );
        let mut child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .spawn()
            .unwrap();
        let job = Job::holding(child.as_raw_handle() as HANDLE).expect("job created");
        let deadline = Instant::now() + Duration::from_secs(30);
        let grandchild = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "grandchild never started");
            std::thread::sleep(Duration::from_millis(100));
        };

        job.terminate();
        let _ = child.wait();

        let deadline = Instant::now() + Duration::from_secs(10);
        while running(grandchild) {
            assert!(Instant::now() < deadline, "grandchild outlived its job");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn running(pid: u32) -> bool {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
    }
}
