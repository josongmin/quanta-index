//! Canonical blob and packed-record decoding without a writable object store.
use super::{
    object_binary::{corrupt, decode_ref, storage},
    object_codec::{ObjectKind, ObjectRef},
};
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};
pub(crate) const BLOCK_BYTES: usize = 32 * 1024;
pub(crate) const MAX_PACKED_RECORDS: usize = 96;
pub(crate) const NODE_FANOUT: usize = 64;
pub(crate) fn packed_dependencies_valid(kind: ObjectKind, dependencies: &[ObjectRef]) -> bool {
    match (kind, dependencies) {
        (ObjectKind::Record | ObjectKind::NativeBody | ObjectKind::FileBody, []) => true,
        (ObjectKind::Record, [body]) => {
            matches!(body.kind, ObjectKind::NativeBody | ObjectKind::FileBody)
        }
        _ => false,
    }
}

/// An authenticated, bounded index into one immutable Record payload.
/// Offsets are relative to the payload after its tag byte.
pub(crate) struct RecordGroupIndex {
    entries: Vec<([u8; 32], std::ops::Range<usize>)>,
    payload_bytes: usize,
}

impl RecordGroupIndex {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        if bytes.len().checked_add(1).is_none_or(|n| n > BLOCK_BYTES) {
            return Err(corrupt("packed record exceeds physical bound"));
        }
        let count_bytes: [u8; 2] = bytes
            .get(..2)
            .ok_or_else(|| corrupt("packed count missing"))?
            .try_into()
            .map_err(|e| storage("packed count width", e))?;
        let count = usize::from(u16::from_be_bytes(count_bytes));
        if !(2..=MAX_PACKED_RECORDS).contains(&count) {
            return Err(corrupt("packed record count differs"));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|e| storage("reserve packed index", e))?;
        let mut offset = 2_usize;
        let mut previous = None;
        for _ in 0..count {
            let end = offset
                .checked_add(2)
                .ok_or_else(|| corrupt("packed width overflow"))?;
            let length: [u8; 2] = bytes
                .get(offset..end)
                .ok_or_else(|| corrupt("packed width missing"))?
                .try_into()
                .map_err(|e| storage("packed width", e))?;
            let length = usize::from(u16::from_be_bytes(length));
            offset = end;
            let end = offset
                .checked_add(length)
                .ok_or_else(|| corrupt("packed extent overflow"))?;
            let record = bytes
                .get(offset..end)
                .ok_or_else(|| corrupt("packed entry truncated"))?;
            let digest: [u8; 32] = Sha256::digest(record).into();
            if previous.is_some_and(|prior| prior >= digest) {
                return Err(corrupt("packed records are not uniquely digest ordered"));
            }
            previous = Some(digest);
            entries.push((digest, offset..end));
            offset = end;
        }
        if offset != bytes.len() {
            return Err(corrupt("packed record has trailing bytes"));
        }
        Ok(Self {
            entries,
            payload_bytes: bytes.len(),
        })
    }

    #[cfg(feature = "legacy-tantivy")]
    pub(crate) fn charge(&self) -> Option<usize> {
        self.entries
            .capacity()
            .checked_mul(std::mem::size_of::<([u8; 32], std::ops::Range<usize>)>())?
            .checked_add(std::mem::size_of::<Self>())?
            .checked_add(std::mem::size_of::<[usize; 2]>())
    }

    pub(crate) fn select<'a>(
        &self,
        bytes: &'a [u8],
        expected_length: u64,
        digest: &[u8; 32],
    ) -> Result<&'a [u8], CoreError> {
        if bytes.len() != self.payload_bytes {
            return Err(corrupt("packed cached payload width differs"));
        }
        let index = self
            .entries
            .binary_search_by_key(digest, |(digest, _)| *digest)
            .map_err(|_absent| corrupt("packed root does not contain logical digest"))?;
        let (_, range) = self
            .entries
            .get(index)
            .ok_or_else(|| corrupt("packed index disappeared"))?;
        let selected = bytes
            .get(range.clone())
            .ok_or_else(|| corrupt("packed cached extent differs"))?;
        if u64::try_from(selected.len()).map_err(|e| storage("packed logical width", e))?
            != expected_length
        {
            return Err(corrupt("packed logical length differs"));
        }
        Ok(selected)
    }
}

pub(crate) fn decode_blob_node(bytes: &[u8]) -> Result<Vec<(ObjectRef, u64)>, CoreError> {
    let count_bytes = bytes
        .get(..2)
        .ok_or_else(|| corrupt("blob node count is truncated"))?;
    let count = usize::from(u16::from_be_bytes(
        count_bytes
            .try_into()
            .map_err(|_conversion_error| corrupt("blob child count width"))?,
    ));
    if count == 0
        || count > NODE_FANOUT
        || bytes.len()
            != 2_usize
                .checked_add(
                    count
                        .checked_mul(49)
                        .ok_or_else(|| corrupt("blob node length overflow"))?,
                )
                .ok_or_else(|| corrupt("blob node length overflow"))?
    {
        return Err(corrupt("blob node count or length differs"));
    }
    let mut offset = 2;
    let mut children = Vec::new();
    children
        .try_reserve_exact(count)
        .map_err(|error| storage("reserve blob children", error))?;
    for _ in 0..count {
        let reference = decode_ref(bytes, &mut offset)?;
        let end = offset
            .checked_add(8)
            .ok_or_else(|| corrupt("blob child offset overflow"))?;
        let mut width = [0_u8; 8];
        width.copy_from_slice(
            bytes
                .get(offset..end)
                .ok_or_else(|| corrupt("truncated blob child length"))?,
        );
        offset = end;
        let length = u64::from_be_bytes(width);
        if length == 0 {
            return Err(corrupt("zero-length blob child"));
        }
        children.push((reference, length));
    }
    Ok(children)
}
