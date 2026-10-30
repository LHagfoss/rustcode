//! What RustCode keeps in its configuration directory and how much of it
//! there is (#2599): a read-only report of each store, what it holds and
//! how it is bounded, so a person or support tooling can see where the space
//! goes before anything is cleaned. Nothing here changes a file.

use std::path::{Path, PathBuf};

/// One store of the configuration directory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StoreUsage {
    pub name: &'static str,
    /// What the store holds, in a few words.
    pub holds: &'static str,
    /// How the store is bounded today, as the code does it.
    pub retention: &'static str,
    /// The entries counted, relative to the configuration directory.
    pub paths: Vec<String>,
    pub bytes: u64,
    pub files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StorageUsage {
    pub config_dir: PathBuf,
    pub stores: Vec<StoreUsage>,
    pub total_bytes: u64,
}

/// A store and the top-level entries it is made of.
struct Store {
    name: &'static str,
    holds: &'static str,
    retention: &'static str,
    /// Entry names; one ending in `*` matches by prefix.
    entries: &'static [&'static str],
}

const STORES: &[Store] = &[
    Store {
        name: "threads",
        holds: "each thread's history, logs and artifacts",
        retention: "kept; scratch threads are pruned",
        entries: &["threads"],
    },
    Store {
        name: "history archive",
        holds: "exact copies of history that compaction removed from a thread",
        retention: "kept",
        entries: &["history_archive"],
    },
    Store {
        name: "tool output",
        holds: "full output of commands too long for the conversation",
        retention: "kept; generated, referenced from thread histories",
        entries: &["tool_output"],
    },
    Store {
        name: "attachments",
        holds: "images attached to messages",
        retention: "kept; referenced from thread histories",
        entries: &["attachments"],
    },
    Store {
        name: "workspaces",
        holds: "isolated workspaces of subagents",
        retention: "kept",
        entries: &["workspaces"],
    },
    Store {
        name: "backups",
        holds: "copies of config files taken before a change",
        retention: "kept",
        entries: &["backups", "*.bak"],
    },
    Store {
        name: "logs",
        holds: "the engine's debug log",
        retention: "rotated: the current log and one older one",
        entries: &["debug.log", "debug.log.1"],
    },
    Store {
        name: "indexes",
        holds: "the symbol index and saved memory",
        retention: "kept",
        entries: &["symbols.db*", "project-memory", "global-memory.json"],
    },
    Store {
        name: "sync history",
        holds: "the Git history of `rustcode sync`",
        retention: "kept; grows with each sync",
        entries: &[".git"],
    },
    Store {
        name: "remote",
        holds: "paired devices and the remote gateway's state",
        retention: "kept",
        entries: &["remote"],
    },
];

fn matches(pattern: &str, name: &str) -> bool {
    match (pattern.strip_prefix('*'), pattern.strip_suffix('*')) {
        (Some(suffix), _) => name.ends_with(suffix),
        (_, Some(prefix)) => name.starts_with(prefix),
        _ => name == pattern,
    }
}

/// Bytes and files under `path`, not following symlinks: `sessions` is a
/// link to `threads` after the folder's rename and is not counted twice.
fn measure(path: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return (0, 0);
    };
    if meta.file_type().is_symlink() {
        return (0, 0);
    }
    if !meta.is_dir() {
        return (meta.len(), 1);
    }
    let mut bytes = 0;
    let mut files = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let (b, f) = measure(&entry.path());
            bytes += b;
            files += f;
        }
    }
    (bytes, files)
}

/// The usage of `config_dir`: each known store, then everything else as
/// "other", largest first.
pub fn usage(config_dir: &Path) -> StorageUsage {
    let mut names: Vec<String> = std::fs::read_dir(config_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut stores = Vec::new();
    for store in STORES {
        let paths: Vec<String> = names
            .iter()
            .filter(|name| store.entries.iter().any(|pattern| matches(pattern, name)))
            .cloned()
            .collect();
        names.retain(|name| !paths.contains(name));
        let (bytes, files) = paths
            .iter()
            .map(|name| measure(&config_dir.join(name)))
            .fold((0, 0), |(b, f), (b2, f2)| (b + b2, f + f2));
        stores.push(StoreUsage {
            name: store.name,
            holds: store.holds,
            retention: store.retention,
            paths,
            bytes,
            files,
        });
    }
    let (bytes, files) = names
        .iter()
        .map(|name| measure(&config_dir.join(name)))
        .fold((0, 0), |(b, f), (b2, f2)| (b + b2, f + f2));
    stores.push(StoreUsage {
        name: "other",
        holds: "configuration, credentials, models and locks",
        retention: "kept",
        paths: names,
        bytes,
        files,
    });
    stores.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    let total_bytes = stores.iter().map(|store| store.bytes).sum();
    StorageUsage {
        config_dir: config_dir.to_path_buf(),
        stores,
        total_bytes,
    }
}

/// `421.3 MB`.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

impl StorageUsage {
    pub fn render(&self) -> String {
        let mut out = format!(
            "RustCode data in {}: {}\n\n",
            self.config_dir.display(),
            size(self.total_bytes)
        );
        for store in &self.stores {
            if store.paths.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "{:>10}  {:<16} {} files. {}; {}.\n",
                size(store.bytes),
                store.name,
                store.files,
                store.holds,
                store.retention
            ));
        }
        out.push_str("\nNothing was changed. Use --json for the paths of each store.\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_counts_each_store_once_and_leaves_the_rest_as_other() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("threads/2026/10/30/a")).unwrap();
        std::fs::write(root.join("threads/2026/10/30/a/history.json"), vec![0; 300]).unwrap();
        std::fs::create_dir_all(root.join("tool_output")).unwrap();
        std::fs::write(root.join("tool_output/x.txt"), vec![0; 1000]).unwrap();
        std::fs::write(root.join("debug.log"), vec![0; 50]).unwrap();
        std::fs::write(root.join("config.json.before-x.bak"), vec![0; 7]).unwrap();
        std::fs::write(root.join("config.toml"), vec![0; 20]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("threads"), root.join("sessions")).unwrap();

        let usage = usage(root);
        let store = |name: &str| {
            usage
                .stores
                .iter()
                .find(|s| s.name == name)
                .unwrap()
                .clone()
        };
        assert_eq!(store("threads").bytes, 300);
        assert_eq!(store("threads").files, 1);
        assert_eq!(store("tool output").bytes, 1000);
        assert_eq!(store("logs").paths, vec!["debug.log"]);
        assert_eq!(store("backups").paths, vec!["config.json.before-x.bak"]);
        // The renamed folder's link counts nothing; only config.toml is other.
        assert_eq!(store("other").bytes, 20);
        assert_eq!(usage.total_bytes, 300 + 1000 + 50 + 7 + 20);
        assert_eq!(usage.stores[0].name, "tool output", "largest first");
        let text = usage.render();
        assert!(text.contains("Nothing was changed"));
        assert!(
            !text.contains("workspaces"),
            "an absent store is not listed"
        );
    }

    #[test]
    fn sizes_read_in_decimal_units() {
        assert_eq!(size(999), "999 B");
        assert_eq!(size(421_300_000), "421.3 MB");
    }
}
