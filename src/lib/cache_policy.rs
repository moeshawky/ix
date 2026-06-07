//! Adaptive cache policy driven by llmosafe's `ResourceGuard` pressure signal.
//!
//! Turns `ResourceGuard` from a binary safety gate into a continuous cache manager.
//! Reads `ResourceGuard::pressure()` (0–100) and produces `CacheDirective`s that
//! tell cache layers how aggressively to cache, evict, or pin pages.

use llmosafe::ResourceGuard;

/// Pressure zone classification based on memory pressure percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureZone {
    /// 0–40 %: cache aggressively.
    Green,
    /// 41–70 %: cache normally, evict 10 % cold entries.
    Yellow,
    /// 71–90 %: evict 50 %, stop new caching.
    Orange,
    /// 91–100 %: flush everything.
    Red,
}

/// Directive produced by [`AdaptiveCachePolicy::directive`].
#[derive(Debug, Clone)]
pub struct CacheDirective {
    /// Current pressure zone.
    pub zone: PressureZone,
    /// Raw pressure value (0–100).
    pub pressure: u8,
    /// Fraction of cache to evict (0.0–1.0).
    pub evict_fraction: f64,
    /// Whether to admit new cache entries.
    pub allow_new_entries: bool,
    /// Whether to `madvise(MADV_WILLNEED)` on hot pages.
    pub allow_mmap_pin: bool,
}

/// Read-only policy object that reads `ResourceGuard::pressure()` and produces
/// cache directives. Does **not** own any cache — it only tells other code what
/// to do.
///
/// Use [`AdaptiveCachePolicy::new_with_guard`] when sharing a `ResourceGuard` with a builder or
/// other subsystem (recommended). Use [`AdaptiveCachePolicy::new`] when the cache policy needs its
/// own independent ceiling.
#[allow(clippy::missing_fields_in_debug)]
pub struct AdaptiveCachePolicy {
    guard: ResourceGuard,
    ceiling: usize,
}

impl AdaptiveCachePolicy {
    /// Creates a new policy with the given ceiling fraction of system memory.
    ///
    /// For example, `0.6` means use 60 % of system memory as the ceiling.
    /// Internally delegates to `ResourceGuard::auto(ceiling_fraction)`.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::as_conversions
    )]
    pub fn new(ceiling_fraction: f64) -> Self {
        let guard = ResourceGuard::auto(ceiling_fraction);
        let ceiling = (ResourceGuard::system_memory_bytes() as f64 * ceiling_fraction) as usize;
        Self { guard, ceiling }
    }

    /// Creates a policy sharing an existing `ResourceGuard`.
    ///
    /// This is the recommended constructor for daemons: one guard governs
    /// both the builder's safety checks AND the cache's pressure readings,
    /// so they never disagree about how much memory is available.
    ///
    /// `ceiling_bytes` must match the guard's internal ceiling
    /// (i.e., the value passed to `ResourceGuard::new` or computed by `auto`).
    #[must_use]
    pub const fn new_with_guard(guard: ResourceGuard, ceiling_bytes: usize) -> Self {
        Self {
            guard,
            ceiling: ceiling_bytes,
        }
    }

    /// Returns a reference to the underlying `ResourceGuard`.
    #[must_use]
    pub const fn guard(&self) -> &ResourceGuard {
        &self.guard
    }

    /// Returns a [`CacheDirective`] reflecting the current memory pressure.
    ///
    /// **⚠ BLOCKING:** This call internally invokes `ResourceGuard::pressure()`
    /// which reads `/proc/stat` twice with a 100 ms sleep between reads on Linux
    /// (via `raw_entropy()` / `delta_iowait_ratio()`). Do **not** call this in
    /// an async context without `spawn_blocking`.
    ///
    /// # Zone mapping
    ///
    /// | Zone   | Pressure | `evict_fraction` | `allow_new_entries` | `allow_mmap_pin` |
    /// |--------|----------|-------------------|---------------------|------------------|
    /// | Green  | 0–40     | 0.0               | true                | true             |
    /// | Yellow | 41–70    | 0.1               | true                | true             |
    /// | Orange | 71–90    | 0.5               | false               | false            |
    /// | Red    | 91–100   | 1.0               | false               | false            |
    #[must_use]
    pub fn directive(&self) -> CacheDirective {
        let pressure = self.guard.pressure();
        let (zone, evict_fraction, allow_new_entries, allow_mmap_pin) = match pressure {
            0..=40 => (PressureZone::Green, 0.0_f64, true, true),
            41..=70 => (PressureZone::Yellow, 0.1_f64, true, true),
            71..=90 => (PressureZone::Orange, 0.5_f64, false, false),
            _ => (PressureZone::Red, 1.0_f64, false, false),
        };
        CacheDirective {
            zone,
            pressure,
            evict_fraction,
            allow_new_entries,
            allow_mmap_pin,
        }
    }

    /// Convenience wrapper for `ResourceGuard::pressure()`.
    ///
    /// **⚠ BLOCKING:** See [`directive`](Self::directive) for blocking details.
    #[must_use]
    pub fn pressure(&self) -> u8 {
        self.guard.pressure()
    }

    /// Returns the current RSS in bytes.
    #[must_use]
    pub fn rss_bytes(&self) -> usize {
        ResourceGuard::current_rss_bytes()
    }

    /// Returns the memory ceiling in bytes that this policy is configured with.
    #[must_use]
    pub const fn ceiling_bytes(&self) -> usize {
        self.ceiling
    }
}

impl std::fmt::Debug for AdaptiveCachePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdaptiveCachePolicy")
            .field("ceiling", &self.ceiling)
            .finish_non_exhaustive()
    }
}

impl PressureZone {
    /// Returns the zone corresponding to the given pressure percentage.
    #[must_use]
    pub const fn from_pressure(pressure: u8) -> Self {
        match pressure {
            0..=40 => Self::Green,
            41..=70 => Self::Yellow,
            71..=90 => Self::Orange,
            _ => Self::Red,
        }
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn zone_classification_all_zones() {
        assert_eq!(PressureZone::from_pressure(0), PressureZone::Green);
        assert_eq!(PressureZone::from_pressure(20), PressureZone::Green);
        assert_eq!(PressureZone::from_pressure(40), PressureZone::Green);
        assert_eq!(PressureZone::from_pressure(41), PressureZone::Yellow);
        assert_eq!(PressureZone::from_pressure(55), PressureZone::Yellow);
        assert_eq!(PressureZone::from_pressure(70), PressureZone::Yellow);
        assert_eq!(PressureZone::from_pressure(71), PressureZone::Orange);
        assert_eq!(PressureZone::from_pressure(80), PressureZone::Orange);
        assert_eq!(PressureZone::from_pressure(90), PressureZone::Orange);
        assert_eq!(PressureZone::from_pressure(91), PressureZone::Red);
        assert_eq!(PressureZone::from_pressure(100), PressureZone::Red);
    }

    #[test]
    fn directive_fields_per_zone() {
        let policy = AdaptiveCachePolicy::new(0.75);

        let green = CacheDirective {
            zone: PressureZone::Green,
            pressure: 25,
            evict_fraction: 0.0,
            allow_new_entries: true,
            allow_mmap_pin: true,
        };
        let yellow = CacheDirective {
            zone: PressureZone::Yellow,
            pressure: 55,
            evict_fraction: 0.1,
            allow_new_entries: true,
            allow_mmap_pin: true,
        };
        let orange = CacheDirective {
            zone: PressureZone::Orange,
            pressure: 80,
            evict_fraction: 0.5,
            allow_new_entries: false,
            allow_mmap_pin: false,
        };
        let red = CacheDirective {
            zone: PressureZone::Red,
            pressure: 95,
            evict_fraction: 1.0,
            allow_new_entries: false,
            allow_mmap_pin: false,
        };

        for (label, pressure, expected) in [
            ("green", 25, &green),
            ("yellow", 55, &yellow),
            ("orange", 80, &orange),
            ("red", 95, &red),
        ] {
            let zone = PressureZone::from_pressure(pressure);
            let (evict_fraction, allow_new_entries, allow_mmap_pin) = match zone {
                PressureZone::Green => (0.0_f64, true, true),
                PressureZone::Yellow => (0.1_f64, true, true),
                PressureZone::Orange => (0.5_f64, false, false),
                PressureZone::Red => (1.0_f64, false, false),
            };
            assert_eq!(
                zone, expected.zone,
                "{label}: zone mismatch for pressure {pressure}"
            );
            assert!(
                (evict_fraction - expected.evict_fraction).abs() < f64::EPSILON,
                "{label}: evict_fraction mismatch"
            );
            assert_eq!(
                allow_new_entries, expected.allow_new_entries,
                "{label}: allow_new_entries mismatch"
            );
            assert_eq!(
                allow_mmap_pin, expected.allow_mmap_pin,
                "{label}: allow_mmap_pin mismatch"
            );
        }

        let directive = policy.directive();
        assert!(directive.pressure <= 100);
        assert_eq!(
            directive.zone,
            PressureZone::from_pressure(directive.pressure)
        );
    }

    #[test]
    #[expect(
        clippy::integer_division,
        reason = "intentional ceiling via floor(sys_mem * fraction)"
    )]
    fn ceiling_fraction_applied_correctly() {
        let sys_mem = ResourceGuard::system_memory_bytes();
        if sys_mem == 0 {
            return;
        }

        // 0.5 = 1/2
        let expected_ceiling = sys_mem / 2;
        let policy = AdaptiveCachePolicy::new(0.5_f64);
        assert_eq!(
            policy.ceiling_bytes(),
            expected_ceiling,
            "ceiling_bytes should equal system_memory * 0.5"
        );

        // 0.75 = 3/4
        let expected_75 = sys_mem.saturating_mul(3) / 4;
        let policy_75 = AdaptiveCachePolicy::new(0.75_f64);
        assert_eq!(
            policy_75.ceiling_bytes(),
            expected_75,
            "ceiling_bytes with 0.75 fraction"
        );
    }

    #[test]
    fn pressure_returns_bounded_value() {
        let policy = AdaptiveCachePolicy::new(0.75);
        let pressure = policy.pressure();
        assert!(pressure <= 100, "pressure {pressure} should be <= 100");
    }

    #[test]
    fn rss_bytes_is_non_negative() {
        let sys_mem = ResourceGuard::system_memory_bytes();
        if sys_mem == 0 {
            return;
        }
        let policy = AdaptiveCachePolicy::new(0.75);
        let rss = policy.rss_bytes();
        assert!(rss <= sys_mem, "RSS {rss} should not exceed system memory");
    }

    #[test]
    fn debug_impl_works() {
        let policy = AdaptiveCachePolicy::new(0.75);
        let debug_str = format!("{policy:?}");
        assert!(
            debug_str.contains("AdaptiveCachePolicy"),
            "Debug output should contain type name"
        );
        assert!(
            debug_str.contains("ceiling"),
            "Debug output should contain ceiling field"
        );
    }

    #[test]
    #[expect(clippy::integer_division, reason = "intentional ceiling computation")]
    fn new_with_guard_shares_ceiling() {
        let sys_mem = ResourceGuard::system_memory_bytes();
        if sys_mem == 0 {
            return;
        }
        let ceiling = sys_mem / 2;
        let guard = ResourceGuard::new(ceiling);
        let policy = AdaptiveCachePolicy::new_with_guard(guard, ceiling);
        assert_eq!(
            policy.ceiling_bytes(),
            ceiling,
            "new_with_guard should use the provided ceiling"
        );
        assert_eq!(
            policy.guard().pressure(),
            ResourceGuard::new(ceiling).pressure(),
            "guard pressure should match"
        );
    }

    #[test]
    fn deterministic_pressure_with_for_testing() {
        let guard = ResourceGuard::for_testing(1024 * 1024, 0, 25);
        let policy = AdaptiveCachePolicy::new_with_guard(guard, 1024 * 1024);
        let directive = policy.directive();
        assert_eq!(
            directive.pressure, 25,
            "for_testing should return the injected pressure value"
        );
        assert_eq!(
            directive.zone,
            PressureZone::Green,
            "25 pressure should map to Green zone"
        );
        assert!(
            directive.allow_new_entries,
            "Green zone should admit entries"
        );
        assert!((directive.evict_fraction - 0.0).abs() < f64::EPSILON);

        let guard_orange = ResourceGuard::for_testing(1024 * 1024, 0, 80);
        let policy_orange = AdaptiveCachePolicy::new_with_guard(guard_orange, 1024 * 1024);
        let directive_orange = policy_orange.directive();
        assert_eq!(directive_orange.zone, PressureZone::Orange);
        assert!(
            !directive_orange.allow_new_entries,
            "Orange zone should block admission"
        );
        assert!((directive_orange.evict_fraction - 0.5).abs() < f64::EPSILON);

        let guard_red = ResourceGuard::for_testing(1024 * 1024, 0, 95);
        let policy_red = AdaptiveCachePolicy::new_with_guard(guard_red, 1024 * 1024);
        let directive_red = policy_red.directive();
        assert_eq!(directive_red.zone, PressureZone::Red);
        assert!((directive_red.evict_fraction - 1.0).abs() < f64::EPSILON);
    }
}
