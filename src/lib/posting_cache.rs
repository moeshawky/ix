//! LRU cache for decoded posting lists, keyed by trigram.
//!
//! Avoids re-decoding compressed posting lists from the mmap when the same
//! trigram is queried repeatedly (e.g., across multiple queries in a daemon
//! session, or when the same trigram appears in multiple query plan variants).
//!
//! Eviction is FIFO oldest-first: when the memory ceiling is exceeded, the
//! oldest entries (front of the access-order list) are removed first.
//! `evict_fraction` removes a specified
//! fraction of the oldest entries under cache-policy control.

use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::posting::{PostingEntry, PostingList};
use crate::trigram::Trigram;

fn fraction_count_ceil(total: usize, fraction: f64) -> usize {
    if total == 0 || fraction <= 0.0_f64 || fraction.is_nan() {
        return 0;
    }
    if fraction >= 1.0_f64 {
        return total;
    }
    let bits = fraction.to_bits();
    let exp_bits = u32::try_from((bits >> 52) & 0x7FF).unwrap_or(0);
    if exp_bits == 0 {
        return 1;
    }
    let shift = 1023u32.saturating_sub(exp_bits);
    let denom_shift = 52u32.saturating_add(shift);
    if denom_shift >= 128 {
        return 1;
    }
    let mantissa = u128::from((bits & ((1u64 << 52) - 1)) | (1u64 << 52));
    let total_u128 = u128::try_from(total).unwrap_or(u128::MAX);
    let prod = total_u128.saturating_mul(mantissa);
    let q = prod >> denom_shift;
    let mask = (1u128 << denom_shift) - 1;
    let ceil_val = if (prod & mask) != 0 { q + 1 } else { q };
    usize::try_from(ceil_val).unwrap_or(total).min(total)
}

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

/// Deterministic test-only pause hook injected into cache-clear methods so
/// concurrency tests can prove lock coverage across invalidation without sleep.
#[cfg(all(test, feature = "notify", unix))]
#[derive(Clone, Debug)]
pub(crate) struct ClearPauseHook {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

/// RAII release guard for [`ClearPauseHook`]. Dropping the guard (including on
/// test panic unwind) wakes any paused cache-clear call immediately.
#[cfg(all(test, feature = "notify", unix))]
pub(crate) struct ClearPauseRelease {
    release: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

#[cfg(all(test, feature = "notify", unix))]
impl ClearPauseRelease {
    pub(crate) fn release(self) {
        drop(self);
    }
}

#[cfg(all(test, feature = "notify", unix))]
impl Drop for ClearPauseRelease {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.release;
        let mut released = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *released = true;
        cvar.notify_all();
    }
}

#[cfg(all(test, feature = "notify", unix))]
impl ClearPauseHook {
    pub(crate) fn channel() -> (Self, std::sync::mpsc::Receiver<()>, ClearPauseRelease) {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let release =
            std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let guard = ClearPauseRelease {
            release: std::sync::Arc::clone(&release),
        };
        let hook = Self {
            entered: entered_tx,
            release,
        };
        (hook, entered_rx, guard)
    }

    pub(crate) fn pause(&self) {
        let _ = self.entered.send(());
        let (lock, cvar) = &*self.release;
        let mut released = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !*released {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (guard, wait_res) = cvar
                .wait_timeout(released, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            released = guard;
            if wait_res.timed_out() && !*released {
                break;
            }
        }
    }
}

/// Thread-safe, memory-bounded LRU cache mapping [`Trigram`] → [`PostingList`].
///
/// Uses FIFO eviction (oldest first) when the `memory_ceiling` is exceeded.
/// [`evict_fraction`](Self::evict_fraction) removes the specified fraction of
/// oldest entries for adaptive cache policy control. Admission can be toggled
/// via [`set_admit`](Self::set_admit).
#[derive(Debug)]
pub struct PostingCache {
    map: RwLock<HashMap<Trigram, PostingList>>,
    order: RwLock<Vec<Trigram>>,
    memory_used: RwLock<usize>,
    memory_ceiling: usize,
    admit: RwLock<bool>,
    stats: RwLock<CacheStats>,
    shard_epoch: AtomicU64,
    #[cfg(all(test, feature = "notify", unix))]
    clear_pause_hook: RwLock<Option<ClearPauseHook>>,
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
            shard_epoch: AtomicU64::new(0),
            #[cfg(all(test, feature = "notify", unix))]
            clear_pause_hook: RwLock::new(None),
        }
    }

    /// Synchronize the cache to the given shard generation fingerprint.
    ///
    /// If the cache was previously bound to a different non-zero shard
    /// fingerprint (or already holds entries from an unbound shard), all
    /// cached posting lists are atomically evicted before updating the epoch.
    /// Returns `true` when existing cached entries were cleared due to a shard
    /// generation change.
    pub fn ensure_shard(&self, fingerprint: u64) -> bool {
        if self.shard_epoch.load(Ordering::Acquire) == fingerprint {
            return false;
        }
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
        let prev = self.shard_epoch.swap(fingerprint, Ordering::AcqRel);
        if prev == fingerprint {
            return false;
        }
        let had_entries = !map.is_empty();
        if prev != 0 || had_entries {
            map.clear();
            order.clear();
            *mem = 0;
        }
        drop(mem);
        drop(order);
        drop(map);
        prev != 0 || had_entries
    }

    #[cfg(all(test, feature = "notify", unix))]
    pub(crate) fn set_clear_pause_hook(&self, hook: Option<ClearPauseHook>) {
        *self
            .clear_pause_hook
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
    }

    /// Create a cache with the given ceiling, synced to the cache-policy
    /// ceiling. This is the constructor to use when a [`crate::cache_policy::AdaptiveCachePolicy`]
    /// manages the same memory budget.
    ///
    /// # Parameters
    ///
    /// - `ceiling`: byte budget, must match the policy's [`ceiling_bytes`](crate::cache_policy::AdaptiveCachePolicy::ceiling_bytes). 0 = reject all inserts.
    #[must_use]
    pub fn with_ceiling(ceiling: usize) -> Self {
        Self::new(ceiling)
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

        if self.memory_ceiling == 0 {
            return;
        }

        let entry_size = estimate_posting_size(&list);
        if entry_size > self.memory_ceiling {
            return;
        }

        // Unified lock acquisition order: map -> order -> memory_used -> stats
        // (matching invalidate, evict_fraction, and invalidate_all) to prevent
        // AB-BA deadlocks (PC-1), and check/remove any existing key under the
        // write lock to eliminate the concurrent-miss TOCTOU (PC-2).
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
        let mut stats = self
            .stats
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(removed) = map.remove(&trigram) {
            let removed_size = estimate_posting_size(&removed);
            *mem = mem.saturating_sub(removed_size);
            order.retain(|t| *t != trigram);
        }

        while mem.saturating_add(entry_size) > self.memory_ceiling {
            if let Some(evict) = order.first().copied() {
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

    /// Evict the oldest `fraction` of entries (FIFO order from the front of
    /// the access-order list). Used by the adaptive cache policy under memory
    /// pressure.
    ///
    /// # Parameters
    ///
    /// - `fraction` (`f64`): portion of entries to evict. Must be in (0.0, 1.0];
    ///   values outside this range are no-ops (0.0 or less) or flush everything
    ///   (≥ 1.0, delegates to [`invalidate_all`](Self::invalidate_all)).
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn evict_fraction(&self, fraction: f64) {
        if fraction <= 0.0_f64 || fraction.is_nan() {
            return;
        }
        if fraction >= 1.0_f64 {
            self.invalidate_all();
            return;
        }
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
        let mut stats = self
            .stats
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = order.len();
        let to_remove = fraction_count_ceil(total, fraction);
        for _ in 0..to_remove {
            if let Some(evict) = order.first().copied() {
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
        drop(stats);
        drop(mem);
        drop(order);
        drop(map);
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn invalidate_all(&self) {
        #[cfg(all(test, feature = "notify", unix))]
        {
            let hook = self
                .clear_pause_hook
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(h) = hook {
                h.pause();
            }
        }
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
mod tests {
    use super::*;

    fn trigram(a: u8, b: u8, c: u8) -> Trigram {
        crate::trigram::from_bytes(a, b, c)
    }

    #[test]
    fn basic_insert_get() {
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
        assert_eq!(result.expect("expected cached posting"), list);

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
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

    #[test]
    fn oversized_entry_rejected() {
        let cache = PostingCache::new(100); // 100 byte ceiling
        let t = trigram(b'a', b'b', b'c');
        // Create a posting list larger than 100 bytes
        let list = PostingList {
            entries: vec![PostingEntry {
                file_id: 1,
                offsets: vec![0; 500], // 500 offsets * 8 bytes = 4000+ bytes
            }],
        };

        cache.insert(t, list);
        // Oversized entry should NOT be stored
        assert!(cache.get(t).is_none());
        assert_eq!(*cache.memory_used.read().unwrap(), 0);
    }

    #[test]
    fn evict_fraction_removes_oldest() {
        let cache = PostingCache::new(10_000);
        let t1 = trigram(b'a', b'b', b'c');
        let t2 = trigram(b'b', b'c', b'd');
        let t3 = trigram(b'c', b'd', b'e');
        let t4 = trigram(b'd', b'e', b'f');

        // Insert 4 entries in order
        for &t in &[t1, t2, t3, t4] {
            cache.insert(
                t,
                PostingList {
                    entries: vec![PostingEntry {
                        file_id: 1,
                        offsets: vec![10],
                    }],
                },
            );
        }

        // Evict 50% (should remove ~2 oldest entries)
        cache.evict_fraction(0.5);

        // t1 and t2 (oldest) should be evicted, t3 and t4 should remain
        assert!(cache.get(t1).is_none());
        assert!(cache.get(t2).is_none());
        assert!(cache.get(t3).is_some());
        assert!(cache.get(t4).is_some());
    }

    #[test]
    fn concurrent_insert_evict_invalidate_no_deadlock_or_leak() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let sample = PostingList {
            entries: vec![PostingEntry {
                file_id: 1,
                offsets: vec![1, 2, 3, 4],
            }],
        };
        let entry_size = estimate_posting_size(&sample);
        let ceiling = entry_size * 12;
        let cache = Arc::new(PostingCache::new(ceiling));
        let barrier = Arc::new(Barrier::new(8));

        let mut handles = Vec::new();
        for tid in 0..8u32 {
            let c = Arc::clone(&cache);
            let b = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                b.wait();
                for iter in 0..200u32 {
                    // Shared hot key + per-thread keys to stress PC-2 and PC-1 simultaneously
                    let hot_tri = trigram(b'a', b'a', b'a');
                    let key_tri = trigram(
                        b'a' + u8::try_from((tid + iter) % 20).unwrap_or(0),
                        b'b',
                        b'c' + u8::try_from(iter % 10).unwrap_or(0),
                    );
                    let list = PostingList {
                        entries: vec![PostingEntry {
                            file_id: iter,
                            offsets: vec![tid, iter],
                        }],
                    };
                    c.insert(hot_tri, list.clone());
                    c.insert(key_tri, list);
                    let _ = c.get(hot_tri);
                    if iter % 13 == 0 {
                        c.evict_fraction(0.25);
                    }
                    if iter % 37 == 0 {
                        c.invalidate(key_tri);
                    }
                    if iter % 71 == 0 {
                        c.invalidate_all();
                    }
                }
            }));
        }

        for h in handles {
            h.join().expect("worker thread must not panic or deadlock");
        }

        assert!(
            cache.memory_used() <= ceiling,
            "memory_used {} exceeded ceiling {}",
            cache.memory_used(),
            ceiling
        );
        cache.invalidate_all();
        assert!(cache.is_empty());
        assert_eq!(cache.memory_used(), 0);

        // Verify order vec has no leaked duplicates after concurrent same-key inserts
        assert!(cache.order.read().unwrap().is_empty());
    }
}
