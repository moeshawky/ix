//! Transparent decompression by file extension.

use std::path::Path;
use crate::error::Result;

const DECOMPRESSION_LIMIT: u64 = 10 * 1024 * 1024; // 10MB

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

/// Detect compression from extension, decompress to Vec<u8>.
/// Returns None if not a compressed file or feature not enabled.
pub fn maybe_decompress(path: &Path, _raw: &[u8]) -> Result<Option<Vec<u8>>> {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        #[cfg(feature = "decompress")]
        "gz" => {
            use std::io::Read;
            let decoder = flate2::read::GzDecoder::new(_raw);
            let mut buffer = Vec::new();
            decoder.take(DECOMPRESSION_LIMIT).read_to_end(&mut buffer)?;
            if is_binary(&buffer) {
                return Ok(None);
            }
            Ok(Some(buffer))
        }
        #[cfg(feature = "decompress")]
        "zst" => {
            use std::io::Read;
            let decoder = zstd::stream::read::Decoder::new(_raw)?;
            let mut buffer = Vec::new();
            decoder.take(DECOMPRESSION_LIMIT).read_to_end(&mut buffer)?;
            if is_binary(&buffer) {
                return Ok(None);
            }
            Ok(Some(buffer))
        }
        #[cfg(feature = "decompress")]
        "bz2" => {
            use std::io::Read;
            let decoder = bzip2::read::BzDecoder::new(_raw);
            let mut buffer = Vec::new();
            decoder.take(DECOMPRESSION_LIMIT).read_to_end(&mut buffer)?;
            if is_binary(&buffer) {
                return Ok(None);
            }
            Ok(Some(buffer))
        }
        #[cfg(feature = "decompress")]
        "xz" => {
            use std::io::Read;
            let decoder = xz2::read::XzDecoder::new(_raw);
            let mut buffer = Vec::new();
            decoder.take(DECOMPRESSION_LIMIT).read_to_end(&mut buffer)?;
            if is_binary(&buffer) {
                return Ok(None);
            }
            Ok(Some(buffer))
        }
        _ => Ok(None),
    }
}
