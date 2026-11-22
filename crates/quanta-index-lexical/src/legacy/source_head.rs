//! Canonical physical Source head codec, shared by Legacy writer and Native recovery.
use super::{
    file_root::{corrupt, invalid},
    object_codec::BlobRef,
};
use crate::channel_payloads::{decode_cbor_exact, encode_cbor};
use quanta_index_contract::{SourceFileCoverage, SourceFileKey};
use quanta_index_core::CoreError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};
pub(crate) const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_RECORD_BYTES: usize = 32 * 1024;
/// One current source and its exact ingress coverage commitment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceHead {
    pub(crate) coverage: SourceFileCoverage,
    pub(crate) body: BlobRef,
    pub(crate) logical_id: u64,
    pub(crate) source_bytes: u64,
    pub(crate) indexed_text_bytes: u64,
    pub(crate) folded_text_bytes: u64,
    pub(crate) posting_memberships: u64,
    pub(crate) derived_input_sha256: [u8; 32],
}

impl Serialize for SourceHead {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            &self.coverage,
            &self.body,
            self.logical_id,
            self.source_bytes,
            self.indexed_text_bytes,
            self.folded_text_bytes,
            self.posting_memberships,
            self.derived_input_sha256,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourceHead {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (
            coverage,
            body,
            logical_id,
            source_bytes,
            indexed_text_bytes,
            folded_text_bytes,
            posting_memberships,
            derived_input_sha256,
        ) = <(
            SourceFileCoverage,
            BlobRef,
            u64,
            u64,
            u64,
            u64,
            u64,
            [u8; 32],
        )>::deserialize(deserializer)?;
        Ok(Self {
            coverage,
            body,
            logical_id,
            source_bytes,
            indexed_text_bytes,
            folded_text_bytes,
            posting_memberships,
            derived_input_sha256,
        })
    }
}

impl SourceHead {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        self.coverage
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        let max_source_bytes = u64::try_from(MAX_FILE_BYTES)
            .map_err(|error| invalid(&format!("source byte limit width: {error}")))?;
        if self.logical_id == 0
            || self.body.bytes != self.source_bytes
            || self.body.sha256 != self.coverage.source.source_sha256
            || self.source_bytes > max_source_bytes
            || self.posting_memberships > super::file_policy::policy().total_memberships
        {
            return Err(invalid("source head body, ID or membership differs"));
        }
        Ok(())
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let head: Self = decode_bounded(bytes, MAX_RECORD_BYTES, "source head")?;
        head.validate()
            .map_err(|error| corrupt(&error.to_string()))?;
        Ok(head)
    }
}

pub(crate) fn source_key_digest(key: &SourceFileKey) -> Result<[u8; 32], CoreError> {
    key.validate().map_err(invalid)?;
    let encoded = encode_cbor(key, "file source key")?;
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-file-source-key-v17\0");
    hasher.update(encoded);
    Ok(hasher.finalize().into())
}

fn decode_bounded<T: for<'de> Deserialize<'de> + Serialize>(
    bytes: &[u8],
    ceiling: usize,
    name: &str,
) -> Result<T, CoreError> {
    if bytes.len() > ceiling {
        return Err(corrupt(&format!("{name} exceeds byte admission")));
    }
    let value: T =
        decode_cbor_exact(bytes).map_err(|error| corrupt(&format!("{name} decode: {error}")))?;
    let canonical = encode_cbor(&value, name)?;
    if canonical != bytes {
        return Err(corrupt(&format!("{name} encoding is not canonical")));
    }
    Ok(value)
}
