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

    /// No literals extractable — full scan fallback
    FullScan { regex: Regex },
}

pub struct Planner;

impl Planner {
    pub fn plan(pattern: &str, is_regex: bool) -> QueryPlan {
        if !is_regex {
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

        let regex = match Regex::new(pattern) {
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
