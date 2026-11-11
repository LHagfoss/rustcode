//! The user's home folder, and the `~` forms paths are written in.
//!
//! Windows keeps the home folder in `USERPROFILE`, not `HOME`, so a site that
//! reads `HOME` alone finds nothing there and skills, usage logs and the remote
//! gateway's home root silently go missing (#3467). This module is the one
//! place that answers the question, and the one place that turns `~` into a
//! folder and back.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The user's home folder: `HOME` (Git Bash and MSYS set it on Windows too),
/// else `USERPROFILE`, else the `HOMEDRIVE`/`HOMEPATH` pair a Windows process
/// started with a stripped environment still carries.
pub fn home_dir() -> Option<PathBuf> {
    home_dir_from(|name| std::env::var_os(name))
}

/// [`home_dir`] with the environment given, so a test can pass one in instead
/// of mutating the process's own.
pub fn home_dir_from(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(named) = ["HOME", "USERPROFILE"]
        .into_iter()
        .find_map(|name| var(name).filter(|value| !value.is_empty()))
    {
        return Some(PathBuf::from(named));
    }
    let mut drive = var("HOMEDRIVE").filter(|value| !value.is_empty())?;
    drive.push(var("HOMEPATH").filter(|value| !value.is_empty())?);
    Some(PathBuf::from(drive))
}

/// `~`, `~/…` and, on Windows, `~\…` relative to `home`; anything else, and
/// anything written with no known home, as written.
pub fn expand_home(value: &str, home: Option<&Path>) -> PathBuf {
    match home {
        Some(home) => expand_home_in(value, home, cfg!(windows)),
        None => PathBuf::from(value),
    }
}

/// [`expand_home`] with the platform given, so a test can cover both: a
/// backslash is a file name character on Unix, where `~\x` is not a home path.
fn expand_home_in(value: &str, home: &Path, windows: bool) -> PathBuf {
    let Some(rest) = value.strip_prefix('~') else {
        return PathBuf::from(value);
    };
    if rest.is_empty() {
        return home.to_path_buf();
    }
    let rest = rest.strip_prefix('/').or_else(|| {
        if windows {
            rest.strip_prefix('\\')
        } else {
            None
        }
    });
    let Some(rest) = rest else {
        // `~someone`, which is another user's folder, not this one.
        return PathBuf::from(value);
    };
    if rest.is_empty() {
        home.to_path_buf()
    } else {
        home.join(rest)
    }
}

/// `path` with a leading home folder written as `~`. The cut is made on path
/// components, so a home of `/home/a` does not shorten `/home/ab/x`. A path
/// outside the home folder, or no known home, is returned as written.
pub fn contract_home(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home.filter(|home| !home.as_os_str().is_empty()) else {
        return path.to_path_buf();
    };
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => PathBuf::from("~"),
        Ok(rest) => PathBuf::from("~").join(rest),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_home_folder_is_home_then_userprofile() {
        assert_eq!(
            home_dir_from(env(&[("HOME", "/home/me"), ("USERPROFILE", "C:/Users/me")])),
            Some(PathBuf::from("/home/me"))
        );
        // Windows sets `USERPROFILE` and normally leaves `HOME` unset.
        assert_eq!(
            home_dir_from(env(&[("USERPROFILE", "C:/Users/me")])),
            Some(PathBuf::from("C:/Users/me"))
        );
        // Git Bash sets `HOME` to a shell path; it wins over `USERPROFILE`.
        assert_eq!(
            home_dir_from(env(&[
                ("HOME", "/c/Users/me"),
                ("USERPROFILE", "C:/Users/me")
            ])),
            Some(PathBuf::from("/c/Users/me"))
        );
        // A blank value is no value.
        assert_eq!(
            home_dir_from(env(&[("HOME", ""), ("USERPROFILE", "C:/Users/me")])),
            Some(PathBuf::from("C:/Users/me"))
        );
        assert_eq!(
            home_dir_from(env(&[("HOMEDRIVE", "C:"), ("HOMEPATH", "\\Users\\me")])),
            Some(PathBuf::from("C:\\Users\\me"))
        );
        assert_eq!(home_dir_from(env(&[])), None);
    }

    #[test]
    fn tilde_expands_with_either_separator() {
        let home = Path::new("/home/me");
        assert_eq!(expand_home_in("~", home, false), PathBuf::from("/home/me"));
        assert_eq!(expand_home_in("~", home, true), PathBuf::from("/home/me"));
        assert_eq!(expand_home_in("~/", home, true), PathBuf::from("/home/me"));
        assert_eq!(
            expand_home_in("~/code/app", home, false),
            home.join("code/app")
        );
        assert_eq!(expand_home_in("~\\code", home, true), home.join("code"));
        assert_eq!(expand_home_in("~\\", home, true), PathBuf::from("/home/me"));
        // A backslash names a file on Unix, so `~\code` is not a home path.
        assert_eq!(
            expand_home_in("~\\code", home, false),
            PathBuf::from("~\\code")
        );
        // Another user's folder, and a path that names no home, stay as written.
        assert_eq!(
            expand_home_in("~bob/code", home, true),
            PathBuf::from("~bob/code")
        );
        assert_eq!(
            expand_home_in("/etc/hosts", home, true),
            PathBuf::from("/etc/hosts")
        );
        assert_eq!(expand_home("~/code", None), PathBuf::from("~/code"));
    }

    #[test]
    fn home_is_contracted_only_at_a_folder_boundary() {
        let home = Path::new("/home/a");
        assert_eq!(
            contract_home(Path::new("/home/a"), Some(home)),
            PathBuf::from("~")
        );
        assert_eq!(
            contract_home(Path::new("/home/a/code/app"), Some(home)),
            Path::new("~").join("code/app")
        );
        // `/home/ab/x` is another folder, not a file in the home one.
        assert_eq!(
            contract_home(Path::new("/home/ab/x"), Some(home)),
            PathBuf::from("/home/ab/x")
        );
        assert_eq!(
            contract_home(Path::new("/work/x"), Some(home)),
            PathBuf::from("/work/x")
        );
        // No home, or an empty one, shortens nothing.
        assert_eq!(
            contract_home(Path::new("/home/a/code"), None),
            PathBuf::from("/home/a/code")
        );
        assert_eq!(
            contract_home(Path::new("/home/a/code"), Some(Path::new(""))),
            PathBuf::from("/home/a/code")
        );
    }
}
