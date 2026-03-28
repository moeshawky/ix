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

    pub fn scan(&self, pattern: &str, is_regex: bool) -> Result<Vec<Match>> {
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
                if let Ok(file_matches) = self.scan_file(entry.path(), &regex) {
                    matches.extend(file_matches);
                }
            }
        }

        Ok(matches)
    }

    fn scan_file(&self, path: &Path, regex: &Regex) -> Result<Vec<Match>> {
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

        let data = match std::str::from_utf8(&mmap) {
            Ok(d) => d,
            Err(_) => return Ok(vec![]),
        };

        let mut matches = Vec::new();
        for (i, line) in data.lines().enumerate() {
            if let Some(m) = regex.find(line) {
                matches.push(Match {
                    file_path: path.to_owned(),
                    line_number: (i + 1) as u32,
                    line_content: line.to_string(),
                    byte_offset: m.start() as u64,
                });
            }
        }
        Ok(matches)
    }
}
