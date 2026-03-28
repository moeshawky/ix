/// All ix error types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("index too small (< 256 bytes)")]
    IndexTooSmall,

    #[error("bad magic (expected IX01)")]
    BadMagic,

    #[error("unsupported version {major}.{minor}")]
    UnsupportedVersion { major: u16, minor: u16 },

    #[error("header CRC mismatch (expected {expected:#010x}, got {actual:#010x})")]
    HeaderCorrupted { expected: u32, actual: u32 },

    #[error("section offset out of bounds: {section} at {offset}+{size} > {file_len}")]
    SectionOutOfBounds {
        section: &'static str,
        offset: u64,
        size: u64,
        file_len: u64,
    },

    #[error("truncated varint at position {0}")]
    TruncatedVarint(usize),

    #[error("varint overflow (> 10 bytes)")]
    OverflowVarint,

    #[error("posting list out of bounds")]
    PostingOutOfBounds,

    #[error("file_id {0} out of bounds")]
    FileIdOutOfBounds(u32),

    #[error("string pool offset out of bounds")]
    StringPoolOutOfBounds,

    #[error("invalid UTF-8 in path")]
    InvalidPath,

    #[error("regex: {0}")]
    Regex(#[from] regex::Error),

    #[cfg(feature = "notify")]
    #[error("Watcher error: {0}")]
    Watcher(#[from] notify::Error),

    #[cfg(feature = "archive")]
    #[error("Zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("config: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, Error>;
