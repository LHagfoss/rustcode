use super::*;
use base64::Engine as _;
use std::fs;

fn skill(dir: &Path, name: &str, body: &str) {
    let path = dir.join("skills").join(name);
    fs::create_dir_all(path.join("scripts")).unwrap();
    fs::write(path.join("SKILL.md"), body).unwrap();
    fs::write(path.join("scripts/run.sh"), "echo hi").unwrap();
}

fn copy(main: &Path, follower: &Path, kinds: &[ConfigKind]) -> CopyReport {
    let remote = list(main, kinds);
    let fetched: Vec<Entry> = plan(follower, kinds, &remote)
        .into_iter()
        .map(|(kind, name)| read(main, kind, &name).unwrap())
        .collect();
    apply(follower, "main-id", kinds, &remote, &fetched).unwrap()
}

#[test]
fn a_skill_travels_with_its_extra_files_and_updates_in_place() {
    let main = tempfile::tempdir().unwrap();
    let follower = tempfile::tempdir().unwrap();
    skill(main.path(), "github-voice", "v1");
    let report = copy(main.path(), follower.path(), &ConfigKind::ALL);
    assert_eq!(report.written, vec!["skills/github-voice"]);
    let copied = follower.path().join("skills/github-voice");
    assert_eq!(fs::read_to_string(copied.join("SKILL.md")).unwrap(), "v1");
    assert_eq!(
        fs::read_to_string(copied.join("scripts/run.sh")).unwrap(),
        "echo hi"
    );

    let again = copy(main.path(), follower.path(), &ConfigKind::ALL);
    assert!(again.written.is_empty() && again.removed.is_empty());

    skill(main.path(), "github-voice", "v2");
    fs::remove_file(main.path().join("skills/github-voice/scripts/run.sh")).unwrap();
    let report = copy(main.path(), follower.path(), &ConfigKind::ALL);
    assert_eq!(report.written, vec!["skills/github-voice"]);
    assert_eq!(fs::read_to_string(copied.join("SKILL.md")).unwrap(), "v2");
    assert!(!copied.join("scripts/run.sh").exists());
}

#[test]
fn pruning_removes_only_copies_from_that_machine() {
    let main = tempfile::tempdir().unwrap();
    let follower = tempfile::tempdir().unwrap();
    skill(main.path(), "shared", "from main");
    skill(follower.path(), "mine", "my own");
    copy(main.path(), follower.path(), &ConfigKind::ALL);

    fs::remove_dir_all(main.path().join("skills/shared")).unwrap();
    let report = copy(main.path(), follower.path(), &ConfigKind::ALL);
    assert_eq!(report.removed, vec!["skills/shared"]);
    assert!(!follower.path().join("skills/shared").exists());
    assert_eq!(
        fs::read_to_string(follower.path().join("skills/mine/SKILL.md")).unwrap(),
        "my own"
    );
}

#[test]
fn themes_and_commands_follow_their_own_toggle() {
    let main = tempfile::tempdir().unwrap();
    let follower = tempfile::tempdir().unwrap();
    fs::create_dir_all(main.path().join("themes")).unwrap();
    fs::create_dir_all(main.path().join("commands")).unwrap();
    fs::write(main.path().join("themes/rain.toml"), "name = 'rain'").unwrap();
    fs::write(main.path().join("themes/notes.txt"), "not a theme").unwrap();
    fs::write(main.path().join("commands/review.md"), "review it").unwrap();

    let report = copy(main.path(), follower.path(), &[ConfigKind::Themes]);
    assert_eq!(report.written, vec!["themes/rain.toml"]);
    assert!(!follower.path().join("themes/notes.txt").exists());
    assert!(!follower.path().join("commands/review.md").exists());

    let report = copy(main.path(), follower.path(), &[ConfigKind::Commands]);
    assert_eq!(report.written, vec!["commands/review.md"]);
}

#[test]
fn a_put_refuses_paths_outside_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let engine = base64::engine::general_purpose::STANDARD;
    let file = |path: &str| EntryFile {
        path: path.to_owned(),
        content: engine.encode("x"),
    };
    for (name, path) in [
        ("../escape", "SKILL.md"),
        ("ok", "../SKILL.md"),
        ("ok", "/etc/passwd"),
        ("ok", "a/.hidden"),
    ] {
        let entry = Entry {
            kind: ConfigKind::Skills,
            name: name.to_owned(),
            files: vec![file("SKILL.md"), file(path)],
        };
        assert!(put(dir.path(), "main-id", &entry).is_err(), "{name} {path}");
    }
    let theme = Entry {
        kind: ConfigKind::Themes,
        name: "credentials.json".to_owned(),
        files: vec![file(SINGLE_FILE)],
    };
    assert!(put(dir.path(), "main-id", &theme).is_err());
    assert!(!dir.path().join("credentials.json").exists());
}

#[test]
fn an_entry_over_the_limit_is_listed_as_skipped() {
    let dir = tempfile::tempdir().unwrap();
    skill(dir.path(), "huge", "big");
    fs::write(
        dir.path().join("skills/huge/blob.bin"),
        vec![0u8; ENTRY_LIMIT as usize + 1],
    )
    .unwrap();
    let listed = list(dir.path(), &[ConfigKind::Skills]);
    assert_eq!(listed.len(), 1);
    assert!(listed[0].skipped.is_some());
}
