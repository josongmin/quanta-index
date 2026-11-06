//! One private Catalog encoding for persisted registration metadata.
//!
//! Public `RegisteredObject` JSON and immutable object-file bytes are unrelated.

use quanta_index_core::index::{Digest, IndexError, ObjectRef, RegisteredObject};

pub(super) const VERSION: i64 = 1;
pub(super) const MAX_CHILDREN: usize = 4096;
pub(super) const MAX_OBJECT_BYTES: u64 = 16 * 1024 * 1024 + 4096 * 32 + 16;
const HEADER_BYTES: usize = 32 + 8 + 4;
pub(super) const MAX_ENCODED_BYTES: usize = HEADER_BYTES + MAX_CHILDREN * 32;

fn malformed() -> IndexError {
    IndexError::Storage("native registration payload shape invalid".into())
}

pub(super) fn encode(object: &RegisteredObject) -> Result<Vec<u8>, IndexError> {
    if object.bytes == 0 || object.bytes > MAX_OBJECT_BYTES || object.children.len() > MAX_CHILDREN
    {
        return Err(IndexError::ResourceLimited);
    }
    if object.children.contains(&object.reference) {
        return Err(IndexError::InvalidInput("self object edge".into()));
    }
    let length = object
        .children
        .len()
        .checked_mul(32)
        .and_then(|children| HEADER_BYTES.checked_add(children))
        .ok_or(IndexError::ResourceLimited)?;
    let count =
        u32::try_from(object.children.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(length)
        .map_err(|_error| IndexError::ResourceLimited)?;
    payload.extend_from_slice(&object.reference.0.0);
    payload.extend_from_slice(&object.bytes.to_be_bytes());
    payload.extend_from_slice(&count.to_be_bytes());
    for child in &object.children {
        payload.extend_from_slice(&child.0.0);
    }
    Ok(payload)
}

/// Decode only the bounded structural artifact. The caller retains semantic
/// authority for byte bounds, self-edges and lifecycle state.
pub(super) fn decode(payload: &[u8]) -> Result<RegisteredObject, IndexError> {
    let header = payload.get(..HEADER_BYTES).ok_or_else(malformed)?;
    let reference = ObjectRef(Digest(
        <[u8; 32]>::try_from(header.get(..32).ok_or_else(malformed)?)
            .map_err(|_error| malformed())?,
    ));
    let bytes = u64::from_be_bytes(
        <[u8; 8]>::try_from(header.get(32..40).ok_or_else(malformed)?)
            .map_err(|_error| malformed())?,
    );
    let count = usize::try_from(u32::from_be_bytes(
        <[u8; 4]>::try_from(header.get(40..44).ok_or_else(malformed)?)
            .map_err(|_error| malformed())?,
    ))
    .map_err(|_error| malformed())?;
    if count > MAX_CHILDREN {
        return Err(malformed());
    }
    let expected = count
        .checked_mul(32)
        .and_then(|children| HEADER_BYTES.checked_add(children))
        .ok_or_else(malformed)?;
    if payload.len() != expected {
        return Err(malformed());
    }
    let mut children = Vec::new();
    children
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for chunk in payload
        .get(HEADER_BYTES..)
        .ok_or_else(malformed)?
        .chunks_exact(32)
    {
        let child = ObjectRef(Digest(
            <[u8; 32]>::try_from(chunk).map_err(|_error| malformed())?,
        ));
        children.push(child);
    }
    Ok(RegisteredObject::unscoped(reference, bytes, children))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(condition: bool) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "registration codec golden differs".into(),
            ))
        }
    }

    #[test]
    fn independent_fixed_bytes_and_ordered_duplicates() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([0x11; 32]));
        let second = ObjectRef(Digest([0x22; 32]));
        let third = ObjectRef(Digest([0x33; 32]));
        let object = RegisteredObject::unscoped(reference, 0x01_02_03, vec![second, second, third]);
        let mut golden = vec![0x11; 32];
        golden.extend_from_slice(&[0, 0, 0, 0, 0, 1, 2, 3]);
        golden.extend_from_slice(&[0, 0, 0, 3]);
        golden.extend_from_slice(&[0x22; 32]);
        golden.extend_from_slice(&[0x22; 32]);
        golden.extend_from_slice(&[0x33; 32]);
        check(encode(&object)? == golden)?;
        check(decode(&golden)? == object)?;
        let public_json =
            serde_json::to_vec(&object).map_err(|error| IndexError::Storage(error.to_string()))?;
        check(public_json != golden)?;
        check(matches!(decode(&public_json), Err(IndexError::Storage(_))))?;
        check(
            serde_json::from_slice::<RegisteredObject>(&public_json)
                .map_err(|error| IndexError::Storage(error.to_string()))?
                == object,
        )
    }

    #[test]
    fn malformed_length_and_count_refuse() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([0x11; 32]));
        let child = ObjectRef(Digest([0x22; 32]));
        let valid = encode(&RegisteredObject::unscoped(reference, 1, vec![child]))?;
        for payload in [
            valid.get(..43).ok_or_else(malformed)?.to_vec(),
            valid.get(..75).ok_or_else(malformed)?.to_vec(),
            {
                let mut value = valid.clone();
                value.push(0);
                value
            },
            {
                let mut value = valid.clone();
                value
                    .get_mut(40..44)
                    .ok_or_else(malformed)?
                    .copy_from_slice(&[0, 0, 16, 1]);
                value
            },
        ] {
            check(matches!(decode(&payload), Err(IndexError::Storage(_))))?;
        }
        Ok(())
    }

    #[test]
    fn semantic_values_survive_decode_for_existing_consumer_checks() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([0x11; 32]));
        let child = ObjectRef(Digest([0x22; 32]));
        let valid = encode(&RegisteredObject::unscoped(reference, 1, vec![child]))?;
        for (bytes, edge) in [(0_u64, child), (16_908_305_u64, child), (1_u64, reference)] {
            let mut payload = valid.clone();
            payload
                .get_mut(32..40)
                .ok_or_else(malformed)?
                .copy_from_slice(&bytes.to_be_bytes());
            payload
                .get_mut(44..76)
                .ok_or_else(malformed)?
                .copy_from_slice(&edge.0.0);
            let decoded = decode(&payload)?;
            check(decoded.reference == reference)?;
            check(decoded.bytes == bytes)?;
            check(decoded.children == vec![edge])?;
        }
        check(matches!(
            encode(&RegisteredObject::unscoped(reference, 1, vec![reference])),
            Err(IndexError::InvalidInput(_))
        ))?;
        check(matches!(
            encode(&RegisteredObject::unscoped(reference, 0, vec![])),
            Err(IndexError::ResourceLimited)
        ))?;
        check(matches!(
            encode(&RegisteredObject::unscoped(reference, 16_908_305, vec![])),
            Err(IndexError::ResourceLimited)
        ))
    }

    #[test]
    fn exact_child_and_object_bounds_roundtrip_without_reordering() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([0x11; 32]));
        let child = ObjectRef(Digest([0x22; 32]));
        let maximum = RegisteredObject::unscoped(reference, 16_908_304, vec![child; 4096]);
        let payload = encode(&maximum)?;
        check(payload.len() == 131_116)?;
        check(decode(&payload)? == maximum)?;
        check(matches!(
            encode(&RegisteredObject::unscoped(reference, 1, vec![child; 4097])),
            Err(IndexError::ResourceLimited)
        ))
    }
}
