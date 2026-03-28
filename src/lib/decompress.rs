//! Transparent decompression by file extension.

use std::io::Read;
use std::path::Path;
use crate::error::Result;

/// Detect compression from extension, return streaming reader.
/// Returns None if not a compressed file or feature not enabled.
pub fn maybe_decompress<'a>(path: &Path, raw: &'a [u8]) -> Result<Option<Box<dyn Read + Send + 'a>>> {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        #[cfg(feature = "decompress")]
        "gz" => {
            let decoder = flate2::read::GzDecoder::new(raw);
            Ok(Some(Box::new(decoder)))
        }
        #[cfg(feature = "decompress")]
        "zst" => {
            let decoder = zstd::stream::read::Decoder::new(raw)?;
            Ok(Some(Box::new(decoder)))
        }
        #[cfg(feature = "decompress")]
        "bz2" => {
            let decoder = bzip2::read::BzDecoder::new(raw);
            Ok(Some(Box::new(decoder)))
        }
        #[cfg(feature = "decompress")]
        "xz" => {
            let decoder = xz2::read::XzDecoder::new(raw);
            Ok(Some(Box::new(decoder)))
        }
        _ => {
            let _ = raw; // avoid unused warning
            Ok(None)
        }
    }
}
