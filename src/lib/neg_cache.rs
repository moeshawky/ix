//! Negative result cache.
//!
//! Stores "file X with `content_hash` H does NOT match query Q" results.
//! Prevents re-verification of unchanged files that previously had no matches.

use std::collections::HashSet;
use std::sync::RwLock;
use xxhash_rust::xxh64::xxh64;

/// Hit/miss/eviction counters for a [`NegCache`].
///
/// Public for external monitoring via [`NegCache::stats`].
#[derive(Debug, Default, Clone)]
pub struct NegCacheStats {
    /// Number of negative cache hits (file skipped).
    pub hits: u64,
    /// Number of negative cache misses (file not in cache).
    pub misses: u64,
    /// Number of entries evicted due to capacity or policy.
    pub evictions: u64,
}

/// Thread-safe negative result cache.
///
/// Keys are `(query_fingerprint, content_hash)` pairs. When a file is verified
/// and produces zero matches, it is recorded here so subsequent queries for the
/// same pattern can skip re-reading the file if its content hasn't changed.
///
/// # Eviction policy
///
/// Unlike [`crate::posting_cache::PostingCache`] which uses FIFO (oldest-first)
/// eviction, `NegCache` evicts entries in **arbitrary order** (determined by
/// `HashSet` iteration). There is no access tracking, so hot entries may be
/// evicted before cold ones. This is acceptable because the cost of an eviction
/// is a single re-verification (reading a file that previously produced zero
/// matches), which is cheap relative to trigram posting list decoding.
#[derive(Debug)]
pub struct NegCache {
    set: RwLock<HashSet<(u64, u64)>>,
    max_entries: usize,
    admit: RwLock<bool>,
    stats: RwLock<NegCacheStats>,
    #[cfg(all(test, feature = "notify", unix))]
    clear_pause_hook: RwLock<Option<crate::posting_cache::ClearPauseHook>>,
}

impl NegCache {
    /// Create a negative cache with the given maximum entry count.
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            set: RwLock::new(HashSet::new()),
            max_entries,
            admit: RwLock::new(true),
            stats: RwLock::new(NegCacheStats::default()),
            #[cfg(all(test, feature = "notify", unix))]
            clear_pause_hook: RwLock::new(None),
        }
    }

    #[cfg(all(test, feature = "notify", unix))]
    pub(crate) fn set_clear_pause_hook(&self, hook: Option<crate::posting_cache::ClearPauseHook>) {
        *self
            .clear_pause_hook
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
    }

    /// Set whether new entries are admitted.
    ///
    /// When `false`, `record_negative` returns immediately without caching.
    /// Lookups still work on existing entries.
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
    #[must_use]
    pub fn is_known_negative(&self, query_fingerprint: u64, content_hash: u64) -> bool {
        let set = self
            .set
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let found = set.contains(&(query_fingerprint, content_hash));
        drop(set);
        let mut stats = self
            .stats
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if found {
            stats.hits += 1;
        } else {
            stats.misses += 1;
        }
        found
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn record_negative(&self, query_fingerprint: u64, content_hash: u64) {
        if self.max_entries == 0 {
            return;
        }
        if !*self
            .admit
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return;
        }

        let mut set = self
            .set
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if set.len() >= self.max_entries {
            let to_evict = self
                .max_entries
                .checked_div(10usize)
                .unwrap_or(1usize)
                .max(1);
            let mut remaining = to_evict;
            // NOTE: HashSet iteration order is arbitrary — eviction is NOT
            // true FIFO. Hot entries may be evicted before cold ones.
            set.retain(|_| {
                if remaining > 0 {
                    remaining -= 1;
                    false
                } else {
                    true
                }
            });
            self.stats
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .evictions += u64::try_from(to_evict).unwrap_or(0);
        }
        set.insert((query_fingerprint, content_hash));
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn invalidate_file(&self, content_hash: u64) {
        let mut set = self
            .set
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set.retain(|&(_, ch)| ch != content_hash);
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn invalidate_query(&self, query_fingerprint: u64) {
        let mut set = self
            .set
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set.retain(|&(qf, _)| qf != query_fingerprint);
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn clear(&self) {
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
        let mut set = self
            .set
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set.clear();
    }

    /// Evict a fraction of entries (e.g., 0.5 evicts half).
    /// Used by adaptive cache policy under memory pressure.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    pub fn evict_fraction(&self, fraction: f64) {
        if fraction <= 0.0_f64 || fraction.is_nan() {
            return;
        }
        if fraction >= 1.0_f64 {
            self.clear();
            return;
        }
        let mut set = self
            .set
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let len = set.len();
        let to_remove = fraction_count_floor(len, fraction);
        let mut remaining = to_remove;
        set.retain(|_| {
            if remaining > 0 {
                remaining -= 1;
                false
            } else {
                true
            }
        });
        drop(set);
        self.stats
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .evictions += u64::try_from(to_remove).unwrap_or(0);
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn stats(&self) -> NegCacheStats {
        self.stats
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.set
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }
}

/// Compute a stable fingerprint for a query pattern using XXH64.
#[must_use]
pub fn query_fingerprint(pattern: &str) -> u64 {
    xxh64(pattern.as_bytes(), 0)
}

/// Compute a stable XXH64 fingerprint combining the query pattern with the
/// execution flags (`binary`, `decompress`, `multiline`, `word_boundary`) that
/// alter verification outcomes.
#[must_use]
pub fn query_fingerprint_with_options(
    pattern: &str,
    binary: bool,
    decompress: bool,
    multiline: bool,
    word_boundary: bool,
) -> u64 {
    let mut buf = Vec::with_capacity(pattern.len() + 5);
    buf.extend_from_slice(pattern.as_bytes());
    buf.push(0);
    buf.push(u8::from(binary));
    buf.push(u8::from(decompress));
    buf.push(u8::from(multiline));
    buf.push(u8::from(word_boundary));
    xxh64(&buf, 0)
}

fn fraction_count_floor(total: usize, fraction: f64) -> usize {
    if total == 0 || fraction <= 0.0_f64 || fraction.is_nan() {
        return 0;
    }
    if fraction >= 1.0_f64 {
        return total;
    }
    let bits = fraction.to_bits();
    let exp_bits = u32::try_from((bits >> 52) & 0x7FF).unwrap_or(0);
    if exp_bits == 0 {
        return 0;
    }
    let shift = 1023u32.saturating_sub(exp_bits);
    let denom_shift = 52u32.saturating_add(shift);
    if denom_shift >= 128 {
        return 0;
    }
    let mantissa = u128::from((bits & ((1u64 << 52) - 1)) | (1u64 << 52));
    let total_u128 = u128::try_from(total).unwrap_or(u128::MAX);
    let prod = total_u128.saturating_mul(mantissa);
    let q = prod >> denom_shift;
    usize::try_from(q).unwrap_or(total).min(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_known_negative_hit_and_miss() {
        let cache = NegCache::new(100);
        assert!(!cache.is_known_negative(1, 2));
        let stats = cache.stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 0);

        cache.record_negative(1, 2);
        assert!(cache.is_known_negative(1, 2));
        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn record_negative_adds_entry() {
        let cache = NegCache::new(100);
        assert!(!cache.is_known_negative(10, 20));
        cache.record_negative(10, 20);
        assert!(cache.is_known_negative(10, 20));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn invalidate_file_removes_all_entries_for_hash() {
        let cache = NegCache::new(100);
        cache.record_negative(1, 100);
        cache.record_negative(2, 100);
        cache.record_negative(3, 200);
        assert_eq!(cache.len(), 3);

        cache.invalidate_file(100);
        assert_eq!(cache.len(), 1);
        assert!(!cache.is_known_negative(1, 100));
        assert!(!cache.is_known_negative(2, 100));
        assert!(cache.is_known_negative(3, 200));
    }

    #[test]
    fn eviction_at_max_entries() {
        let cache = NegCache::new(10);
        for i in 0..15u64 {
            cache.record_negative(i, i * 10);
        }
        assert!(cache.len() <= 10);
        assert!(cache.stats().evictions > 0);
    }

    #[test]
    fn clear_empties_cache() {
        let cache = NegCache::new(100);
        cache.record_negative(1, 2);
        cache.record_negative(3, 4);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn invalidate_query_removes_all_entries_for_query() {
        let cache = NegCache::new(100);
        cache.record_negative(42, 1);
        cache.record_negative(42, 2);
        cache.record_negative(99, 1);
        cache.invalidate_query(42);
        assert!(!cache.is_known_negative(42, 1));
        assert!(!cache.is_known_negative(42, 2));
        assert!(cache.is_known_negative(99, 1));
    }

    #[test]
    fn query_fingerprint_is_deterministic() {
        let a = query_fingerprint("hello world");
        let b = query_fingerprint("hello world");
        let c = query_fingerprint("goodbye");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn is_empty_works() {
        let cache = NegCache::new(100);
        assert!(cache.is_empty());
        cache.record_negative(1, 2);
        assert!(!cache.is_empty());
        cache.invalidate_file(2);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_neg_cache_zero_max_entries_is_noop() {
        let cache = NegCache::new(0);
        cache.record_negative(1, 2);
        cache.record_negative(3, 4);
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.stats().evictions, 0);
        assert!(!cache.is_known_negative(1, 2));
    }
}
