//! Explicit, durable Git workspaces for tasks and delegated agents.
//!
//! The manager deliberately owns only worktrees created by RustCode.  It keeps
//! metadata beside (rather than inside) a checkout and verifies that ownership
//! before every operation which can remove anything.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

pub const WORKSPACES_DIR: &str = "workspaces";
const DESCRIPTOR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    NotGitRepository(PathBuf),
    BareRepository(PathBuf),
    InvalidWorkspaceName(String),
    InvalidBranch(String),
    MissingBase(String),
    DirtySource(PathBuf),
    Collision(String),
    Busy(PathBuf),
    NotOwned(PathBuf),
    CleanupRefused(String),
    StaleDescriptor(PathBuf),
    Git(String),
    Io(String),
    Json(String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotGitRepository(path) => write!(f, "not a Git repository: {}", path.display()),
            Self::BareRepository(path) => write!(
                f,
                "bare repositories cannot host worktrees: {}",
                path.display()
            ),
            Self::InvalidWorkspaceName(name) => write!(f, "invalid workspace name `{name}`"),
            Self::InvalidBranch(branch) => write!(f, "invalid branch `{branch}`"),
            Self::MissingBase(base) => {
                write!(f, "base commit/ref does not resolve to a commit: `{base}`")
            }
            Self::DirtySource(path) => write!(f, "source checkout is dirty: {}", path.display()),
            Self::Collision(detail) => write!(f, "workspace collision: {detail}"),
            Self::Busy(path) => write!(
                f,
                "workspace creation is already in progress: {}",
                path.display()
            ),
            Self::NotOwned(path) => {
                write!(f, "workspace is not owned by RustCode: {}", path.display())
            }
            Self::CleanupRefused(detail) => write!(f, "cleanup refused: {detail}"),
            Self::StaleDescriptor(path) => {
                write!(f, "workspace descriptor is stale: {}", path.display())
            }
            Self::Git(detail) => write!(f, "git: {detail}"),
            Self::Io(detail) => write!(f, "I/O: {detail}"),
            Self::Json(detail) => write!(f, "descriptor JSON: {detail}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<std::io::Error> for WorkspaceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryDescriptor {
    pub repository_root: PathBuf,
    pub common_git_dir: PathBuf,
    pub source_worktree: PathBuf,
    pub current_branch: Option<String>,
    pub current_sha: String,
    pub default_branch: Option<String>,
    pub origin: Option<String>,
    pub source_dirty: bool,
    pub linked_worktree: bool,
}

impl RepositoryDescriptor {
    /// Resolve the repository from the supplied directory, including when the
    /// directory is nested inside a linked worktree.
    pub fn discover(cwd: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let cwd = cwd.as_ref();
        let inspected_path = fs::canonicalize(cwd)
            .map_err(|_| WorkspaceError::NotGitRepository(cwd.to_path_buf()))?;
        let bare_output = git_output(&inspected_path, &["rev-parse", "--is-bare-repository"])?;
        if !bare_output.status.success() {
            return Err(WorkspaceError::NotGitRepository(inspected_path));
        }
        let is_bare = String::from_utf8_lossy(&bare_output.stdout).trim() == "true";
        if is_bare {
            return Err(WorkspaceError::BareRepository(inspected_path));
        }

        let source_worktree = fs::canonicalize(
            git_stdout(&inspected_path, &["rev-parse", "--show-toplevel"])?.trim(),
        )
        .map_err(|_| WorkspaceError::NotGitRepository(inspected_path.clone()))?;
        let common_git_dir_raw = git_stdout(&source_worktree, &["rev-parse", "--git-common-dir"])?;
        let common_git_dir_path = Path::new(common_git_dir_raw.trim());
        let common_git_dir = fs::canonicalize(if common_git_dir_path.is_absolute() {
            common_git_dir_path.to_path_buf()
        } else {
            source_worktree.join(common_git_dir_path)
        })
        .map_err(|_| WorkspaceError::NotGitRepository(source_worktree.clone()))?;
        let repository_root = common_git_dir
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| WorkspaceError::NotGitRepository(source_worktree.clone()))?;
        let current_sha = git_stdout(&source_worktree, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        let current_branch = git_stdout_optional(
            &source_worktree,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        );
        let default_branch = default_branch(&source_worktree);
        let origin = git_stdout_optional(&source_worktree, &["remote", "get-url", "origin"]);
        let source_dirty = !git_stdout(
            &source_worktree,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty();

        Ok(Self {
            linked_worktree: source_worktree != repository_root,
            repository_root,
            common_git_dir,
            source_worktree,
            current_branch,
            current_sha,
            default_branch,
            origin,
            source_dirty,
        })
    }
}

fn default_branch(cwd: &Path) -> Option<String> {
    let remote_head = git_stdout_optional(
        cwd,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .and_then(|head| head.strip_prefix("origin/").map(str::to_owned));
    remote_head.or_else(|| {
        ["main", "master"].iter().find_map(|branch| {
            git_ok(
                cwd,
                &["show-ref", "--verify", &format!("refs/heads/{branch}")],
            )
            .then(|| (*branch).to_string())
        })
    })
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<Output, WorkspaceError> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| WorkspaceError::Git(format!("cannot run git: {error}")))
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String, WorkspaceError> {
    let output = git_output(cwd, args)?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if args.starts_with(&["rev-parse"]) && args.contains(&"--show-toplevel") {
            return Err(WorkspaceError::NotGitRepository(cwd.to_path_buf()));
        }
        return Err(WorkspaceError::Git(if error.is_empty() {
            format!("`git {}` exited with {}", args.join(" "), output.status)
        } else {
            error
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_stdout_optional(cwd: &Path, args: &[&str]) -> Option<String> {
    git_output(cwd, args)
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!value.is_empty()).then_some(value)
        })
}

fn git_ok(cwd: &Path, args: &[&str]) -> bool {
    git_output(cwd, args).is_ok_and(|output| output.status.success())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceLifecycle {
    Creating,
    Ready,
    Running,
    Completed,
    Cancelled,
    Archived,
    Removed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationResult {
    pub command: String,
    pub success: bool,
    pub output: Option<String>,
    pub recorded_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDescriptor {
    pub version: u32,
    pub id: String,
    pub repository_root: PathBuf,
    pub common_git_dir: PathBuf,
    pub source_worktree: PathBuf,
    pub workspace_path: PathBuf,
    pub descriptor_path: PathBuf,
    pub branch: String,
    pub base_sha: String,
    pub current_head: String,
    pub origin: Option<String>,
    pub default_branch: Option<String>,
    #[serde(default)]
    pub source_branch: Option<String>,
    #[serde(default)]
    pub source_dirty: bool,
    pub owner: String,
    pub owner_session_id: String,
    pub owner_task_id: String,
    pub created_at: u64,
    pub lifecycle: WorkspaceLifecycle,
    #[serde(default)]
    pub has_uncommitted_changes: bool,
    #[serde(default)]
    pub has_unpushed_commits: bool,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub diff_stat: String,
    #[serde(default)]
    pub verification: Option<VerificationResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub descriptor: WorkspaceDescriptor,
    pub changed_files: Vec<String>,
    pub diff_stat: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRequest {
    pub source_cwd: PathBuf,
    pub name: String,
    pub branch: Option<String>,
    pub base_sha: String,
    pub owner: String,
    pub owner_session_id: String,
    pub owner_task_id: String,
    pub workspace_path: Option<PathBuf>,
}

impl WorkspaceRequest {
    pub fn for_task(
        source_cwd: impl Into<PathBuf>,
        name: impl Into<String>,
        base_sha: impl Into<String>,
        owner: impl Into<String>,
        owner_session_id: impl Into<String>,
        owner_task_id: impl Into<String>,
    ) -> Self {
        Self {
            source_cwd: source_cwd.into(),
            name: name.into(),
            branch: None,
            base_sha: base_sha.into(),
            owner: owner.into(),
            owner_session_id: owner_session_id.into(),
            owner_task_id: owner_task_id.into(),
            workspace_path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupAction {
    Archive,
    Remove { delete_branch: bool },
}

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    root: PathBuf,
}

impl WorkspaceManager {
    /// `root` is RustCode's persistence root. Checkouts are placed below its
    /// `workspaces` directory; callers should keep it outside the source repo.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspace_metadata_root(&self) -> PathBuf {
        self.root.join(WORKSPACES_DIR)
    }

    pub fn create(
        &self,
        request: &WorkspaceRequest,
    ) -> Result<WorkspaceDescriptor, WorkspaceError> {
        validate_component(&request.name, false)?;
        for value in [
            &request.owner,
            &request.owner_session_id,
            &request.owner_task_id,
        ] {
            if value.trim().is_empty() {
                return Err(WorkspaceError::Collision(
                    "workspace ownership is incomplete".to_string(),
                ));
            }
        }
        let repository = RepositoryDescriptor::discover(&request.source_cwd)?;
        let id = workspace_id(
            &repository.repository_root,
            &request.owner_session_id,
            &request.owner_task_id,
            &request.name,
        );
        if let Some(existing) = self.load_descriptor(&id)? {
            if existing.lifecycle == WorkspaceLifecycle::Removed {
                return Err(WorkspaceError::Collision(format!(
                    "workspace {id} was already removed"
                )));
            }
            if !matches!(
                existing.lifecycle,
                WorkspaceLifecycle::Creating | WorkspaceLifecycle::Failed
            ) {
                return self.resume(&existing.id);
            }
        }

        let metadata_dir = self.workspace_metadata_root();
        fs::create_dir_all(&metadata_dir)?;
        let lock_path = metadata_dir.join(format!(".{id}.lock"));
        let _lock = CreationLock::acquire(&lock_path)?;
        if let Some(existing) = self.load_descriptor(&id)? {
            if matches!(
                existing.lifecycle,
                WorkspaceLifecycle::Creating | WorkspaceLifecycle::Failed
            ) {
                let branch_exists = git_ok(
                    &existing.source_worktree,
                    &[
                        "show-ref",
                        "--verify",
                        &format!("refs/heads/{}", existing.branch),
                    ],
                );
                if existing.workspace_path.exists() || branch_exists {
                    return self.resume(&existing.id);
                }
                fs::remove_file(&existing.descriptor_path)?;
            } else if existing.lifecycle == WorkspaceLifecycle::Removed {
                return Err(WorkspaceError::Collision(format!(
                    "workspace {id} was already removed"
                )));
            } else {
                return Ok(existing);
            }
        }

        let branch = request.branch.clone().unwrap_or_else(|| {
            format!(
                "rustcode/{}/{}/{}",
                request.owner_session_id, request.owner_task_id, request.name
            )
        });
        validate_branch(&branch)?;
        let base_sha = resolve_base(&repository.source_worktree, &request.base_sha)?;
        if git_ok(
            &repository.source_worktree,
            &["show-ref", "--verify", &format!("refs/heads/{branch}")],
        ) {
            return Err(WorkspaceError::Collision(format!(
                "branch `{branch}` already exists"
            )));
        }
        let workspace_path = request.workspace_path.clone().unwrap_or_else(|| {
            metadata_dir
                .join(repo_key(&repository.repository_root))
                .join(&request.owner_session_id)
                .join(&request.owner_task_id)
                .join(&request.name)
                .join("checkout")
        });
        let workspace_path = absolute_target(&workspace_path)?;
        if workspace_path.exists() {
            return Err(WorkspaceError::Collision(format!(
                "path `{}` already exists",
                workspace_path.display()
            )));
        }
        if workspace_path.starts_with(&repository.source_worktree)
            || workspace_path.starts_with(&repository.repository_root)
        {
            return Err(WorkspaceError::Collision(
                "workspace path must be outside the source repository".to_string(),
            ));
        }
        if let Some(parent) = workspace_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let descriptor_path = metadata_dir.join(format!("{id}.json"));
        let mut descriptor = WorkspaceDescriptor {
            version: DESCRIPTOR_VERSION,
            id,
            repository_root: repository.repository_root.clone(),
            common_git_dir: repository.common_git_dir.clone(),
            source_worktree: repository.source_worktree.clone(),
            workspace_path: workspace_path.clone(),
            descriptor_path,
            branch: branch.clone(),
            base_sha,
            current_head: repository.current_sha.clone(),
            origin: repository.origin.clone(),
            default_branch: repository.default_branch.clone(),
            source_branch: repository.current_branch.clone(),
            source_dirty: repository.source_dirty,
            owner: request.owner.clone(),
            owner_session_id: request.owner_session_id.clone(),
            owner_task_id: request.owner_task_id.clone(),
            created_at: now_unix(),
            lifecycle: WorkspaceLifecycle::Creating,
            has_uncommitted_changes: false,
            has_unpushed_commits: false,
            changed_files: Vec::new(),
            diff_stat: String::new(),
            verification: None,
        };
        write_descriptor(&descriptor)?;

        let add_result = git_output(
            &repository.source_worktree,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                workspace_path.to_string_lossy().as_ref(),
                &descriptor.base_sha,
            ],
        );
        let output = match add_result {
            Ok(output) if output.status.success() => output,
            Ok(output) => {
                descriptor.lifecycle = WorkspaceLifecycle::Failed;
                write_descriptor(&descriptor)?;
                return Err(WorkspaceError::Git(git_error(&output)));
            }
            Err(error) => {
                descriptor.lifecycle = WorkspaceLifecycle::Failed;
                write_descriptor(&descriptor)?;
                return Err(error);
            }
        };
        let _ = output;
        descriptor.current_head = git_stdout(&workspace_path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        descriptor.lifecycle = WorkspaceLifecycle::Ready;
        write_descriptor(&descriptor)?;
        Ok(descriptor)
    }

    pub fn load_descriptor(&self, id: &str) -> Result<Option<WorkspaceDescriptor>, WorkspaceError> {
        let path = self.workspace_metadata_root().join(format!("{id}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        let content = fs::read_to_string(path).map_err(WorkspaceError::from)?;
        serde_json::from_str(&content)
            .map(Some)
            .map_err(|error| WorkspaceError::Json(error.to_string()))
    }

    pub fn find_by_owner(
        &self,
        repository_root: &Path,
        session_id: &str,
        task_id: &str,
        name: &str,
    ) -> Result<Option<WorkspaceDescriptor>, WorkspaceError> {
        let Ok(entries) = fs::read_dir(self.workspace_metadata_root()) else {
            return Ok(None);
        };
        for entry in entries.filter_map(Result::ok) {
            if entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("json")
            {
                continue;
            }
            let Ok(content) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(descriptor) = serde_json::from_str::<WorkspaceDescriptor>(&content) else {
                continue;
            };
            if descriptor.id == workspace_id(repository_root, session_id, task_id, name)
                && descriptor.repository_root == repository_root
                && descriptor.owner_session_id == session_id
                && descriptor.owner_task_id == task_id
            {
                return Ok(Some(descriptor));
            }
        }
        Ok(None)
    }

    pub fn status(&self, id: &str) -> Result<WorkspaceStatus, WorkspaceError> {
        let mut descriptor = self.load_descriptor(id)?.ok_or_else(|| {
            WorkspaceError::StaleDescriptor(
                self.workspace_metadata_root().join(format!("{id}.json")),
            )
        })?;
        if descriptor.lifecycle == WorkspaceLifecycle::Removed {
            return Ok(WorkspaceStatus {
                changed_files: descriptor.changed_files.clone(),
                diff_stat: descriptor.diff_stat.clone(),
                descriptor,
            });
        }
        self.verify_owned_worktree(&descriptor)?;
        let status = git_stdout(
            &descriptor.workspace_path,
            &["status", "--porcelain", "--untracked-files=all"],
        )?;
        let working_changed_files = status
            .lines()
            .filter_map(parse_status_path)
            .collect::<Vec<_>>();
        let mut changed_files = working_changed_files.clone();
        changed_files.extend(
            git_stdout(
                &descriptor.workspace_path,
                &[
                    "diff",
                    "--name-only",
                    &format!("{}..HEAD", descriptor.base_sha),
                ],
            )?
            .lines()
            .map(str::to_string),
        );
        changed_files.sort();
        changed_files.dedup();
        let diff_stat = format_diff_stat(&descriptor.workspace_path, &descriptor.base_sha)?;
        let head = git_stdout(&descriptor.workspace_path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        let has_unpushed_commits = if git_ok(
            &descriptor.workspace_path,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        ) {
            git_ok(
                &descriptor.workspace_path,
                &["rev-list", "--quiet", "--max-count=1", "@{upstream}..HEAD"],
            ) && git_stdout(
                &descriptor.workspace_path,
                &["rev-list", "--count", "@{upstream}..HEAD"],
            )
            .is_ok_and(|count| count.trim() != "0")
        } else {
            head != descriptor.base_sha
        };
        descriptor.current_head = head;
        descriptor.has_uncommitted_changes = !working_changed_files.is_empty();
        descriptor.has_unpushed_commits = has_unpushed_commits;
        descriptor.changed_files = changed_files.clone();
        descriptor.diff_stat = diff_stat.clone();
        write_descriptor(&descriptor)?;
        Ok(WorkspaceStatus {
            descriptor,
            changed_files,
            diff_stat,
        })
    }

    /// Reattach to a descriptor after an application restart. No new branch or
    /// worktree is created, and ownership is checked before the checkout is used.
    pub fn resume(&self, id: &str) -> Result<WorkspaceDescriptor, WorkspaceError> {
        let mut descriptor = self.status(id)?.descriptor;
        if descriptor.lifecycle == WorkspaceLifecycle::Creating {
            descriptor.lifecycle = WorkspaceLifecycle::Ready;
            write_descriptor(&descriptor)?;
        }
        Ok(descriptor)
    }

    pub fn record_verification(
        &self,
        id: &str,
        command: impl Into<String>,
        success: bool,
        output: Option<String>,
    ) -> Result<WorkspaceDescriptor, WorkspaceError> {
        let mut descriptor = self.status(id)?.descriptor;
        descriptor.verification = Some(VerificationResult {
            command: command.into(),
            success,
            output,
            recorded_at: now_unix(),
        });
        write_descriptor(&descriptor)?;
        Ok(descriptor)
    }

    pub fn find_by_workspace_path(
        &self,
        workspace_path: &Path,
    ) -> Result<Option<WorkspaceDescriptor>, WorkspaceError> {
        let expected = absolute_target(workspace_path)?;
        let Ok(entries) = fs::read_dir(self.workspace_metadata_root()) else {
            return Ok(None);
        };
        for entry in entries.filter_map(Result::ok) {
            if entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("json")
            {
                continue;
            }
            let Ok(content) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(descriptor) = serde_json::from_str::<WorkspaceDescriptor>(&content) else {
                continue;
            };
            if absolute_target(&descriptor.workspace_path).ok().as_deref() == Some(&expected) {
                return Ok(Some(descriptor));
            }
        }
        Ok(None)
    }

    pub fn set_lifecycle(
        &self,
        id: &str,
        lifecycle: WorkspaceLifecycle,
    ) -> Result<WorkspaceDescriptor, WorkspaceError> {
        let mut descriptor = self.load_descriptor(id)?.ok_or_else(|| {
            WorkspaceError::StaleDescriptor(
                self.workspace_metadata_root().join(format!("{id}.json")),
            )
        })?;
        if descriptor.lifecycle != WorkspaceLifecycle::Removed {
            self.verify_owned_worktree(&descriptor)?;
            descriptor.lifecycle = lifecycle;
            write_descriptor(&descriptor)?;
        }
        Ok(descriptor)
    }

    pub fn handoff_for_workspace_path(
        &self,
        workspace_path: &Path,
    ) -> Result<Option<String>, WorkspaceError> {
        let Some(descriptor) = self.find_by_workspace_path(workspace_path)? else {
            return Ok(None);
        };
        self.handoff(&descriptor.id).map(Some)
    }

    pub fn handoff(&self, id: &str) -> Result<String, WorkspaceError> {
        let status = self.status(id)?;
        let descriptor = &status.descriptor;
        let verification = descriptor
            .verification
            .as_ref()
            .map(|result| {
                format!(
                    "{} ({})",
                    result.command,
                    if result.success { "passed" } else { "failed" }
                )
            })
            .unwrap_or_else(|| "not recorded".to_string());
        Ok(format!(
            "Workspace {}\npath: {}\nbranch: {}\nbase: {}\nhead: {}\nstatus: {:?}\nchanged files: {}\n{}verification: {}\nnext actions: inspect, retain, archive, or explicitly approve cleanup/publish operations.",
            descriptor.id,
            descriptor.workspace_path.display(),
            descriptor.branch,
            descriptor.base_sha,
            descriptor.current_head,
            descriptor.lifecycle,
            if status.changed_files.is_empty() {
                "none".to_string()
            } else {
                status.changed_files.join(", ")
            },
            if status.diff_stat.is_empty() {
                String::new()
            } else {
                format!("diff stat: {}\n", status.diff_stat.trim())
            },
            verification
        ))
    }

    pub fn cleanup(
        &self,
        id: &str,
        action: CleanupAction,
        confirmed: bool,
    ) -> Result<WorkspaceDescriptor, WorkspaceError> {
        let mut descriptor = self.load_descriptor(id)?.ok_or_else(|| {
            WorkspaceError::StaleDescriptor(
                self.workspace_metadata_root().join(format!("{id}.json")),
            )
        })?;
        if matches!(action, CleanupAction::Remove { .. })
            && descriptor.lifecycle != WorkspaceLifecycle::Removed
        {
            let status = self.status(id)?;
            if (status.descriptor.has_uncommitted_changes || status.descriptor.has_unpushed_commits)
                && !confirmed
            {
                return Err(WorkspaceError::CleanupRefused(
                    "uncommitted or unpushed work remains; inspect or confirm explicitly"
                        .to_string(),
                ));
            }
            descriptor = status.descriptor;
            self.verify_owned_worktree(&descriptor)?;
        }
        match action {
            CleanupAction::Archive => {
                descriptor.lifecycle = WorkspaceLifecycle::Archived;
                write_descriptor(&descriptor)?;
            }
            CleanupAction::Remove { delete_branch } => {
                if delete_branch && !confirmed {
                    return Err(WorkspaceError::CleanupRefused(
                        "branch deletion requires explicit confirmation".to_string(),
                    ));
                }
                if descriptor.lifecycle != WorkspaceLifecycle::Removed {
                    let mut remove_args = vec!["worktree", "remove"];
                    if confirmed
                        && (descriptor.has_uncommitted_changes || descriptor.has_unpushed_commits)
                    {
                        remove_args.push("--force");
                    }
                    let workspace_path = descriptor.workspace_path.to_string_lossy().to_string();
                    remove_args.push(&workspace_path);
                    let output = git_output(&descriptor.source_worktree, &remove_args)?;
                    if !output.status.success() {
                        return Err(WorkspaceError::Git(git_error(&output)));
                    }
                    if delete_branch {
                        let output = git_output(
                            &descriptor.source_worktree,
                            &["branch", "-d", "--", &descriptor.branch],
                        )?;
                        if !output.status.success() {
                            return Err(WorkspaceError::Git(git_error(&output)));
                        }
                    }
                    descriptor.lifecycle = WorkspaceLifecycle::Removed;
                    write_descriptor(&descriptor)?;
                }
            }
        }
        Ok(descriptor)
    }

    fn verify_owned_worktree(
        &self,
        descriptor: &WorkspaceDescriptor,
    ) -> Result<(), WorkspaceError> {
        let metadata_root = fs::canonicalize(self.workspace_metadata_root())
            .unwrap_or_else(|_| self.workspace_metadata_root());
        let path = fs::canonicalize(absolute_target(&descriptor.workspace_path)?)
            .map_err(|_| WorkspaceError::StaleDescriptor(descriptor.workspace_path.clone()))?;
        if !path.starts_with(&metadata_root)
            || path.starts_with(&descriptor.repository_root)
            || descriptor.descriptor_path.parent() != Some(self.workspace_metadata_root().as_path())
        {
            return Err(WorkspaceError::NotOwned(path));
        }
        if !path.is_dir() {
            return Err(WorkspaceError::StaleDescriptor(path));
        }
        let worktrees = git_stdout(
            &descriptor.source_worktree,
            &["worktree", "list", "--porcelain"],
        )?;
        let owned = worktree_entry(&worktrees, &path)
            .is_some_and(|branch| branch == format!("refs/heads/{}", descriptor.branch));
        if !owned {
            return Err(WorkspaceError::NotOwned(path));
        }
        Ok(())
    }
}

struct CreationLock {
    path: PathBuf,
}

impl CreationLock {
    fn acquire(path: &Path) -> Result<Self, WorkspaceError> {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                writeln!(file, "pid={}", std::process::id())?;
                Ok(Self {
                    path: path.to_path_buf(),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let pid = fs::read_to_string(path).ok().and_then(|content| {
                    content
                        .strip_prefix("pid=")
                        .and_then(|value| value.trim().parse().ok())
                });
                let Some(pid) = pid else {
                    return Err(WorkspaceError::Busy(path.to_path_buf()));
                };
                if process_is_running(pid) {
                    return Err(WorkspaceError::Busy(path.to_path_buf()));
                }
                fs::remove_file(path)?;
                Self::acquire(path)
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
        })
}

impl Drop for CreationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn validate_component(value: &str, branch: bool) -> Result<(), WorkspaceError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || (branch && value.starts_with('-'))
    {
        return Err(WorkspaceError::InvalidWorkspaceName(value.to_string()));
    }
    Ok(())
}

fn validate_branch(branch: &str) -> Result<(), WorkspaceError> {
    let valid = Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .output()
        .is_ok_and(|output| output.status.success());
    if !valid {
        return Err(WorkspaceError::InvalidBranch(branch.to_string()));
    }
    Ok(())
}

fn resolve_base(cwd: &Path, base: &str) -> Result<String, WorkspaceError> {
    let output = git_output(
        cwd,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    if !output.status.success() {
        return Err(WorkspaceError::MissingBase(base.to_string()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn absolute_target(path: &Path) -> Result<PathBuf, WorkspaceError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn repo_key(path: &Path) -> String {
    format!("{:016x}", stable_hash(&path.to_string_lossy()))
}

fn workspace_id(repo: &Path, session: &str, task: &str, name: &str) -> String {
    format!(
        "{}-{:016x}",
        repo_key(repo),
        stable_hash(&format!("{session}\0{task}\0{name}"))
    )
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

fn write_descriptor(descriptor: &WorkspaceDescriptor) -> Result<(), WorkspaceError> {
    if let Some(parent) = descriptor.descriptor_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(descriptor)
        .map_err(|error| WorkspaceError::Json(error.to_string()))?;
    let temporary = descriptor
        .descriptor_path
        .with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temporary, json)?;
    fs::rename(&temporary, &descriptor.descriptor_path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        WorkspaceError::Io(error.to_string())
    })
}

fn git_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("git exited with {}", output.status)
    } else {
        stderr
    }
}

fn parse_status_path(line: &str) -> Option<String> {
    (line.len() > 3)
        .then(|| line[3..].trim().to_string())
        .filter(|path| !path.is_empty())
}

fn format_diff_stat(cwd: &Path, base: &str) -> Result<String, WorkspaceError> {
    let committed = git_stdout(cwd, &["diff", "--stat", &format!("{base}..HEAD")])?;
    let working = git_stdout(cwd, &["diff", "--stat"])?;
    Ok([committed.trim(), working.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

fn worktree_entry(output: &str, expected: &Path) -> Option<String> {
    let mut path = None;
    let mut branch = None;
    for line in output.lines().chain(std::iter::once("")) {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
            branch = None;
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(value.to_string());
        } else if line.is_empty() && path.as_deref() == Some(expected) {
            return branch;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use tempfile::TempDir;

    fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repo() -> (TempDir, TempDir) {
        let root = tempfile::tempdir().expect("root");
        git(root.path(), &["init", "-b", "main"]);
        git(
            root.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(root.path(), &["config", "user.name", "Test"]);
        fs::write(root.path().join("README.md"), "base\n").expect("file");
        git(root.path(), &["add", "."]);
        git(root.path(), &["commit", "-m", "base"]);
        let persistence = tempfile::tempdir().expect("persistence");
        (root, persistence)
    }

    #[test]
    fn discovers_regular_and_linked_worktrees() {
        let (root, persistence) = repo();
        let regular = RepositoryDescriptor::discover(root.path()).expect("regular");
        assert!(!regular.linked_worktree);
        let linked_path = persistence.path().join("linked");
        git(
            root.path(),
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                linked_path.to_str().unwrap(),
                "HEAD",
            ],
        );
        let linked = RepositoryDescriptor::discover(&linked_path).expect("linked");
        assert!(linked.linked_worktree);
        assert_eq!(linked.common_git_dir, regular.common_git_dir);
        assert_eq!(linked.repository_root, regular.repository_root);
    }

    #[test]
    fn dirty_source_is_not_copied_or_modified() {
        let (root, persistence) = repo();
        fs::write(root.path().join("dirty.txt"), "do not copy\n").expect("dirty");
        let base = git_stdout(root.path(), &["rev-parse", "HEAD"])
            .expect("head")
            .trim()
            .to_string();
        let manager = WorkspaceManager::new(persistence.path());
        let request =
            WorkspaceRequest::for_task(root.path(), "edit", &base, "rustcode", "session", "task");
        let descriptor = manager.create(&request).expect("workspace");
        assert!(
            RepositoryDescriptor::discover(root.path())
                .expect("repo")
                .source_dirty
        );
        assert_eq!(
            fs::read_to_string(root.path().join("dirty.txt")).unwrap(),
            "do not copy\n"
        );
        assert!(!descriptor.workspace_path.join("dirty.txt").exists());
    }

    #[test]
    fn branch_and_path_collisions_are_rejected_and_duplicates_resume() {
        let (root, persistence) = repo();
        let base = git_stdout(root.path(), &["rev-parse", "HEAD"])
            .expect("head")
            .trim()
            .to_string();
        let manager = WorkspaceManager::new(persistence.path());
        let mut request =
            WorkspaceRequest::for_task(root.path(), "edit", &base, "rustcode", "session", "task");
        request.branch = Some("feature/collision".to_string());
        let first = manager.create(&request).expect("first");
        assert_eq!(manager.create(&request).expect("duplicate"), first);
        let resumed = WorkspaceManager::new(persistence.path())
            .resume(&first.id)
            .expect("resume");
        assert_eq!(resumed.workspace_path, first.workspace_path);
        let mut second = WorkspaceRequest::for_task(
            root.path(),
            "other",
            &base,
            "rustcode",
            "session",
            "task-2",
        );
        second.branch = Some("feature/collision".to_string());
        assert!(matches!(
            manager.create(&second),
            Err(WorkspaceError::Collision(_))
        ));
        second.branch = Some("feature/other".to_string());
        second.workspace_path = Some(first.workspace_path.clone());
        assert!(matches!(
            manager.create(&second),
            Err(WorkspaceError::Collision(_))
        ));
    }

    #[test]
    fn interrupted_creation_resumes_existing_worktree() {
        let (root, persistence) = repo();
        let base = git_stdout(root.path(), &["rev-parse", "HEAD"])
            .expect("head")
            .trim()
            .to_string();
        let manager = WorkspaceManager::new(persistence.path());
        let request = WorkspaceRequest::for_task(
            root.path(),
            "interrupted",
            &base,
            "rustcode",
            "session",
            "interrupted",
        );
        let descriptor = manager.create(&request).expect("workspace");
        manager
            .set_lifecycle(&descriptor.id, WorkspaceLifecycle::Creating)
            .expect("mark interrupted");

        let resumed = manager.create(&request).expect("resume");
        assert_eq!(resumed.id, descriptor.id);
        assert_eq!(resumed.lifecycle, WorkspaceLifecycle::Ready);
        assert!(resumed.workspace_path.is_dir());
    }

    #[test]
    fn detached_and_non_git_sources_have_bounded_results() {
        let (root, persistence) = repo();
        git(root.path(), &["checkout", "--detach"]);
        let repository = RepositoryDescriptor::discover(root.path()).expect("detached");
        assert!(repository.current_branch.is_none());
        let manager = WorkspaceManager::new(persistence.path());
        let request = WorkspaceRequest::for_task(
            root.path(),
            "detached",
            &repository.current_sha,
            "rustcode",
            "session",
            "detached",
        );
        assert!(manager.create(&request).is_ok());
        let non_git = persistence.path().join("plain");
        fs::create_dir(&non_git).expect("plain");
        assert!(matches!(
            RepositoryDescriptor::discover(&non_git),
            Err(WorkspaceError::NotGitRepository(_))
        ));
    }

    #[test]
    fn cleanup_refuses_dirty_work_and_source_stays_unchanged() {
        let (root, persistence) = repo();
        let base = git_stdout(root.path(), &["rev-parse", "HEAD"])
            .expect("head")
            .trim()
            .to_string();
        let manager = WorkspaceManager::new(persistence.path());
        let request = WorkspaceRequest::for_task(
            root.path(),
            "cleanup",
            &base,
            "rustcode",
            "session",
            "cleanup",
        );
        let descriptor = manager.create(&request).expect("workspace");
        fs::write(descriptor.workspace_path.join("change.txt"), "change\n").expect("change");
        let status = manager.status(&descriptor.id).expect("status");
        assert!(status.descriptor.has_uncommitted_changes);
        assert!(
            manager
                .handoff(&descriptor.id)
                .expect("handoff")
                .contains("change.txt")
        );
        let cleanup = manager.cleanup(
            &descriptor.id,
            CleanupAction::Remove {
                delete_branch: false,
            },
            false,
        );
        assert!(
            matches!(cleanup, Err(WorkspaceError::CleanupRefused(_))),
            "cleanup result: {cleanup:?}"
        );
        assert!(root.path().join("change.txt").is_file() == false);
        let removed = manager
            .cleanup(
                &descriptor.id,
                CleanupAction::Remove {
                    delete_branch: false,
                },
                true,
            )
            .expect("confirmed");
        assert_eq!(removed.lifecycle, WorkspaceLifecycle::Removed);
    }

    #[test]
    fn cleanup_also_guards_unpushed_commits() {
        let (root, persistence) = repo();
        let base = git_stdout(root.path(), &["rev-parse", "HEAD"])
            .expect("head")
            .trim()
            .to_string();
        let manager = WorkspaceManager::new(persistence.path());
        let request = WorkspaceRequest::for_task(
            root.path(),
            "commit",
            &base,
            "rustcode",
            "session",
            "commit",
        );
        let descriptor = manager.create(&request).expect("workspace");
        fs::write(descriptor.workspace_path.join("commit.txt"), "commit\n").expect("file");
        git(&descriptor.workspace_path, &["add", "commit.txt"]);
        git(&descriptor.workspace_path, &["commit", "-m", "commit"]);
        let status = manager.status(&descriptor.id).expect("status");
        assert!(!status.descriptor.has_uncommitted_changes);
        assert!(status.descriptor.has_unpushed_commits);
        assert!(matches!(
            manager.cleanup(
                &descriptor.id,
                CleanupAction::Remove {
                    delete_branch: false
                },
                false
            ),
            Err(WorkspaceError::CleanupRefused(_))
        ));
        assert!(
            manager
                .cleanup(
                    &descriptor.id,
                    CleanupAction::Remove {
                        delete_branch: false
                    },
                    true,
                )
                .is_ok()
        );
    }

    #[test]
    fn concurrent_creation_has_one_owned_descriptor() {
        let (root, persistence) = repo();
        let base = git_stdout(root.path(), &["rev-parse", "HEAD"])
            .expect("head")
            .trim()
            .to_string();
        let manager = WorkspaceManager::new(persistence.path());
        let request = WorkspaceRequest::for_task(
            root.path(),
            "parallel",
            &base,
            "rustcode",
            "session",
            "parallel",
        );
        let handles = (0..4)
            .map(|_| {
                let manager = manager.clone();
                let request = request.clone();
                thread::spawn(move || manager.create(&request))
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            manager
                .find_by_owner(
                    &RepositoryDescriptor::discover(root.path())
                        .unwrap()
                        .repository_root,
                    "session",
                    "parallel",
                    "parallel"
                )
                .unwrap()
                .unwrap()
                .owner_task_id,
            "parallel"
        );
    }
}
