use std::fs;
use tempfile::tempdir;
use ix::reader::Reader;
use ix::format::*;

#[test]
fn test_invalid_magic() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let index_path = root.join("shard.ix");

    let mut f = fs::File::create(&index_path).unwrap();
    f.write_all(b"NOTMAGIC").unwrap();
    f.write_all(&[0u8; 256]).unwrap();

    let result = Reader::open(&index_path);
    assert!(result.is_err());
}

#[test]
fn test_unsupported_version() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let index_path = root.join("shard.ix");

    let mut header = [0u8; HEADER_SIZE];
    header[0..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&99u16.to_le_bytes()); // Major version 99
    
    fs::write(&index_path, header).unwrap();

    let result = Reader::open(&index_path);
    assert!(result.is_err());
}

#[test]
fn test_crc_mismatch() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let index_path = root.join("shard.ix");

    let mut header = [0u8; HEADER_SIZE];
    header[0..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
    header[6..8].copy_from_slice(&VERSION_MINOR.to_le_bytes());
    
    // Set a wrong CRC
    header[0xF8..0xFC].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
    
    fs::write(&index_path, header).unwrap();

    let result = Reader::open(&index_path);
    assert!(result.is_err());
}

#[test]
fn test_section_out_of_bounds() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let index_path = root.join("shard.ix");

    let mut header = [0u8; HEADER_SIZE];
    header[0..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
    header[6..8].copy_from_slice(&VERSION_MINOR.to_le_bytes());
    
    // File table says it's at offset 1000, but file is only 256 bytes
    header[0x28..0x30].copy_from_slice(&1000u64.to_le_bytes());
    header[0x30..0x38].copy_from_slice(&100u64.to_le_bytes());
    
    // Recompute CRC for valid header structure but invalid data
    let crc = crc32c::crc32c(&header[0..0xF8]);
    header[0xF8..0xFC].copy_from_slice(&crc.to_le_bytes());
    
    fs::write(&index_path, header).unwrap();

    let result = Reader::open(&index_path);
    assert!(result.is_err());
}
