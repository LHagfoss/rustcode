//! App version values and self-update state types.
//!
//! Pure version arithmetic, platform install detection, and release-asset
//! mapping shared by the update checker, the update prompt, and every
//! frontend. Network I/O (checking, downloading, replacing the binary) stays
//! in the engine; the running binary's own version stays there too, because
//! `env!("CARGO_PKG_VERSION")` must resolve to the shipping binary, not this
//! library.

pub const BREW_UPDATE_COMMAND: &str = "brew update";
pub const BREW_UPGRADE_COMMAND: &str = "brew upgrade rustcode";

/// A semantic version as `(major, minor, patch)`. Ordered field-by-field, so
/// tuple comparison is the version comparison.
pub type Version = (u32, u32, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate { current: Version, latest: Version },
    Available { current: Version, latest: Version },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    Unknown,
    Checking,
    UpToDate(Version),
    Available(Version),
    Failed,
}

pub fn format_version(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

pub fn parse_semver(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    // The patch segment may carry a pre-release/build suffix (e.g. "3-beta");
    // keep only the leading digits.
    let patch_digits: String = it
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

/// Detect if the current binary is installed via Homebrew.
pub fn is_brew_install() -> bool {
    if cfg!(target_os = "windows") {
        return false;
    }
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.to_string_lossy();
        if path.contains("/Cellar/rustcode")
            || path.contains("/opt/homebrew/")
            || path.contains("/usr/local/Cellar/")
            || path.contains("/home/linuxbrew/")
        {
            return true;
        }
    }
    false
}

/// Expected asset name for the current platform/architecture.
pub fn target_asset_name() -> Option<&'static str> {
    target_asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn target_asset_name_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("rustcode-linux-x86_64.tar.gz"),
        ("macos", "aarch64") => Some("rustcode-macos-aarch64.tar.gz"),
        ("windows", "x86_64") => Some("rustcode-windows-x86_64.zip"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering_is_field_wise() {
        assert!((0, 5, 0) < (0, 5, 1));
        assert!((0, 5, 9) < (0, 6, 0));
        assert!((1, 0, 0) > (0, 99, 99));
    }

    #[test]
    fn update_command_is_the_formula_upgrade() {
        assert_eq!(BREW_UPDATE_COMMAND, "brew update");
        assert_eq!(BREW_UPGRADE_COMMAND, "brew upgrade rustcode");
    }

    #[test]
    fn target_asset_detection() {
        assert!(target_asset_name().is_some());
    }

    #[test]
    fn target_asset_mapping_covers_supported_platforms() {
        assert_eq!(
            target_asset_name_for("linux", "x86_64"),
            Some("rustcode-linux-x86_64.tar.gz")
        );
        assert_eq!(
            target_asset_name_for("macos", "aarch64"),
            Some("rustcode-macos-aarch64.tar.gz")
        );
        assert_eq!(target_asset_name_for("macos", "x86_64"), None);
        assert_eq!(
            target_asset_name_for("windows", "x86_64"),
            Some("rustcode-windows-x86_64.zip")
        );
        assert_eq!(target_asset_name_for("windows", "aarch64"), None);
    }
}
