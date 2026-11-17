//! Persisted run-page codecs. Old page bytes remain readable and canonical;
//! v3 stores each fixed digest as one CBOR byte string.

use quanta_index_core::CoreError;
use serde::de::Visitor;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

use super::{BoundedVec, MAX_BUCKET_ROWS, RunPostingPage, RunPostingRow, corrupt, invalid};
use crate::channel_payloads::{decode_cbor_exact, encode_cbor};

const V1: u8 = 1;
const V2: u8 = 2;
const V3: u8 = 3;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct DigestBytes([u8; 32]);

impl Serialize for DigestBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

struct DigestDictionary<'a>(&'a [[u8; 32]]);

impl Serialize for DigestDictionary<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for digest in self.0 {
            sequence.serialize_element(&DigestBytes(*digest))?;
        }
        sequence.end()
    }
}

impl<'de> Deserialize<'de> for DigestBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExactDigest;

        impl Visitor<'_> for ExactDigest {
            type Value = DigestBytes;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("exactly 32 digest bytes")
            }

            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                let digest = bytes
                    .try_into()
                    .map_err(|_width| E::custom("run page digest must be 32 bytes"))?;
                Ok(DigestBytes(digest))
            }
        }

        deserializer.deserialize_bytes(ExactDigest)
    }
}

fn dictionary_and_indices(
    page: &RunPostingPage,
) -> Result<(Vec<[u8; 32]>, Vec<(u64, u8)>), CoreError> {
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
    Ok((dictionary, indexed))
}

pub(super) fn encode_v2(page: &RunPostingPage) -> Result<Vec<u8>, CoreError> {
    let (dictionary, indexed) = dictionary_and_indices(page)?;
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

pub(super) fn encode_v3(page: &RunPostingPage) -> Result<Vec<u8>, CoreError> {
    let (dictionary, indexed) = dictionary_and_indices(page)?;
    encode_cbor(
        &(
            V3,
            page.surface,
            page.gram,
            page.bucket,
            DigestDictionary(&dictionary),
            &indexed,
        ),
        "file run posting page v3",
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
        Some([0x86, V3]) => decode_v3(bytes),
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

fn decode_v3(bytes: &[u8]) -> Result<RunPostingPage, CoreError> {
    let (version, surface, gram, bucket, BoundedVec(dictionary), BoundedVec(indexed)): (
        u8,
        u8,
        [u8; 3],
        u64,
        BoundedVec<DigestBytes, MAX_BUCKET_ROWS>,
        BoundedVec<(u64, u8), MAX_BUCKET_ROWS>,
    ) = decode_cbor_exact(bytes).map_err(|error| corrupt(&format!("v3 page decode: {error}")))?;
    if version != V3 || dictionary.is_empty() || dictionary.len() > indexed.len() {
        return Err(corrupt("v3 page version or dictionary count differs"));
    }
    if dictionary.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(corrupt("v3 page dictionary is not uniquely sorted"));
    }
    let mut used = vec![false; dictionary.len()];
    let mut rows = Vec::new();
    rows.try_reserve_exact(indexed.len())
        .map_err(|error| CoreError::Storage(format!("reserve decoded run page: {error}")))?;
    for (logical_id, index) in indexed {
        let position = usize::from(index);
        let digest = dictionary
            .get(position)
            .ok_or_else(|| corrupt("v3 page dictionary index exceeds count"))?;
        used[position] = true;
        rows.push(RunPostingRow {
            logical_id,
            segment_sha256: digest.0,
        });
    }
    if used.iter().any(|used| !used) {
        return Err(corrupt("v3 page has unused dictionary digest"));
    }
    let page = RunPostingPage {
        surface,
        gram,
        bucket,
        rows,
    };
    page.validate()
        .map_err(|error| corrupt(&error.to_string()))?;
    if encode_v3(&page)? != bytes {
        return Err(corrupt("v3 page encoding is not canonical"));
    }
    Ok(page)
}

#[cfg(test)]
pub(super) fn encode_legacy_v1_for_test(page: &RunPostingPage) -> Result<Vec<u8>, CoreError> {
    encode_v1(page)
}
