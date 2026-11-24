use super::*;

/// A saved thread in the store under `config`, in `workspace` when given.
fn thread(config: &Path, title: &str, workspace: Option<&Path>) -> String {
    let store = SessionStore::new(config);
    let id = rustcode_session::next_session_id();
    let history = vec![
        rustcode_core::ChatMessage::new("user", title),
        rustcode_core::ChatMessage::new("assistant", "answer"),
    ];
    store.save_session_history(&id, &history);
    store.save_session_title(&id, title);
    if let Some(cwd) = workspace {
        store
            .save_session_workspace(
                &id,
                &rustcode_session::SessionWorkspace {
                    cwd: cwd.to_path_buf(),
                    additional_directories: Vec::new(),
                    task_workspace_id: None,
                },
            )
            .unwrap();
    }
    rustcode_session::flush_history();
    id
}

fn directory(config: &Path, id: &str) -> PathBuf {
    SessionStore::new(config).session_dir(id)
}

fn write(config: &Path, id: &str, file: &str, value: serde_json::Value) {
    std::fs::write(directory(config, id).join(file), value.to_string()).unwrap();
}

fn saved(_: &str) -> Live {
    Live {
        state: SavedSessionState::Saved,
        activity: None,
    }
}

fn find<'a>(entries: &'a [RemoteSavedSession], id: &str) -> &'a RemoteSavedSession {
    entries
        .iter()
        .find(|entry| entry.session_id == id)
        .unwrap_or_else(|| panic!("{id} is not listed"))
}

fn plain(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A listed thread with only what `projects` reads.
fn listed(id: &str, project: Option<&str>, status: &str, updated: &str) -> RemoteSavedSession {
    RemoteSavedSession {
        session_id: id.to_owned(),
        title: id.to_owned(),
        project: project.map(str::to_owned),
        parent: None,
        workspace: project.map(str::to_owned),
        updated_at: Some(updated.to_owned()),
        message_count: 2,
        state: SavedSessionState::Saved,
        open_in: None,
        status: Some(status.to_owned()),
        pinned: false,
        snoozed_until: None,
        harness: None,
        model: None,
        scheduled_job: None,
    }
}

#[test]
fn an_entry_carries_what_the_sidebar_shows() {
    let config = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let pinned = thread(config.path(), "Pinned and snoozed", Some(project.path()));
    let worktree = thread(config.path(), "In a worktree", Some(project.path()));
    let loose = thread(config.path(), "No folder", None);

    write(
        config.path(),
        &pinned,
        "pin.json",
        serde_json::json!({"pinned": true}),
    );
    write(
        config.path(),
        &pinned,
        "status.json",
        serde_json::json!({"status": "review"}),
    );
    let wakes = chrono::Utc::now().timestamp_millis() + 3_600_000;
    write(
        config.path(),
        &pinned,
        "snooze.json",
        serde_json::json!(wakes),
    );
    write(
        config.path(),
        &pinned,
        "harness.json",
        serde_json::json!({"harness": "codex", "model": "gpt-5.5"}),
    );
    SessionStore::new(config.path())
        .save_session_scheduled_job(&pinned, "nightly")
        .unwrap();
    write(
        config.path(),
        &worktree,
        "worktree.json",
        serde_json::json!({"repo": plain(repo.path())}),
    );
    write(
        config.path(),
        &worktree,
        "status.json",
        serde_json::json!({"status": "done"}),
    );

    let entries = SavedIndex::default().scan(config.path(), &|id| {
        if id == worktree {
            Live {
                state: SavedSessionState::Shared,
                activity: Some(SessionActivity::Running),
            }
        } else {
            saved(id)
        }
    });

    let first = find(&entries, &pinned);
    assert_eq!(first.title, "Pinned and snoozed");
    assert!(first.pinned);
    assert_eq!(first.status.as_deref(), Some("review"));
    assert!(
        first
            .snoozed_until
            .as_deref()
            .is_some_and(|at| at.ends_with('Z'))
    );
    assert_eq!(first.harness.as_deref(), Some("codex"));
    assert_eq!(first.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(first.scheduled_job.as_deref(), Some("nightly"));
    assert_eq!(first.project, first.workspace);

    let second = find(&entries, &worktree);
    assert_eq!(second.state, SavedSessionState::Shared);
    assert_eq!(
        second.status.as_deref(),
        Some("done"),
        "settling outranks the last turn"
    );
    assert_eq!(second.project.as_deref(), Some(plain(repo.path()).as_str()));
    assert_eq!(
        second.workspace.as_deref(),
        Some(plain(project.path()).as_str())
    );
    assert!(!second.pinned);

    let third = find(&entries, &loose);
    assert_eq!(third.project, None);
    assert_eq!(third.status.as_deref(), Some("idle"));
    assert_eq!(third.harness.as_deref(), Some(crate::harness::BUILTIN_ID));
    assert_eq!(third.snoozed_until, None);
}

#[test]
fn a_past_snooze_is_not_reported() {
    let config = tempfile::tempdir().unwrap();
    let id = thread(config.path(), "Woke up", None);
    write(config.path(), &id, "snooze.json", serde_json::json!(1_000));
    assert_eq!(snoozed_until(&directory(config.path(), &id)), None);
}

#[test]
fn live_activity_comes_after_settling_and_before_what_is_stored() {
    let config = tempfile::tempdir().unwrap();
    let id = thread(config.path(), "Status", None);
    let dir = directory(config.path(), &id);
    let store = |status: &str| {
        write(
            config.path(),
            &id,
            "status.json",
            serde_json::json!({"status": status}),
        )
    };

    assert_eq!(status_of(&dir, None), "idle");
    store("review");
    assert_eq!(status_of(&dir, None), "review");
    assert_eq!(status_of(&dir, Some(SessionActivity::Idle)), "review");
    assert_eq!(status_of(&dir, Some(SessionActivity::Running)), "running");
    assert_eq!(
        status_of(&dir, Some(SessionActivity::AwaitingQuestion)),
        "needs_input"
    );
    store("stalled");
    assert_eq!(status_of(&dir, None), "stalled");
    store("done");
    assert_eq!(
        status_of(&dir, Some(SessionActivity::AwaitingApproval)),
        "done"
    );
}

#[test]
fn projects_group_threads_newest_first() {
    let entries = [
        listed("a", Some("/code/app"), "idle", "2026-11-01T10:00:00Z"),
        listed(
            "b",
            Some("/code/app"),
            "needs_input",
            "2026-11-03T10:00:00Z",
        ),
        listed("c", Some("/code/lib"), "done", "2026-11-02T10:00:00Z"),
        listed("d", Some("/code/web"), "running", "2026-10-01T10:00:00Z"),
        listed("e", None, "running", "2026-11-04T10:00:00Z"),
    ];
    let projects = projects(&entries);
    assert_eq!(
        projects.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
        ["/code/app", "/code/lib", "/code/web"]
    );
    let app = &projects[0];
    assert_eq!(app.name, "app");
    assert_eq!(app.thread_count, 2);
    assert!(app.running && app.needs_input);
    assert_eq!(app.updated_at.as_deref(), Some("2026-11-03T10:00:00Z"));
    assert!(!projects[1].running);
    assert!(projects[2].running && !projects[2].needs_input);
}

#[test]
fn a_root_or_the_data_folder_is_no_project() {
    let config = tempfile::tempdir().unwrap();
    let dir = config.path().join("threads");
    assert_eq!(
        thread_project(config.path(), &dir, Some(Path::new("/")), None),
        None
    );
    let inside = config.path().join("worktrees").join("x");
    assert_eq!(
        thread_project(config.path(), &dir, Some(&inside), None),
        None
    );
    assert_eq!(thread_project(config.path(), &dir, None, None), None);
    assert_eq!(
        thread_project(config.path(), &dir, Some(Path::new("/code/app")), None).as_deref(),
        Some("/code/app")
    );
}

#[test]
fn the_project_list_is_bounded() {
    let entries: Vec<_> = (0..MAX_PROJECTS + 20)
        .map(|n| {
            let path = format!("/code/project-{n:04}");
            listed(
                &n.to_string(),
                Some(&path),
                "idle",
                &format!("2026-11-01T10:{:02}:00Z", n % 60),
            )
        })
        .collect();
    let RemoteResult::Projects {
        projects,
        truncated,
    } = list_projects(&entries).unwrap()
    else {
        panic!("not a projects result");
    };
    assert_eq!(projects.len(), MAX_PROJECTS);
    assert!(truncated);
    let RemoteResult::Projects {
        projects,
        truncated,
    } = list_projects(&entries[..3]).unwrap()
    else {
        panic!("not a projects result");
    };
    assert_eq!(projects.len(), 3);
    assert!(!truncated);
}

#[test]
fn a_rescan_reports_only_what_changed() {
    let config = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let pin = thread(config.path(), "Pin me", Some(project.path()));
    let rename = thread(config.path(), "Rename me", Some(project.path()));
    let delete = thread(config.path(), "Delete me", Some(project.path()));
    let mut index = SavedIndex::default();
    let before = Baseline::of(index.scan(config.path(), &saved));
    assert!(change_frames(&before, &Baseline::of(index.scan(config.path(), &saved))).is_empty());

    write(
        config.path(),
        &pin,
        "pin.json",
        serde_json::json!({"pinned": true}),
    );
    SessionStore::new(config.path()).save_session_title(&rename, "A much longer new title");
    std::fs::remove_dir_all(directory(config.path(), &delete)).unwrap();
    let added = thread(config.path(), "New", Some(other.path()));
    let after = Baseline::of(index.scan(config.path(), &saved));

    let frames = change_frames(&before, &after);
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    let mut changed: Vec<&str> = frame
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    changed.sort_unstable();
    let mut expected = vec![pin.as_str(), rename.as_str(), added.as_str()];
    expected.sort_unstable();
    assert_eq!(changed, expected);
    assert!(find(&frame.sessions, &pin).pinned);
    assert_eq!(
        find(&frame.sessions, &rename).title,
        "A much longer new title"
    );
    assert_eq!(frame.removed, [delete]);
    let mut paths: Vec<&str> = frame.projects.iter().map(|p| p.path.as_str()).collect();
    paths.sort_unstable();
    let mut expected = vec![plain(project.path()), plain(other.path())];
    expected.sort_unstable();
    assert_eq!(paths, expected);
    assert!(frame.removed_projects.is_empty());

    std::fs::remove_dir_all(directory(config.path(), &added)).unwrap();
    let last = Baseline::of(index.scan(config.path(), &saved));
    let frames = change_frames(&after, &last);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].removed, [added]);
    assert_eq!(frames[0].removed_projects, [plain(other.path())]);
    assert!(frames[0].sessions.is_empty());
}

#[test]
fn many_changes_are_split_across_frames() {
    let long = format!("/code/{}", "x".repeat(2_000));
    let after = Baseline::of(
        (0..400)
            .map(|n| {
                listed(
                    &format!("{n:04}"),
                    Some(&long),
                    "idle",
                    "2026-11-01T10:00:00Z",
                )
            })
            .collect(),
    );
    let frames = change_frames(&Baseline::default(), &after);
    assert!(frames.len() > 1);
    for frame in &frames {
        let text =
            serde_json::to_string(&RemoteFrame::SavedSessionsChanged(frame.clone())).unwrap();
        assert!(text.len() <= MAX_REMOTE_FRAME_BYTES);
    }
    assert_eq!(frames.iter().map(|f| f.sessions.len()).sum::<usize>(), 400);
}

async fn next_frame(queue: &mut crate::remote_gateway::router::FrameQueue) -> RemoteFrame {
    let text = tokio::time::timeout(Duration::from_secs(10), queue.recv())
        .await
        .expect("a frame in time")
        .expect("the sink is open");
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn a_subscriber_is_answered_then_told_of_each_change() {
    let config = tempfile::tempdir().unwrap();
    let id = thread(config.path(), "Watched", None);
    let watch = Arc::new(SavedWatch::default());
    let (sink, mut queue) = FrameSink::detached(16);
    let root = config.path().to_path_buf();
    let index = Arc::new(Mutex::new(SavedIndex::default()));
    let scan: Scan = Arc::new(move || index.lock().unwrap().scan(&root, &saved));

    watch.subscribe(7, &sink, "req-subscribe".into(), scan);
    let RemoteFrame::Response(answer) = next_frame(&mut queue).await else {
        panic!("not a response");
    };
    assert_eq!(answer.request_id, "req-subscribe");
    assert_eq!(answer.result, RemoteResult::SavedSessionsSubscribed);

    write(
        config.path(),
        &id,
        "pin.json",
        serde_json::json!({"pinned": true}),
    );
    watch.wake.notify_one();
    let RemoteFrame::SavedSessionsChanged(changed) = next_frame(&mut queue).await else {
        panic!("not a change frame");
    };
    assert_eq!(changed.sessions.len(), 1);
    assert_eq!(changed.sessions[0].session_id, id);
    assert!(changed.sessions[0].pinned);

    watch.unsubscribe(7);
    watch.wake.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        while watch.subscribers().running {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the watcher stops with no subscriber");
}

#[test]
fn a_project_outside_the_roots_can_be_started_in() {
    let config = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let roots = allowed_roots(config.path()).unwrap();
    if roots
        .iter()
        .any(|root| elsewhere.path().canonicalize().unwrap().starts_with(root))
    {
        return;
    }
    thread(config.path(), "Outside home", Some(project.path()));
    let canonical = project.path().canonicalize().unwrap();
    assert_eq!(
        resolve_creation_directory(config.path(), project.path().to_str()).unwrap(),
        canonical
    );
    let inside = project.path().join("sub");
    std::fs::create_dir(&inside).unwrap();
    for refused_path in [elsewhere.path(), inside.as_path()] {
        let error = resolve_creation_directory(config.path(), refused_path.to_str())
            .expect_err("not a project");
        let code = error.downcast_ref::<Refused>().map(|refused| refused.0);
        assert_eq!(code, Some(RemoteErrorCode::InvalidRequest));
    }
}
