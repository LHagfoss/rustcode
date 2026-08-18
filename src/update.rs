//! Self-update check against the Homebrew tap `lhagfoss/tap` (formula `rustcode`).
//!
//! `/update` reads the formula straight from the tap repo on GitHub, compares
//! its version to the running binary, and — only when the tap is newer — runs
//! `brew upgrade rustcode`.

use regex::Regex;
use std::process::Command;
use std::sync::LazyLock;

/// The formula lives in the tap repo `lhagfoss/homebrew-tap` at
/// `Formula/rustcode.rb`. Read it directly from GitHub raw so the check works
/// even before `brew tap lhagfoss/tap` has been run locally. Try `main` first,
/// then `master`, since taps differ on their default branch name.
const FORMULA_URLS: [&str; 2] = [
    "https://raw.githubusercontent.com/lhagfoss/homebrew-tap/main/Formula/rustcode.rb",
    "https://raw.githubusercontent.com/lhagfoss/homebrew-tap/master/Formula/rustcode.rb",
];

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

pub async fn check_for_update(client: &reqwest::Client) -> Result<UpdateCheck, String> {
    let current = current_version();
    let latest = latest_tap_version(client)
        .await
        .ok_or_else(|| "couldn't read the Homebrew tap".to_string())?;
    Ok(if latest > current {
        UpdateCheck::Available { current, latest }
    } else {
        UpdateCheck::UpToDate { current, latest }
    })
}

#[allow(dead_code)]
pub async fn upgrade_if_available(client: &reqwest::Client) -> Result<UpdateCheck, String> {
    let check = check_for_update(client).await?;
    if matches!(check, UpdateCheck::Available { .. }) {
        tokio::task::spawn_blocking(run_brew_upgrade)
            .await
            .map_err(|e| format!("update task error: {e}"))??;
    }
    Ok(check)
}

/// The version this binary was built as.
pub fn current_version() -> Version {
    parse_semver(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

pub fn format_version(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

fn parse_semver(s: &str) -> Option<Version> {
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

/// Extract the formula's version. Prefer an explicit `version "x.y.z"` or
/// `tag: "vx.y.z"` declaration; otherwise fall back to the highest semver found
/// on a url/archive line (Homebrew usually derives the version from the tag in
/// the source URL).
fn parse_formula_version(rb: &str) -> Option<Version> {
    static EXPLICIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?:version\s+"|tag:\s*")v?(\d+)\.(\d+)\.(\d+)"#).unwrap());
    static ANY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"v?(\d+)\.(\d+)\.(\d+)").unwrap());

    let cap3 = |c: &regex::Captures| -> Option<Version> {
        Some((c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?))
    };

    if let Some(c) = EXPLICIT.captures(rb).and_then(|c| cap3(&c)) {
        return Some(c);
    }
    rb.lines()
        .filter(|l| l.contains("url") || l.contains("archive") || l.contains(".tar"))
        .filter_map(|l| ANY.captures(l).and_then(|c| cap3(&c)))
        .max()
}

/// Fetch the latest version published in the Homebrew tap, or `None` if the tap
/// is unreachable or its formula couldn't be parsed.
pub async fn latest_tap_version(client: &reqwest::Client) -> Option<Version> {
    for url in FORMULA_URLS {
        let resp = client
            .get(url)
            .header("User-Agent", "rustcode")
            .send()
            .await;
        if let Ok(resp) = resp
            && resp.status().is_success()
            && let Ok(body) = resp.text().await
            && let Some(v) = parse_formula_version(&body)
        {
            return Some(v);
        }
    }
    None
}

/// Run `brew update` followed by `brew upgrade rustcode`. Blocking — call from `spawn_blocking`.
pub fn run_brew_upgrade() -> Result<(), String> {
    let _ = Command::new("brew").arg("update").output();

    let out = Command::new("brew")
        .args(["upgrade", "rustcode"])
        .output()
        .map_err(|e| format!("failed to run brew (is Homebrew installed?): {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            "brew exited with a non-zero status".to_string()
        } else {
            err
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_version_line() {
        let rb = r#"
            class Rustcode < Formula
              desc "Terminal coding assistant"
              version "0.6.1"
              url "https://example.com/x.tar.gz"
            end
        "#;
        assert_eq!(parse_formula_version(rb), Some((0, 6, 1)));
    }

    #[test]
    fn parses_tag_from_url_when_no_version_line() {
        let rb = r#"
            class Rustcode < Formula
              url "https://github.com/lhagfoss/rustcode/archive/refs/tags/v0.7.3.tar.gz"
              sha256 "abc123"
            end
        "#;
        assert_eq!(parse_formula_version(rb), Some((0, 7, 3)));
    }

    #[test]
    fn picks_highest_semver_across_url_lines() {
        let rb = r#"
              url "https://host/rustcode/archive/v0.6.0.tar.gz"
              head "https://host/rustcode/archive/v0.9.0.tar.gz"
              sha256 "deadbeef1234"
        "#;
        assert_eq!(parse_formula_version(rb), Some((0, 9, 0)));
    }

    #[test]
    fn version_ordering_is_field_wise() {
        assert!((0, 5, 0) < (0, 5, 1));
        assert!((0, 5, 9) < (0, 6, 0));
        assert!((1, 0, 0) > (0, 99, 99));
    }

    #[test]
    fn current_version_matches_cargo() {
        assert_eq!(
            current_version(),
            parse_semver(env!("CARGO_PKG_VERSION")).unwrap()
        );
    }
}
