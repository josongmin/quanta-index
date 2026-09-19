//! Decoding the payloads a lexical channel op carries.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{BatchIngestMode, ChunkRecord, ManifestGeneration};
use quanta_index_core::CoreError;

pub(crate) fn decode_chunk_payload(bytes: &[u8]) -> Result<ChunkRecord, CoreError> {
    ciborium::from_reader::<ChunkRecord, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: chunk payload decode: {err}")))
}

pub(crate) fn decode_symbol_payload(bytes: &[u8]) -> Result<SymbolRecord, CoreError> {
    ciborium::from_reader::<SymbolRecord, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: symbol payload decode: {err}")))
}

/// A length or a count as `u64`; a platform where `usize` exceeds `u64`
/// is refused rather than truncated.
pub(crate) fn count_from_len(value: usize) -> Result<u64, CoreError> {
    u64::try_from(value)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: count overflow: {err}")))
}

pub(crate) fn encode_cbor<T>(value: &T, label: &str) -> Result<Vec<u8>, CoreError>
where
    T: serde::Serialize,
{
    let mut payload = Vec::new();
    ciborium::into_writer(value, &mut payload)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: encode {label}: {err}")))?;
    Ok(payload)
}

pub(crate) fn decode_replace_scope_payload(
    bytes: &[u8],
) -> Result<
    (
        BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusReplaceScope,
    ),
    CoreError,
> {
    ciborium::from_reader::<
        (
            BatchIngestMode,
            Option<ManifestGeneration>,
            quanta_index_contract::SearchCorpusReplaceScope,
        ),
        _,
    >(bytes)
    .map_err(|err| {
        CoreError::InvalidContract(format!("lexical: replace scope payload decode: {err}"))
    })
}

pub(crate) fn decode_tombstone_scope_payload(
    bytes: &[u8],
) -> Result<
    (
        BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusTombstoneScope,
    ),
    CoreError,
> {
    ciborium::from_reader::<
        (
            BatchIngestMode,
            Option<ManifestGeneration>,
            quanta_index_contract::SearchCorpusTombstoneScope,
        ),
        _,
    >(bytes)
    .map_err(|err| {
        CoreError::InvalidContract(format!("lexical: tombstone scope payload decode: {err}"))
    })
}
