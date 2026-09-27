use super::PromptCache;

#[test]
fn skill_metadata_reuses_the_cached_allocation() {
    let mut cache = PromptCache::default();
    let first = cache.skill_metadata();
    let first_ptr = first.as_ptr();
    let first_len = first.len();

    let second = cache.skill_metadata();

    assert_eq!(second.as_ptr(), first_ptr);
    assert_eq!(second.len(), first_len);
}
