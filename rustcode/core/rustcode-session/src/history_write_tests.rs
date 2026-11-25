use super::*;

fn reported() -> &'static Mutex<Vec<PathBuf>> {
    static REPORTED: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    REPORTED.get_or_init(Default::default)
}

fn record_failure(path: &Path, _error: &std::io::Error) {
    lock(reported()).push(path.to_path_buf());
}

fn reports_of(path: &Path) -> usize {
    lock(reported()).iter().filter(|seen| *seen == path).count()
}

#[test]
fn a_failing_history_write_is_reported_once_until_it_succeeds() {
    on_history_write_failure(record_failure);
    let root = tempfile::TempDir::new().unwrap();
    let blocker = root.path().join("thread");
    std::fs::write(&blocker, b"a file where the folder should be").unwrap();
    let path = blocker.join(HISTORY_FILE);
    let history = vec![ChatMessage::new("user", "hello")];

    write_history_file_unguarded(&path, &history);
    write_history_file_unguarded(&path, &history);

    assert!(history_write_failing(&path));
    assert_eq!(reports_of(&path), 1);

    std::fs::remove_file(&blocker).unwrap();
    write_history_file_unguarded(&path, &history);

    assert!(!history_write_failing(&path));
    assert!(path.is_file());

    std::fs::remove_dir_all(&blocker).unwrap();
    std::fs::write(&blocker, b"blocked again").unwrap();
    write_history_file_unguarded(&path, &history);

    assert_eq!(reports_of(&path), 2);
}

#[test]
fn history_writes_leave_no_temporary_beside_the_file() {
    let root = tempfile::TempDir::new().unwrap();
    let path = root.path().join(HISTORY_FILE);
    let history = vec![ChatMessage::new("user", "hello")];

    write_history_file_unguarded(&path, &history);
    write_history_file_unguarded(&path, &history);

    let names: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [HISTORY_FILE]);
}
