//! Fallback scanner (no index, competitive with ripgrep).
//!
//! Used when .ix index is missing or explicitly disabled.

use crate::decompress::maybe_decompress;
use crate::error::Result;
use crate::executor::{Match, QueryOptions};
use ignore::WalkBuilder;
use memmap2::Mmap;
use rayon::prelude::*;
use regex::Regex;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

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
        ignore_case: bool,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        let raw = if is_regex {
            pattern.to_string()
        } else {
            regex::escape(pattern)
        };
        let regex_pat = if ignore_case { format!("(?i){raw}") } else { raw };
        let regex = Regex::new(&regex_pat)?;

        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .build();

        let paths: Vec<PathBuf> = walker
            .filter_map(|result| result.ok())
            .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|entry| entry.path().to_owned())
            .collect();

        let matches_found = AtomicU32::new(0);
        let mut matches: Vec<Match> = paths
            .into_par_iter()
            .filter_map(|path| {
                if options.max_results > 0
                    && matches_found.load(Ordering::Relaxed) >= options.max_results as u32
                {
                    return None;
                }

                // Filter by extension
                if !options.type_filter.is_empty() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if !options.type_filter.iter().any(|e| e == ext) {
                        return None;
                    }
                }

                // Archive support
                if options.archive {
                    let _ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let _is_tar_gz = path.to_str().map(|s| s.ends_with(".tar.gz")).unwrap_or(false);

                    #[cfg(feature = "archive")]
                    {
                        if _ext == "zip"
                            && let Ok(archive_matches) = crate::archive::scan_zip(&path, &regex, options)
                        {
                            matches_found.fetch_add(archive_matches.len() as u32, Ordering::Relaxed);
                            return Some(archive_matches);
                        } else if _is_tar_gz
                            && let Ok(archive_matches) = crate::archive::scan_tar_gz(&path, &regex, options)
                        {
                            matches_found.fetch_add(archive_matches.len() as u32, Ordering::Relaxed);
                            return Some(archive_matches);
                        }
                    }
                }

                let file_matches = self.scan_file(&path, &regex, options).ok()?;
                matches_found.fetch_add(file_matches.len() as u32, Ordering::Relaxed);
                Some(file_matches)
            })
            .flatten()
            .collect();

        if options.max_results > 0 && matches.len() > options.max_results {
            matches.truncate(options.max_results);
        }

        Ok(matches)
    }

    fn is_binary(data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }
        let check_len = data.len().min(512);
        let non_printable = data[..check_len]
            .iter()
            .filter(|&&b| !matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E))
            .count();

        (non_printable as f32 / check_len as f32) > 0.3
    }

    fn scan_file(
        &self,
        path: &Path,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if metadata.len() > 100 * 1024 * 1024 {
            return Ok(vec![]);
        }

        let mmap = unsafe { Mmap::map(&file)? };

        // Handle decompression
        let raw_data = if options.decompress {
            maybe_decompress(path, &mmap)?
                .map(std::borrow::Cow::Owned)
                .unwrap_or_else(|| std::borrow::Cow::Borrowed(&mmap[..]))
        } else {
            std::borrow::Cow::Borrowed(&mmap[..])
        };

        // Binary check
        let is_bin = Self::is_binary(&raw_data);
        if is_bin && !options.binary {
            return Ok(vec![]);
        }

        let data = String::from_utf8_lossy(&raw_data);

        let mut matches = Vec::new();

        if options.multiline {
            for m in regex.find_iter(&data) {
                let byte_offset = m.start();
                let line_number = data[..byte_offset].chars().filter(|&c| c == '\n').count() + 1;
                let line_start = data[..byte_offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let line_end = data[byte_offset..].find('\n').map(|i| byte_offset + i).unwrap_or(data.len());
                let line_content = &data[line_start..line_end];

                let context_before = if options.context_lines > 0 {
                    let all_lines: Vec<&str> = data.lines().collect();
                    let current_line_idx = line_number - 1;
                    let start = current_line_idx.saturating_sub(options.context_lines);
                    all_lines[start..current_line_idx].iter().map(|s| s.to_string()).collect()
                } else {
                    vec![]
                };

                let context_after = if options.context_lines > 0 {
                    let all_lines: Vec<&str> = data.lines().collect();
                    let current_line_idx = line_number - 1;
                    let end = (current_line_idx + 1 + options.context_lines).min(all_lines.len());
                    all_lines[current_line_idx + 1..end].iter().map(|s| s.to_string()).collect()
                } else {
                    vec![]
                };

                matches.push(Match {
                    file_path: path.to_owned(),
                    line_number: line_number as u32,
                    col: (byte_offset - line_start + 1) as u32,
                    line_content: if options.count_only {
                        String::new()
                    } else {
                        line_content.to_string()
                    },
                    byte_offset: byte_offset as u64,
                    context_before,
                    context_after,
                    is_binary: is_bin,
                });

                if options.max_results > 0 && matches.len() >= options.max_results {
                    break;
                }
            }
        } else {
            let lines: Vec<&str> = data.lines().collect();
            let mut line_start_offset = 0;
            for (i, line) in lines.iter().enumerate() {
                if let Some(m) = regex.find(line) {
                    let context_before = if options.context_lines > 0 {
                        let start = i.saturating_sub(options.context_lines);
                        lines[start..i].iter().map(|s| s.to_string()).collect()
                    } else {
                        vec![]
                    };

                    let context_after = if options.context_lines > 0 {
                        let end = (i + 1 + options.context_lines).min(lines.len());
                        lines[i + 1..end].iter().map(|s| s.to_string()).collect()
                    } else {
                        vec![]
                    };

                    matches.push(Match {
                        file_path: path.to_owned(),
                        line_number: (i + 1) as u32,
                        col: (m.start() + 1) as u32,
                        line_content: if options.count_only {
                            String::new()
                        } else {
                            line.to_string()
                        },
                        byte_offset: (line_start_offset + m.start()) as u64,
                        context_before,
                        context_after,
                        is_binary: is_bin,
                    });
                }
                line_start_offset += line.len() + 1;
            }
        }
        Ok(matches)
    }
}
