//! LRU cache for decoded posting lists, keyed by trigram.
//!
//! Avoids re-decoding compressed posting lists from the mmap when the same
//! trigram is queried repeatedly (e.g., across multiple queries in a daemon
//! session, or when the same trigram appears in multiple query plan variants).

use std::collections::HashMap;
use std::sync::RwLock;

use crate::posting::{PostingEntry, PostingList};
use crate::trigram::Trigram;

fn estimate_posting_size(list: &PostingList) -> usize {
    let mut size: usize = 16;
    for entry in &list.entries {
        size += 4 + entry.offsets.len() * 4 + 16;
    }
    size += list.entries.capacity() * std::mem::size_of::<PostingEntry>();
    size
}

/// Hit/miss/eviction counters for a [`PostingCache`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheStats {
    /// Number of cache hits.
    pub hits: u64,
    /// Number of cache misses.
    pub misses: u64,
    /// Number of entries evicted due to memory pressure.
    pub evictions: u64,
}

/// Thread-safe, memory-bounded LRU cache mapping [`Trigram`] → [`PostingList`].
///
/// Uses FIFO eviction (oldest first) when the `memory_ceiling` is exceeded.
/// Admission can be toggled via [`set_admit`](Self::set_admit) for adaptive
/// cache policy integration.
#[derive(Debug)]
pub struct PostingCache {
    map: RwLock<HashMap<Trigram, PostingList>>,
    order: RwLock<Vec<Trigram>>,
    memory_used: RwLock<usize>,
    memory_ceiling: usize,
    admit: RwLock<bool>,
    stats: RwLock<CacheStats>,
}

impl PostingCache {
    /// Create a cache with the given byte budget (0 = reject all inserts).
    #[must_use]
    pub fn new(memory_ceiling: usize) -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
            order: RwLock::new(Vec::new()),
            memory_used: RwLock::new(0),
            memory_ceiling,
            admit: RwLock::new(true),
            stats: RwLock::new(CacheStats {
                hits: 0,
                misses: 0,
                evictions: 0,
            }),
        }
    }

    /// Set whether new entries are admitted.
    ///
    /// When `false`, `insert` returns immediately without caching.
    /// Existing entries are retained (not evicted) — call `invalidate_all` to flush.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn set_admit(&self, allow: bool) {
        *self
            .admit
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = allow;
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn get(&self, trigram: Trigram) -> Option<PostingList> {
        let map = self
            .map
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(list) = map.get(&trigram) {
            let result = list.clone();
            drop(map);
            let mut stats = self
                .stats
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stats.hits += 1;
            drop(stats);
            Some(result)
        } else {
            drop(map);
            let mut stats = self
                .stats
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stats.misses += 1;
            drop(stats);
            None
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn insert(&self, trigram: Trigram, list: PostingList) {
        if !*self
            .admit
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return;
        }

        let entry_size = estimate_posting_size(&list);

        if self.memory_ceiling == 0 {
            return;
        }

        let existing_size = {
            let map = self
                .map
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.get(&trigram).map_or(0, estimate_posting_size)
        };

        if existing_size > 0 {
            let mut map = self
                .map
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut order = self
                .order
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut mem = self
                .memory_used
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.remove(&trigram);
            order.retain(|t| *t != trigram);
            *mem = mem.saturating_sub(existing_size);
            map.insert(trigram, list);
            order.push(trigram);
            *mem += entry_size;
            drop(mem);
            drop(order);
            drop(map);
            return;
        }

        {
            let mut mem = self
                .memory_used
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut map = self
                .map
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut order = self
                .order
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut stats = self
                .stats
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            while *mem + entry_size > self.memory_ceiling {
                if let Some(evict) = order.first().copied() {
                    if evict == trigram {
                        break;
                    }
                    order.remove(0);
                    if let Some(removed) = map.remove(&evict) {
                        let removed_size = estimate_posting_size(&removed);
                        *mem = mem.saturating_sub(removed_size);
                    }
                    stats.evictions += 1;
                } else {
                    break;
                }
            }

            map.insert(trigram, list);
            order.push(trigram);
            *mem += entry_size;
            drop(stats);
            drop(mem);
            drop(order);
            drop(map);
        }
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn invalidate(&self, trigram: Trigram) {
        let mut map = self
            .map
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut order = self
            .order
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut mem = self
            .memory_used
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(removed) = map.remove(&trigram) {
            let removed_size = estimate_posting_size(&removed);
            *mem = mem.saturating_sub(removed_size);
            order.retain(|t| *t != trigram);
        }
        drop(mem);
        drop(order);
        drop(map);
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn invalidate_all(&self) {
        let mut map = self
            .map
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut order = self
            .order
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut mem = self
            .memory_used
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.clear();
        order.clear();
        *mem = 0;
        drop(mem);
        drop(order);
        drop(map);
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn stats(&self) -> CacheStats {
        self.stats
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn memory_used(&self) -> usize {
        *self
            .memory_used
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for PostingCache {
    fn default() -> Self {
        Self::new(64 * 1024 * 1024)
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn trigram(a: u8, b: u8, c: u8) -> Trigram {
        crate::trigram::from_bytes(a, b, c)
    }

    #[test]
    fn basic_insert_get() -> Result<(), Box<dyn std::error::Error>> {
        let cache = PostingCache::new(1024 * 1024);
        let t = trigram(b'a', b'b', b'c');
        let list = PostingList {
            entries: vec![PostingEntry {
                file_id: 1,
                offsets: vec![10, 20],
            }],
        };

        assert!(cache.get(t).is_none());
        let stats = cache.stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 0);

        cache.insert(t, list.clone());
        let result = cache.get(t);
        assert!(result.is_some());
        assert_eq!(result.ok_or("expected cached posting")?, list);

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        Ok(())
    }

    #[test]
    fn eviction_under_memory_pressure() {
        let small_list = PostingList {
            entries: vec![PostingEntry {
                file_id: 0,
                offsets: vec![10, 20],
            }],
        };
        let entry_size = estimate_posting_size(&small_list);
        let ceiling = entry_size * 2 + 10;
        let cache = PostingCache::new(ceiling);

        let t1 = trigram(b'a', b'b', b'c');
        let t2 = trigram(b'd', b'e', b'f');
        let t3 = trigram(b'g', b'h', b'i');

        cache.insert(t1, small_list.clone());
        cache.insert(t2, small_list.clone());
        assert!(cache.get(t1).is_some());
        assert!(cache.get(t2).is_some());

        cache.insert(t3, small_list);
        let stats = cache.stats();
        assert!(stats.evictions > 0, "evictions should have occurred");
        assert!(cache.len() <= 2);
    }

    #[test]
    fn invalidate_all() {
        let cache = PostingCache::new(1024 * 1024);
        let t1 = trigram(b'a', b'b', b'c');
        let t2 = trigram(b'd', b'e', b'f');

        let list = PostingList {
            entries: vec![PostingEntry {
                file_id: 1,
                offsets: vec![10],
            }],
        };

        cache.insert(t1, list.clone());
        cache.insert(t2, list);
        assert!(cache.get(t1).is_some());
        assert!(cache.get(t2).is_some());

        cache.invalidate_all();
        assert!(cache.get(t1).is_none());
        assert!(cache.get(t2).is_none());
        assert_eq!(cache.memory_used(), 0);
    }

    #[test]
    fn invalidate_single() {
        let cache = PostingCache::new(1024 * 1024);
        let t1 = trigram(b'a', b'b', b'c');
        let t2 = trigram(b'd', b'e', b'f');

        let list = PostingList {
            entries: vec![PostingEntry {
                file_id: 1,
                offsets: vec![10],
            }],
        };

        cache.insert(t1, list.clone());
        cache.insert(t2, list);
        cache.invalidate(t1);
        assert!(cache.get(t1).is_none());
        assert!(cache.get(t2).is_some());
    }

    #[test]
    fn zero_ceiling_rejects_all() {
        let cache = PostingCache::new(0);
        let t = trigram(b'a', b'b', b'c');
        let list = PostingList {
            entries: vec![PostingEntry {
                file_id: 1,
                offsets: vec![10],
            }],
        };

        cache.insert(t, list);
        assert!(cache.get(t).is_none());
    }
}
