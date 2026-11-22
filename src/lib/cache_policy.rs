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
pub struct AdaptiveCachePolicy {
    guard: ResourceGuard,
    ceiling: usize,
}

/// Returns the total system memory in bytes cross-platform.
///
/// Reads `/proc/meminfo` via `llmosafe::ResourceGuard` on Linux, and falls back
/// to POSIX `sysconf` on macOS and other Unix systems. If neither is available,
/// returns a safe default floor of 2 `GiB`.
#[must_use]
pub fn system_memory_bytes() -> usize {
    let sys_mem = ResourceGuard::system_memory_bytes();
    if sys_mem > 0 {
        return sys_mem;
    }
    #[cfg(unix)]
    {
        // SAFETY: sysconf is POSIX and thread-safe for reading system configuration.
        let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if pages > 0 && page_size > 0 {
            let pages_usize = usize::try_from(pages).unwrap_or(0);
            let page_size_usize = usize::try_from(page_size).unwrap_or(0);
            if let Some(total) = pages_usize.checked_mul(page_size_usize) {
                if total > 0 {
                    return total;
                }
            }
        }
    }
    2 * 1024 * 1024 * 1024
}

/// Creates a `ResourceGuard` with a ceiling based on a rational fraction (numerator / denominator) of system memory.
#[must_use]
pub fn resource_guard_fraction(numerator: usize, denominator: usize) -> ResourceGuard {
    let total = system_memory_bytes();
    let den = denominator.max(1);
    let ceiling = total.saturating_mul(numerator) / den;
    ResourceGuard::new(ceiling.max(256 * 1024 * 1024))
}

/// Creates a `ResourceGuard` with a ceiling based on a float fraction of system memory.
///
/// Uses standard rational buckets to avoid float-to-int cast clippy lints while providing
/// accurate ceilings for standard ratios (e.g. 0.6 → 3/5, 0.5 → 1/2).
/// Returns the rational bucket (numerator, denominator) used by
/// [`resource_guard_auto`] for a given float fraction, so callers can derive
/// a ceiling that exactly matches the guard's internal ceiling.
fn resource_guard_bucket(ceiling_fraction: f64) -> (usize, usize) {
    if ceiling_fraction >= 0.75 {
        (4, 5)
    } else if ceiling_fraction >= 0.55 {
        (3, 5)
    } else if ceiling_fraction >= 0.45 {
        (1, 2)
    } else if ceiling_fraction >= 0.3 {
        (1, 3)
    } else if ceiling_fraction >= 0.2 {
        (1, 5)
    } else {
        (1, 10)
    }
}

/// Creates a `ResourceGuard` with a ceiling based on a float fraction of system memory.
///
/// Uses standard rational buckets to avoid float-to-int cast clippy lints while providing
/// accurate ceilings for standard ratios (e.g. 0.6 → 3/5, 0.5 → 1/2).
#[must_use]
pub fn resource_guard_auto(ceiling_fraction: f64) -> ResourceGuard {
    let (n, d) = resource_guard_bucket(ceiling_fraction);
    resource_guard_fraction(n, d)
}

/// Returns the byte ceiling used by [`resource_guard_auto`] for the given fraction.
#[must_use]
pub fn ceiling_bytes_for_fraction(ceiling_fraction: f64) -> usize {
    let (n, d) = resource_guard_bucket(ceiling_fraction);
    let total = system_memory_bytes();
    (total.saturating_mul(n) / d.max(1)).max(256 * 1024 * 1024)
}

/// Computes memory pressure percentage (0..=100) using live process `VmRSS`
/// against `ceiling_bytes` on Linux, falling back to `guard.pressure()` when
/// live RSS is unavailable.
#[must_use]
pub fn live_pressure_or_guard(guard: &ResourceGuard, ceiling_bytes: usize) -> u8 {
    live_pressure_from_rss(guard, ceiling_bytes, live_rss_bytes())
}

fn live_pressure_from_rss(
    guard: &ResourceGuard,
    ceiling_bytes: usize,
    live_rss: Option<u64>,
) -> u8 {
    if ceiling_bytes == 0 {
        return 100;
    }
    if let Some(rss) = live_rss {
        let ceiling_u128 = u128::try_from(ceiling_bytes).unwrap_or(u128::MAX);
        let pct = (u128::from(rss).saturating_mul(100) / ceiling_u128).min(100);
        return u8::try_from(pct).unwrap_or(100);
    }
    guard.pressure()
}

/// Return the current resident set size (live RSS) in bytes when available.
///
/// On Linux, reads `VmRSS:` from `/proc/self/status` and converts from `KiB`
/// to bytes. Unlike `getrusage(RUSAGE_SELF)` `ru_maxrss` (which tracks the
/// monotonic process-lifetime peak RSS), `VmRSS` reflects current resident
/// memory after pages are freed or caches are evicted. Returns `None` on
/// non-Linux targets or if `/proc/self/status` cannot be read so callers can
/// fall back to `ResourceGuard`.
#[must_use]
pub fn live_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        parse_vm_rss_bytes(&status)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_vm_rss_bytes(status: &str) -> Option<u64> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return kb.checked_mul(1024);
        }
    }
    None
}

impl AdaptiveCachePolicy {
    /// Creates a new policy with the given ceiling fraction of system memory.
    ///
    /// For example, `0.6` means use 60 % of system memory as the ceiling.
    /// Internally delegates to [`resource_guard_auto`].
    #[must_use]
    pub fn new(ceiling_fraction: f64) -> Self {
        let guard = resource_guard_auto(ceiling_fraction);
        // Derive the ceiling from the SAME rational bucket the guard uses,
        // so ceiling_bytes() agrees with the guard's internal ceiling (C
        // G-SEM-1). Previously this used the raw float, causing a divergence
        // (e.g. 0.75 → guard 0.8*sysmem but ceiling_bytes() 0.75*sysmem).
        let (n, d) = resource_guard_bucket(ceiling_fraction);
        let ceiling = (system_memory_bytes().saturating_mul(n) / d.max(1)).max(256 * 1024 * 1024);
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
    /// Internally invokes `ResourceGuard::pressure()`, which reads
    /// `getrusage(RUSAGE_SELF)` (`ru_maxrss`, monotonic process high-water mark)
    /// synchronously without sleeping (unlike `ResourceGuard::check()`, which
    /// samples `/proc/stat` across a 100 ms interval).
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
        self.directive_for_pressure(self.guard.pressure())
    }

    /// Returns a [`CacheDirective`] using a pre-computed pressure value.
    ///
    /// This is the non-blocking variant — use it when pressure has already been
    /// read by the caller (e.g. daemon's `handle_changes` deduplicates the
    /// `guard.pressure()` call across `evaluate_safety` and cache policy).
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
    pub const fn directive_for_pressure(&self, pressure: u8) -> CacheDirective {
        let zone = PressureZone::from_pressure(pressure);
        let (evict_fraction, allow_new_entries, allow_mmap_pin) = zone.fields();
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
    /// Reads process peak RSS via `getrusage(RUSAGE_SELF)` synchronously without
    /// sleeping; see [`directive`](Self::directive).
    #[must_use]
    pub fn pressure(&self) -> u8 {
        self.guard.pressure()
    }

    /// Return the current memory pressure percentage (0..=100), preferring live
    /// process `VmRSS` over monotonic peak `ru_maxrss` when available.
    #[must_use]
    pub fn live_pressure(&self) -> u8 {
        live_pressure_or_guard(&self.guard, self.ceiling)
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

    /// Returns the cache directive fields for this zone.
    ///
    /// | Zone   | `evict_fraction` | `allow_new_entries` | `allow_mmap_pin` |
    /// |--------|-------------------|---------------------|------------------|
    /// | Green  | 0.0               | true                | true             |
    /// | Yellow | 0.1               | true                | true             |
    /// | Orange | 0.5               | false               | false            |
    /// | Red    | 1.0               | false               | false            |
    #[must_use]
    pub const fn fields(&self) -> (f64, bool, bool) {
        match self {
            Self::Green => (0.0_f64, true, true),
            Self::Yellow => (0.1_f64, true, true),
            Self::Orange => (0.5_f64, false, false),
            Self::Red => (1.0_f64, false, false),
        }
    }
}

#[cfg(test)]
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
            let (evict_fraction, allow_new_entries, allow_mmap_pin) = zone.fields();
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

        // 0.75 buckets to 4/5 (matching resource_guard_auto), so the
        // ceiling must agree with the guard's internal ceiling (G-SEM-1 fix).
        let expected_75 = sys_mem.saturating_mul(4) / 5;
        let policy_75 = AdaptiveCachePolicy::new(0.75_f64);
        assert_eq!(
            policy_75.ceiling_bytes(),
            expected_75,
            "ceiling_bytes with 0.75 fraction (bucketed to 4/5 to match guard)"
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

    #[test]
    fn directive_for_pressure_all_zones() {
        let guard = ResourceGuard::for_testing(1024 * 1024, 0, 25);
        let policy = AdaptiveCachePolicy::new_with_guard(guard, 1024 * 1024);

        let cases = [
            (0u8, PressureZone::Green, 0.0_f64, true, true),
            (40, PressureZone::Green, 0.0, true, true),
            (41, PressureZone::Yellow, 0.1, true, true),
            (70, PressureZone::Yellow, 0.1, true, true),
            (71, PressureZone::Orange, 0.5, false, false),
            (90, PressureZone::Orange, 0.5, false, false),
            (91, PressureZone::Red, 1.0, false, false),
            (100, PressureZone::Red, 1.0, false, false),
        ];

        for (pressure, expected_zone, expected_evict, expected_admit, expected_pin) in cases {
            let directive = policy.directive_for_pressure(pressure);
            assert_eq!(
                directive.pressure, pressure,
                "pressure {pressure}: should be passed through"
            );
            assert_eq!(
                directive.zone, expected_zone,
                "pressure {pressure}: expected zone {expected_zone:?}, got {:?}",
                directive.zone
            );
            assert!(
                (directive.evict_fraction - expected_evict).abs() < f64::EPSILON,
                "pressure {pressure}: evict_fraction mismatch"
            );
            assert_eq!(
                directive.allow_new_entries, expected_admit,
                "pressure {pressure}: allow_new_entries mismatch"
            );
            assert_eq!(
                directive.allow_mmap_pin, expected_pin,
                "pressure {pressure}: allow_mmap_pin mismatch"
            );
        }
    }

    #[test]
    fn test_parse_vm_rss_bytes_and_live_rss() {
        let sample = "Name:\tix\nVmPeak:\t   10240 kB\nVmRSS:\t    4096 kB\n";
        assert_eq!(parse_vm_rss_bytes(sample), Some(4096 * 1024));
        assert_eq!(parse_vm_rss_bytes("Name:\tix\n"), None);

        #[cfg(target_os = "linux")]
        assert!(live_rss_bytes().is_some_and(|rss| rss > 0));
    }

    #[test]
    fn live_pressure_transitions_and_vm_rss_divergence_recovery() {
        use llmosafe::llmosafe_integration::EscalationReason;
        use llmosafe::{DesignAssuranceLevel, EscalationPolicy, PressureLevel, SafetyDecision};

        let escalation_policy = EscalationPolicy::default().with_dal(DesignAssuranceLevel::A);

        // Ceiling 0 fails closed to 100%.
        let guard_zero = ResourceGuard::for_testing(1024 * 1024, 0, 0);
        assert_eq!(live_pressure_or_guard(&guard_zero, 0), 100);
        let policy_zero = AdaptiveCachePolicy::new_with_guard(guard_zero, 0);
        assert_eq!(policy_zero.live_pressure(), 100);

        // Divergence recovery: simulated ru_maxrss >= 100% (guard.pressure() == 100)
        // must still recover to Green / Proceed when live VmRSS is tiny relative to ceiling.
        #[cfg(target_os = "linux")]
        {
            let guard_stuck_peak = ResourceGuard::for_testing(usize::MAX, 0, 100);
            assert_eq!(guard_stuck_peak.pressure(), 100);
            let recovered_pct = live_pressure_or_guard(&guard_stuck_peak, usize::MAX);
            assert!(
                recovered_pct <= 5,
                "live VmRSS against usize::MAX ceiling should recover to Green, got {recovered_pct}"
            );
            let recovered_policy =
                AdaptiveCachePolicy::new_with_guard(guard_stuck_peak, usize::MAX);
            assert_eq!(recovered_policy.live_pressure(), recovered_pct);
            assert_eq!(
                recovered_policy.directive_for_pressure(recovered_pct).zone,
                PressureZone::Green
            );
            assert_eq!(
                escalation_policy.decide_with_pressure(
                    0,
                    0,
                    false,
                    PressureLevel::from(recovered_pct)
                ),
                SafetyDecision::Proceed
            );

            // Live VmRSS exceeding tiny ceiling drives live_pressure to 100% Red / Escalate
            // even when guard.pressure() is overridden to 0.
            let guard_low = ResourceGuard::for_testing(1024, 0, 0);
            let saturated_pct = live_pressure_or_guard(&guard_low, 1024);
            assert_eq!(saturated_pct, 100);
            let saturated_policy = AdaptiveCachePolicy::new_with_guard(guard_low, 1024);
            assert_eq!(saturated_policy.live_pressure(), 100);
            let red_dir = saturated_policy.directive_for_pressure(saturated_pct);
            assert_eq!(red_dir.zone, PressureZone::Red);
            assert!(!red_dir.allow_new_entries);
            assert!(matches!(
                escalation_policy.decide_with_pressure(
                    0,
                    0,
                    false,
                    PressureLevel::from(saturated_pct)
                ),
                SafetyDecision::Escalate {
                    reason: EscalationReason::ResourcePressure,
                    ..
                }
            ));
        }

        // Exact transition ladder via live_pressure_from_rss (including None fallback).
        let guard_fallback = ResourceGuard::for_testing(1000, 0, 82);
        assert_eq!(live_pressure_from_rss(&guard_fallback, 1000, None), 82);

        let guard_high = ResourceGuard::for_testing(1000, 0, 100);
        let policy = AdaptiveCachePolicy::new_with_guard(guard_high.clone(), 1000);
        let ladder = [
            (250u64, 25u8, PressureZone::Green, false),
            (500u64, 50u8, PressureZone::Yellow, false),
            (600u64, 60u8, PressureZone::Yellow, true),
            (800u64, 80u8, PressureZone::Orange, true),
            (1200u64, 100u8, PressureZone::Red, true),
        ];
        for (rss, expected_pct, expected_zone, expect_escalate) in ladder {
            let pct = live_pressure_from_rss(&guard_high, 1000, Some(rss));
            assert_eq!(pct, expected_pct);
            let dir = policy.directive_for_pressure(pct);
            assert_eq!(dir.zone, expected_zone);
            let decision =
                escalation_policy.decide_with_pressure(0, 0, false, PressureLevel::from(pct));
            if expect_escalate {
                assert!(
                    matches!(
                        decision,
                        SafetyDecision::Escalate {
                            reason: EscalationReason::ResourcePressure,
                            ..
                        }
                    ),
                    "pct={pct} should escalate, got {decision:?}"
                );
            } else {
                assert_eq!(decision, SafetyDecision::Proceed);
            }
        }
    }
}
