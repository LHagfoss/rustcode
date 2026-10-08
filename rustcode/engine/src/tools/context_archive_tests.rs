use super::*;
use crate::app::{ChatMessage, CompactionBoundary, ToolCallRef};
use rustcode_session::SessionStore;
use serde_json::json;
use sha2::{Digest, Sha256};

const SESSION: &str = "01a1103224ad-7000-a206-4498-44988b520034";

fn archive(root: &Path, messages: &[ChatMessage]) -> PathBuf {
    let directory = root.join("history_archive");
    std::fs::create_dir_all(&directory).unwrap();
    let mut bytes = Vec::new();
    for message in messages {
        serde_json::to_writer(&mut bytes, message).unwrap();
        bytes.push(b'\n');
    }
    let path = directory.join(format!("{}.jsonl", hex::encode(Sha256::digest(&bytes))));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn summary(path: &Path) -> ChatMessage {
    ChatMessage::new("system", "[Session History Summary]\nEarlier work").with_compaction_boundary(
        CompactionBoundary {
            version: 1,
            summary: "Earlier work".into(),
            first_retained_entry: None,
            history_archive: Some(path.to_string_lossy().into_owned()),
        },
    )
}

fn save(root: &Path, history: &[ChatMessage]) {
    SessionStore::new(root.to_path_buf()).save_session_history(SESSION, history);
    rustcode_session::flush_history();
}

fn zoom(root: &Path, args: Value) -> Value {
    let output = zoom_in_session(root, SESSION, &args).unwrap();
    assert!(output.len() <= MAX_OUTPUT_BYTES);
    serde_json::from_str(&output).unwrap()
}

#[test]
fn zoom_recovers_exact_messages_across_compactions_after_reload() {
    let root = tempfile::tempdir().unwrap();
    let first = archive(
        root.path(),
        &[ChatMessage::new("user", "Correction: port 5433, not 5432.")],
    );
    let second = archive(
        root.path(),
        &[
            summary(&first),
            ChatMessage::new("assistant", "The database connection is verified."),
        ],
    );
    save(
        root.path(),
        &[
            summary(&second),
            ChatMessage::new("user", "What port did I specify?"),
        ],
    );
    let before = SessionStore::new(root.path().to_path_buf()).load_session_history_direct(SESSION);

    let listing = zoom(root.path(), json!({}));
    assert_eq!(listing["messages"][0]["child_path"], json!([1]));
    assert_eq!(listing["messages"][1]["role"], "assistant");
    let original = zoom(root.path(), json!({"path":[1], "message":1}));
    assert_eq!(original["role"], "user");
    assert_eq!(original["text"], "Correction: port 5433, not 5432.");
    assert!(original["next_offset"].is_null());
    assert_eq!(
        before,
        SessionStore::new(root.path().to_path_buf()).load_session_history_direct(SESSION)
    );
}

#[test]
fn zoom_pages_inventory_without_dropping_messages() {
    let root = tempfile::tempdir().unwrap();
    let messages = (1..=25)
        .map(|i| ChatMessage::new("user", format!("instruction {i}")))
        .collect::<Vec<_>>();
    let path = archive(root.path(), &messages);
    save(root.path(), &[summary(&path)]);
    let mut start = 1;
    let mut ids = Vec::new();
    loop {
        let page = zoom(root.path(), json!({"start":start}));
        ids.extend(
            page["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["message"].as_u64().unwrap()),
        );
        match page["next_start"].as_u64() {
            Some(next) => {
                assert!(next > start);
                start = next;
            }
            None => break,
        }
    }
    assert_eq!(ids, (1..=25).collect::<Vec<_>>());
}

#[test]
fn zoom_pages_unicode_and_escaped_text_without_losing_bytes() {
    let root = tempfile::tempdir().unwrap();
    let text = "å🦀\n\t\u{0001}\"\\".repeat(3_000);
    let path = archive(root.path(), &[ChatMessage::new("tool", &text)]);
    save(root.path(), &[summary(&path)]);
    let mut offset = 0;
    let mut recovered = String::new();
    loop {
        let page = zoom(root.path(), json!({"message":1, "offset":offset}));
        recovered.push_str(page["text"].as_str().unwrap());
        match page["next_offset"].as_u64() {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(recovered, text);
    let error =
        zoom_in_session(root.path(), SESSION, &json!({"message":1,"offset":1})).unwrap_err();
    assert!(error.contains("UTF-8"), "{error}");
}

#[test]
fn zoom_retains_native_tool_arguments_and_result_identity() {
    let root = tempfile::tempdir().unwrap();
    let path = archive(
        root.path(),
        &[
            ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
                id: "call-17".into(),
                name: "run_command".into(),
                arguments: "{\"command\":\"cargo test --workspace\"}".into(),
            }]),
            ChatMessage::new("tool", "All tests passed").answering(Some("call-17".into())),
        ],
    );
    save(root.path(), &[summary(&path)]);
    let call = zoom(root.path(), json!({"message":1}));
    assert!(
        call["text"]
            .as_str()
            .unwrap()
            .contains("cargo test --workspace")
    );
    assert!(call["text"].as_str().unwrap().contains("call-17"));
    let result = zoom(root.path(), json!({"message":2}));
    assert_eq!(result["tool_call_id"], "call-17");
    assert_eq!(result["text"], "All tests passed");
}

#[test]
fn zoom_rejects_arbitrary_archive_and_session_arguments() {
    let root = tempfile::tempdir().unwrap();
    let path = archive(
        root.path(),
        &[ChatMessage::new("user", "private to this session")],
    );
    save(root.path(), &[summary(&path)]);
    for args in [
        json!({"archive":path}),
        json!({"session_id":"other"}),
        json!({"path":"../../other"}),
        json!({"path":[0]}),
        json!({"message":0}),
        json!({"offset":1}),
        json!({"start":-1}),
        json!({"message":1,"start":2}),
        json!({"message":"1"}),
    ] {
        assert!(
            zoom_in_session(root.path(), SESSION, &args).is_err(),
            "{args}"
        );
    }
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({"path":[1]}))
            .unwrap_err()
            .contains("compaction")
    );
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({"message":2}))
            .unwrap_err()
            .contains("range")
    );
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({"message":1,"offset":1000}))
            .unwrap_err()
            .contains("range")
    );
}

#[test]
fn zoom_fails_closed_on_corruption_missing_archives_and_foreign_links() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = archive(
        outside.path(),
        &[ChatMessage::new("user", "foreign archive")],
    );
    save(root.path(), &[summary(&path)]);
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({}))
            .unwrap_err()
            .contains("archive directory")
    );

    let path = archive(root.path(), &[ChatMessage::new("user", "original text")]);
    save(root.path(), &[summary(&path)]);
    std::fs::write(&path, "tampered").unwrap();
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({}))
            .unwrap_err()
            .contains("content address")
    );
    std::fs::remove_file(&path).unwrap();
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({}))
            .unwrap_err()
            .contains("archive")
    );
}

#[test]
fn zoom_reports_legacy_compaction_without_an_archive() {
    let root = tempfile::tempdir().unwrap();
    save(
        root.path(),
        &[ChatMessage::new(
            "system",
            "[Session History Summary]\nLegacy summary",
        )],
    );
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({}))
            .unwrap_err()
            .contains("No recoverable")
    );
}

#[test]
fn zoom_rejects_malformed_and_oversized_archives_before_returning_data() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("history_archive");
    std::fs::create_dir_all(&directory).unwrap();
    let bytes = b"not a JSON message\n";
    let path = directory.join(format!("{}.jsonl", hex::encode(Sha256::digest(bytes))));
    std::fs::write(&path, bytes).unwrap();
    save(root.path(), &[summary(&path)]);
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({}))
            .unwrap_err()
            .contains("Invalid archived message")
    );
    std::fs::File::create(&path)
        .unwrap()
        .set_len(MAX_ARCHIVE_BYTES as u64 + 1)
        .unwrap();
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({}))
            .unwrap_err()
            .contains("byte limit")
    );
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({"path":vec![1;33]}))
            .unwrap_err()
            .contains("navigation limit")
    );
}

#[test]
fn zoom_rejects_a_stale_root_after_a_new_compaction() {
    let root = tempfile::tempdir().unwrap();
    let first = archive(root.path(), &[ChatMessage::new("user", "original request")]);
    save(root.path(), &[summary(&first)]);
    let old_root = zoom(root.path(), json!({}))["root"].clone();
    let second = archive(
        root.path(),
        &[summary(&first), ChatMessage::new("user", "new request")],
    );
    save(root.path(), &[summary(&second)]);
    let error =
        zoom_in_session(root.path(), SESSION, &json!({"root":old_root,"message":1})).unwrap_err();
    assert!(error.contains("Compaction changed"), "{error}");
    let current = zoom(root.path(), json!({}));
    assert_ne!(current["root"], old_root);
    assert_eq!(
        zoom(
            root.path(),
            json!({"root":current["root"],"path":[1],"message":1})
        )["text"],
        "original request"
    );
}

#[test]
fn zoom_stops_at_the_cumulative_archive_byte_limit() {
    let root = tempfile::tempdir().unwrap();
    let text = "x".repeat(14 * 1024 * 1024);
    let mut path = archive(root.path(), &[ChatMessage::new("tool", &text)]);
    for _ in 0..4 {
        path = archive(
            root.path(),
            &[summary(&path), ChatMessage::new("tool", &text)],
        );
    }
    save(root.path(), &[summary(&path)]);
    let error = zoom_in_session(root.path(), SESSION, &json!({"path":[1,1,1,1]})).unwrap_err();
    assert!(error.contains("byte limit"), "{error}");
}

#[test]
fn zoom_reports_the_depth_limit_without_emitting_an_unusable_cursor() {
    let root = tempfile::tempdir().unwrap();
    let mut path = archive(root.path(), &[ChatMessage::new("user", "original request")]);
    for _ in 0..33 {
        path = archive(root.path(), &[summary(&path)]);
    }
    save(root.path(), &[summary(&path)]);
    let listing = zoom(root.path(), json!({"path":vec![1;32]}));
    assert!(listing["messages"][0]["child_path"].is_null());
    assert_eq!(listing["messages"][0]["depth_limit_reached"], true);
}

#[cfg(unix)]
#[test]
fn zoom_rejects_archive_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = archive(
        outside.path(),
        &[ChatMessage::new("user", "foreign archive")],
    );
    let directory = root.path().join("history_archive");
    std::fs::create_dir_all(&directory).unwrap();
    let link = directory.join(source.file_name().unwrap());
    std::os::unix::fs::symlink(source, &link).unwrap();
    save(root.path(), &[summary(&link)]);
    assert!(
        zoom_in_session(root.path(), SESSION, &json!({}))
            .unwrap_err()
            .contains("regular file")
    );
}
