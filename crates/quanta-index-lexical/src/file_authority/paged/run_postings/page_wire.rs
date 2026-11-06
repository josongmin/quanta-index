//! Persisted run-page codecs. v1 remains readable in its original encoding;
//! new pages use a canonical dictionary of segment digests.

use quanta_index_core::CoreError;

use super::{BoundedVec, MAX_BUCKET_ROWS, RunPostingPage, RunPostingRow, corrupt, invalid};
use crate::channel_payloads::{decode_cbor_exact, encode_cbor};

const V1: u8 = 1;
const V2: u8 = 2;

pub(super) fn encode_v2(page: &RunPostingPage) -> Result<Vec<u8>, CoreError> {
    let mut dictionary = page
        .rows
        .iter()
        .map(|row| row.segment_sha256)
        .collect::<Vec<_>>();
    dictionary.sort_unstable();
    dictionary.dedup();
    let mut indexed = Vec::new();
    indexed
        .try_reserve_exact(page.rows.len())
        .map_err(|error| CoreError::Storage(format!("reserve run page indices: {error}")))?;
    for row in &page.rows {
        let position = dictionary
            .binary_search(&row.segment_sha256)
            .map_err(|_missing| invalid("run page digest absent from dictionary"))?;
        let index =
            u8::try_from(position).map_err(|_width| invalid("run page digest index exceeds u8"))?;
        indexed.push((row.logical_id, index));
    }
    // Dictionary and indices cost at most 256*32 + 256*16 = 12 KiB. Merge
    // admission charges their overlap with the retained page and CBOR output;
    // the new-run build budget covers them through its bucket/map reserves.
    encode_cbor(
        &(
            V2,
            page.surface,
            page.gram,
            page.bucket,
            &dictionary,
            &indexed,
        ),
        "file run posting page v2",
    )
}

fn encode_v1(page: &RunPostingPage) -> Result<Vec<u8>, CoreError> {
    encode_cbor(
        &(V1, page.surface, page.gram, page.bucket, &page.rows),
        "file run posting page v1",
    )
}

pub(super) fn decode(bytes: &[u8]) -> Result<RunPostingPage, CoreError> {
    // The canonical five/six-item definite CBOR arrays start with these exact
    // bytes. All alternative outer representations are refused, as the v1
    // reader did through its exact re-encode check.
    match bytes.get(..2) {
        Some([0x85, V1]) => decode_v1(bytes),
        Some([0x86, V2]) => decode_v2(bytes),
        _ => Err(corrupt("page version or outer encoding differs")),
    }
}

fn decode_v1(bytes: &[u8]) -> Result<RunPostingPage, CoreError> {
    let (version, surface, gram, bucket, BoundedVec(rows)): (
        u8,
        u8,
        [u8; 3],
        u64,
        BoundedVec<RunPostingRow, MAX_BUCKET_ROWS>,
    ) = decode_cbor_exact(bytes).map_err(|error| corrupt(&format!("v1 page decode: {error}")))?;
    if version != V1 {
        return Err(corrupt("v1 page version differs"));
    }
    let page = RunPostingPage {
        surface,
        gram,
        bucket,
        rows,
    };
    page.validate()
        .map_err(|error| corrupt(&error.to_string()))?;
    if encode_v1(&page)? != bytes {
        return Err(corrupt("v1 page encoding is not canonical"));
    }
    Ok(page)
}

fn decode_v2(bytes: &[u8]) -> Result<RunPostingPage, CoreError> {
    let (version, surface, gram, bucket, BoundedVec(dictionary), BoundedVec(indexed)): (
        u8,
        u8,
        [u8; 3],
        u64,
        BoundedVec<[u8; 32], MAX_BUCKET_ROWS>,
        BoundedVec<(u64, u8), MAX_BUCKET_ROWS>,
    ) = decode_cbor_exact(bytes).map_err(|error| corrupt(&format!("v2 page decode: {error}")))?;
    if version != V2 || dictionary.is_empty() || dictionary.len() > indexed.len() {
        return Err(corrupt("v2 page version or dictionary count differs"));
    }
    if dictionary.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(corrupt("v2 page dictionary is not uniquely sorted"));
    }
    let mut used = vec![false; dictionary.len()];
    let mut rows = Vec::new();
    rows.try_reserve_exact(indexed.len())
        .map_err(|error| CoreError::Storage(format!("reserve decoded run page: {error}")))?;
    for (logical_id, index) in indexed {
        let position = usize::from(index);
        let digest = dictionary
            .get(position)
            .ok_or_else(|| corrupt("v2 page dictionary index exceeds count"))?;
        used[position] = true;
        rows.push(RunPostingRow {
            logical_id,
            segment_sha256: *digest,
        });
    }
    if used.iter().any(|used| !used) {
        return Err(corrupt("v2 page has unused dictionary digest"));
    }
    let page = RunPostingPage {
        surface,
        gram,
        bucket,
        rows,
    };
    page.validate()
        .map_err(|error| corrupt(&error.to_string()))?;
    if encode_v2(&page)? != bytes {
        return Err(corrupt("v2 page encoding is not canonical"));
    }
    Ok(page)
}

#[cfg(test)]
pub(super) fn encode_legacy_v1_for_test(page: &RunPostingPage) -> Result<Vec<u8>, CoreError> {
    encode_v1(page)
}
