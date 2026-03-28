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
use std::io::{BufRead, BufReader, Read};
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

    fn scan_stream<R: Read>(
        &self,
        reader: R,
        path: &Path,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        let mut buf_reader = BufReader::new(reader);
        let mut matches = Vec::new();
        let mut line_number = 0u32;
        let mut byte_offset = 0u64;

        // Binary check on first 8KB
        {
            let buffer = buf_reader.fill_buf()?;
            let is_bin = Self::is_binary(buffer);
            if is_bin && !options.binary {
                return Ok(vec![]);
            }
        }

        let mut line = String::new();
        let mut context_before = std::collections::VecDeque::new();
        let mut pending_matches: Vec<Match> = Vec::new();

        while buf_reader.read_line(&mut line)? > 0 {
            line_number += 1;
            let line_len = line.len() as u64;
            let trimmed_line = line.trim_end().to_string();

            // Fill context_after for pending matches
            for m in &mut pending_matches {
                if m.context_after.len() < options.context_lines {
                    m.context_after.push(trimmed_line.clone());
                }
            }

            // Move completed matches to final list
            let (completed, still_pending): (Vec<_>, Vec<_>) = pending_matches
                .into_iter()
                .partition(|m| m.context_after.len() >= options.context_lines);
            matches.extend(completed);
            pending_matches = still_pending;

            if let Some(m) = regex.find(&line) {
                let context_before_vec: Vec<String> =
                    context_before.iter().map(|s: &String| s.trim_end().to_string()).collect();

                let new_match = Match {
                    file_path: path.to_owned(),
                    line_number,
                    col: (m.start() + 1) as u32,
                    line_content: if options.count_only {
                        String::new()
                    } else {
                        trimmed_line.clone()
                    },
                    byte_offset: byte_offset + m.start() as u64,
                    context_before: context_before_vec,
                    context_after: vec![],
                    is_binary: false,
                };

                if options.context_lines > 0 {
                    pending_matches.push(new_match);
                } else {
                    matches.push(new_match);
                }

                if options.max_results > 0 && (matches.len() + pending_matches.len()) >= options.max_results {
                    if pending_matches.is_empty() || matches.len() >= options.max_results {
                        break;
                    }
                }
            }

            if options.context_lines > 0 {
                context_before.push_back(line.clone());
                if context_before.len() > options.context_lines {
                    context_before.pop_front();
                }
            }

            byte_offset += line_len;
            line.clear();
        }

        matches.extend(pending_matches);
        Ok(matches)
    }

    fn scan_file(
        &self,
        path: &Path,
        regex: &Regex,
        options: &QueryOptions,
    ) -> Result<Vec<Match>> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if metadata.len() > 100 * 1024 * 1024 && !options.decompress {
            // Keep 100MB limit for raw files to avoid huge mmaps in parallel
            // But if it's compressed, we stream it, so no size limit needed.
            return Ok(vec![]);
        }

        let mmap = unsafe { Mmap::map(&file)? };

        if options.decompress {
            if let Some(reader) = maybe_decompress(path, &mmap)? {
                return self.scan_stream(reader, path, regex, options);
            }
        }

        // Binary check
        let is_bin = Self::is_binary(&mmap);
        if is_bin && !options.binary {
            return Ok(vec![]);
        }

        let mut matches = Vec::new();
        if options.multiline {
            let data = String::from_utf8_lossy(&mmap);
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
            let data = String::from_utf8_lossy(&mmap);
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

                    if options.max_results > 0 && matches.len() >= options.max_results {
                        break;
                    }
                }
                line_start_offset += line.len() + 1;
            }
        }
        Ok(matches)
    }
}
