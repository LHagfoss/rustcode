use super::PromptCache;
use std::fs;
use std::path::{Path, PathBuf};

struct ActiveWorkspaceGuard;

impl ActiveWorkspaceGuard {
    fn set(path: &Path) -> Self {
        crate::tools::set_active_workspace_root(Some(path.to_path_buf()));
        Self
    }
}

impl Drop for ActiveWorkspaceGuard {
    fn drop(&mut self) {
        crate::tools::set_active_workspace_root(None);
    }
}

fn write_skill(workspace: &Path, name: &str, description: &str) -> PathBuf {
    let directory = workspace.join(".rustcode/skills").join(name);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nBody"),
    )
    .unwrap();
    directory
}

fn list_skills() {
    let tool = crate::tools::TOOLS
        .iter()
        .find(|tool| tool.name == "list_skills")
        .unwrap();
    (tool.handler)(&serde_json::json!({})).unwrap();
}

fn use_skill(name: &str) {
    let tool = crate::tools::TOOLS
        .iter()
        .find(|tool| tool.name == "use_skill")
        .unwrap();
    (tool.handler)(&serde_json::json!({ "name": name })).unwrap();
}

#[test]
fn skill_metadata_reuses_the_cached_allocation() {
    let _catalog = crate::skills::lock_skill_catalog_tests();
    let mut cache = PromptCache::default();
    let first = cache.skill_metadata();
    let first_ptr = first.as_ptr();
    let first_len = first.len();

    let second = cache.skill_metadata();

    assert_eq!(second.as_ptr(), first_ptr);
    assert_eq!(second.len(), first_len);
}

#[test]
fn live_skill_listing_refreshes_cached_metadata_after_edit() {
    let _catalog = crate::skills::lock_skill_catalog_tests();
    let workspace = tempfile::tempdir().unwrap();
    let _workspace = ActiveWorkspaceGuard::set(workspace.path());
    let name = "cache-refresh-edit-repro";
    let directory = write_skill(workspace.path(), name, "before");
    let mut cache = PromptCache::default();
    assert_eq!(
        cache
            .skill_metadata()
            .iter()
            .find(|skill| skill.name == name)
            .unwrap()
            .description,
        "before"
    );

    write_skill(workspace.path(), name, "after");
    list_skills();

    assert_eq!(
        cache
            .skill_metadata()
            .iter()
            .find(|skill| skill.name == name)
            .unwrap()
            .description,
        "after"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn live_skill_listing_refreshes_cached_metadata_after_add() {
    let _catalog = crate::skills::lock_skill_catalog_tests();
    let workspace = tempfile::tempdir().unwrap();
    let _workspace = ActiveWorkspaceGuard::set(workspace.path());
    let name = "cache-refresh-add-repro";
    let mut cache = PromptCache::default();
    assert!(
        !cache
            .skill_metadata()
            .iter()
            .any(|skill| skill.name == name)
    );

    write_skill(workspace.path(), name, "new skill");
    list_skills();

    assert!(
        cache
            .skill_metadata()
            .iter()
            .any(|skill| skill.name == name)
    );
}

#[test]
fn live_skill_listing_refreshes_cached_metadata_after_remove() {
    let _catalog = crate::skills::lock_skill_catalog_tests();
    let workspace = tempfile::tempdir().unwrap();
    let _workspace = ActiveWorkspaceGuard::set(workspace.path());
    let name = "cache-refresh-remove-repro";
    let directory = write_skill(workspace.path(), name, "to remove");
    let mut cache = PromptCache::default();
    assert!(
        cache
            .skill_metadata()
            .iter()
            .any(|skill| skill.name == name)
    );

    fs::remove_dir_all(directory).unwrap();
    list_skills();

    assert!(
        !cache
            .skill_metadata()
            .iter()
            .any(|skill| skill.name == name)
    );
}

#[test]
fn using_a_skill_refreshes_cached_metadata_after_edit() {
    let _catalog = crate::skills::lock_skill_catalog_tests();
    let workspace = tempfile::tempdir().unwrap();
    let _workspace = ActiveWorkspaceGuard::set(workspace.path());
    let name = "cache-refresh-use-skill-repro";
    write_skill(workspace.path(), name, "before");
    let mut cache = PromptCache::default();
    assert_eq!(
        cache
            .skill_metadata()
            .iter()
            .find(|skill| skill.name == name)
            .unwrap()
            .description,
        "before"
    );

    write_skill(workspace.path(), name, "after");
    use_skill(name);

    assert_eq!(
        cache
            .skill_metadata()
            .iter()
            .find(|skill| skill.name == name)
            .unwrap()
            .description,
        "after"
    );
}

#[test]
fn live_catalog_generation_refreshes_each_cache_once() {
    let _catalog = crate::skills::lock_skill_catalog_tests();
    let workspace = tempfile::tempdir().unwrap();
    let _workspace = ActiveWorkspaceGuard::set(workspace.path());
    let name = "cache-generation-repro";
    write_skill(workspace.path(), name, "before");
    let mut first_cache = PromptCache::default();
    let mut second_cache = PromptCache::default();
    let first_initial = first_cache.skill_metadata();
    let second_initial = second_cache.skill_metadata();
    let generation_before_live_read = crate::skills::skill_catalog_generation();

    write_skill(workspace.path(), name, "after");
    list_skills();

    let generation_after_live_read = crate::skills::skill_catalog_generation();
    assert!(generation_after_live_read > generation_before_live_read);
    let first_refreshed = first_cache.skill_metadata();
    let second_refreshed = second_cache.skill_metadata();
    for metadata in [&first_refreshed, &second_refreshed] {
        assert_eq!(
            metadata
                .iter()
                .find(|skill| skill.name == name)
                .unwrap()
                .description,
            "after"
        );
    }
    assert!(!std::sync::Arc::ptr_eq(&first_initial, &first_refreshed));
    assert!(!std::sync::Arc::ptr_eq(&second_initial, &second_refreshed));

    let first_stable = first_cache.skill_metadata();
    let second_stable = second_cache.skill_metadata();
    assert!(std::sync::Arc::ptr_eq(&first_refreshed, &first_stable));
    assert!(std::sync::Arc::ptr_eq(&second_refreshed, &second_stable));
    assert_eq!(
        crate::skills::skill_catalog_generation(),
        generation_after_live_read,
        "cache reads must not invalidate their own snapshot"
    );
}
