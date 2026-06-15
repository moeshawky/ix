//! Path string pool with prefix deduplication.
//!
//! Saves space by storing common directory prefixes once.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::io::{Seek, Write};
use std::path::Path;

/// Path string pool with prefix deduplication.
///
/// Stores common directory prefixes and stores only the suffix for each path.
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
    pub fn set_prefixes(&mut self, prefixes: Vec<String>) {
        self.prefixes = vec![String::new()];
        self.prefix_map = HashMap::new();
        self.prefix_map.insert(String::new(), 0);

        for p in prefixes {
            if p.is_empty() {
                continue;
            }
            let id = u16::try_from(self.prefixes.len()).unwrap_or(0);
            self.prefix_map.insert(p.clone(), id);
            self.prefixes.push(p);
        }
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
    /// Returns an I/O error if the writer fails.
    #[allow(clippy::as_conversions)] // binary format: suffix/filename len fits in u16
    pub fn serialize<W: Write + Seek>(&mut self, mut w: W) -> std::io::Result<()> {
        let start_pos = w.stream_position()?;

        w.write_all(
            &u32::try_from(self.prefixes.len())
                .unwrap_or(0)
                .to_le_bytes(),
        )?;

        for (i, p) in self.prefixes.iter().enumerate() {
            w.write_all(&u16::try_from(i).unwrap_or(0).to_le_bytes())?;
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
            let offset = u32::try_from(w.stream_position()? - start_pos)
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
            self.path_info.insert(path_str.clone(), (offset, total_len));
        }

        Ok(())
    }
}

/// Read-only reader for a serialized string pool.
pub struct StringPoolReader<'a> {
    data: &'a [u8],
    prefixes: Vec<&'a [u8]>,
}

impl<'a> StringPoolReader<'a> {
    /// Create a new reader from serialized string pool data.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is too small or the prefix table is
    /// truncated/invalid.
    #[allow(clippy::as_conversions)] // binary format: usize quantities within u16 range
    #[allow(clippy::indexing_slicing)] // length checks guarantee valid range
    pub fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 4 {
            return Err(Error::StringPoolOutOfBounds);
        }
        let prefix_count = data[0..4].try_into().ok().map_or(0, u32::from_le_bytes) as usize;
        let mut prefixes = Vec::with_capacity(prefix_count);
        let mut pos = 4;

        for _ in 0..prefix_count {
            if pos + 4 > data.len() {
                return Err(Error::StringPoolOutOfBounds);
            }
            let _id = data[pos..pos + 2]
                .try_into()
                .ok()
                .map_or(0, u16::from_le_bytes);
            let len = data[pos + 2..pos + 4]
                .try_into()
                .ok()
                .map_or(0, u16::from_le_bytes) as usize;
            pos += 4;
            if pos + len > data.len() {
                return Err(Error::StringPoolOutOfBounds);
            }
            prefixes.push(&data[pos..pos + len]);
            pos += len;
        }

        Ok(Self { data, prefixes })
    }

    /// Resolve a string from the pool at the given offset.
    ///
    /// # Errors
    ///
    /// Returns an error if the offset is out of bounds, prefix ID is invalid,
    /// or the stored bytes are not valid UTF-8.
    #[allow(clippy::as_conversions)] // binary format: offset/size within u16 range
    #[allow(clippy::indexing_slicing)] // length checks above guarantee valid range
    pub fn resolve(&self, offset: u32) -> Result<String> {
        let pos = offset as usize;
        if pos + 4 > self.data.len() {
            return Err(Error::StringPoolOutOfBounds);
        }

        let prefix_id = self.data[pos..pos + 2]
            .try_into()
            .ok()
            .map_or(0, u16::from_le_bytes) as usize;
        let suffix_len = self.data[pos + 2..pos + 4]
            .try_into()
            .ok()
            .map_or(0, u16::from_le_bytes) as usize;

        if prefix_id >= self.prefixes.len() {
            return Err(Error::StringPoolOutOfBounds);
        }

        let prefix = self.prefixes[prefix_id];
        let suffix_pos = pos + 4;
        if suffix_pos + suffix_len > self.data.len() {
            return Err(Error::StringPoolOutOfBounds);
        }
        let suffix = &self.data[suffix_pos..suffix_pos + suffix_len];

        let mut res = String::with_capacity(prefix.len() + suffix.len());
        res.push_str(std::str::from_utf8(prefix).map_err(|_| Error::InvalidPath)?);
        res.push_str(std::str::from_utf8(suffix).map_err(|_| Error::InvalidPath)?);

        Ok(res)
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
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
}
