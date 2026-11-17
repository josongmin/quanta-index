//! Canonical persisted lexical filenames and fixed control limits.
pub(crate) const TANTIVY_INDEX_META_FILE_NAME: &str = "meta.json";
pub(crate) const SOURCE_FILE_COVERAGE_FILE_NAME: &str = "source-file-coverage.cbor";
pub(crate) const MAX_INDEX_CONTROL_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const RANKED_KEYS_PREFIX: &str = "ranked-keys-";
pub(crate) const RANKED_KEYS_SUFFIX: &str = ".bin";
pub(crate) mod live_bm25 {
    pub(crate) const FILE_NAME: &str = "search-corpus-live-bm25.cbor";
    pub(crate) const MAX_BYTES: usize = super::MAX_INDEX_CONTROL_BYTES;
}
pub(crate) fn is_ranked_key_file_name(name: &str) -> bool {
    name.strip_prefix(RANKED_KEYS_PREFIX)
        .and_then(|id| id.strip_suffix(RANKED_KEYS_SUFFIX))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(crate) const MAX_MAP_HEIGHT: u8 = 12;

pub(crate) const LEXICAL_SEALED_IDENTITY_FILE_NAME: &str = "search-corpus-generation-identity.cbor";
pub(crate) const MAX_SEALED_IDENTITY_BYTES: usize = 4096;
pub(crate) const LEXICAL_QUARANTINE_RECEIPT_FILE_NAME: &str =
    "search-corpus-generation-quarantine.cbor";
