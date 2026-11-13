//! Builds of parallel delegates. Several delegates compiling a Rust project in
//! worktrees at once each start from an empty `target/`, queue on cargo's
//! package-cache and build-directory locks for minutes, and drain the laptop.
//!
//! A harness started in a RustCode worktree of a Rust project therefore gets:
//!
//! - a shared build directory per project outside the worktree
//!   (`CARGO_TARGET_DIR`), so a new worktree starts from a warm one;
//! - low priority, so a build does not slow the machine the user works on
//!   (`nice` on macOS and Linux, below normal on Windows). Not the macOS
//!   background policy: it confines a process to the efficiency cores and
//!   made a delegate's builds 2–5× slower;
//! - on Unix a `cargo` wrapper first on `PATH` ([`SHIM`]): at most
//!   [`DEFAULT_SLOTS`] builds run at once across all delegates, and each runs
//!   in a target directory of its own (`slot-N`), so no two share a lock. A
//!   delegate that waits says "waiting for a build slot" instead of going
//!   silent.
//!
//! A user who set `CARGO_TARGET_DIR` keeps it, and anything outside a worktree
//! of a Rust project is untouched.

use std::path::{Path, PathBuf};

/// Builds that may run at once across delegates: `RUSTCODE_BUILD_SLOTS`.
pub(crate) const DEFAULT_SLOTS: usize = 2;
pub(crate) const SLOTS_ENV: &str = "RUSTCODE_BUILD_SLOTS";
const TARGET_ENV: &str = "CARGO_TARGET_DIR";

/// The `cargo` wrapper that queues builds for a slot (`cargo_slot.sh`).
const SHIM: &str = include_str!("cargo_slot.sh");

/// How the harness process and everything it starts is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    /// The operating system's default.
    Normal,
    /// Unix: `nice` by this much.
    Nice(i32),
    /// Windows: `BELOW_NORMAL_PRIORITY_CLASS`.
    BelowNormal,
}

impl Priority {
    /// The background priority `os` (`std::env::consts::OS`) offers.
    pub(crate) fn background_for(os: &str) -> Self {
        match os {
            "windows" => Self::BelowNormal,
            "macos" | "linux" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" => Self::Nice(10),
            _ => Self::Normal,
        }
    }
}

/// What a worktree build needs of the harness's launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub env: Vec<(String, String)>,
    pub priority: Priority,
}

/// Everything [`plan`] decides from, so each operating system is testable.
pub(crate) struct Inputs<'a> {
    /// The harness's working directory.
    pub cwd: &'a Path,
    /// Where RustCode puts worktrees (`worktrees::root`).
    pub worktrees: &'a Path,
    /// Where builds live: `<config>/build`.
    pub build: &'a Path,
    /// `std::env::consts::OS`.
    pub os: &'a str,
    /// How many builds may run at once.
    pub slots: usize,
    /// The directory holding the `cargo` wrapper, where the OS can run it.
    pub shim_dir: Option<&'a Path>,
    /// The `PATH` the harness would get.
    pub path: &'a std::ffi::OsStr,
    /// Whether the user already chose a `CARGO_TARGET_DIR`.
    pub target_dir_set: bool,
}

/// The first directory below `worktrees` that `cwd` is in: the project the
/// worktree belongs to.
fn project_of(cwd: &Path, worktrees: &Path) -> Option<String> {
    let relative = cwd.strip_prefix(worktrees).ok()?;
    // `<project>/<worktree>`: a thread in the project directory itself is no worktree.
    let mut parts = relative.components();
    let project = parts.next()?.as_os_str().to_str()?.to_owned();
    parts.next()?;
    Some(project)
}

/// The launch changes for a harness in `inputs.cwd`, or `None` outside a
/// worktree of a Rust project.
pub(crate) fn plan(inputs: &Inputs<'_>) -> Option<Plan> {
    let project = project_of(inputs.cwd, inputs.worktrees)?;
    if !inputs.cwd.join("Cargo.toml").is_file() {
        return None;
    }
    let root = inputs.build.join(&project);
    let mut env = Vec::new();
    if !inputs.target_dir_set {
        env.push((
            TARGET_ENV.to_owned(),
            root.join("slot-1").to_string_lossy().into_owned(),
        ));
    }
    if let Some(shim) = inputs.shim_dir {
        let slots = inputs.slots.max(1);
        env.push((
            "RUSTCODE_BUILD_ROOT".to_owned(),
            root.to_string_lossy().into_owned(),
        ));
        env.push((
            "RUSTCODE_BUILD_LOCKS".to_owned(),
            inputs.build.join("locks").to_string_lossy().into_owned(),
        ));
        env.push((SLOTS_ENV.to_owned(), slots.to_string()));
        env.push((
            "RUSTCODE_BUILD_SHIM_DIR".to_owned(),
            shim.to_string_lossy().into_owned(),
        ));
        let mut dirs = vec![shim.to_path_buf()];
        dirs.extend(std::env::split_paths(inputs.path));
        if let Ok(path) = std::env::join_paths(dirs) {
            env.push(("PATH".to_owned(), path.to_string_lossy().into_owned()));
        }
    }
    Some(Plan {
        env,
        priority: Priority::background_for(inputs.os),
    })
}

/// Write the wrapper into `dir` (replacing an older copy) and return `dir`.
#[cfg(unix)]
fn install_shim(dir: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(dir)?;
    let path = dir.join("cargo");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(SHIM) {
        let staged = dir.join(format!(".cargo.{}", std::process::id()));
        std::fs::write(&staged, SHIM)?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&staged, &path)?;
    }
    Ok(dir.to_path_buf())
}

/// The slots to allow: `RUSTCODE_BUILD_SLOTS`, else [`DEFAULT_SLOTS`].
fn configured_slots() -> usize {
    std::env::var(SLOTS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|slots| *slots > 0)
        .unwrap_or(DEFAULT_SLOTS)
}

/// The plan for a harness about to start in `cwd`, from this machine's
/// settings. `env` is what the harness already gets: a `CARGO_TARGET_DIR` in
/// it wins.
pub(crate) fn for_launch(cwd: &Path, env: &[(String, String)]) -> Option<Plan> {
    let worktrees = crate::worktrees::root()?;
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let worktrees = std::fs::canonicalize(&worktrees).unwrap_or(worktrees);
    // Most threads are no Rust worktree: leave the config directory alone for them.
    project_of(&canonical, &worktrees)?;
    if !canonical.join("Cargo.toml").is_file() {
        return None;
    }
    let build = crate::config::get_config_dir()?.join("build");
    #[cfg(unix)]
    let shim_dir = install_shim(&build.join("bin")).ok();
    #[cfg(not(unix))]
    let shim_dir: Option<PathBuf> = None;
    let target_dir_set =
        std::env::var_os(TARGET_ENV).is_some() || env.iter().any(|(name, _)| name == TARGET_ENV);
    plan(&Inputs {
        cwd: &canonical,
        worktrees: &worktrees,
        build: &build,
        os: std::env::consts::OS,
        slots: configured_slots(),
        shim_dir: shim_dir.as_deref(),
        path: &super::detect::search_path(),
        target_dir_set,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let worktrees = dir.path().join("worktrees");
        let cwd = worktrees.join("rustcode").join("feat-x");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("Cargo.toml"), "[package]\n").unwrap();
        (dir, worktrees, cwd)
    }

    fn inputs<'a>(
        cwd: &'a Path,
        worktrees: &'a Path,
        build: &'a Path,
        os: &'a str,
        shim: Option<&'a Path>,
        target_dir_set: bool,
    ) -> Inputs<'a> {
        Inputs {
            cwd,
            worktrees,
            build,
            os,
            slots: 2,
            shim_dir: shim,
            path: std::ffi::OsStr::new(if cfg!(windows) {
                "C:\\bin"
            } else {
                "/usr/bin:/bin"
            }),
            target_dir_set,
        }
    }

    fn value<'a>(plan: &'a Plan, name: &str) -> Option<&'a str> {
        plan.env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn each_os_gets_its_own_background_priority() {
        assert_eq!(Priority::background_for("macos"), Priority::Nice(10));
        assert_eq!(Priority::background_for("linux"), Priority::Nice(10));
        assert_eq!(Priority::background_for("windows"), Priority::BelowNormal);
        assert_eq!(Priority::background_for("haiku"), Priority::Normal);
    }

    #[test]
    fn a_rust_worktree_gets_a_project_build_dir_slots_and_the_wrapper() {
        let (dir, worktrees, cwd) = rust_worktree();
        let build = dir.path().join("build");
        let shim = build.join("bin");
        let plan = plan(&inputs(
            &cwd,
            &worktrees,
            &build,
            "macos",
            Some(&shim),
            false,
        ))
        .unwrap();
        assert_eq!(
            value(&plan, "CARGO_TARGET_DIR").map(PathBuf::from),
            Some(build.join("rustcode").join("slot-1"))
        );
        assert_eq!(
            value(&plan, "RUSTCODE_BUILD_ROOT").map(PathBuf::from),
            Some(build.join("rustcode"))
        );
        assert_eq!(
            value(&plan, "RUSTCODE_BUILD_LOCKS").map(PathBuf::from),
            Some(build.join("locks"))
        );
        assert_eq!(value(&plan, "RUSTCODE_BUILD_SLOTS"), Some("2"));
        let path = std::env::split_paths(value(&plan, "PATH").unwrap()).collect::<Vec<_>>();
        assert_eq!(path.first(), Some(&shim), "the wrapper comes first");
        assert!(path.len() >= 2, "the rest of PATH follows");
        assert_eq!(plan.priority, Priority::Nice(10));
    }

    #[test]
    fn windows_has_no_wrapper_but_keeps_the_shared_dir_and_priority() {
        let (dir, worktrees, cwd) = rust_worktree();
        let build = dir.path().join("build");
        let plan = plan(&inputs(&cwd, &worktrees, &build, "windows", None, false)).unwrap();
        assert!(value(&plan, "CARGO_TARGET_DIR").is_some());
        assert!(value(&plan, "PATH").is_none());
        assert!(value(&plan, "RUSTCODE_BUILD_ROOT").is_none());
        assert_eq!(plan.priority, Priority::BelowNormal);
    }

    #[test]
    fn linux_is_niced() {
        let (dir, worktrees, cwd) = rust_worktree();
        let build = dir.path().join("build");
        let shim = build.join("bin");
        let plan = plan(&inputs(
            &cwd,
            &worktrees,
            &build,
            "linux",
            Some(&shim),
            false,
        ))
        .unwrap();
        assert_eq!(plan.priority, Priority::Nice(10));
    }

    #[test]
    fn a_target_dir_the_user_chose_is_kept() {
        let (dir, worktrees, cwd) = rust_worktree();
        let build = dir.path().join("build");
        let shim = build.join("bin");
        let plan = plan(&inputs(
            &cwd,
            &worktrees,
            &build,
            "linux",
            Some(&shim),
            true,
        ))
        .unwrap();
        assert!(value(&plan, "CARGO_TARGET_DIR").is_none());
        assert_eq!(value(&plan, "RUSTCODE_BUILD_SLOTS"), Some("2"));
    }

    #[test]
    fn only_a_worktree_of_a_rust_project_is_planned() {
        let (dir, worktrees, cwd) = rust_worktree();
        let build = dir.path().join("build");
        // A checkout elsewhere.
        let elsewhere = dir.path().join("checkout");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("Cargo.toml"), "").unwrap();
        assert!(
            plan(&inputs(
                &elsewhere, &worktrees, &build, "linux", None, false
            ))
            .is_none()
        );
        // A worktree without a Cargo.toml.
        let web = worktrees.join("site").join("feat-y");
        std::fs::create_dir_all(&web).unwrap();
        assert!(plan(&inputs(&web, &worktrees, &build, "linux", None, false)).is_none());
        // The project directory itself is no worktree.
        let project = worktrees.join("rustcode");
        std::fs::write(project.join("Cargo.toml"), "").unwrap();
        assert!(plan(&inputs(&project, &worktrees, &build, "linux", None, false)).is_none());
        assert!(plan(&inputs(&cwd, &worktrees, &build, "linux", None, false)).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn the_wrapper_is_installed_executable_and_replaced_when_it_changes() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("cargo"), "old").unwrap();
        install_shim(&bin).unwrap();
        let text = std::fs::read_to_string(bin.join("cargo")).unwrap();
        assert_eq!(text, SHIM);
        let mode = std::fs::metadata(bin.join("cargo"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
    }
}
