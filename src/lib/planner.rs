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
    ///
    /// # Errors
    ///
    /// Returns an error if regex compilation fails.
    pub fn plan(pattern: &str, is_regex: bool) -> crate::error::Result<QueryPlan> {
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
    ///
    /// # Errors
    ///
    /// Returns an error if regex compilation fails.
    pub fn plan_with_pool(
        pattern: &str,
        options: QueryOptions,
        pool: &RegexPool,
    ) -> crate::error::Result<QueryPlan> {
        Self::plan_impl(pattern, options, Some(pool))
    }

    /// Plan a query with full options.
    ///
    /// # Errors
    ///
    /// Returns an error if regex compilation fails.
    pub fn plan_with_options(
        pattern: &str,
        options: QueryOptions,
    ) -> crate::error::Result<QueryPlan> {
        Self::plan_impl(pattern, options, None)
    }

    /// Compile a regex, preferring the pool if available.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::Regex`] if the pattern is invalid.
    fn compile_regex(pat: &str, pool: Option<&RegexPool>) -> crate::error::Result<Regex> {
        if let Some(p) = pool
            && let Ok(re) = p.get_or_compile(pat)
        {
            return Ok(re);
        }
        Ok(Regex::new(pat)?)
    }

    fn plan_impl(
        pattern: &str,
        options: QueryOptions,
        pool: Option<&RegexPool>,
    ) -> crate::error::Result<QueryPlan> {
        // Word-boundary mode: wrap literal in `\b...\b` or regex in `\b(?:...)\b`.
        // When `word_boundary=true` on a literal, we switch to regex mode internally
        // while keeping whole-word semantics.
        if !options.is_regex && !options.word_boundary {
            if !options.ignore_case {
                let bytes = pattern.as_bytes().to_vec();
                let trigrams = Extractor::extract_set(&bytes);

                let escaped = regex::escape(pattern);
                let regex = Self::compile_regex(&escaped, pool)?;

                if trigrams.is_empty() {
                    return Ok(QueryPlan::FullScan { regex });
                }

                return Ok(QueryPlan::Literal {
                    pattern: bytes,
                    trigrams,
                    regex,
                });
            }

            // Case-insensitive literal: per-position trigram groups for ASCII.
            // Non-ASCII patterns cannot rely on ASCII-only trigram case folding
            // without missing Unicode case equivalents, so fall back to FullScan.
            let regex_pat = format!("(?i){}", regex::escape(pattern));
            let regex = Self::compile_regex(&regex_pat, pool)?;

            if !pattern.is_ascii() {
                return Ok(QueryPlan::FullScan { regex });
            }

            let groups = Extractor::extract_groups_case_insensitive(pattern.as_bytes());
            if groups.is_empty() {
                return Ok(QueryPlan::FullScan { regex });
            }

            return Ok(QueryPlan::CaseInsensitive {
                regex,
                trigram_groups: groups,
            });
        }

        let (hir_source, regex_pat) = build_regex_patterns(pattern, options);
        let regex = Self::compile_regex(&regex_pat, pool)?;

        // For case-insensitive regex, fall back to FullScan — the (?i) regex
        // handles matching, and extracting trigram groups from regex literals
        // adds complexity without much narrowing benefit.
        if options.ignore_case {
            return Ok(QueryPlan::FullScan { regex });
        }

        let Ok(hir) = regex_syntax::parse(&hir_source) else {
            return Ok(QueryPlan::FullScan { regex });
        };

        let mut literals = Vec::new();
        Self::walk_hir(&hir, &mut literals);

        let required_trigram_sets: Vec<Vec<Trigram>> = literals
            .iter()
            .map(|lit| Extractor::extract_set(lit))
            .filter(|t| !t.is_empty())
            .collect();

        if required_trigram_sets.is_empty() {
            Ok(QueryPlan::FullScan { regex })
        } else {
            Ok(QueryPlan::RegexWithLiterals {
                regex,
                required_trigram_sets,
            })
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

/// Split leading inline regex flag directives (such as `(?i)` or `(?ms)`)
/// from the pattern body so wrappers like `\b(?:...)\b` keep flags outside.
pub(crate) fn split_leading_flags(pat: &str) -> (&str, &str) {
    let mut split_at = 0;
    while let Some(rest) = pat.get(split_at..) {
        let Some(after_open) = rest.strip_prefix("(?") else {
            break;
        };
        let Some(close_idx) = after_open.find(')') else {
            break;
        };
        let Some(flags) = after_open.get(..close_idx) else {
            break;
        };
        if flags.is_empty()
            || !flags
                .bytes()
                .all(|b| matches!(b, b'i' | b'm' | b's' | b'U' | b'x' | b'R' | b'-'))
        {
            break;
        }
        split_at += 2 + close_idx + 1;
    }
    (
        pat.get(..split_at).unwrap_or(""),
        pat.get(split_at..).unwrap_or(pat),
    )
}

/// Build `(hir_source, compiled_regex_pattern)` for regex and word-boundary plans.
///
/// `hir_source` preserves inline flags without `\b(?:...)\b` wrapping so literal
/// extraction inspects the inner pattern cleanly, while `compiled_regex_pattern`
/// applies `(?i)`, `(?s)`, and leading inline flags outside `\b(?:...)\b`.
fn build_regex_patterns(pattern: &str, options: QueryOptions) -> (String, String) {
    let i_prefix = if options.ignore_case { "(?i)" } else { "" };
    let s_prefix = if options.multiline && options.is_regex {
        "(?s)"
    } else {
        ""
    };

    if !options.is_regex {
        let escaped = regex::escape(pattern);
        let wrapped = if options.word_boundary {
            format!("\\b{escaped}\\b")
        } else {
            escaped
        };
        let compiled = format!("{i_prefix}{wrapped}");
        return (wrapped, compiled);
    }

    let (leading_flags, body) = split_leading_flags(pattern);
    let hir_source = format!("{s_prefix}{leading_flags}{body}");
    let body_or_wrapped = if options.word_boundary {
        format!("\\b(?:{body})\\b")
    } else {
        body.to_string()
    };
    let effective_i = if options.ignore_case && !leading_flags.starts_with("(?i)") {
        "(?i)"
    } else {
        ""
    };
    let compiled = format!("{effective_i}{s_prefix}{leading_flags}{body_or_wrapped}");
    (hir_source, compiled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_boundary_with_regex_wraps_in_non_capturing_group() {
        let plan = Planner::plan_with_options(
            "foo|bar",
            QueryOptions {
                is_regex: true,
                word_boundary: true,
                ..Default::default()
            },
        )
        .unwrap();

        match plan {
            QueryPlan::FullScan { regex } => {
                assert_eq!(regex.as_str(), r"\b(?:foo|bar)\b");
                assert!(regex.is_match("foo"));
                assert!(regex.is_match("bar"));
                assert!(!regex.is_match("foobar"));
            }
            other => panic!("expected FullScan for alternation, got {other:?}"),
        }
    }

    #[test]
    fn word_boundary_with_regex_hoists_inline_flags_and_extracts_literals() {
        let plan = Planner::plan_with_options(
            "(?s)alpha.*omega",
            QueryOptions {
                is_regex: true,
                word_boundary: true,
                ..Default::default()
            },
        )
        .unwrap();

        match plan {
            QueryPlan::RegexWithLiterals {
                regex,
                required_trigram_sets,
            } => {
                assert_eq!(regex.as_str(), r"(?s)\b(?:alpha.*omega)\b");
                assert_eq!(required_trigram_sets.len(), 2);
                assert!(regex.is_match("alpha\nomega"));
                assert!(!regex.is_match("xalpha\nomegax"));
            }
            other => panic!("expected RegexWithLiterals, got {other:?}"),
        }

        let ic_plan = Planner::plan_with_options(
            "(?i)foo|bar",
            QueryOptions {
                is_regex: true,
                word_boundary: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(ic_plan.pattern_str(), r"(?i)\b(?:foo|bar)\b");
    }

    #[test]
    fn non_ascii_ignore_case_literal_falls_back_to_full_scan() {
        // Non-ASCII literal ("caf\u{e9}") must emit FullScan so "CAF\u{c9}" is never missed.
        let plan = Planner::plan_with_options(
            "caf\u{e9}",
            QueryOptions {
                ignore_case: true,
                ..Default::default()
            },
        )
        .unwrap();

        match plan {
            QueryPlan::FullScan { regex } => {
                assert!(regex.is_match("CAF\u{c9}"));
                assert!(regex.is_match("caf\u{e9}"));
            }
            other => panic!("expected FullScan for non-ASCII ignore_case, got {other:?}"),
        }

        // ASCII ignore_case literal continues to use indexed CaseInsensitive plan.
        let ascii_plan = Planner::plan_with_options(
            "hello",
            QueryOptions {
                ignore_case: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(matches!(ascii_plan, QueryPlan::CaseInsensitive { .. }));
    }
}
