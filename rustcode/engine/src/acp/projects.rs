//! `_rustcode/project/*`: what the project tools need from the filesystem,
//! asked by the app, which is a web view with no filesystem of its own.
//!
//! `read` answers a folder's git facts (remote, branch, default branch),
//! `create` makes an empty folder a repository, and `clone` runs `git clone`
//! with a bounded wait. The app keeps the project list itself; nothing here
//! writes to it, and nothing here deletes a folder.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// How long a clone may take: a large repository over a slow link is minutes,
/// and the app's request carries this long too.
pub(crate) const CLONE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The longest name a cloned folder may be given.
pub(crate) const MAX_NAME_CHARS: usize = 120;

/// `_rustcode/project/read`: one folder's git facts.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/project/read", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectReadRequest {
    pub path: String,
}

/// `_rustcode/project/create`: make `path` a folder, and a repository when it
/// is not one already.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/project/create", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectCreateRequest {
    pub path: String,
    /// `git init` a folder that is not a repository; true by default.
    #[serde(default)]
    pub init: Option<bool>,
}

/// `_rustcode/project/clone`: `url` into `parent/name`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/project/clone", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectCloneRequest {
    pub url: String,
    pub parent: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// An absolute folder path, resolved (macOS links `/tmp` into `/private`), and
/// checked to be a folder.
fn folder(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("path must be an absolute folder".to_owned());
    }
    if !path.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    std::fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))
}

/// An absolute folder path that may not exist yet.
fn new_folder(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("path must be an absolute folder".to_owned());
    }
    if path.exists() && !path.is_dir() {
        return Err(format!("{} is a file, not a folder", path.display()));
    }
    Ok(path)
}

/// A git line, trimmed; `None` when git says nothing or fails.
fn line(path: &Path, args: &[&str]) -> Option<String> {
    crate::worktrees::git(path, args)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The remote a repository was cloned from, when it has one.
fn remote(path: &Path) -> Option<String> {
    line(path, &["remote", "get-url", "origin"]).filter(|url| !url.starts_with('-'))
}

/// The branch `HEAD` points at; a detached `HEAD` names none.
fn branch(path: &Path) -> Option<String> {
    line(path, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|name| name != "HEAD")
}

/// The branch the repository's remote is on: `origin/HEAD` when it is set,
/// else a `main` or `master` the checkout has, so a fresh clone answers.
fn default_branch(path: &Path) -> Option<String> {
    if let Some(name) = line(
        path,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        return Some(name.strip_prefix("origin/").unwrap_or(&name).to_owned());
    }
    ["main", "master"].into_iter().find_map(|name| {
        line(
            path,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .map(|_| name.to_owned())
    })
}

/// A folder's facts, without the app's own project entries.
pub(crate) fn facts(path: &Path) -> Value {
    let repository = crate::worktrees::main_checkout(path);
    let is_git = repository.is_some();
    json!({
        "path": path,
        "name": path.file_name().map(|name| name.to_string_lossy().into_owned()),
        "is_git": is_git,
        "remote": remote(path),
        "branch": branch(path),
        "default_branch": if is_git { default_branch(path) } else { None },
    })
}

pub(crate) fn read(request: ProjectReadRequest) -> Result<Value, String> {
    Ok(facts(&folder(&request.path)?))
}

pub(crate) fn create(request: ProjectCreateRequest) -> Result<Value, String> {
    let path = new_folder(&request.path)?;
    let created = !path.exists();
    if created {
        std::fs::create_dir_all(&path)
            .map_err(|error| format!("{} could not be created: {error}", path.display()))?;
    }
    let path =
        std::fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut initialized = false;
    if request.init.unwrap_or(true) && crate::worktrees::main_checkout(&path).is_none() {
        crate::worktrees::git(&path, &["init", "--quiet"])?;
        initialized = true;
    }
    let mut answer = facts(&path);
    answer["created"] = json!(created);
    answer["initialized"] = json!(initialized);
    Ok(answer)
}

/// The folder name a clone of `url` takes: its last path segment without
/// `.git`, which is what `git clone` would use too.
pub(crate) fn clone_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    // `--` keeps a URL from being read as an option; a name still may not be
    // one, and may not walk up out of the parent folder.
    let name = name.trim();
    (!name.is_empty() && !name.starts_with('-') && name != "." && name != "..")
        .then(|| name.to_owned())
}

pub(crate) fn clone(request: ProjectCloneRequest) -> Result<Value, String> {
    let url = request.url.trim();
    if url.is_empty() || url.starts_with('-') {
        return Err("url must be a git URL".to_owned());
    }
    let parent = folder(&request.parent)?;
    let name = match request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        Some(name) => {
            if name.chars().count() > MAX_NAME_CHARS || name.contains(['/', '\\']) || name == ".." {
                return Err(format!(
                    "name must be one folder name of at most {MAX_NAME_CHARS} characters"
                ));
            }
            name.to_owned()
        }
        None => clone_name(url).ok_or("the url names no folder; pass name")?,
    };
    let destination = parent.join(&name);
    if destination.exists() {
        return Err(format!("{} already exists", destination.display()));
    }
    let destination_text = destination
        .to_str()
        .ok_or("the destination folder is not valid UTF-8")?;
    crate::worktrees::git_timed(
        &parent,
        &["clone", "--", url, destination_text],
        CLONE_TIMEOUT,
    )
    .map_err(|error| format!("git clone failed: {error}"))?;
    let destination = std::fs::canonicalize(&destination)
        .map_err(|error| format!("{}: {error}", destination.display()))?;
    let mut answer = facts(&destination);
    answer["created"] = json!(true);
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &Path) {
        for args in [
            vec!["init", "--quiet"],
            vec![
                "-c",
                "user.email=a@b",
                "-c",
                "user.name=A",
                "commit",
                "--allow-empty",
                "-m",
                "one",
            ],
        ] {
            crate::worktrees::git(dir, &args).unwrap();
        }
    }

    #[test]
    fn a_folder_reads_its_git_facts() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("code");
        std::fs::create_dir(&folder).unwrap();
        let plain = read(ProjectReadRequest {
            path: folder.to_string_lossy().into_owned(),
        })
        .unwrap();
        assert_eq!(plain["is_git"], false);
        assert_eq!(plain["name"], "code");
        assert!(plain["remote"].is_null());

        repo(&folder);
        let answer = read(ProjectReadRequest {
            path: folder.to_string_lossy().into_owned(),
        })
        .unwrap();
        assert_eq!(answer["is_git"], true);
        // `git init` in this checkout defaults to the repository's own branch
        // name, so the branch is what `HEAD` says rather than a guess.
        assert!(answer["branch"].is_string(), "{answer}");
        assert!(answer["default_branch"].is_string(), "{answer}");
    }

    #[test]
    fn create_makes_a_folder_and_initializes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh");
        let answer = create(ProjectCreateRequest {
            path: path.to_string_lossy().into_owned(),
            init: None,
        })
        .unwrap();
        assert_eq!(answer["created"], true);
        assert_eq!(answer["initialized"], true);
        assert_eq!(answer["is_git"], true);
        assert!(path.join(".git").is_dir());
        // A second call changes nothing.
        let again = create(ProjectCreateRequest {
            path: path.to_string_lossy().into_owned(),
            init: None,
        })
        .unwrap();
        assert_eq!(again["created"], false);
        assert_eq!(again["initialized"], false);
    }

    #[test]
    fn create_with_init_off_leaves_a_plain_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain");
        let answer = create(ProjectCreateRequest {
            path: path.to_string_lossy().into_owned(),
            init: Some(false),
        })
        .unwrap();
        assert_eq!(answer["created"], true);
        assert_eq!(answer["initialized"], false);
        assert_eq!(answer["is_git"], false);
        assert!(!path.join(".git").exists());
    }

    #[test]
    fn a_relative_or_missing_path_is_refused() {
        assert!(
            read(ProjectReadRequest {
                path: "code".into()
            })
            .is_err()
        );
        assert!(
            read(ProjectReadRequest {
                path: "/definitely/not/here".into()
            })
            .is_err()
        );
    }

    #[test]
    fn a_clone_name_comes_from_the_url() {
        assert_eq!(
            clone_name("https://github.com/o/repo.git").as_deref(),
            Some("repo")
        );
        assert_eq!(
            clone_name("git@github.com:o/repo.git").as_deref(),
            Some("repo")
        );
        assert_eq!(
            clone_name("https://github.com/o/repo/").as_deref(),
            Some("repo")
        );
        assert_eq!(clone_name("--upload-pack=x").as_deref(), None);
        assert_eq!(clone_name("").as_deref(), None);
    }
}
