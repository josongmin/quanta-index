//! ONE canonical bounded lexical object container decoder. No pool is opened.
use super::object_codec::{ObjectKind, ObjectRef};
use quanta_index_core::CoreError;
pub(crate) const OBJECT_MAGIC: &[u8; 8] = b"QLSTOR01";
pub(crate) const OBJECT_FORMAT: u16 = 1;
pub(crate) const MAX_OBJECT_PAYLOAD: usize = 64 * 1024;
pub(crate) const MAX_OBJECT_BYTES: usize = 96 * 1024;
pub(crate) const MAX_OBJECT_DEPS: usize = 128;
pub(crate) fn strictly_sorted(values: &[ObjectRef]) -> bool {
    values
        .windows(2)
        .all(|pair| match (pair.first(), pair.get(1)) {
            (Some(a), Some(b)) => a < b,
            _ => false,
        })
}

pub(crate) fn decode_ref(bytes: &[u8], offset: &mut usize) -> Result<ObjectRef, CoreError> {
    let end = offset
        .checked_add(41)
        .ok_or_else(|| corrupt("reference offset overflow"))?;
    let slice = bytes
        .get(*offset..end)
        .ok_or_else(|| corrupt("truncated object reference"))?;
    *offset = end;
    let kind = ObjectKind::from_code(
        *slice
            .first()
            .ok_or_else(|| corrupt("empty object reference"))?,
    )
    .ok_or_else(|| corrupt("unknown object reference kind"))?;
    let mut width = [0_u8; 8];
    width.copy_from_slice(
        slice
            .get(1..9)
            .ok_or_else(|| corrupt("truncated reference width"))?,
    );
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(
        slice
            .get(9..41)
            .ok_or_else(|| corrupt("truncated reference digest"))?,
    );
    Ok(ObjectRef {
        kind,
        bytes: u64::from_be_bytes(width),
        sha256,
    })
}

pub(crate) fn decode_object(
    bytes: &[u8],
) -> Result<(ObjectKind, Vec<ObjectRef>, Vec<u8>), CoreError> {
    if bytes.len() < 21 || bytes.get(..8) != Some(OBJECT_MAGIC.as_slice()) {
        return Err(corrupt("bad object header"));
    }
    let format = u16::from_be_bytes(
        bytes
            .get(8..10)
            .ok_or_else(|| corrupt("truncated format"))?
            .try_into()
            .map_err(|_conversion_error| corrupt("format width"))?,
    );
    if format != OBJECT_FORMAT {
        return Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
            message: format!(
                "lexical object store: unsupported persisted object format {format}; this build supports {OBJECT_FORMAT}"
            ),
        });
    }
    let kind = ObjectKind::from_code(
        *bytes
            .get(10)
            .ok_or_else(|| corrupt("truncated object kind"))?,
    )
    .ok_or_else(|| corrupt("unknown object kind"))?;
    let count = usize::from(u16::from_be_bytes(
        bytes
            .get(11..13)
            .ok_or_else(|| corrupt("truncated dependency count"))?
            .try_into()
            .map_err(|_conversion_error| corrupt("dependency count width"))?,
    ));
    let mut payload_len_bytes = [0_u8; 8];
    payload_len_bytes.copy_from_slice(
        bytes
            .get(13..21)
            .ok_or_else(|| corrupt("truncated payload length"))?,
    );
    let payload_len = usize::try_from(u64::from_be_bytes(payload_len_bytes))
        .map_err(|_conversion_error| corrupt("payload length width"))?;
    if count > MAX_OBJECT_DEPS || payload_len > MAX_OBJECT_PAYLOAD {
        return Err(corrupt("object bounds exceeded"));
    }
    let expected = 21_usize
        .checked_add(
            count
                .checked_mul(41)
                .ok_or_else(|| corrupt("dependency length overflow"))?,
        )
        .and_then(|value| value.checked_add(payload_len))
        .ok_or_else(|| corrupt("object length overflow"))?;
    if bytes.len() != expected {
        return Err(corrupt("object encoded length differs"));
    }
    let mut cursor = 21;
    let mut deps = Vec::new();
    deps.try_reserve_exact(count)
        .map_err(|error| storage("reserve dependencies", error))?;
    for _ in 0..count {
        deps.push(decode_ref(bytes, &mut cursor)?);
    }
    if !strictly_sorted(&deps) {
        return Err(corrupt("object dependencies are not canonical"));
    }
    Ok((
        kind,
        deps,
        bytes
            .get(cursor..)
            .ok_or_else(|| corrupt("payload cursor beyond object"))?
            .to_vec(),
    ))
}

pub(crate) fn storage(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("lexical object store {context}: {error}"))
}

pub(crate) fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!("lexical object store: {reason}"),
    }
}

pub(crate) fn hex_digest(digest: &[u8; 32]) -> String {
    let mut text = String::with_capacity(64);
    for byte in digest {
        for nibble in [byte >> 4, byte & 0x0f] {
            let digit = if nibble < 10 {
                b'0'.saturating_add(nibble)
            } else {
                b'a'.saturating_add(nibble.saturating_sub(10))
            };
            text.push(char::from(digit));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    const GOLDEN: &[u8] = b"QLSTOR01\x00\x01\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03abc";
    #[test]
    fn lexical_object_binary_fixed_container_golden_and_exact_length()
    -> Result<(), Box<dyn std::error::Error>> {
        let (kind, deps, payload) = decode_object(GOLDEN)?;
        assert_eq!(kind, ObjectKind::NativeBody);
        assert!(deps.is_empty());
        assert_eq!(payload, b"abc");
        let mut trailing = GOLDEN.to_vec();
        trailing.push(0);
        assert!(decode_object(&trailing).is_err());
        assert!(decode_object(&GOLDEN[..GOLDEN.len() - 1]).is_err());
        Ok(())
    }
    #[test]
    fn lexical_object_binary_refuses_prior_format_and_unknown_kind() {
        let mut prior = GOLDEN.to_vec();
        prior[9] = 0;
        assert!(matches!(decode_object(&prior), Err(CoreError::Typed {
            code:quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,..})));
        let mut kind = GOLDEN.to_vec();
        kind[10] = 255;
        assert!(decode_object(&kind).is_err());
    }
}

pub(crate) const POOL_DIR: &str = "lexical-object-pool";

pub(crate) const OBJECTS_DIR: &str = "objects";
