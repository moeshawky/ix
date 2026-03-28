//! Archive searching support (.zip, .tar.gz).

#[cfg(feature = "archive")]
use std::fs::File;
#[cfg(feature = "archive")]
use std::io::Read;
#[cfg(feature = "archive")]
use std::path::{Path, PathBuf};
#[cfg(feature = "archive")]
use crate::error::Result;
#[cfg(feature = "archive")]
use crate::executor::{Match, QueryOptions};
#[cfg(feature = "archive")]
use regex::Regex;

const DECOMPRESSION_LIMIT: u64 = 10 * 1024 * 1024; // 10MB

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

        let mut buffer = Vec::new();
        // Limit entry size to 10MB to avoid OOM
        entry.take(DECOMPRESSION_LIMIT).read_to_end(&mut buffer)?;

        if is_binary(&buffer) {
            continue;
        }

        let display_path = format!("{}:{}", path.display(), entry_name);
        let entry_matches = match_content(&buffer, &PathBuf::from(display_path), regex, options);
        
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
        
        let mut buffer = Vec::new();
        // Limit entry size to 10MB
        entry.take(DECOMPRESSION_LIMIT).read_to_end(&mut buffer)?;

        if is_binary(&buffer) {
            continue;
        }

        let display_path = format!("{}:{}", path.display(), path_in_tar.display());
        let entry_matches = match_content(&buffer, &PathBuf::from(display_path), regex, options);
        
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
fn match_content(data: &[u8], path: &Path, regex: &Regex, options: &QueryOptions) -> Vec<Match> {
    let content = String::from_utf8_lossy(data);
    let lines: Vec<&str> = content.lines().collect();
    let mut matches = Vec::new();

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
                byte_offset: 0,
                context_before,
                context_after,
                is_binary: false,
            });
        }
    }
    matches
}
