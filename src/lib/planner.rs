//! Query planner — transforms user input into an optimal index query plan.
//!
//! Decomposes regex patterns into required trigram sets.

use crate::trigram::{Extractor, Trigram};
use regex::Regex;
use regex_syntax::hir::{Hir, HirKind};

#[derive(Debug)]
pub enum QueryPlan {
    /// Fast path: literal string search
    Literal {
        pattern: Vec<u8>,
        trigrams: Vec<Trigram>,
    },

    /// Regex with extractable literals
    RegexWithLiterals {
        regex: Regex,
        required_trigram_sets: Vec<Vec<Trigram>>,
    },

    /// Case-insensitive indexed search. Each group = case variants for one
    /// trigram position. Executor UNIONs within groups, INTERSECTs across.
    CaseInsensitive {
        regex: Regex,
        trigram_groups: Vec<Vec<Trigram>>,
    },

    /// No literals extractable — full scan fallback
    FullScan { regex: Regex },
}

pub struct Planner;

impl Planner {
    pub fn plan(pattern: &str, is_regex: bool) -> QueryPlan {
        Self::plan_with_options(pattern, is_regex, false)
    }

    pub fn plan_with_options(pattern: &str, is_regex: bool, ignore_case: bool) -> QueryPlan {
        if !is_regex && !ignore_case {
            let bytes = pattern.as_bytes().to_vec();
            let trigrams = Extractor::extract_set(&bytes);

            if trigrams.is_empty() {
                // Pattern too short for trigrams (< 3 bytes)
                return QueryPlan::FullScan {
                    regex: Regex::new(&regex::escape(pattern)).unwrap(),
                };
            }

            return QueryPlan::Literal {
                pattern: bytes,
                trigrams,
            };
        }

        // Case-insensitive literal: per-position trigram groups.
        // Executor UNIONs within each group, INTERSECTs across groups.
        if !is_regex && ignore_case {
            let bytes = pattern.as_bytes();
            let groups = Extractor::extract_groups_case_insensitive(bytes);
            let regex_pat = format!("(?i){}", regex::escape(pattern));
            let regex = match Regex::new(&regex_pat) {
                Ok(r) => r,
                Err(_) => return QueryPlan::FullScan {
                    regex: Regex::new("").unwrap(),
                },
            };

            if groups.is_empty() {
                return QueryPlan::FullScan { regex };
            }

            return QueryPlan::CaseInsensitive {
                regex,
                trigram_groups: groups,
            };
        }

        let regex_pat = if ignore_case && !pattern.starts_with("(?i)") {
            format!("(?i){pattern}")
        } else {
            pattern.to_string()
        };

        let regex = match Regex::new(&regex_pat) {
            Ok(r) => r,
            Err(_) => {
                return QueryPlan::FullScan {
                    regex: Regex::new("").unwrap(),
                };
            } // Should be handled by CLI
        };

        let hir = match regex_syntax::parse(pattern) {
            Ok(h) => h,
            Err(_) => return QueryPlan::FullScan { regex },
        };

        let mut literals = Vec::new();
        Self::walk_hir(&hir, &mut literals);

        // For case-insensitive regex, fall back to FullScan — the (?i) regex
        // handles matching, and extracting trigram groups from regex literals
        // adds complexity without much narrowing benefit.
        if ignore_case {
            return QueryPlan::FullScan { regex };
        }

        let required_trigram_sets: Vec<Vec<Trigram>> = literals
            .iter()
            .map(|lit| Extractor::extract_set(lit.as_bytes()))
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

    fn walk_hir(hir: &Hir, out: &mut Vec<String>) {
        match hir.kind() {
            HirKind::Literal(lit) => {
                out.push(String::from_utf8_lossy(&lit.0).to_string());
            }
            HirKind::Concat(children) => {
                let mut current = String::new();
                for child in children {
                    match child.kind() {
                        HirKind::Literal(lit) => {
                            current.push_str(&String::from_utf8_lossy(&lit.0));
                        }
                        _ => {
                            if current.len() >= 3 {
                                out.push(current.clone());
                            }
                            current.clear();
                            Self::walk_hir(child, out);
                        }
                    }
                }
                if current.len() >= 3 {
                    out.push(current);
                }
            }
            HirKind::Repetition(rep) => {
                if rep.min >= 1 {
                    Self::walk_hir(&rep.sub, out);
                }
            }
            // Simplified: we don't extract from Alternation for now as per DESIGN.md
            _ => {}
        }
    }
}
