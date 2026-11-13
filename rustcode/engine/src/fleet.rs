//! The environment fleet: which computers run RustCode, what each is called,
//! and what version and platform it reports, as every computer in a pairing
//! chain sees them (`docs/remote.md` › Environment names and the fleet).
//!
//! Pairing is one trust relationship between two computers. This module adds
//! no trust: it carries *metadata* about computers, over the ACP connection a
//! pairing already opened, so a rename made on one computer reaches the rest.
//!
//! - Each computer keeps its own name in `environment.json`, with the time it
//!   was set. Without that file the name is the operating system's.
//! - Each computer keeps what it learned about the others in `fleet.json`.
//! - Two computers exchange everything they know with `_rustcode/environment/sync`
//!   (`acp/environment.rs`); each side merges the other's entries.
//!
//! The merge is last-writer-wins on the name, per computer: the entry with the
//! newer `name_updated_at` wins, and on a tie the greater name does, so every
//! computer ends on the same one. An entry that was never renamed has no time
//! and loses to any rename. The rest of an entry (platform, version, when the
//! computer was last seen) comes from whichever report is more recent. A
//! computer offline during a rename learns it the next time it syncs with any
//! computer that has it.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::daemon::private_files::{publish_private_json, read_private_json};

/// The most computers one fleet file keeps; the least recently seen go first.
const MAX_ENTRIES: usize = 256;
/// The longest name kept, in characters.
const MAX_NAME_CHARS: usize = 80;
const MAX_FIELD_CHARS: usize = 64;
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// A computer nobody has seen for this long is forgotten.
const FORGET_AFTER_DAYS: i64 = 90;

const OWN_NAME_FILE: &str = "environment.json";
const FLEET_FILE: &str = "fleet.json";

/// One computer, as a report about it says. Sent and stored as camelCase JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FleetEntry {
    /// The computer's gateway id, the same `host_id` a pairing is keyed by.
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub name: String,
    /// When `name` was chosen by a person. `None` for the name the operating
    /// system gave, which any rename replaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_updated_at: Option<DateTime<Utc>>,
    #[serde(default = "unknown_platform")]
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// When some computer last had word from this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<DateTime<Utc>>,
    /// The computer this one was learned through, when it was not told by the
    /// computer itself. Set by the receiver; whatever a sender writes is ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

fn unknown_platform() -> String {
    "unknown".to_owned()
}

impl FleetEntry {
    /// Bounded and printable, so a hostile peer cannot fill the file or the UI.
    fn cleaned(&self) -> Self {
        Self {
            host_id: bounded(&self.host_id, MAX_FIELD_CHARS),
            name: bounded(&self.name, MAX_NAME_CHARS),
            name_updated_at: self.name_updated_at,
            platform: Some(bounded(&self.platform, MAX_FIELD_CHARS))
                .filter(|platform| !platform.is_empty())
                .unwrap_or_else(unknown_platform),
            version: self
                .version
                .as_deref()
                .map(|version| bounded(version, MAX_FIELD_CHARS))
                .filter(|version| !version.is_empty()),
            last_seen_at: self.last_seen_at,
            via: None,
        }
    }
}

fn bounded(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Whether a name set at `candidate_at` as `candidate` replaces the one set at
/// `current_at` as `current`: the later time wins, then the greater name, and
/// a name nobody chose loses to one somebody did.
pub(crate) fn name_wins(
    candidate: (Option<DateTime<Utc>>, &str),
    current: (Option<DateTime<Utc>>, &str),
) -> bool {
    candidate > current
}

/// Fold `incoming` into `current`, both about the same computer. Returns
/// whether `current` changed.
fn merge_entry(current: &mut FleetEntry, incoming: &FleetEntry) -> bool {
    let mut changed = false;
    if name_wins(
        (incoming.name_updated_at, &incoming.name),
        (current.name_updated_at, &current.name),
    ) && !incoming.name.is_empty()
    {
        current.name = incoming.name.clone();
        current.name_updated_at = incoming.name_updated_at;
        changed = true;
    }
    if incoming.last_seen_at > current.last_seen_at {
        if incoming.platform != "unknown" && current.platform != incoming.platform {
            current.platform = incoming.platform.clone();
            changed = true;
        }
        if incoming.version.is_some() && current.version != incoming.version {
            current.version = incoming.version.clone();
            changed = true;
        }
        current.last_seen_at = incoming.last_seen_at;
        current.via = incoming.via.clone();
        changed = true;
    }
    changed
}

/// What this computer learned about the others, `fleet.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Fleet {
    #[serde(default)]
    pub entries: Vec<FleetEntry>,
}

impl Fleet {
    pub fn load(dir: &Path) -> Self {
        read_private_json(&dir.join(FLEET_FILE), MAX_FILE_BYTES).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        publish_private_json(&dir.join(FLEET_FILE), self)
    }

    pub fn get(&self, host_id: &str) -> Option<&FleetEntry> {
        self.entries.iter().find(|entry| entry.host_id == host_id)
    }

    /// Merge what the computer `from` knows. Its own entry (the one whose id is
    /// `from`) is taken as told; every other is marked as learned through it.
    /// An entry about `own_id` is not stored: it is this computer's own name,
    /// handled by [`apply_own_name`]. Returns whether anything changed.
    pub fn merge(
        &mut self,
        from: &str,
        incoming: &[FleetEntry],
        own_id: &str,
        now: DateTime<Utc>,
    ) -> bool {
        let mut changed = false;
        for entry in incoming {
            let mut entry = entry.cleaned();
            if entry.host_id.is_empty() || entry.host_id == own_id {
                continue;
            }
            // Nobody has seen a computer later than now: a clock that is ahead
            // must not outrank a real report forever.
            entry.last_seen_at = entry.last_seen_at.map(|seen| seen.min(now));
            if !from.is_empty() && entry.host_id != from {
                entry.via = Some(from.to_owned());
            }
            match self
                .entries
                .iter_mut()
                .find(|known| known.host_id == entry.host_id)
            {
                Some(known) => changed |= merge_entry(known, &entry),
                None => {
                    self.entries.push(entry);
                    changed = true;
                }
            }
        }
        changed |= self.prune(now);
        changed
    }

    /// Forget computers nobody has seen in a long while, and the least
    /// recently seen past the cap. Returns whether anything went.
    fn prune(&mut self, now: DateTime<Utc>) -> bool {
        let before = self.entries.len();
        let cutoff = now - Duration::days(FORGET_AFTER_DAYS);
        self.entries
            .retain(|entry| entry.last_seen_at.is_none_or(|seen| seen >= cutoff));
        if self.entries.len() > MAX_ENTRIES {
            self.entries
                .sort_by(|left, right| right.last_seen_at.cmp(&left.last_seen_at));
            self.entries.truncate(MAX_ENTRIES);
        }
        self.entries.len() != before
    }
}

// ----------------------------------------------------------------- own name

/// This computer's chosen name and when it was chosen: `environment.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OwnName {
    pub name: String,
    pub updated_at: DateTime<Utc>,
}

pub(crate) fn own_name(dir: &Path) -> Option<OwnName> {
    read_private_json::<OwnName>(&dir.join(OWN_NAME_FILE), 4 * 1024)
        .ok()
        .filter(|own| !own.name.is_empty())
}

/// The name this computer was given, for the gateway to announce, or `None`
/// while it still goes by the one the operating system gave.
pub(crate) fn own_name_override() -> Option<String> {
    own_name(&crate::config::get_config_dir()?).map(|own| own.name)
}

/// Adopt a name for this computer when it is newer than the one it has.
/// Returns whether it changed. An older rename, or the same one, is a no-op.
pub(crate) fn apply_own_name(
    dir: &Path,
    name: &str,
    updated_at: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let name = bounded(name, MAX_NAME_CHARS);
    if name.is_empty() {
        return Ok(false);
    }
    let current = own_name(dir);
    let wins = match &current {
        Some(own) => name_wins((Some(updated_at), &name), (Some(own.updated_at), &own.name)),
        // Any rename replaces the name the operating system gave.
        None => true,
    };
    if !wins {
        return Ok(false);
    }
    publish_private_json(&dir.join(OWN_NAME_FILE), &OwnName { name, updated_at })?;
    Ok(true)
}

// -------------------------------------------------------------------- views

/// The gateway id of this computer, or empty while no gateway ever ran here.
pub(crate) fn own_id(dir: &Path) -> String {
    #[derive(Deserialize)]
    struct Identity {
        #[serde(default)]
        gateway_id: String,
    }
    read_private_json::<Identity>(&dir.join("remote").join("gateway.json"), 4 * 1024)
        .map(|identity| identity.gateway_id)
        .unwrap_or_default()
}

/// This computer as it reports itself.
pub(crate) fn host_entry(dir: &Path, now: DateTime<Utc>) -> FleetEntry {
    let own = own_name(dir);
    FleetEntry {
        host_id: own_id(dir),
        name: own
            .as_ref()
            .map(|own| own.name.clone())
            .or_else(crate::remote_gateway::gateway::os_host_name)
            .unwrap_or_default(),
        name_updated_at: own.map(|own| own.updated_at),
        platform: crate::remote_gateway::gateway::host_platform().to_owned(),
        version: Some(crate::remote_gateway::gateway::host_version()),
        last_seen_at: Some(now),
        via: None,
    }
}

/// A machine this computer's app paired with, as `machines.json` keeps it.
/// Only what a fleet entry needs is read; unknown fields are ignored.
#[derive(Debug, Default, Deserialize)]
struct Paired {
    #[serde(default)]
    host_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    name_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    last_connected_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize)]
struct PairedFile {
    #[serde(default)]
    machines: Vec<Paired>,
}

/// The computers this one is paired with, as fleet entries. The name is the
/// one this app shows (its rename, if any), with the time of that rename.
pub(crate) fn paired_entries(dir: &Path) -> Vec<FleetEntry> {
    let file: PairedFile =
        read_private_json(&dir.join("machines.json"), MAX_FILE_BYTES).unwrap_or_default();
    file.machines
        .into_iter()
        .filter(|machine| !machine.host_id.is_empty())
        .map(|machine| {
            let renamed = machine
                .display_name
                .as_deref()
                .filter(|name| !name.is_empty());
            FleetEntry {
                host_id: machine.host_id,
                name: renamed.map_or(machine.name.clone(), str::to_owned),
                // A rename of the reported name back (a cleared nickname) still
                // carries its time, so it spreads like any other.
                name_updated_at: machine.name_updated_at,
                platform: if machine.platform.is_empty() {
                    unknown_platform()
                } else {
                    machine.platform
                },
                version: machine.version,
                last_seen_at: machine.last_connected_at,
                via: None,
            }
            .cleaned()
        })
        .collect()
}

/// What `_rustcode/environment/sync` answers: this computer, and every other
/// computer it knows of (the ones it is paired with and the ones it heard of).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FleetView {
    pub host: FleetEntry,
    /// The name the operating system gives this computer: what a cleared
    /// rename returns to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_name: Option<String>,
    pub entries: Vec<FleetEntry>,
}

/// Merge one report into this computer's state, then say what this computer
/// knows. `from` is the id of the computer that sent `incoming`, if it knows it.
pub(crate) fn sync(
    dir: &Path,
    from: &str,
    incoming: &[FleetEntry],
    now: DateTime<Utc>,
) -> anyhow::Result<FleetView> {
    let me = own_id(dir);
    // A report about this computer is a rename if it is newer than ours.
    for entry in incoming {
        let about_us = !me.is_empty() && entry.host_id == me;
        if about_us && let Some(stamp) = entry.name_updated_at {
            apply_own_name(dir, &entry.name, stamp)?;
        }
    }
    let mut fleet = Fleet::load(dir);
    if fleet.merge(from, incoming, &me, now) {
        fleet.save(dir)?;
    }
    // The computers this one is paired with are folded in last, so the app's
    // own rename of one of them (its stamp is in `machines.json`) takes part
    // in the same last-writer-wins as any other.
    let mut combined = fleet;
    let direct = paired_entries(dir);
    for entry in &direct {
        match combined
            .entries
            .iter_mut()
            .find(|known| known.host_id == entry.host_id)
        {
            Some(known) => {
                merge_entry(known, entry);
                // Our own record of a paired computer is first-hand: it is
                // not "learned through" anyone, and a nickname nobody stamped
                // (set before names were shared) stays ours.
                known.via = None;
                if known.name_updated_at.is_none() {
                    known.name = entry.name.clone();
                }
            }
            None => combined.entries.push(entry.clone()),
        }
    }
    let mut entries = combined.entries;
    entries.retain(|entry| entry.host_id != me);
    entries.sort_by(|left, right| left.host_id.cmp(&right.host_id));
    Ok(FleetView {
        host: host_entry(dir, now),
        os_name: crate::remote_gateway::gateway::os_host_name(),
        entries,
    })
}

/// The configuration directory, for the request handlers.
pub(crate) fn config_dir() -> Option<PathBuf> {
    crate::config::get_config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-13T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
            + Duration::minutes(minutes)
    }

    fn entry(host_id: &str, name: &str, renamed: Option<i64>, seen: Option<i64>) -> FleetEntry {
        FleetEntry {
            host_id: host_id.into(),
            name: name.into(),
            name_updated_at: renamed.map(at),
            platform: "macos".into(),
            version: Some("1.33.0".into()),
            last_seen_at: seen.map(at),
            via: None,
        }
    }

    #[test]
    fn the_newer_rename_wins_whichever_side_it_comes_from() {
        let mut older = entry("b", "Studio", Some(1), Some(1));
        let newer = entry("b", "Mac Studio", Some(5), Some(1));
        assert!(merge_entry(&mut older, &newer));
        assert_eq!(older.name, "Mac Studio");
        // The other way round, the older rename changes nothing.
        let mut current = newer.clone();
        let stale = entry("b", "Studio", Some(1), Some(1));
        assert!(!merge_entry(&mut current, &stale));
        assert_eq!(current.name, "Mac Studio");
        assert_eq!(current.name_updated_at, Some(at(5)));
    }

    #[test]
    fn a_tie_on_time_ends_on_the_greater_name_everywhere() {
        let left = entry("b", "Alpha", Some(3), None);
        let right = entry("b", "Beta", Some(3), None);
        let mut one = left.clone();
        merge_entry(&mut one, &right);
        let mut other = right.clone();
        merge_entry(&mut other, &left);
        assert_eq!(one.name, "Beta");
        assert_eq!(other.name, "Beta");
    }

    #[test]
    fn a_rename_beats_the_name_the_system_gave() {
        let mut given = entry("b", "studio.local", None, Some(1));
        let renamed = entry("b", "Desk", Some(2), Some(0));
        assert!(merge_entry(&mut given, &renamed));
        assert_eq!(given.name, "Desk");
        // And a name nobody chose never replaces one somebody did.
        let mut chosen = renamed.clone();
        assert!(!merge_entry(
            &mut chosen,
            &entry("b", "studio.local", None, Some(9))
        ));
        assert_eq!(chosen.name, "Desk");
    }

    #[test]
    fn version_and_last_seen_follow_the_latest_report_not_the_name() {
        let mut known = entry("b", "Desk", Some(5), Some(1));
        let mut report = entry("b", "Old name", Some(2), Some(9));
        report.version = Some("1.34.0".into());
        merge_entry(&mut known, &report);
        assert_eq!(known.name, "Desk", "the name keeps its own clock");
        assert_eq!(known.version.as_deref(), Some("1.34.0"));
        assert_eq!(known.last_seen_at, Some(at(9)));
    }

    #[test]
    fn learning_through_a_computer_marks_the_route_and_never_stores_ourselves() {
        let mut fleet = Fleet::default();
        let told = [
            entry("b", "B", None, Some(1)),
            entry("c", "C", Some(2), Some(1)),
            entry("a", "A", Some(9), Some(1)),
            entry("", "nameless", None, None),
        ];
        assert!(fleet.merge("b", &told, "a", at(10)));
        assert_eq!(fleet.entries.len(), 2);
        assert_eq!(fleet.get("b").unwrap().via, None, "told by itself");
        assert_eq!(fleet.get("c").unwrap().via.as_deref(), Some("b"));
        assert!(
            fleet.get("a").is_none(),
            "this computer is not in its own fleet"
        );
        // The same report again changes nothing.
        assert!(!fleet.merge("b", &told, "a", at(10)));
    }

    #[test]
    fn a_sender_cannot_name_the_computer_it_was_learned_through() {
        let mut spoofed = entry("c", "C", None, Some(1));
        spoofed.via = Some("someone-trusted".into());
        let mut fleet = Fleet::default();
        fleet.merge("b", &[spoofed], "a", at(10));
        assert_eq!(fleet.get("c").unwrap().via.as_deref(), Some("b"));
    }

    #[test]
    fn a_last_seen_in_the_future_is_clamped_and_old_computers_are_forgotten() {
        let mut fleet = Fleet::default();
        fleet.merge(
            "b",
            &[
                entry("c", "C", None, Some(60 * 24 * 365)),
                entry("d", "D", None, Some(-60 * 24 * 200)),
            ],
            "a",
            at(0),
        );
        assert_eq!(fleet.get("c").unwrap().last_seen_at, Some(at(0)));
        assert!(fleet.get("d").is_none(), "unseen for months");
    }

    #[test]
    fn names_are_bounded_and_printable() {
        let hostile = entry("c", &format!("a\u{7}b{}", "x".repeat(500)), Some(1), None);
        let cleaned = hostile.cleaned();
        assert!(cleaned.name.starts_with("ab"));
        assert_eq!(cleaned.name.chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn own_name_takes_only_a_newer_rename_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        assert!(apply_own_name(dir.path(), "Desk", at(5)).unwrap());
        assert!(!apply_own_name(dir.path(), "Old", at(1)).unwrap());
        assert!(!apply_own_name(dir.path(), "Desk", at(5)).unwrap());
        assert!(apply_own_name(dir.path(), "Mac Studio", at(6)).unwrap());
        let stored = own_name(dir.path()).unwrap();
        assert_eq!(stored.name, "Mac Studio");
        assert_eq!(stored.updated_at, at(6));
        assert!(
            !apply_own_name(dir.path(), "   ", at(99)).unwrap(),
            "blank is no name"
        );
    }

    fn write_identity(dir: &Path, id: &str) {
        std::fs::create_dir_all(dir.join("remote")).unwrap();
        std::fs::write(
            dir.join("remote").join("gateway.json"),
            format!(r#"{{"gateway_id":"{id}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn a_rename_announced_about_this_computer_is_adopted_and_spreads_on() {
        let dir = tempfile::tempdir().unwrap();
        write_identity(dir.path(), "b");
        // A (paired with this computer, B) renames B while B is away, then syncs.
        let view = sync(
            dir.path(),
            "a",
            &[
                entry("b", "Mac Studio", Some(5), Some(5)),
                entry("a", "Laptop", None, Some(5)),
            ],
            at(6),
        )
        .unwrap();
        assert_eq!(view.host.host_id, "b");
        assert_eq!(view.host.name, "Mac Studio");
        assert_eq!(view.host.name_updated_at, Some(at(5)));
        // C syncs with B later and learns the new name and that A exists.
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].host_id, "a");
        // An older rename arriving late changes nothing.
        let view = sync(
            dir.path(),
            "c",
            &[entry("b", "Studio", Some(2), None)],
            at(7),
        )
        .unwrap();
        assert_eq!(view.host.name, "Mac Studio");
    }

    #[test]
    fn a_paired_computers_rename_in_machines_json_joins_the_merge() {
        let dir = tempfile::tempdir().unwrap();
        write_identity(dir.path(), "b");
        std::fs::write(
            dir.path().join("machines.json"),
            r#"{"version":1,"machines":[{"host_id":"c","name":"mini","display_name":"Mac mini","name_updated_at":"2026-11-13T10:09:00Z","platform":"macos","version":"1.30.0","last_connected_at":"2026-11-13T10:01:00Z","routes":[],"device_id":"d","added_at":"2026-01-01T00:00:00Z"}]}"#,
        )
        .unwrap();
        // A report from A says C is called something older; ours is newer.
        let view = sync(
            dir.path(),
            "a",
            &[entry("c", "Old mini", Some(2), Some(0))],
            at(10),
        )
        .unwrap();
        let c = view.entries.iter().find(|e| e.host_id == "c").unwrap();
        assert_eq!(c.name, "Mac mini");
        assert_eq!(c.version.as_deref(), Some("1.30.0"));
        assert_eq!(c.via, None, "paired here, so first-hand");
        // The cached report about C is stored for when C is later unpaired.
        assert!(Fleet::load(dir.path()).get("c").is_some());
    }

    #[test]
    fn an_old_fleet_file_or_none_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Fleet::load(dir.path()).entries.is_empty());
        std::fs::write(dir.path().join(FLEET_FILE), "not json").unwrap();
        assert!(Fleet::load(dir.path()).entries.is_empty());
    }
}
