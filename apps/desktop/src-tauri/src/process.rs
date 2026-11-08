//! Process creation for non-interactive desktop helpers.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(unused_mut))]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(target_os = "linux")]
    for (name, value) in appimage_env() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command
}

/// [`appimage_env_fixes`] for this process, worked out once.
#[cfg(target_os = "linux")]
fn appimage_env() -> &'static [(OsString, Option<OsString>)] {
    static FIXES: OnceLock<Vec<(OsString, Option<OsString>)>> = OnceLock::new();
    FIXES.get_or_init(|| appimage_env_fixes(std::env::vars_os(), std::env::var_os("APPDIR")))
}

/// What a helper's environment loses when the app runs as an AppImage.
///
/// The AppImage launcher points `LD_LIBRARY_PATH` (and `PATH`, `XDG_DATA_DIRS`,
/// the GTK module paths, …) into its mount so the webview finds the bundled
/// GTK and WebKit libraries. Nothing the app starts needs them: the RustCode
/// server, `xdg-open` and git are system programs. But they inherit them, and
/// so does everything the server starts in turn: a harness run on the system
/// `node` (Claude Code and Pi through `npx`) loads the bundle's older
/// `libcrypto`/`libffi` and exits at once with
/// "version `OPENSSL_3.4.0' not found (required by node)".
///
/// Each variable with an entry inside `appdir` gets that entry dropped
/// (`Some` with what is left), or is removed (`None`) when nothing is left.
/// Outside an AppImage (`appdir` unset) nothing changes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn appimage_env_fixes(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    appdir: Option<OsString>,
) -> Vec<(OsString, Option<OsString>)> {
    let Some(appdir) = appdir.filter(|dir| !dir.is_empty()) else {
        return Vec::new();
    };
    let appdir = Path::new(&appdir);
    let inside = |entry: &Path| entry.starts_with(appdir);
    let mut fixes = Vec::new();
    for (name, value) in vars {
        let entries: Vec<PathBuf> = std::env::split_paths(&value).collect();
        if !entries.iter().any(|entry| inside(entry)) {
            continue;
        }
        let kept: Vec<PathBuf> = entries
            .into_iter()
            .filter(|entry| !entry.as_os_str().is_empty() && !inside(entry))
            .collect();
        let value = if kept.is_empty() {
            None
        } else {
            std::env::join_paths(kept).ok()
        };
        fixes.push((name, value));
    }
    fixes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn leaves_the_environment_alone_outside_an_appimage() {
        let env = vars(&[("LD_LIBRARY_PATH", "/opt/lib"), ("PATH", "/usr/bin")]);
        assert!(appimage_env_fixes(env, None).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn drops_the_appimage_mount_from_inherited_paths() {
        let mount = "/tmp/.mount_RustCoJpCJlA";
        let env = vars(&[
            (
                "LD_LIBRARY_PATH",
                "/tmp/.mount_RustCoJpCJlA/usr/lib/:/tmp/.mount_RustCoJpCJlA/lib64/:",
            ),
            (
                "PATH",
                "/tmp/.mount_RustCoJpCJlA/usr/bin/:/home/u/.local/bin:/usr/bin",
            ),
            (
                "XDG_DATA_DIRS",
                "/tmp/.mount_RustCoJpCJlA/usr/share:/usr/share",
            ),
            ("GTK_PATH", "/tmp/.mount_RustCoJpCJlA//usr/lib/gtk-3.0"),
            ("APPDIR", mount),
            ("APPIMAGE", "/home/u/Applications/RustCode.AppImage"),
            ("GTK_THEME", "Adwaita:dark"),
            ("HOME", "/home/u"),
            // A look-alike prefix is not inside the mount.
            ("OTHER", "/tmp/.mount_RustCoJpCJlAx/lib"),
        ]);
        let mut fixes = appimage_env_fixes(env, Some(mount.into()));
        fixes.sort();
        let expected: Vec<(OsString, Option<OsString>)> = vec![
            ("APPDIR".into(), None),
            ("GTK_PATH".into(), None),
            ("LD_LIBRARY_PATH".into(), None),
            ("PATH".into(), Some("/home/u/.local/bin:/usr/bin".into())),
            ("XDG_DATA_DIRS".into(), Some("/usr/share".into())),
        ];
        assert_eq!(fixes, expected);
    }
}
