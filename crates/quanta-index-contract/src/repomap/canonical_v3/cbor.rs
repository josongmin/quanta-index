//! Exact definite canonical CBOR framing and digest helpers for layout V3.

use sha2::{Digest as _, Sha256};

use super::CanonicalRepoMapCodecErrorV1;

pub(super) struct Decoder<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) offset: usize,
}

impl<'a> Decoder<'a> {
    pub(super) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(super) fn finish(&self) -> Result<(), CanonicalRepoMapCodecErrorV1> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(CanonicalRepoMapCodecErrorV1::TrailingBytes)
        }
    }

    pub(super) fn one(&mut self) -> Result<u8, CanonicalRepoMapCodecErrorV1> {
        let Some(byte) = self.bytes.get(self.offset).copied() else {
            return Err(CanonicalRepoMapCodecErrorV1::UnexpectedEnd);
        };
        self.offset = self.offset.saturating_add(1);
        Ok(byte)
    }

    pub(super) fn exact(
        &mut self,
        length: usize,
    ) -> Result<&'a [u8], CanonicalRepoMapCodecErrorV1> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(CanonicalRepoMapCodecErrorV1::LengthOutOfRange)?;
        let Some(value) = self.bytes.get(self.offset..end) else {
            return Err(CanonicalRepoMapCodecErrorV1::UnexpectedEnd);
        };
        self.offset = end;
        Ok(value)
    }

    pub(super) fn len(&mut self, expected_major: u8) -> Result<u64, CanonicalRepoMapCodecErrorV1> {
        let initial = self.one()?;
        if initial >> 5 != expected_major {
            return Err(CanonicalRepoMapCodecErrorV1::WrongType("major_type"));
        }
        let additional = initial & 0x1f;
        match additional {
            value @ 0..=23 => Ok(u64::from(value)),
            24 => {
                let value = u64::from(self.one()?);
                if value < 24 {
                    Err(CanonicalRepoMapCodecErrorV1::NonCanonicalInteger)
                } else {
                    Ok(value)
                }
            }
            25 => {
                let raw = self.exact(2)?;
                let value = u64::from(u16::from_be_bytes(
                    <[u8; 2]>::try_from(raw)
                        .map_err(|_error| CanonicalRepoMapCodecErrorV1::UnexpectedEnd)?,
                ));
                if u8::try_from(value).is_ok() {
                    Err(CanonicalRepoMapCodecErrorV1::NonCanonicalInteger)
                } else {
                    Ok(value)
                }
            }
            26 => {
                let raw = self.exact(4)?;
                let value = u64::from(u32::from_be_bytes(
                    <[u8; 4]>::try_from(raw)
                        .map_err(|_error| CanonicalRepoMapCodecErrorV1::UnexpectedEnd)?,
                ));
                if u16::try_from(value).is_ok() {
                    Err(CanonicalRepoMapCodecErrorV1::NonCanonicalInteger)
                } else {
                    Ok(value)
                }
            }
            27 => {
                let raw = self.exact(8)?;
                let value = u64::from_be_bytes(
                    <[u8; 8]>::try_from(raw)
                        .map_err(|_error| CanonicalRepoMapCodecErrorV1::UnexpectedEnd)?,
                );
                if u32::try_from(value).is_ok() {
                    Err(CanonicalRepoMapCodecErrorV1::NonCanonicalInteger)
                } else {
                    Ok(value)
                }
            }
            _ => Err(CanonicalRepoMapCodecErrorV1::WrongType("definite_length")),
        }
    }

    pub(super) fn expect_len(
        &mut self,
        major: u8,
        expected: u64,
        field: &'static str,
    ) -> Result<(), CanonicalRepoMapCodecErrorV1> {
        if self.len(major)? == expected {
            Ok(())
        } else {
            Err(CanonicalRepoMapCodecErrorV1::InvalidValue(field))
        }
    }

    pub(super) fn uint(&mut self) -> Result<u64, CanonicalRepoMapCodecErrorV1> {
        self.len(0)
    }

    pub(super) fn expect_uint(
        &mut self,
        expected: u64,
        field: &'static str,
    ) -> Result<(), CanonicalRepoMapCodecErrorV1> {
        if self.uint()? == expected {
            Ok(())
        } else {
            Err(CanonicalRepoMapCodecErrorV1::InvalidValue(field))
        }
    }

    pub(super) fn bytes(&mut self) -> Result<&'a [u8], CanonicalRepoMapCodecErrorV1> {
        let length = usize::try_from(self.len(2)?)
            .map_err(|_error| CanonicalRepoMapCodecErrorV1::LengthOutOfRange)?;
        self.exact(length)
    }

    pub(super) fn text(&mut self) -> Result<&'a str, CanonicalRepoMapCodecErrorV1> {
        let length = usize::try_from(self.len(3)?)
            .map_err(|_error| CanonicalRepoMapCodecErrorV1::LengthOutOfRange)?;
        core::str::from_utf8(self.exact(length)?)
            .map_err(|_error| CanonicalRepoMapCodecErrorV1::InvalidUtf8)
    }

    pub(super) fn digest(&mut self) -> Result<[u8; 32], CanonicalRepoMapCodecErrorV1> {
        let bytes = self.bytes()?;
        <[u8; 32]>::try_from(bytes)
            .map_err(|_error| CanonicalRepoMapCodecErrorV1::InvalidValue("digest_width"))
    }

    pub(super) fn nullable_uint(&mut self) -> Result<Option<u64>, CanonicalRepoMapCodecErrorV1> {
        if self.bytes.get(self.offset) == Some(&0xf6) {
            self.offset = self.offset.saturating_add(1);
            Ok(None)
        } else {
            self.uint().map(Some)
        }
    }

    pub(super) fn nullable_digest(
        &mut self,
    ) -> Result<Option<[u8; 32]>, CanonicalRepoMapCodecErrorV1> {
        if self.bytes.get(self.offset) == Some(&0xf6) {
            self.offset = self.offset.saturating_add(1);
            Ok(None)
        } else {
            self.digest().map(Some)
        }
    }
}

pub(super) fn push_key(bytes: &mut Vec<u8>, key: u64) {
    push_major(bytes, 0, key);
}

pub(super) fn push_uint_pair(bytes: &mut Vec<u8>, key: u64, value: u64) {
    push_key(bytes, key);
    push_major(bytes, 0, value);
}

pub(super) fn push_map_len(bytes: &mut Vec<u8>, length: u64) {
    push_major(bytes, 5, length);
}

pub(super) fn push_array_len(bytes: &mut Vec<u8>, length: u64) {
    push_major(bytes, 4, length);
}

pub(super) fn push_array_len_checked(
    bytes: &mut Vec<u8>,
    length: usize,
) -> Result<(), CanonicalRepoMapCodecErrorV1> {
    let length =
        u64::try_from(length).map_err(|_error| CanonicalRepoMapCodecErrorV1::LengthOutOfRange)?;
    push_array_len(bytes, length);
    Ok(())
}

pub(super) fn push_bytes(
    bytes: &mut Vec<u8>,
    value: &[u8],
) -> Result<(), CanonicalRepoMapCodecErrorV1> {
    let length = u64::try_from(value.len())
        .map_err(|_error| CanonicalRepoMapCodecErrorV1::LengthOutOfRange)?;
    push_major(bytes, 2, length);
    bytes.extend_from_slice(value);
    Ok(())
}

pub(super) fn push_text(
    bytes: &mut Vec<u8>,
    value: &str,
) -> Result<(), CanonicalRepoMapCodecErrorV1> {
    let length = u64::try_from(value.len())
        .map_err(|_error| CanonicalRepoMapCodecErrorV1::LengthOutOfRange)?;
    push_major(bytes, 3, length);
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

pub(super) fn push_nullable_uint(bytes: &mut Vec<u8>, value: Option<u64>) {
    if let Some(value) = value {
        push_major(bytes, 0, value);
    } else {
        bytes.push(0xf6);
    }
}

pub(super) fn push_nullable_digest(
    bytes: &mut Vec<u8>,
    value: Option<[u8; 32]>,
) -> Result<(), CanonicalRepoMapCodecErrorV1> {
    if let Some(value) = value {
        push_bytes(bytes, &value)
    } else {
        bytes.push(0xf6);
        Ok(())
    }
}

fn push_major(bytes: &mut Vec<u8>, major: u8, value: u64) {
    let prefix = major << 5;
    if value <= 23 {
        bytes.push(prefix | value.to_be_bytes()[7]);
    } else if u8::try_from(value).is_ok() {
        bytes.push(prefix | 24);
        bytes.push(value.to_be_bytes()[7]);
    } else if u16::try_from(value).is_ok() {
        bytes.push(prefix | 25);
        bytes.extend_from_slice(&value.to_be_bytes()[6..]);
    } else if u32::try_from(value).is_ok() {
        bytes.push(prefix | 26);
        bytes.extend_from_slice(&value.to_be_bytes()[4..]);
    } else {
        bytes.push(prefix | 27);
        bytes.extend_from_slice(&value.to_be_bytes());
    }
}

/// Infallible by construction: SHA-256 over arbitrary bytes cannot fail.
pub(super) fn plain_digest(payload: &[u8]) -> [u8; 32] {
    Sha256::digest(payload).into()
}

/// Infallible by construction: SHA-256 over the domain-framed payload cannot fail.
pub(super) fn domain_digest(domain: &str, payload: &[u8]) -> [u8; 32] {
    let domain_length = domain.len().to_be_bytes();
    let mut hasher = Sha256::new();
    let [_, _, _, _, a, b, c, d] = domain_length;
    hasher.update([a, b, c, d]);
    hasher.update(domain.as_bytes());
    hasher.update(payload);
    hasher.finalize().into()
}

#[expect(
    clippy::indexing_slicing,
    reason = "indices are masked to 4 bits, so both HEX lookups are provably in bounds"
)]
pub(super) fn digest_wire_string(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut wire = String::with_capacity(71);
    wire.push_str("sha256:");
    for byte in digest {
        // Indices are masked to 4 bits, so both lookups are provably in bounds.
        wire.push(char::from(HEX[usize::from(byte >> 4)]));
        wire.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    wire
}

pub(super) fn decode_digest_wire_string(
    value: &str,
) -> Result<[u8; 32], CanonicalRepoMapCodecErrorV1> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(CanonicalRepoMapCodecErrorV1::InvalidDigestText);
    };
    if hex.len() != 64 {
        return Err(CanonicalRepoMapCodecErrorV1::InvalidDigestText);
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        let pair = <[u8; 2]>::try_from(pair)
            .map_err(|_error| CanonicalRepoMapCodecErrorV1::UnexpectedEnd)?;
        let high = lowercase_hex_nibble(pair[0])?;
        let low = lowercase_hex_nibble(pair[1])?;
        *digest
            .get_mut(index)
            .ok_or(CanonicalRepoMapCodecErrorV1::UnexpectedEnd)? = (high << 4) | low;
    }
    Ok(digest)
}

fn lowercase_hex_nibble(value: u8) -> Result<u8, CanonicalRepoMapCodecErrorV1> {
    match value {
        b'0'..=b'9' => Ok(value.saturating_sub(b'0')),
        b'a'..=b'f' => Ok(value.saturating_sub(b'a').saturating_add(10)),
        _ => Err(CanonicalRepoMapCodecErrorV1::InvalidDigestText),
    }
}
