//! Path string pool with prefix deduplication.
//!
//! Saves space by storing common directory prefixes once.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::io::{Seek, Write};
use std::path::Path;

pub struct StringPool {
    prefixes: Vec<String>,
    prefix_map: HashMap<String, u16>,
    // Full path -> (offset in pool section, total length)
    path_info: HashMap<String, (u32, u16)>,
}

impl Default for StringPool {
    fn default() -> Self {
        Self::new()
    }
}

impl StringPool {
    pub fn new() -> Self {
        let prefixes = vec!["".to_string()];
        let mut prefix_map = HashMap::new();
        prefix_map.insert("".to_string(), 0);

        Self {
            prefixes,
            prefix_map,
            path_info: HashMap::new(),
        }
    }

    /// Add a path to the pool. During indexing, we just collect unique paths.
    /// Real prefix deduplication happens during serialization or via pre-added prefixes.
    pub fn add_path(&mut self, path: &Path) {
        let path_str = path.to_string_lossy().to_string();
        self.path_info.entry(path_str).or_insert((0, 0));
    }

    /// Set the prefixes to be used for deduplication.
    pub fn set_prefixes(&mut self, prefixes: Vec<String>) {
        self.prefixes = vec!["".to_string()];
        self.prefix_map = HashMap::new();
        self.prefix_map.insert("".to_string(), 0);

        for p in prefixes {
            if p.is_empty() {
                continue;
            }
            let id = self.prefixes.len() as u16;
            self.prefix_map.insert(p.clone(), id);
            self.prefixes.push(p);
        }
    }

    pub fn get_info(&self, path: &Path) -> (u32, u16) {
        let path_str = path.to_string_lossy();
        *self.path_info.get(path_str.as_ref()).unwrap_or(&(0, 0))
    }

    pub fn serialize<W: Write + Seek>(&mut self, mut w: W) -> std::io::Result<()> {
        let start_pos = w.stream_position()?;

        // Write prefix count
        w.write_all(&(self.prefixes.len() as u32).to_le_bytes())?;

        // Write prefix table
        for (i, p) in self.prefixes.iter().enumerate() {
            w.write_all(&(i as u16).to_le_bytes())?;
            w.write_all(&(p.len() as u16).to_le_bytes())?;
            w.write_all(p.as_bytes())?;
        }

        // Align to 4 bytes for path entries
        let current = w.stream_position()?;
        let padding = (4 - (current % 4)) % 4;
        for _ in 0..padding {
            w.write_all(&[0])?;
        }

        // Write path entries and record their offsets
        let paths: Vec<String> = self.path_info.keys().cloned().collect();
        for path_str in paths {
            let offset = (w.stream_position()? - start_pos) as u32;

            // Find longest matching prefix
            let mut best_prefix_id = 0u16;
            let mut best_prefix_len = 0;

            for (prefix, &id) in &self.prefix_map {
                if path_str.starts_with(prefix) && prefix.len() > best_prefix_len {
                    best_prefix_id = id;
                    best_prefix_len = prefix.len();
                }
            }

            let suffix = &path_str[best_prefix_len..];
            w.write_all(&best_prefix_id.to_le_bytes())?;
            w.write_all(&(suffix.len() as u16).to_le_bytes())?;
            w.write_all(suffix.as_bytes())?;

            self.path_info
                .insert(path_str.clone(), (offset, path_str.len() as u16));
        }

        Ok(())
    }
}

pub struct StringPoolReader<'a> {
    data: &'a [u8],
    prefixes: Vec<&'a [u8]>,
}

impl<'a> StringPoolReader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 4 {
            return Err(Error::StringPoolOutOfBounds);
        }
        let prefix_count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let mut prefixes = Vec::with_capacity(prefix_count);
        let mut pos = 4;

        for _ in 0..prefix_count {
            if pos + 4 > data.len() {
                return Err(Error::StringPoolOutOfBounds);
            }
            let _id = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap());
            let len = u16::from_le_bytes(data[pos + 2..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if pos + len > data.len() {
                return Err(Error::StringPoolOutOfBounds);
            }
            prefixes.push(&data[pos..pos + len]);
            pos += len;
        }

        Ok(Self { data, prefixes })
    }

    pub fn resolve(&self, offset: u32) -> Result<String> {
        let pos = offset as usize;
        if pos + 4 > self.data.len() {
            return Err(Error::StringPoolOutOfBounds);
        }

        let prefix_id = u16::from_le_bytes(self.data[pos..pos + 2].try_into().unwrap()) as usize;
        let suffix_len =
            u16::from_le_bytes(self.data[pos + 2..pos + 4].try_into().unwrap()) as usize;

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
