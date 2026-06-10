//! Protobuf-style varint encoding/decoding.
//!
//! value < 128:     1 byte  `0xxxxxxx`
//! value < 16384:   2 bytes `1xxxxxxx 0xxxxxxx`
//! ... up to 10 bytes for u64

use crate::error::{Error, Result};

/// Encode a u64 as a varint, appending bytes to `buf`.
#[inline]
pub fn encode(mut value: u64, buf: &mut Vec<u8>) {
    while value >= 0x80 {
        buf.push(u8::try_from(value & 0xFF).unwrap_or(0) | 0x80);
        value >>= 7;
    }
    buf.push(u8::try_from(value & 0xFF).unwrap_or(0));
}

/// Decode a varint from `data` starting at `pos`. Advances `pos` past the varint.
///
/// # Errors
///
/// Returns `TruncatedVarint` if the data is too short, or `OverflowVarint` if
/// the varint is too large (>=70 bits).
#[inline]
pub fn decode(data: &[u8], pos: &mut usize) -> Result<u64> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        if *pos >= data.len() {
            return Err(Error::TruncatedVarint(*pos));
        }
        if shift >= 70 {
            return Err(Error::OverflowVarint);
        }
        let byte = *data.get(*pos).ok_or(Error::TruncatedVarint(*pos))?;
        *pos += 1;
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}

#[cfg(test)]
#[allow(clippy::as_conversions, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_small() {
        for v in 0..300u64 {
            let mut buf = Vec::new();
            encode(v, &mut buf);
            let mut pos = 0;
            assert_eq!(decode(&buf, &mut pos).unwrap(), v);
            assert_eq!(pos, buf.len());
        }
    }

    #[test]
    fn roundtrip_large() {
        let values = [0, 1, 127, 128, 16383, 16384, u64::from(u32::MAX), u64::MAX];
        for &v in &values {
            let mut buf = Vec::new();
            encode(v, &mut buf);
            let mut pos = 0;
            assert_eq!(decode(&buf, &mut pos).unwrap(), v);
        }
    }

    #[test]
    fn truncated_error() {
        let mut pos = 0;
        assert!(decode(&[0x80], &mut pos).is_err());
    }

    #[test]
    fn multiple_sequential() {
        let mut buf = Vec::new();
        encode(42, &mut buf);
        encode(1000, &mut buf);
        encode(0, &mut buf);

        let mut pos = 0;
        assert_eq!(decode(&buf, &mut pos).unwrap(), 42);
        assert_eq!(decode(&buf, &mut pos).unwrap(), 1000);
        assert_eq!(decode(&buf, &mut pos).unwrap(), 0);
        assert_eq!(pos, buf.len());
    }
}
