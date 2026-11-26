//! How much of this machine a paired computer may drive over remote ACP: a
//! ceiling the host owner sets per device. `read` lists and reads threads,
//! `threads` also starts and drives them, `full` is every method the relay
//! carries. A level is a ceiling on RustCode control, not a sandbox: a
//! prompt a `threads` device sends still runs tools here under the thread's
//! own permission mode.

use serde::{Deserialize, Serialize};

/// One device's ceiling, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    None,
    Read,
    Threads,
    Full,
}

impl Access {
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Read => "read",
            Self::Threads => "threads",
            Self::Full => "full",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "read" => Some(Self::Read),
            "threads" => Some(Self::Threads),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// What the level lets a device do, for a refusal.
    fn allows(self) -> &'static str {
        match self {
            Self::None => "nothing over ACP",
            Self::Read => "read threads only",
            Self::Threads => "start and drive threads, not change this computer",
            Self::Full => "everything",
        }
    }
}

/// Methods a `read` device may call: listing and reading, nothing that
/// changes a thread or this machine.
pub(super) const READ: &[&str] = &[
    "initialize",
    "session/list",
    "session/load",
    "_rustcode/session/status",
    "_rustcode/session/replay",
    "_rustcode/threads/search",
    "_rustcode/data/activity",
    "_rustcode/data/usage",
    "_rustcode/project/read",
    "_rustcode/worktrees/state",
    "_rustcode/usage/limits",
    "_rustcode/usage/shared",
    "_rustcode/harnesses",
    "_rustcode/harnesses/models",
    "_rustcode/environment/sync",
    "_rustcode/attachments/read",
];

/// Methods a `threads` device may call besides [`READ`]: starting, driving
/// and arranging threads, and the folders a thread starts in.
pub(super) const THREADS: &[&str] = &[
    "_rustcode/session/snooze",
    "_rustcode/session/title",
    "_rustcode/session/repair",
    "_rustcode/session/stop",
    "_rustcode/session/harness",
    "_rustcode/session/move",
    "_rustcode/steer",
    "_rustcode/attachments/upload",
    "_rustcode/project/browse",
    "_rustcode/project/create",
    "_rustcode/project/clone",
    "_rustcode/settings/get",
    "_rustcode/skills/get",
];
const THREADS_PREFIXES: &[&str] = &["session/", "_rustcode/pulls"];

/// The least level `method` needs. Every method the relay allows has one
/// (`acp_relay` contract test); anything else is `Full`, which the relay's
/// own allowlist refuses anyway.
pub fn needed(method: &str) -> Access {
    if READ.contains(&method) {
        Access::Read
    } else if THREADS.contains(&method)
        || THREADS_PREFIXES
            .iter()
            .any(|prefix| method.starts_with(prefix))
    {
        Access::Threads
    } else {
        Access::Full
    }
}

/// Why a device at `level` may not call `method`, or `None` when it may.
pub fn refusal(level: Access, method: &str) -> Option<String> {
    let needed = needed(method);
    (level < needed).then(|| {
        format!(
            "this computer allows yours to {} ({} access); {method} needs {}. On this computer, raise it in Settings › Machines › Access or with `rc remote devices allow <device> --access {}`",
            level.allows(),
            level.name(),
            needed.name(),
            needed.name(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_order_and_parse() {
        assert!(Access::None < Access::Read && Access::Read < Access::Threads);
        assert!(Access::Threads < Access::Full);
        for level in [Access::None, Access::Read, Access::Threads, Access::Full] {
            assert_eq!(Access::parse(level.name()), Some(level));
        }
        assert_eq!(Access::parse("admin"), None);
    }

    #[test]
    fn a_read_device_reads_and_a_threads_device_drives_but_neither_reconfigures() {
        for method in ["session/list", "session/load", "_rustcode/session/replay"] {
            assert!(refusal(Access::Read, method).is_none(), "{method}");
        }
        for method in [
            "session/new",
            "session/prompt",
            "_rustcode/settings/set",
            "_rustcode/machine/exec",
        ] {
            assert!(refusal(Access::Read, method).is_some(), "{method}");
        }
        for method in [
            "session/new",
            "session/prompt",
            "_rustcode/steer",
            "_rustcode/session/harness",
        ] {
            assert!(refusal(Access::Threads, method).is_none(), "{method}");
        }
        for method in [
            "_rustcode/settings/set",
            "_rustcode/skills/add",
            "_rustcode/harnesses/set",
            "_rustcode/loops/list",
            "_rustcode/outside/list",
            "_rustcode/machine/update",
            "_rustcode/machine/exec",
            "_rustcode/session/import",
        ] {
            assert!(refusal(Access::Threads, method).is_some(), "{method}");
        }
        let why = refusal(Access::Read, "session/prompt").unwrap();
        assert!(why.contains("read threads only") && why.contains("--access threads"));
    }

    #[test]
    fn an_old_grant_reads_as_full_and_a_level_survives_a_reload() {
        let mut devices = super::super::devices::Devices::default();
        let enrolled = devices
            .enroll_as(
                "Studio",
                None,
                &super::super::devices::DeviceProfile::from_wire(Some("desktop"), None, None),
            )
            .unwrap();
        assert_eq!(enrolled.record.access_level(), Access::None);
        devices.set_acp(&enrolled.record.id, true).unwrap();
        let mut old = serde_json::to_value(&devices).unwrap();
        old["devices"][0].as_object_mut().unwrap().remove("access");
        let old = serde_json::from_value::<super::super::devices::Devices>(old).unwrap();
        assert_eq!(old.list()[0].access_level(), Access::Full);

        devices
            .set_access(&enrolled.record.id, Access::Read)
            .unwrap();
        let loaded: super::super::devices::Devices =
            serde_json::from_slice(&serde_json::to_vec(&devices).unwrap()).unwrap();
        assert_eq!(loaded.list()[0].access_level(), Access::Read);
        assert!(loaded.list()[0].acp);
        devices
            .set_access(&enrolled.record.id, Access::None)
            .unwrap();
        assert!(!devices.list()[0].acp);
        assert_eq!(devices.list()[0].access_level(), Access::None);
    }
}
