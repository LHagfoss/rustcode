//! Small platform/process helpers shared by tools and compiler checks.
//!
//! These functions intentionally have no dependency on the agent or network
//! layers.  Keeping executable-path construction here lets tool execution be
//! extracted into its own crate without introducing a reverse dependency on
//! the network implementation.

use std::path::{Path, PathBuf};

/// Return a stable executable search path for commands launched by RustCode's
/// tools.  The order and duplicate-preserving behavior are kept compatible
/// with the former network-owned helper.
pub(crate) fn augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    augmented_path_from(&home, std::env::var("PATH").ok().as_deref(), false)
}

/// Return the compiler-check variant of the search path.  Compiler checks have
/// historically included Bun and the current NVM node directory and removed
/// duplicates from PATH; retain that behavior while sharing ownership with
/// the tool-facing path helper.
pub(crate) fn compiler_augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    augmented_path_from(&home, std::env::var("PATH").ok().as_deref(), true)
}

fn augmented_path_from(home: &str, existing: Option<&str>, compiler_dirs: bool) -> String {
    let mut dirs = vec![format!("{home}/.cargo/bin")];
    if compiler_dirs {
        dirs.push(format!("{home}/.bun/bin"));
        dirs.push(format!("{home}/.nvm/versions/node/current/bin"));
    }
    dirs.extend([
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ]);
    if let Some(existing) = existing {
        for dir in existing.split(':') {
            if !compiler_dirs || !dirs.iter().any(|known| known == dir) {
                dirs.push(dir.to_string());
            }
        }
    }
    dirs.join(":")
}

/// Find a locally installed executable before falling back to PATH lookup.
/// This is used by compiler checks and belongs with the PATH construction
/// helper rather than with network transport.
pub(crate) fn resolve_bin(name: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.cargo/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
        format!("/usr/bin/{name}"),
    ];
    for candidate in candidates {
        if Path::new(&candidate).exists() {
            return PathBuf::from(candidate);
        }
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::{augmented_path, augmented_path_from};

    #[test]
    fn augmented_path_keeps_standard_tool_directories() {
        let path = augmented_path();
        let entries: Vec<&str> = path.split(':').collect();
        assert!(entries.iter().any(|entry| entry.ends_with("/.cargo/bin")));
        assert!(entries.contains(&"/usr/bin"));
        assert!(entries.contains(&"/bin"));
    }

    #[test]
    fn path_variants_preserve_tool_and_compiler_contracts() {
        assert_eq!(
            augmented_path_from("/home/test", Some("/custom/bin:/usr/bin"), false),
            "/home/test/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/custom/bin:/usr/bin"
        );
        assert_eq!(
            augmented_path_from("/home/test", Some("/custom/bin:/usr/bin"), true),
            "/home/test/.cargo/bin:/home/test/.bun/bin:/home/test/.nvm/versions/node/current/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/custom/bin"
        );
    }
}
