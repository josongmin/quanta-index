//! Pure immutable-object wire identity shared by the store and Native producers.

use super::{Digest, ObjectRef};
use sha2::{Digest as _, Sha256};

pub const NATIVE_IMMUTABLE_OBJECT_MAGIC_V1: &[u8; 8] = b"QIOBJ001";
pub const NATIVE_IMMUTABLE_OBJECT_HASH_DOMAIN_V1: &[u8] = b"quanta-index-immutable-object-v1\0";
pub const NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1: usize = 16;

#[must_use]
pub fn native_immutable_object_header_v1(
    payload_bytes: u32,
    child_count: u32,
) -> [u8; NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1] {
    let mut header = [0; NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1];
    header[..8].copy_from_slice(NATIVE_IMMUTABLE_OBJECT_MAGIC_V1);
    header[8..12].copy_from_slice(&payload_bytes.to_be_bytes());
    header[12..].copy_from_slice(&child_count.to_be_bytes());
    header
}

/// Compute an immutable identity with the supplied child order. This only
/// describes bytes; the caller still pays for hash work and Q still checks
/// physical custody, registry rules and the source admission.
#[must_use]
pub fn native_immutable_object_ref_v1(payload: &[u8], children: &[ObjectRef]) -> Option<ObjectRef> {
    let mut digest = native_immutable_object_hasher_v1(payload.len(), children)?;
    digest.update(payload);
    Some(ObjectRef(Digest(digest.finalize().into())))
}

pub(super) fn native_immutable_object_hasher_v1(
    payload_bytes: usize,
    children: &[ObjectRef],
) -> Option<Sha256> {
    let (Ok(payload_bytes), Ok(child_count)) =
        (u32::try_from(payload_bytes), u32::try_from(children.len()))
    else {
        return None;
    };
    let header = native_immutable_object_header_v1(payload_bytes, child_count);
    let mut digest = Sha256::new();
    digest.update(NATIVE_IMMUTABLE_OBJECT_HASH_DOMAIN_V1);
    digest.update(header);
    for child in children {
        digest.update((child.0).0);
    }
    Some(digest)
}
