//! LEB128 unsigned varint codec used by the per-term posting lists.
//!
//! The encoder appends `1..=5` bytes for any `u32`. Each byte stores 7
//! payload bits plus a continuation bit in the MSB; the high-bit-clear byte
//! terminates the value. The decoder consumes the same byte run and reports
//! how many bytes it took, so callers can chain reads through a single
//! buffer without intermediate copies.
//!
//! Failure modes:
//!
//! - truncated input (continuation bit set on the last available byte) →
//!   [`PositionsErrorCode::IndexCorrupted`]
//! - a 6th continuation byte (would overflow `u32`) →
//!   [`PositionsErrorCode::IndexCorrupted`]
//!
//! These are corruption signals; the disk shape never legitimately emits a
//! `> 5` byte sequence for a `u32`.

use crate::errors::{PositionsError, PositionsErrorCode};

/// Maximum number of bytes any `u32` can encode to under LEB128.
const MAX_U32_VARINT_BYTES: usize = 5;

/// Encode `v` as LEB128 unsigned varint, appending to `out`.
pub fn encode_u32(mut v: u32, out: &mut Vec<u8>) {
    while v >= 0x80 {
        // Payload byte: low 7 bits + continuation bit.
        let lo = u32_low_byte(v & 0x7f) | 0x80;
        out.push(lo);
        v >>= 7;
    }
    // Final byte: top bit clear.
    out.push(u32_low_byte(v));
}

/// Narrow a `u32` whose value is known to fit in `u8`.
///
/// Used inside the encoder where the masked payload `v & 0x7f` is always
/// `<= 0x7f`, or where the loop has exited with `v < 0x80`. We funnel the
/// `as u8` cast through this helper so the `expect` lives in one spot.
fn u32_low_byte(v: u32) -> u8 {
    #[expect(
        clippy::as_conversions,
        reason = "no stable safe converter narrows a u32 whose value is known <= 0xff into a u8 without runtime branching"
    )]
    let b = (v & 0xff) as u8;
    b
}

/// Decode one LEB128 unsigned varint from `input`.
///
/// Returns `(value, bytes_consumed)` on success, or a typed error on
/// truncation / overflow. The caller advances its cursor by the
/// `bytes_consumed` return.
pub fn decode_u32(input: &[u8]) -> Result<(u32, usize), PositionsError> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    for (i, byte_ref) in input.iter().enumerate().take(MAX_U32_VARINT_BYTES) {
        let byte = *byte_ref;
        let payload = u32::from(byte & 0x7f);
        // shift is bounded to 0..=28 (MAX_U32_VARINT_BYTES - 1) * 7 = 28.
        // Shifting a 7-bit payload by 28 fits in u32 unless the 5th byte's
        // payload exceeds 4 bits (= 0x0f). Reject the overflow case below.
        if i == MAX_U32_VARINT_BYTES.saturating_sub(1) && (byte & 0x7f) > 0x0f {
            return Err(PositionsError::new(
                PositionsErrorCode::IndexCorrupted,
                "varint payload overflows u32",
            ));
        }
        let shifted = payload.checked_shl(shift).ok_or_else(|| {
            PositionsError::new(PositionsErrorCode::IndexCorrupted, "varint shift overflow")
        })?;
        result = result.checked_add(shifted).ok_or_else(|| {
            PositionsError::new(
                PositionsErrorCode::IndexCorrupted,
                "varint payload overflows u32",
            )
        })?;
        if byte & 0x80 == 0 {
            return Ok((result, i.saturating_add(1)));
        }
        shift = shift.saturating_add(7);
    }
    Err(PositionsError::new(
        PositionsErrorCode::IndexCorrupted,
        "varint stream truncated or exceeded u32 capacity",
    ))
}

#[cfg(test)]
mod tests {
    use super::{decode_u32, encode_u32};
    use crate::errors::PositionsErrorCode;

    fn roundtrip(v: u32) {
        let mut buf: Vec<u8> = Vec::new();
        encode_u32(v, &mut buf);
        match decode_u32(&buf) {
            Ok((got, consumed)) => {
                assert_eq!(got, v, "value roundtrip");
                assert_eq!(consumed, buf.len(), "consumed all bytes");
            }
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn roundtrip_small() {
        for v in [0u32, 1, 2, 0x7f, 0x80, 0x3fff, 0x4000] {
            roundtrip(v);
        }
    }

    #[test]
    fn roundtrip_large() {
        for v in [
            0xffff,
            0x1f_ffff,
            0x0fff_ffff,
            0x1fff_ffff,
            0x7fff_ffffu32,
            u32::MAX,
        ] {
            roundtrip(v);
        }
    }

    #[test]
    fn chained_decode_walks_buffer() {
        let mut buf: Vec<u8> = Vec::new();
        let inputs: [u32; 5] = [0, 1, 0x7f, 0x80, 0xffff_ffff];
        for v in inputs {
            encode_u32(v, &mut buf);
        }
        let mut cursor: usize = 0;
        let mut got: Vec<u32> = Vec::new();
        while cursor < buf.len() {
            let Some(rest) = buf.get(cursor..) else {
                assert!(false, "cursor past end");
                break;
            };
            match decode_u32(rest) {
                Ok((v, n)) => {
                    got.push(v);
                    cursor = cursor.saturating_add(n);
                }
                Err(e) => {
                    assert!(false, "{e}");
                    break;
                }
            }
        }
        assert_eq!(got, inputs);
    }

    #[test]
    fn decode_truncated_returns_corrupted() {
        // 0x80 alone has continuation bit set but no terminator.
        let buf = [0x80u8];
        match decode_u32(&buf) {
            Ok(_) => assert!(false, "truncated must fail"),
            Err(e) => assert_eq!(e.code, PositionsErrorCode::IndexCorrupted),
        }
    }

    #[test]
    fn decode_empty_returns_corrupted() {
        match decode_u32(&[]) {
            Ok(_) => assert!(false, "empty must fail"),
            Err(e) => assert_eq!(e.code, PositionsErrorCode::IndexCorrupted),
        }
    }

    #[test]
    fn decode_overlong_returns_corrupted() {
        // 6 continuation bytes — exceeds u32 capacity.
        let buf = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        match decode_u32(&buf) {
            Ok(_) => assert!(false, "overlong must fail"),
            Err(e) => assert_eq!(e.code, PositionsErrorCode::IndexCorrupted),
        }
    }

    #[test]
    fn decode_fifth_byte_payload_overflow() {
        // 4 continuation bytes accumulating then a 5th byte whose payload
        // bit-shifted by 28 would overflow u32: payload > 0x0f.
        let buf = [0x80, 0x80, 0x80, 0x80, 0x10];
        match decode_u32(&buf) {
            Ok(_) => assert!(false, "5th-byte overflow must fail"),
            Err(e) => assert_eq!(e.code, PositionsErrorCode::IndexCorrupted),
        }
    }
}
