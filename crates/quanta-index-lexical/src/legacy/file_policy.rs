//! Canonical File authority policy identity; values are unchanged.
use sha2::{Digest as _, Sha256};
const MAX_TOTAL_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
// A CodeSearch request can perform language, short-literal size and exact
// scans, then language and shortlist scans on an empty-result typo fallback.
pub(crate) const MAX_QUERY_SOURCE_WALKS: u64 = 5;

#[derive(Clone, Copy)]
pub(crate) struct FilePolicy {
    pub(crate) source_files: u64,
    pub(crate) source_bytes: u64,
    pub(crate) total_memberships: u64,
    pub(crate) partitions: u64,
    pub(crate) bucket_scratch_bytes: u64,
    pub(crate) query_list_reads: u64,
    pub(crate) query_posting_ids: u64,
    pub(crate) query_decoded_bytes: u64,
    pub(crate) query_decoded_ids: u64,
}

pub(crate) fn policy() -> FilePolicy {
    FilePolicy {
        source_files: 32_768,
        source_bytes: MAX_TOTAL_SOURCE_BYTES,
        total_memberships: u64::from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT),
        partitions: 256,
        bucket_scratch_bytes: 128 * 1024 * 1024,
        query_list_reads: 16_384,
        query_posting_ids: 2_000_000,
        query_decoded_bytes: 512 * 1024 * 1024,
        query_decoded_ids: u64::from(crate::FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT),
    }
}

pub(crate) fn policy_digest_for_paged() -> [u8; 32] {
    let policy = policy();
    let mut digest = Sha256::new();
    digest.update(b"quanta-file-authority-v17-paged-policy-fields\0");
    for field in [
        policy.source_files,
        policy.source_bytes,
        policy.total_memberships,
        policy.partitions,
        policy.bucket_scratch_bytes,
        policy.query_list_reads,
        policy.query_posting_ids,
        policy.query_decoded_bytes,
        policy.query_decoded_ids,
        MAX_QUERY_SOURCE_WALKS,
        u64::from(super::names::MAX_MAP_HEIGHT),
    ] {
        digest.update(field.to_le_bytes());
    }
    digest.finalize().into()
}
