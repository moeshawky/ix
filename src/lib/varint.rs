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

    // ── Rule 1: Error Path Tests ──────────────────────────────────────

    /// Decoding from an empty slice must return TruncatedVarint.
    #[test]
    fn test_decode_empty_slice_error() {
        let mut pos = 0;
        let result = decode(&[], &mut pos);
        assert!(result.is_err(), "empty slice should fail");
        match result {
            Err(Error::TruncatedVarint(_)) => {}
            other => panic!("expected TruncatedVarint, got {other:?}"),
        }
    }

    /// A varint with 11 consecutive continuation bytes must return OverflowVarint.
    #[test]
    fn test_decode_overflow_varint_error() {
        // 11 bytes, each with continuation bit set (0x80), followed by a final byte
        let data = [0x80u8; 12];
        let mut pos = 0;
        let result = decode(&data, &mut pos);
        assert!(result.is_err(), "11 continuation bytes should overflow");
        match result {
            Err(Error::OverflowVarint) => {}
            other => panic!("expected OverflowVarint, got {other:?}"),
        }
    }

    /// Decoding from a slice that starts past the end must return TruncatedVarint.
    #[test]
    fn test_decode_position_past_end_error() {
        let data = [0x01u8];
        let mut pos = 1;
        let result = decode(&data, &mut pos);
        assert!(result.is_err(), "position past end should fail");
    }

    // ── Rule 2: Corruption Proptests ──────────────────────────────────

    /// Encode random varint values, truncate the buffer at every possible
    /// position < full length, and verify decode returns Err (no panic).
    #[test]
    fn prop_varint_truncation() {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let test_values: Vec<u64> = (0..20)
            .map(|_| rng.r#gen::<u64>() % (1 << rng.r#gen_range(0..64)))
            .chain([0, 1, 127, 128, u32::MAX as u64, u64::MAX])
            .collect();

        for &v in &test_values {
            let mut buf = Vec::new();
            encode(v, &mut buf);
            let full_len = buf.len();
            // Truncate at every possible position (0..full_len)
            for truncate_at in 0..full_len {
                let truncated = &buf[..truncate_at];
                let mut pos = 0;
                let result = decode(truncated, &mut pos);
                assert!(
                    result.is_err(),
                    "varint={v}, len={full_len}, truncate_at={truncate_at}: decode should Err"
                );
            }
            // Full decode must succeed
            let mut pos = 0;
            let decoded = decode(&buf, &mut pos).expect("full decode should succeed");
            assert_eq!(decoded, v, "roundtrip mismatch for value {v}");
            assert_eq!(pos, full_len, "position should be at end");
        }
    }

    // ── Rule 5: Integer Boundary Tests ─────────────────────────────────

    /// Varint encode/decode must work correctly at u64 boundaries.
    #[test]
    fn prop_varint_boundary() {
        let values = [
            0u64,
            1,
            127,
            128,
            255,
            256,
            u32::MAX as u64,
            (u32::MAX as u64) + 1,
            u64::MAX,
        ];
        for &v in &values {
            let mut buf = Vec::new();
            encode(v, &mut buf);
            let mut pos = 0;
            let decoded = decode(&buf, &mut pos).expect("boundary decode should succeed");
            assert_eq!(decoded, v, "boundary roundtrip failed for {v}");
            assert_eq!(pos, buf.len());
        }
    }
}
