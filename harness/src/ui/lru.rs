//! A tiny bounded cache with least-recently-used eviction.
//!
//! Recency is tracked with a monotonic access counter stored next to each
//! entry; on insert past the cap the entry with the lowest counter is dropped.
//! The whole map is never cleared, so unrelated hot entries survive and the
//! render path never pays a periodic "rebuild everything" spike.
//!
//! The caps involved are small (a few hundred entries), so the linear scan for
//! the eviction victim is cheaper than maintaining an intrusive order list.

use std::collections::HashMap;
use std::hash::Hash;

pub(super) struct LruCache<K, V> {
    pub(super) entries: HashMap<K, (u64, V)>,
    cap: usize,
    tick: u64,
}

impl<K: Eq + Hash + Copy, V> LruCache<K, V> {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            cap,
            tick: 0,
        }
    }

    /// Look up `key`, marking it as the most recently used entry on a hit.
    pub(super) fn get(&mut self, key: &K) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        let (stamp, value) = self.entries.get_mut(key)?;
        *stamp = tick;
        Some(value)
    }

    /// Insert `value`, evicting a single least-recently-used entry when the
    /// cache is at capacity and `key` is not already present.
    pub(super) fn insert(&mut self, key: K, value: V) {
        self.tick += 1;
        if !self.entries.contains_key(&key) && self.entries.len() >= self.cap {
            self.evict_lru();
        }
        self.entries.insert(key, (self.tick, value));
    }

    fn evict_lru(&mut self) {
        if let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, (stamp, _))| *stamp)
            .map(|(key, _)| *key)
        {
            self.entries.remove(&oldest);
        }
    }
}
