//! Protobuf-style varint encoding/decoding.
//!
//! value < 128:     1 byte  [0xxxxxxx]
//! value < 16384:   2 bytes [1xxxxxxx 0xxxxxxx]
//! ... up to 10 bytes for u64

use crate::error::{Error, Result};

/// Encode a u64 as a varint, appending bytes to `buf`.
#[inline]
pub fn encode(mut value: u64, buf: &mut Vec<u8>) {
    while value >= 0x80 {
        buf.push((value as u8) | 0x80);
        value >>= 7;
    }
    buf.push(value as u8);
}

/// Decode a varint from `data` starting at `pos`. Advances `pos` past the varint.
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
        let byte = data[*pos];
        *pos += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}

/// Return the encoded byte length of a varint without allocating.
#[inline]
pub fn encoded_len(value: u64) -> usize {
    if value == 0 {
        return 1;
    }
    let bits = 64 - value.leading_zeros() as usize;
    bits.div_ceil(7)
}

#[cfg(test)]
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
        let values = [0, 1, 127, 128, 16383, 16384, u32::MAX as u64, u64::MAX];
        for &v in &values {
            let mut buf = Vec::new();
            encode(v, &mut buf);
            let mut pos = 0;
            assert_eq!(decode(&buf, &mut pos).unwrap(), v);
        }
    }

    #[test]
    fn encoded_lengths() {
        assert_eq!(encoded_len(0), 1);
        assert_eq!(encoded_len(127), 1);
        assert_eq!(encoded_len(128), 2);
        assert_eq!(encoded_len(16383), 2);
        assert_eq!(encoded_len(16384), 3);
    }

    #[test]
    fn truncated_error() {
        let mut pos = 0;
        assert!(decode(&[0x80], &mut pos).is_err()); // continuation bit set but no next byte
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
