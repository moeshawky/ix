//! Archive searching support (.zip, .tar.gz).

#[cfg(feature = "archive")]
use std::fs::File;
#[cfg(feature = "archive")]
use std::io::{BufRead, BufReader, Read};
#[cfg(feature = "archive")]
use std::path::{Path, PathBuf};
#[cfg(feature = "archive")]
use crate::error::Result;
#[cfg(feature = "archive")]
use crate::executor::{Match, QueryOptions};
#[cfg(feature = "archive")]
use regex::Regex;

#[cfg(feature = "archive")]
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

#[cfg(feature = "archive")]
pub fn scan_zip(path: &Path, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut matches = Vec::new();

    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if !entry.is_file() {
            continue;
        }

        let entry_name = entry.name().to_string();
        let display_path = format!("{}:{}", path.display(), entry_name);
        let entry_matches = match_content_stream(entry, &PathBuf::from(display_path), regex, options)?;
        
        for m in entry_matches {
            matches.push(m);
            if options.max_results > 0 && matches.len() >= options.max_results {
                return Ok(matches);
            }
        }
    }

    Ok(matches)
}

#[cfg(feature = "archive")]
pub fn scan_tar_gz(path: &Path, regex: &Regex, options: &QueryOptions) -> Result<Vec<Match>> {
    let file = File::open(path)?;
    let tar_gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(tar_gz);
    let mut matches = Vec::new();

    for entry in archive.entries()? {
        let entry = entry?;
        let path_in_tar = entry.path()?.to_path_buf();
        let display_path = format!("{}:{}", path.display(), path_in_tar.display());

        let entry_matches =
            match_content_stream(entry, &PathBuf::from(display_path), regex, options)?;

        for m in entry_matches {
            matches.push(m);
            if options.max_results > 0 && matches.len() >= options.max_results {
                return Ok(matches);
            }
        }
    }

    Ok(matches)
}

#[cfg(feature = "archive")]
fn match_content_stream<R: Read>(
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
        if buffer.is_empty() {
            return Ok(vec![]);
        }
        if is_binary(buffer) {
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

            if options.max_results > 0
                && (matches.len() + pending_matches.len()) >= options.max_results
                && (pending_matches.is_empty() || matches.len() >= options.max_results)
            {
                break;
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
