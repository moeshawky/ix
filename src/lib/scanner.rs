//! Fallback scanner (no index, competitive with ripgrep).
//!
//! Used when .ix index is missing or explicitly disabled.

use crate::error::Result;
use crate::executor::Match;
use ignore::WalkBuilder;
use memmap2::Mmap;
use regex::Regex;
use std::fs::File;
use std::path::{Path, PathBuf};

pub struct Scanner {
    root: PathBuf,
}

impl Scanner {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
        }
    }

    pub fn scan(
        &self,
        pattern: &str,
        is_regex: bool,
        options: &crate::executor::QueryOptions,
    ) -> Result<Vec<Match>> {
        let regex = if is_regex {
            Regex::new(pattern)?
        } else {
            Regex::new(&regex::escape(pattern))?
        };

        let mut matches = Vec::new();
        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .build();

        for result in walker {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };

            #[allow(clippy::collapsible_if)]
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                // Filter by extension
                if !options.type_filter.is_empty() {
                    let ext = entry
                        .path()
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");
                    if !options.type_filter.iter().any(|e| e == ext) {
                        continue;
                    }
                }

                if let Ok(file_matches) =
                    self.scan_file(entry.path(), &regex, options.count_only, options.context_lines)
                {
                    if options.count_only {
                        matches.extend(file_matches);
                    } else {
                        for m in file_matches {
                            matches.push(m);
                            if options.max_results > 0 && matches.len() >= options.max_results {
                                return Ok(matches);
                            }
                        }
                    }
                }
            }
        }

        Ok(matches)
    }

    fn scan_file(
        &self,
        path: &Path,
        regex: &Regex,
        count_only: bool,
        context: usize,
    ) -> Result<Vec<Match>> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if metadata.len() > 100 * 1024 * 1024 {
            return Ok(vec![]);
        }

        let mmap = unsafe { Mmap::map(&file)? };

        // Binary check
        let check_len = mmap.len().min(8192);
        if mmap[..check_len].contains(&0u8) {
            return Ok(vec![]);
        }

        let data = String::from_utf8_lossy(&mmap);
        let lines: Vec<&str> = data.lines().collect();

        let mut matches = Vec::new();
        let mut line_start_offset = 0;
        for (i, line) in lines.iter().enumerate() {
            if let Some(m) = regex.find(line) {
                let context_before = if context > 0 {
                    let start = i.saturating_sub(context);
                    lines[start..i].iter().map(|s| s.to_string()).collect()
                } else {
                    vec![]
                };

                let context_after = if context > 0 {
                    let end = (i + 1 + context).min(lines.len());
                    lines[i + 1..end].iter().map(|s| s.to_string()).collect()
                } else {
                    vec![]
                };

                matches.push(Match {
                    file_path: path.to_owned(),
                    line_number: (i + 1) as u32,
                    col: (m.start() + 1) as u32,
                    line_content: if count_only {
                        String::new()
                    } else {
                        line.to_string()
                    },
                    byte_offset: (line_start_offset + m.start()) as u64,
                    context_before,
                    context_after,
                });
            }
            line_start_offset += line.len() + 1;
        }
        Ok(matches)
    }
}
