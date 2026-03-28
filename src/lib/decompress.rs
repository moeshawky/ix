//! Transparent decompression by file extension.

use std::path::Path;
use crate::error::Result;

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
            let mut decoder = flate2::read::GzDecoder::new(_raw);
            let mut buffer = Vec::new();
            decoder.read_to_end(&mut buffer)?;
            Ok(Some(buffer))
        }
        #[cfg(feature = "decompress")]
        "zst" => {
            let buffer = zstd::decode_all(_raw)?;
            Ok(Some(buffer))
        }
        #[cfg(feature = "decompress")]
        "bz2" => {
            use std::io::Read;
            let mut decoder = bzip2::read::BzDecoder::new(_raw);
            let mut buffer = Vec::new();
            decoder.read_to_end(&mut buffer)?;
            Ok(Some(buffer))
        }
        #[cfg(feature = "decompress")]
        "xz" => {
            use std::io::Read;
            let mut decoder = xz2::read::XzDecoder::new(_raw);
            let mut buffer = Vec::new();
            decoder.read_to_end(&mut buffer)?;
            Ok(Some(buffer))
        }
        _ => Ok(None),
    }
}
