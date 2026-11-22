//! Path string pool with prefix deduplication.
//!
//! Saves space by storing common directory prefixes once.
//!
//! # On-disk 10-byte streaming header alignment (`STR-1`)
//!
//! [`Builder`](crate::builder::Builder) streams path entries directly into
//! `shard.ix.tmp.strings` (rather than buffering all paths in [`StringPool`])
//! using a 10-byte header: `version = 1u32`, `size = 0u16`, `count = 0u16`,
//! and 2 zero padding bytes, followed by `(prefix_id = 0u16, path_len: u16,
//! path_bytes)` records starting at offset 10. [`StringPoolReader::new`] reads
//! that header as `prefix_count = 1`, prefix `_id = 0`, `len = 0` (empty
//! prefix `""`), leaving the 2 padding bytes unread before offset 10 so
//! `resolve(offset)` resolves every streamed record cleanly.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::io::{Seek, Write};
use std::path::Path;

/// Path string pool with prefix deduplication.
///
/// Stores common directory prefixes and stores only the suffix for each path.
/// Note: [`Builder`](crate::builder::Builder) currently streams the compatible
/// 10-byte empty-prefix header directly to disk (`STR-1`, quarantined in
/// `.annotations/_bugs/str-1.yaml`) to preserve the `<8 MB` build RAM budget.
pub struct StringPool {
    prefixes: Vec<String>,
    prefix_map: HashMap<String, u16>,
    path_info: HashMap<String, (u32, u16)>,
}

impl Default for StringPool {
    fn default() -> Self {
        Self::new()
    }
}

impl StringPool {
    /// Create an empty string pool with a default empty-string prefix.
    #[must_use]
    pub fn new() -> Self {
        let prefixes = vec![String::new()];
        let mut prefix_map = HashMap::new();
        prefix_map.insert(String::new(), 0);

        Self {
            prefixes,
            prefix_map,
            path_info: HashMap::new(),
        }
    }

    /// Add a path to the pool. During indexing, we just collect unique paths.
    ///
    /// Real prefix deduplication happens during serialization or via pre-added prefixes.
    pub fn add_path(&mut self, path: &Path) {
        let path_str = path.to_string_lossy().to_string();
        self.path_info.entry(path_str).or_insert((0, 0));
    }

    /// Set the prefixes to be used for deduplication.
    ///
    /// At most `u16::MAX` (65,535) non-empty prefixes can be indexed alongside the
    /// default empty prefix at ID `0`; excess prefixes are ignored with a
    /// warning. Use [`Self::try_set_prefixes`] to reject overflow with an error.
    pub fn set_prefixes(&mut self, prefixes: Vec<String>) {
        let mut new_prefixes = vec![String::new()];
        let mut new_map = HashMap::new();
        new_map.insert(String::new(), 0);

        for p in prefixes {
            if p.is_empty() || new_map.contains_key(&p) {
                continue;
            }
            let Ok(id) = u16::try_from(new_prefixes.len()) else {
                tracing::warn!(
                    "string pool prefix limit ({}) reached; ignoring excess prefixes",
                    u16::MAX
                );
                break;
            };
            new_map.insert(p.clone(), id);
            new_prefixes.push(p);
        }

        self.prefixes = new_prefixes;
        self.prefix_map = new_map;
    }

    /// Set the prefixes to be used for deduplication, failing if more than
    /// `u16::MAX` (65,535) distinct non-empty prefixes are supplied.
    ///
    /// # Errors
    ///
    /// Returns [`Error::StringPoolOutOfBounds`] if the prefix count overflows `u16`.
    pub fn try_set_prefixes(&mut self, prefixes: Vec<String>) -> Result<()> {
        let mut new_prefixes = vec![String::new()];
        let mut new_map = HashMap::new();
        new_map.insert(String::new(), 0);

        for p in prefixes {
            if p.is_empty() || new_map.contains_key(&p) {
                continue;
            }
            let id = u16::try_from(new_prefixes.len()).map_err(|_| Error::StringPoolOutOfBounds)?;
            new_map.insert(p.clone(), id);
            new_prefixes.push(p);
        }

        self.prefixes = new_prefixes;
        self.prefix_map = new_map;
        Ok(())
    }

    /// Get the serialized offset and total length info for a path.
    #[must_use]
    pub fn get_info(&self, path: &Path) -> (u32, u16) {
        let path_str = path.to_string_lossy();
        *self.path_info.get(path_str.as_ref()).unwrap_or(&(0, 0))
    }

    /// Serialize the string pool to the writer.
    ///
    /// # Errors
    ///
    /// Returns an `InvalidInput` error if prefix counts/IDs exceed `u16::MAX`,
    /// or an I/O error if the writer fails.
    pub fn serialize<W: Write + Seek>(&mut self, mut w: W) -> std::io::Result<()> {
        if self.prefixes.len() > usize::from(u16::MAX) + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("too many string pool prefixes: {}", self.prefixes.len()),
            ));
        }

        let start_pos = w.stream_position()?;
        let prefix_count = u32::try_from(self.prefixes.len())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        w.write_all(&prefix_count.to_le_bytes())?;

        for (i, p) in self.prefixes.iter().enumerate() {
            let id = u16::try_from(i)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            w.write_all(&id.to_le_bytes())?;
            w.write_all(
                &u16::try_from(p.len())
                    .map_err(|e| std::io::Error::other(format!("prefix too long: {e}")))?
                    .to_le_bytes(),
            )?;
            w.write_all(p.as_bytes())?;
        }

        let current = w.stream_position()?;
        let padding = (4 - (current % 4)) % 4;
        for _ in 0..padding {
            w.write_all(&[0])?;
        }

        let paths: Vec<String> = self.path_info.keys().cloned().collect();
        for path_str in paths {
            let rel_pos = w
                .stream_position()?
                .checked_sub(start_pos)
                .ok_or_else(|| std::io::Error::other("string pool offset underflow"))?;
            let offset = u32::try_from(rel_pos)
                .map_err(|_| std::io::Error::other("string pool offset overflow"))?;

            let mut best_prefix_id = 0u16;
            let mut best_prefix_len = 0;

            for (prefix, &id) in &self.prefix_map {
                if path_str.starts_with(prefix) && prefix.len() > best_prefix_len {
                    best_prefix_id = id;
                    best_prefix_len = prefix.len();
                }
            }

            let suffix = path_str.get(best_prefix_len..).unwrap_or("");
            let suffix_len = u16::try_from(suffix.len())
                .map_err(|e| std::io::Error::other(format!("suffix too long: {e}")))?;
            w.write_all(&best_prefix_id.to_le_bytes())?;
            w.write_all(&suffix_len.to_le_bytes())?;
            w.write_all(suffix.as_bytes())?;

            let total_len = u16::try_from(path_str.len())
                .map_err(|e| std::io::Error::other(format!("path too long: {e}")))?;
            self.path_info.insert(path_str, (offset, total_len));
        }

        Ok(())
    }
}

/// Read-only reader for a serialized string pool.
///
/// Parses both [`StringPool::serialize`] payloads and the 10-byte empty-prefix
/// streaming header written by [`Builder`](crate::builder::Builder).
pub struct StringPoolReader<'a> {
    data: &'a [u8],
    prefixes: Vec<&'a [u8]>,
}

impl<'a> StringPoolReader<'a> {
    /// Create a new reader from serialized string pool data.
    ///
    /// # Errors
    ///
    /// Returns [`Error::StringPoolOutOfBounds`] if the data is too small,
    /// `prefix_count` exceeds `65_536` (`u16::MAX + 1`), or the prefix table
    /// is truncated/invalid.
    pub fn new(data: &'a [u8]) -> Result<Self> {
        // u16 prefix_id can address at most 65_536 entries (0..=u16::MAX).
        const MAX_STRING_POOL_PREFIXES: usize = 65_536;

        let count_bytes: [u8; 4] = data
            .get(0..4)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::StringPoolOutOfBounds)?;
        let prefix_count = usize::try_from(u32::from_le_bytes(count_bytes))
            .map_err(|_| Error::StringPoolOutOfBounds)?;
        if prefix_count > MAX_STRING_POOL_PREFIXES {
            return Err(Error::StringPoolOutOfBounds);
        }

        let max_possible_entries = data.len().saturating_sub(4) / 4;
        let mut prefixes = Vec::with_capacity(prefix_count.min(max_possible_entries));
        let mut pos = 4usize;

        for _ in 0..prefix_count {
            let header_end = pos.checked_add(4).ok_or(Error::StringPoolOutOfBounds)?;
            let header = data
                .get(pos..header_end)
                .ok_or(Error::StringPoolOutOfBounds)?;
            let len_bytes: [u8; 2] = header
                .get(2..4)
                .and_then(|s| s.try_into().ok())
                .ok_or(Error::StringPoolOutOfBounds)?;
            let len = usize::from(u16::from_le_bytes(len_bytes));
            pos = header_end;

            let str_end = pos.checked_add(len).ok_or(Error::StringPoolOutOfBounds)?;
            let prefix_slice = data.get(pos..str_end).ok_or(Error::StringPoolOutOfBounds)?;
            prefixes.push(prefix_slice);
            pos = str_end;
        }

        Ok(Self { data, prefixes })
    }

    /// Resolve a string from the pool at the given offset.
    ///
    /// # Errors
    ///
    /// Returns an error if the offset is out of bounds, prefix ID is invalid,
    /// or the stored bytes are not valid UTF-8.
    pub fn resolve(&self, offset: u32) -> Result<String> {
        let pos = usize::try_from(offset).map_err(|_| Error::StringPoolOutOfBounds)?;
        let header_end = pos.checked_add(4).ok_or(Error::StringPoolOutOfBounds)?;
        let header = self
            .data
            .get(pos..header_end)
            .ok_or(Error::StringPoolOutOfBounds)?;

        let prefix_id_bytes: [u8; 2] = header
            .get(0..2)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::StringPoolOutOfBounds)?;
        let suffix_len_bytes: [u8; 2] = header
            .get(2..4)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::StringPoolOutOfBounds)?;
        let prefix_id = usize::from(u16::from_le_bytes(prefix_id_bytes));
        let suffix_len = usize::from(u16::from_le_bytes(suffix_len_bytes));

        let prefix = *self
            .prefixes
            .get(prefix_id)
            .ok_or(Error::StringPoolOutOfBounds)?;
        let suffix_end = header_end
            .checked_add(suffix_len)
            .ok_or(Error::StringPoolOutOfBounds)?;
        let suffix = self
            .data
            .get(header_end..suffix_end)
            .ok_or(Error::StringPoolOutOfBounds)?;

        let mut res = String::with_capacity(prefix.len().saturating_add(suffix.len()));
        res.push_str(std::str::from_utf8(prefix).map_err(|_| Error::InvalidPath)?);
        res.push_str(std::str::from_utf8(suffix).map_err(|_| Error::InvalidPath)?);

        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip() {
        let mut pool = StringPool::new();
        pool.set_prefixes(vec!["/home/user/".to_string(), "/var/log/".to_string()]);
        pool.add_path(Path::new("/home/user/file.rs"));
        pool.add_path(Path::new("/var/log/syslog"));
        pool.add_path(Path::new("/other/path"));

        let mut buf = Cursor::new(Vec::new());
        pool.serialize(&mut buf).unwrap();

        let data = buf.into_inner();
        let reader = StringPoolReader::new(&data).unwrap();

        let (off1, _) = pool.get_info(Path::new("/home/user/file.rs"));
        assert_eq!(reader.resolve(off1).unwrap(), "/home/user/file.rs");

        let (off2, _) = pool.get_info(Path::new("/var/log/syslog"));
        assert_eq!(reader.resolve(off2).unwrap(), "/var/log/syslog");

        let (off3, _) = pool.get_info(Path::new("/other/path"));
        assert_eq!(reader.resolve(off3).unwrap(), "/other/path");
    }

    #[test]
    fn test_reader_parses_builder_10byte_empty_header() {
        let mut data = Vec::new();
        // Builder::init_writers 10-byte header: version=1, size=0, count=0, pad=[0,0]
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&[0u8; 2]);

        let offset = u32::try_from(data.len()).unwrap();
        assert_eq!(offset, 10);
        let path = b"src/lib/foo.rs";
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&u16::try_from(path.len()).unwrap().to_le_bytes());
        data.extend_from_slice(path);

        let reader = StringPoolReader::new(&data).expect("Builder 10-byte header must parse");
        assert_eq!(reader.resolve(offset).unwrap(), "src/lib/foo.rs");
    }

    #[test]
    fn test_try_set_prefixes_rejects_over_u16_max() {
        let mut pool = StringPool::new();
        let too_many: Vec<String> = (0..=usize::from(u16::MAX))
            .map(|i| format!("p{i}/"))
            .collect();
        assert!(matches!(
            pool.try_set_prefixes(too_many.clone()),
            Err(Error::StringPoolOutOfBounds)
        ));
        // set_prefixes caps cleanly without wrapping IDs to 0
        pool.set_prefixes(too_many);
        assert_eq!(pool.prefixes.len(), usize::from(u16::MAX) + 1);
    }

    #[test]
    fn test_reader_rejects_oversized_prefix_count() {
        // 4-byte buffer claiming 65_537 prefixes (> MAX_STRING_POOL_PREFIXES) must error cleanly
        let data = 65_537u32.to_le_bytes();
        assert!(matches!(
            StringPoolReader::new(&data),
            Err(Error::StringPoolOutOfBounds)
        ));
        // Also 0xFFFF_FFFF on 4-byte buffer must not OOM or succeed
        let max_data = u32::MAX.to_le_bytes();
        assert!(matches!(
            StringPoolReader::new(&max_data),
            Err(Error::StringPoolOutOfBounds)
        ));
    }
}
