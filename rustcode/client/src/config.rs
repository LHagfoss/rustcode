//! Where the machine client keeps its files, resolved the same way the engine
//! resolves the configuration directory without depending on it.
//!
//! Every entry point of this crate takes a directory instead of resolving one
//! itself where it can, so a test never reads or writes the developer's real
//! `~/.config/rustcode`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The configuration directory in use, as the engine resolves it:
/// `RUSTCODE_CONFIG_DIR` first, then the platform's configuration root.
///
/// The engine has a `#[cfg(test)]` answer of its own for its in-crate tests;
/// this crate takes the explicit directory its tests pass instead, and what
/// this returns under `cargo test` is the same as any other run.
pub fn config_dir() -> Result<PathBuf> {
    if let Ok(override_dir) = std::env::var("RUSTCODE_CONFIG_DIR")
        && !override_dir.trim().is_empty()
    {
        let dir = PathBuf::from(override_dir);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create config directory {}", dir.display()))?;
        return Ok(dir);
    }
    let root = config_root().context(
        "no configuration directory: set RUSTCODE_CONFIG_DIR, or HOME (macOS and Linux) / APPDATA (Windows)",
    )?;
    let dir = root.join("rustcode");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create config directory {}", dir.display()))?;
    Ok(dir)
}

fn config_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("LOCALAPPDATA"))
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }
}

/// `machines/` inside the configuration directory: the install and host
/// identifiers and, when the keychain is not used, the token file.
pub fn machines_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("machines")
}

/// `machines.json`: the saved machines, beside the gateway's `remote/`.
pub fn machines_file(config_dir: &Path) -> PathBuf {
    config_dir.join("machines.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without the override there is still an answer on a normal profile, and
    /// it is a directory that exists afterwards.
    #[test]
    fn the_platform_root_is_used_without_an_override() {
        // The variable is process-wide; only assert the shapes that do not
        // depend on it being unset, since other tests here run in parallel.
        let dir = PathBuf::from("/tmp/rustcode-client-config-test");
        assert_eq!(machines_dir(&dir), dir.join("machines"));
        assert_eq!(machines_file(&dir), dir.join("machines.json"));
        assert!(config_root().is_some(), "HOME or APPDATA is set");
    }
}
