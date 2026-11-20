//! Test-process isolation for filesystem-backed configuration.
//!
//! Engine `#[cfg(test)]` shims (per-thread config dirs) do not apply when the
//! engine builds as a dependency, so without this the TUI suite would read
//! the developer's real configuration (themes, history, sessions) instead of
//! hermetic fixtures. Point it at a scratch dir once, before any test runs:
//! readers see deterministic empty state and writers cannot touch real data.
use std::path::PathBuf;
use std::sync::OnceLock;

static TEST_CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

#[ctor::ctor(unsafe)]
fn isolate_test_config() {
    let dir = std::env::temp_dir().join(format!("rustcode-tui-tests-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    TEST_CONFIG_DIR
        .set(dir.clone())
        .expect("test config set once");
    // SAFETY: runs at binary startup, before the test harness spawns threads,
    // and the value never changes afterwards, so no thread can observe a race.
    unsafe {
        std::env::set_var("RUSTCODE_CONFIG_DIR", &dir);
    }
}
