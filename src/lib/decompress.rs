//! Transparent decompression by file extension.

use crate::error::Result;
use std::io::Read;
use std::path::Path;

/// Maximum decompressed size allowed (`100 MiB`). Protects against decompression
/// bombs that could cause OOM when a small compressed payload expands to
/// gigabytes of output.
#[cfg(feature = "decompress")]
const MAX_DECOMPRESSED_BYTES: u64 = 100 * 1024 * 1024;

/// A reader wrapper that enforces a total byte limit on decompressed output.
/// Returns `io::ErrorKind::InvalidData` if the limit is exceeded, preventing
/// decompression bombs from consuming unbounded memory.
#[cfg(feature = "decompress")]
struct CountingReader<R> {
    inner: R,
    bytes_read: u64,
    limit: u64,
}

#[cfg(feature = "decompress")]
impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        if self.bytes_read > self.limit {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "decompressed data exceeds {} MB limit",
                    self.limit / (1024 * 1024)
                ),
            ))
        } else {
            Ok(n)
        }
    }
}

/// Wrap a decompression reader with byte-counting bomb protection.
#[cfg(feature = "decompress")]
fn wrap_with_limit<'a, R: Read + Send + 'a>(reader: R) -> Box<dyn Read + Send + 'a> {
    Box::new(CountingReader {
        inner: reader,
        bytes_read: 0,
        limit: MAX_DECOMPRESSED_BYTES,
    })
}

/// Detect compression from extension, return streaming reader.
/// Returns None if not a compressed file or feature not enabled.
///
/// # Errors
///
/// Returns an error if the decompression decoder cannot be initialized
/// (e.g. corrupt zstd data, or required features not compiled in).
pub fn maybe_decompress<'a>(
    path: &Path,
    raw: &'a [u8],
) -> Result<Option<Box<dyn Read + Send + 'a>>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        #[cfg(feature = "decompress")]
        "gz" => {
            let decoder = flate2::read::GzDecoder::new(raw);
            Ok(Some(wrap_with_limit(decoder)))
        }
        #[cfg(feature = "decompress")]
        "zst" => {
            let decoder = zstd::stream::read::Decoder::new(raw)?;
            Ok(Some(wrap_with_limit(decoder)))
        }
        #[cfg(feature = "decompress")]
        "bz2" => {
            let decoder = bzip2::read::BzDecoder::new(raw);
            Ok(Some(wrap_with_limit(decoder)))
        }
        #[cfg(feature = "decompress")]
        "xz" => {
            let decoder = xz2::read::XzDecoder::new(raw);
            Ok(Some(wrap_with_limit(decoder)))
        }
        _ => {
            let _ = raw; // avoid unused warning
            Ok(None)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::path::Path;

    // ---------------------------------------------------------------------------
    // No-compression paths: these should always return Ok(None).
    // ---------------------------------------------------------------------------

    #[test]
    fn plain_txt_returns_none() {
        let path = Path::new("notes.txt");
        let result = maybe_decompress(path, b"hello world").unwrap();
        assert!(result.is_none(), "expected None for .txt file");
    }

    #[test]
    fn no_extension_returns_none() {
        let path = Path::new("Makefile");
        let result = maybe_decompress(path, b"hello world").unwrap();
        assert!(result.is_none(), "expected None for file without extension");
    }

    #[test]
    fn unsupported_extension_returns_none() {
        let path = Path::new("archive.zip");
        let result = maybe_decompress(path, b"hello world").unwrap();
        assert!(
            result.is_none(),
            "expected None for .zip (zip is handled by archive module, not decompress)"
        );
    }

    #[test]
    fn dotfile_is_not_compressed() {
        // Path::new(".gz").extension() returns None because the leading dot
        // marks a hidden file with a single extension segment.
        let path = Path::new(".gz");
        let result = maybe_decompress(path, b"data").unwrap();
        assert!(result.is_none(), "expected None for hidden .gz file");
    }

    #[test]
    fn empty_data_plain_txt() {
        let path = Path::new("empty.txt");
        let result = maybe_decompress(path, b"").unwrap();
        assert!(result.is_none());
    }

    // ---------------------------------------------------------------------------
    // Compression round-trip tests — require the `decompress` feature.
    // ---------------------------------------------------------------------------

    #[cfg(feature = "decompress")]
    mod compression {
        use super::*;
        use std::io::{Read, Write};

        #[test]
        fn gz_roundtrip() {
            use flate2::Compression;
            use flate2::write::GzEncoder;

            let original = b"hello gzip\n";
            let mut enc = GzEncoder::new(Vec::new(), Compression::default());
            enc.write_all(original).unwrap();
            let compressed = enc.finish().unwrap();

            let path = Path::new("data.gz");
            let mut reader = maybe_decompress(path, &compressed).unwrap().unwrap();
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            assert_eq!(out, original);
        }

        #[test]
        fn zst_roundtrip() {
            let original = b"hello zstd\n";
            let compressed = zstd::encode_all(&original[..], 3).expect("zstd encode_all");

            let path = Path::new("data.zst");
            let mut reader = maybe_decompress(path, &compressed).unwrap().unwrap();
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            assert_eq!(out, original);
        }

        #[test]
        fn zst_empty_roundtrip() {
            // zstd can compress/decompress empty data.
            let original: &[u8] = b"";
            let compressed = zstd::encode_all(original, 3).expect("zstd encode_all empty");

            let path = Path::new("empty.zst");
            let mut reader = maybe_decompress(path, &compressed).unwrap().unwrap();
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            assert_eq!(out, original);
        }

        #[test]
        fn bz2_roundtrip() {
            use bzip2::Compression;
            use bzip2::write::BzEncoder;

            let original = b"hello bzip2\n";
            let mut enc = BzEncoder::new(Vec::new(), Compression::default());
            enc.write_all(original).unwrap();
            let compressed = enc.finish().unwrap();

            let path = Path::new("data.bz2");
            let mut reader = maybe_decompress(path, &compressed).unwrap().unwrap();
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            assert_eq!(out, original);
        }

        #[test]
        fn xz_roundtrip() {
            use xz2::write::XzEncoder;

            let original = b"hello xz\n";
            let mut enc = XzEncoder::new(Vec::new(), 6);
            enc.write_all(original).unwrap();
            let compressed = enc.finish().unwrap();

            let path = Path::new("data.xz");
            let mut reader = maybe_decompress(path, &compressed).unwrap().unwrap();
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            assert_eq!(out, original);
        }

        #[test]
        fn corrupt_zst_data_is_error_on_read() {
            // Feed random bytes as .zst — maybe_decompress returns a reader,
            // but reading from it should fail because the data is corrupt.
            let path = Path::new("corrupt.zst");
            let maybe_reader = maybe_decompress(path, b"this is not valid zstd");
            if let Ok(Some(mut reader)) = maybe_reader {
                // Reading from a corrupt zstd stream should produce an error.
                let mut out = Vec::new();
                let read_result = reader.read_to_end(&mut out);
                assert!(
                    read_result.is_err(),
                    "expected read error for corrupt zstd data, but read succeeded with {} bytes",
                    out.len()
                );
            } else {
                // If maybe_decompress returned Err or Ok(None), that's also acceptable
                // (the function detected corruption at header level).
            }
        }

        #[test]
        fn double_extension_uses_last() {
            // Path::new("archive.tar.gz").extension() == Some("gz").
            use flate2::Compression;
            use flate2::write::GzEncoder;

            let original = b"tarball content\n";
            let mut enc = GzEncoder::new(Vec::new(), Compression::default());
            enc.write_all(original).unwrap();
            let compressed = enc.finish().unwrap();

            let path = Path::new("archive.tar.gz");
            let mut reader = maybe_decompress(path, &compressed).unwrap().unwrap();
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            assert_eq!(out, original);
        }
    }
}
