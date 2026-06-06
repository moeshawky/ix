//! Query planner — transforms user input into an optimal index query plan.
//!
//! Decomposes regex patterns into required trigram sets.
//! When a [`crate::regex_pool::RegexPool`] is available, compiled
//! regexes are sourced from the pool to avoid redundant compilation.

use crate::regex_pool::RegexPool;
use crate::trigram::{Extractor, Trigram};
use regex::Regex;
use regex_syntax::hir::{Hir, HirKind};

/// Optimal query execution strategy chosen by the planner.
#[derive(Debug)]
pub enum QueryPlan {
    /// Fast path: literal string search with pre-computed trigrams.
    Literal {
        /// Raw search bytes to match against.
        pattern: Vec<u8>,
        /// Set of trigrams extracted from the pattern.
        trigrams: Vec<Trigram>,
        /// Pre-compiled regex equivalent to the literal.
        regex: Regex,
    },

    /// Regex with extractable literal sub-strings.
    RegexWithLiterals {
        /// Pre-compiled user regex.
        regex: Regex,
        /// Per-literal required trigram sets (AND across sets).
        required_trigram_sets: Vec<Vec<Trigram>>,
    },

    /// Case-insensitive indexed search. Each group = case variants for one
    /// trigram position. Executor UNIONs within groups, INTERSECTs across.
    CaseInsensitive {
        /// Pre-compiled case-insensitive regex.
        regex: Regex,
        /// Per-position trigram groups (union within, intersect across).
        trigram_groups: Vec<Vec<Trigram>>,
    },

    /// No literals extractable — full scan fallback.
    FullScan {
        /// Pre-compiled regex to run against every file.
        regex: Regex,
    },
}

impl QueryPlan {
    /// Return the regex pattern string for this plan.
    ///
    /// Used for cache keying (e.g. negative-result cache fingerprint).
    #[must_use]
    pub fn pattern_str(&self) -> &str {
        match self {
            Self::Literal { regex, .. }
            | Self::RegexWithLiterals { regex, .. }
            | Self::CaseInsensitive { regex, .. }
            | Self::FullScan { regex } => regex.as_str(),
        }
    }
}

/// Query planning options.
#[derive(Debug, Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct QueryOptions {
    /// Treat pattern as a regex rather than a literal string.
    pub is_regex: bool,
    /// Case-insensitive matching.
    pub ignore_case: bool,
    /// Enable multiline mode (dot matches newline). Requires `is_regex`.
    pub multiline: bool,
    /// Match only at word boundaries (wraps pattern in `\b...\b`).
    /// Implies `is_regex` internally but enforces whole-word semantics.
    pub word_boundary: bool,
}

/// Stateless query planner that decomposes user input into a [`QueryPlan`].
pub struct Planner;

impl Planner {
    /// Plan a literal or regex query with default options (non-unicode,
    /// case-sensitive). See [`plan_with_options`](Self::plan_with_options)
    /// for full control.
    #[must_use]
    pub fn plan(pattern: &str, is_regex: bool) -> QueryPlan {
        Self::plan_with_options(
            pattern,
            QueryOptions {
                is_regex,
                ..Default::default()
            },
        )
    }

    /// Plan a query using a regex pool to avoid redundant compilation.
    ///
    /// Identical to [`plan_with_options`](Self::plan_with_options) but
    /// sources compiled regexes from `pool` when available.
    #[must_use]
    pub fn plan_with_pool(pattern: &str, options: QueryOptions, pool: &RegexPool) -> QueryPlan {
        Self::plan_impl(pattern, options, Some(pool))
    }

    /// Plan a query with full options.
    ///
    /// On regex compilation failure, falls back to `^$` (matches nothing)
    /// rather than panicking. Library code must never panic per AGENTS.md.
    #[must_use]
    pub fn plan_with_options(pattern: &str, options: QueryOptions) -> QueryPlan {
        Self::plan_impl(pattern, options, None)
    }

    /// Compile a regex, preferring the pool if available. On failure,
    /// logs a warning and returns `"^$"` (matches nothing) instead of
    /// panicking. Library code must never call expect/panic per AGENTS.md.
    fn compile_regex(pat: &str, pool: Option<&RegexPool>) -> Regex {
        if let Some(p) = pool
            && let Ok(re) = p.get_or_compile(pat)
        {
            return re;
        }
        Regex::new(pat).unwrap_or_else(|_| {
            tracing::warn!(
                "ix: regex compilation failed for '{}', using '^$' fallback",
                pat
            );
            // ^$ matches end-of-line only — effectively matches nothing useful.
            // This is safer than "" which matches EVERY line.
            #[allow(clippy::trivial_regex)]
            match Regex::new("^$") {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("regex crate cannot compile '^$': {e}. Using '.' as fallback.");
                    Regex::new(".").unwrap_or_else(|e2| {
                        tracing::error!("regex crate fatal error: {e2}. Returning empty pattern.");
                        // The regex crate is fundamentally broken.
                        // Return an empty pattern rather than panicking.
                        #[allow(clippy::trivial_regex)]
                        Regex::new("").unwrap_or_else(|e3| {
                            tracing::error!("regex crate is completely non-functional: {e3}");
                            std::process::abort()
                        })
                    })
                }
            }
        })
    }

    fn plan_impl(pattern: &str, options: QueryOptions, pool: Option<&RegexPool>) -> QueryPlan {
        let mut final_pattern = pattern.to_string();
        let mut use_regex = options.is_regex;
        if options.multiline && use_regex {
            final_pattern = format!("(?s){final_pattern}");
        }

        // Word-boundary mode: wrap literal in \b word boundaries.
        // When word_boundary=true, we use regex mode internally but the semantics
        // are "whole word match" rather than arbitrary regex.
        if options.word_boundary && !options.is_regex {
            if options.ignore_case {
                final_pattern = format!("(?i)\\b{}\\b", regex::escape(&final_pattern));
            } else {
                final_pattern = format!("\\b{}\\b", regex::escape(&final_pattern));
            }
            use_regex = true;
        }

        if !use_regex && !options.ignore_case {
            let bytes = final_pattern.as_bytes().to_vec();
            let trigrams = Extractor::extract_set(&bytes);

            let escaped = regex::escape(&final_pattern);
            let regex = Self::compile_regex(&escaped, pool);

            if trigrams.is_empty() {
                return QueryPlan::FullScan { regex };
            }

            return QueryPlan::Literal {
                pattern: bytes,
                trigrams,
                regex,
            };
        }

        // Case-insensitive literal: per-position trigram groups.
        // Executor UNIONs within each group, INTERSECTs across groups.
        if !use_regex && options.ignore_case {
            let bytes = final_pattern.as_bytes();
            let groups = Extractor::extract_groups_case_insensitive(bytes);
            let regex_pat = format!("(?i){}", regex::escape(&final_pattern));
            let regex = Self::compile_regex(&regex_pat, pool);

            if groups.is_empty() {
                return QueryPlan::FullScan { regex };
            }

            return QueryPlan::CaseInsensitive {
                regex,
                trigram_groups: groups,
            };
        }

        let regex_pat = if options.ignore_case && !final_pattern.starts_with("(?i)") {
            format!("(?i){final_pattern}")
        } else {
            final_pattern.clone()
        };

        let regex = Self::compile_regex(&regex_pat, pool);

        let Ok(hir) = regex_syntax::parse(&final_pattern) else {
            return QueryPlan::FullScan { regex };
        };

        let mut literals = Vec::new();
        Self::walk_hir(&hir, &mut literals);

        // For case-insensitive regex, fall back to FullScan — the (?i) regex
        // handles matching, and extracting trigram groups from regex literals
        // adds complexity without much narrowing benefit.
        if options.ignore_case {
            return QueryPlan::FullScan { regex };
        }

        let required_trigram_sets: Vec<Vec<Trigram>> = literals
            .iter()
            .map(|lit| Extractor::extract_set(lit))
            .filter(|t| !t.is_empty())
            .collect();

        if required_trigram_sets.is_empty() {
            QueryPlan::FullScan { regex }
        } else {
            QueryPlan::RegexWithLiterals {
                regex,
                required_trigram_sets,
            }
        }
    }

    fn walk_hir(hir: &Hir, out: &mut Vec<Vec<u8>>) {
        match hir.kind() {
            HirKind::Literal(lit) => {
                out.push(lit.0.to_vec());
            }
            HirKind::Concat(children) => {
                let mut current = Vec::new();
                for child in children {
                    if let HirKind::Literal(lit) = child.kind() {
                        current.extend_from_slice(&lit.0);
                    } else {
                        if current.len() >= 3 {
                            out.push(current.clone());
                        }
                        current.clear();
                        Self::walk_hir(child, out);
                    }
                }
                if current.len() >= 3 {
                    out.push(current);
                }
            }
            HirKind::Repetition(rep) if rep.min >= 1 => {
                Self::walk_hir(&rep.sub, out);
            }
            // Simplified: we don't extract from Alternation for now as per DESIGN.md
            _ => {}
        }
    }
}
