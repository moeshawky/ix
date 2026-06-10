/// All ix error types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O error.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    /// Index file is too small (< 256 bytes).
    #[error("index too small (< 256 bytes)")]
    IndexTooSmall,

    /// Magic bytes mismatch (expected IX01).
    #[error("bad magic (expected IX01)")]
    BadMagic,

    /// Unsupported index file version.
    #[error("unsupported version {major}.{minor}")]
    UnsupportedVersion {
        /// Major version number.
        major: u16,
        /// Minor version number.
        minor: u16,
    },

    /// Header CRC checksum mismatch.
    #[error("header CRC mismatch (expected {expected:#010x}, got {actual:#010x})")]
    HeaderCorrupted {
        /// Expected CRC value.
        expected: u32,
        /// Actual CRC value found.
        actual: u32,
    },

    /// Posting list data is corrupted (CRC mismatch).
    #[error("posting list corrupted (CRC mismatch)")]
    PostingCorrupted,

    /// CDX compressed block is corrupted (decompression or decode failure).
    #[error("cdx block corrupted: {0}")]
    CdxBlockCorrupted(String),

    /// Section offset exceeds the index file bounds.
    /// Section offset exceeds the index file bounds.
    #[error("section offset out of bounds: {section} at {offset}+{size} > {file_len}")]
    SectionOutOfBounds {
        /// Name of the section being accessed.
        section: &'static str,
        /// Byte offset of the section.
        offset: u64,
        /// Size of the section in bytes.
        size: u64,
        /// Total length of the index file.
        file_len: u64,
    },

    /// Truncated varint at the given byte position.
    #[error("truncated varint at position {0}")]
    TruncatedVarint(usize),

    /// Varint overflow (exceeds 10 bytes for u64).
    #[error("varint overflow (> 10 bytes)")]
    OverflowVarint,

    /// Posting list data extends beyond available buffer.
    #[error("posting list out of bounds")]
    PostingOutOfBounds,

    /// File ID exceeds the known file table range.
    #[error("file_id {0} out of bounds")]
    FileIdOutOfBounds(u32),

    /// String pool offset out of bounds.
    #[error("string pool offset out of bounds")]
    StringPoolOutOfBounds,

    /// Invalid UTF-8 in a stored path.
    #[error("invalid UTF-8 in path")]
    InvalidPath,

    /// Regex compilation or matching error.
    #[error("regex: {0}")]
    Regex(#[from] regex::Error),

    /// File system watcher error (notify feature).
    #[cfg(feature = "notify")]
    #[error("Watcher error: {0}")]
    Watcher(#[from] notify::Error),

    /// Zip archive error (archive feature).
    #[cfg(feature = "archive")]
    #[error("Zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// Configuration error.
    #[error("config: {0}")]
    Config(String),
}

/// Convenience type alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;
