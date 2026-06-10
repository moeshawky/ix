//! Compiled regex pool — caches `Regex` objects to avoid recompilation.
//!
//! Thread-safe, size-bounded FIFO cache keyed by pattern string.

use regex::Regex;
use std::collections::HashMap;
use std::sync::RwLock;

/// Hit/miss counters for a [`RegexPool`].
///
/// Public for external monitoring via [`RegexPool::stats`].
#[derive(Debug, Default, Clone)]
pub struct PoolStats {
    /// Number of pool hits (pattern already compiled).
    pub hits: u64,
    /// Number of pool misses (pattern compiled on demand).
    pub misses: u64,
}

/// Thread-safe, size-bounded FIFO cache for compiled [`Regex`] objects.
///
/// Most useful in daemon mode where the same patterns are queried repeatedly.
/// Set `max_entries` to 0 for unlimited capacity.
pub struct RegexPool {
    pool: RwLock<HashMap<String, Regex>>,
    order: RwLock<Vec<String>>,
    max_entries: usize,
    stats: RwLock<PoolStats>,
}

impl std::fmt::Debug for RegexPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegexPool")
            .field("max_entries", &self.max_entries)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl RegexPool {
    /// Create a pool with the given maximum entry count (0 = unlimited).
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            pool: RwLock::new(HashMap::new()),
            order: RwLock::new(Vec::new()),
            max_entries,
            stats: RwLock::new(PoolStats::default()),
        }
    }

    /// Returns a compiled `Regex` for the given pattern, using the cached copy if available.
    ///
    /// # Errors
    ///
    /// Returns `regex::Error` if the pattern fails to compile.
    pub fn get_or_compile(&self, pattern: &str) -> Result<Regex, regex::Error> {
        {
            let pool = self
                .pool
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(re) = pool.get(pattern) {
                let mut stats = self
                    .stats
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                stats.hits += 1;
                drop(stats);
                return Ok(re.clone());
            }
        }

        let re = Regex::new(pattern)?;

        {
            let mut pool = self
                .pool
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

            if self.max_entries > 0 {
                while pool.len() >= self.max_entries {
                    if let Some(evict_key) = order.first().cloned() {
                        pool.remove(&evict_key);
                        order.remove(0);
                    } else {
                        break;
                    }
                }
            }

            pool.insert(pattern.to_owned(), re.clone());
            order.push(pattern.to_owned());
            stats.misses += 1;
            drop(stats);
            drop(order);
            drop(pool);
        }

        Ok(re)
    }

    /// Removes a specific pattern from the pool.
    pub fn invalidate(&self, pattern: &str) {
        let mut pool = self
            .pool
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut order = self
            .order
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pool.remove(pattern);
        order.retain(|k| k != pattern);
        drop(order);
        drop(pool);
    }

    /// Clears all entries from the pool.
    pub fn clear(&self) {
        let mut pool = self
            .pool
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut order = self
            .order
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pool.clear();
        order.clear();
        drop(order);
        drop(pool);
    }

    /// Returns a snapshot of the pool hit/miss statistics.
    #[must_use]
    pub fn stats(&self) -> PoolStats {
        self.stats
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Returns the number of cached patterns.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pool
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Returns `true` if the pool contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::RegexPool;

    #[test]
    fn get_or_compile_hit_and_miss() -> Result<(), Box<dyn std::error::Error>> {
        let pool = RegexPool::new(10);
        let re = pool.get_or_compile(r"\d+")?;
        assert!(re.is_match("123"));

        let re2 = pool.get_or_compile(r"\d+")?;
        assert!(re2.is_match("456"));

        let stats = pool.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        Ok(())
    }

    #[test]
    fn eviction_at_max_entries() -> Result<(), Box<dyn std::error::Error>> {
        let pool = RegexPool::new(2);

        pool.get_or_compile("a")?;
        assert_eq!(pool.len(), 1);

        pool.get_or_compile("b")?;
        assert_eq!(pool.len(), 2);

        pool.get_or_compile("c")?;
        assert_eq!(pool.len(), 2);

        let stats = pool.stats();
        assert_eq!(stats.misses, 3);
        assert_eq!(stats.hits, 0);

        let pool_read = pool
            .pool
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(pool_read.contains_key("b"));
        assert!(pool_read.contains_key("c"));
        assert!(!pool_read.contains_key("a"));
        Ok(())
    }

    #[test]
    fn clear_empties_pool() -> Result<(), Box<dyn std::error::Error>> {
        let pool = RegexPool::new(10);
        pool.get_or_compile("x")?;
        pool.get_or_compile("y")?;
        assert_eq!(pool.len(), 2);

        pool.clear();
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let stats = pool.stats();
        assert_eq!(stats.misses, 2);
        Ok(())
    }

    #[test]
    fn invalidate_removes_specific_entry() -> Result<(), Box<dyn std::error::Error>> {
        let pool = RegexPool::new(10);
        pool.get_or_compile("alpha")?;
        pool.get_or_compile("beta")?;
        assert_eq!(pool.len(), 2);

        pool.invalidate("alpha");
        assert_eq!(pool.len(), 1);

        pool.get_or_compile("alpha")?;
        let stats = pool.stats();
        assert_eq!(stats.misses, 3);
        Ok(())
    }

    #[test]
    fn unlimited_capacity() -> Result<(), Box<dyn std::error::Error>> {
        let pool = RegexPool::new(0);
        for i in 0_u32..50_u32 {
            pool.get_or_compile(&format!("p{i}"))?;
        }
        assert_eq!(pool.len(), 50);
        Ok(())
    }

    #[test]
    fn invalid_pattern_returns_error() {
        let pool = RegexPool::new(10);
        assert!(pool.get_or_compile(r"[invalid").is_err());
    }
}
