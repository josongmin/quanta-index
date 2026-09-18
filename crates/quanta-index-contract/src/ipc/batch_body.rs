//! The identity every receipt-bearing ingest batch carries, and the digest
//! that binds a batch to its body (QI-BB-032).
//!
//! `batch_digest` is not a name a producer picks: it is the canonical
//! digest of the batch body, and the search plane recomputes it before it
//! records anything. Two batches with the same digest therefore carry the
//! same bytes by construction; a batch whose carried digest does not match
//! its body is refused typed (`BATCH_DIGEST_MISMATCH`) before the
//! idempotency record is begun.
//!
//! The digest is fixed as:
//!
//! ```text
//! SHA-256( INGEST_BATCH_DIGEST_DOMAIN_V1
//!          || route code || 0x1f
//!          || canonical encoding of the batch with batch_digest = "" )
//! ```
//!
//! rendered as 64 lowercase hexadecimal characters. The canonical encoding
//! is the batch's IPC wire encoding (the CBOR the ingest socket carries),
//! which every DTO here produces with a fixed field order through its
//! manual `Serialize` impl; the codec owner (`quanta-index-ipc`) computes
//! it over the accessors `quanta-index-core` defines for every batch, and
//! the SDK computes it for its callers. The route code is the
//! [`IngestOperationKindV1`] code, so the same body under two routes is two
//! digests.

use core::fmt;
use core::fmt::Write as _;

/// Domain separator of the canonical batch digest; the `v1` is the digest
/// format's version and changes only with the formula above.
pub const INGEST_BATCH_DIGEST_DOMAIN_V1: &[u8] = b"quanta-index:ingest-batch-digest:v1\0";

/// Separator between the route code and the encoded body inside the hash.
pub const INGEST_BATCH_DIGEST_FIELD_SEPARATOR_V1: &[u8] = b"\x1f";

/// Length of a rendered batch digest: SHA-256 as lowercase hex.
pub const BATCH_DIGEST_TOKEN_LEN_V1: usize = 64;

/// Which receipt-bearing ingest route a batch travels (QI-BB-032).
///
/// The route is part of the batch digest's domain and of the idempotency
/// key: the same `(repo, revision, generation, body)` under two routes are
/// two digests and two records. The repo-map bundle route answers with a
/// mutation ack, not a receipt, and names no batch digest; it is outside
/// the idempotency catalog until it does.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IngestOperationKindV1 {
    SearchCorpus,
    History,
    Dirty,
    RuntimeCatalog,
    Structural,
    RepoCommitRecency,
    RepoTopic,
    RepoDescription,
    FileOwnership,
    FileContributor,
    RepoMeta,
}

impl IngestOperationKindV1 {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::SearchCorpus => "search-corpus",
            Self::History => "history",
            Self::Dirty => "dirty",
            Self::RuntimeCatalog => "runtime-catalog",
            Self::Structural => "structural",
            Self::RepoCommitRecency => "repo-commit-recency",
            Self::RepoTopic => "repo-topic",
            Self::RepoDescription => "repo-description",
            Self::FileOwnership => "file-ownership",
            Self::FileContributor => "file-contributor",
            Self::RepoMeta => "repo-meta",
        }
    }
}

impl fmt::Display for IngestOperationKindV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

/// `true` when `value` has the shape of a rendered canonical batch digest.
///
/// The shape is exactly [`BATCH_DIGEST_TOKEN_LEN_V1`] lowercase hexadecimal
/// characters. Shape only; whether it is *the* digest of a body is the codec
/// owner's recomputation to prove.
#[must_use]
pub fn is_canonical_batch_digest_token_v1(value: &str) -> bool {
    value.len() == BATCH_DIGEST_TOKEN_LEN_V1
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Render a batch digest as its wire token: lowercase hex, no prefix.
#[must_use]
pub fn batch_digest_token_v1(digest: &[u8; 32]) -> String {
    let mut token = String::with_capacity(BATCH_DIGEST_TOKEN_LEN_V1);
    for byte in digest {
        // Writing into a `String` cannot fail; the result carries no error
        // a caller could act on.
        let _written = write!(&mut token, "{byte:02x}");
    }
    token
}

#[cfg(test)]
mod tests {
    use super::{
        BATCH_DIGEST_TOKEN_LEN_V1, IngestOperationKindV1, batch_digest_token_v1,
        is_canonical_batch_digest_token_v1,
    };

    #[test]
    fn the_token_is_lowercase_hex_of_every_byte() {
        let mut digest = [0_u8; 32];
        digest[0] = 0x0a;
        digest[1] = 0xf0;
        digest[31] = 0xff;
        let token = batch_digest_token_v1(&digest);
        assert_eq!(token.len(), BATCH_DIGEST_TOKEN_LEN_V1);
        assert!(token.starts_with("0af0"));
        assert!(token.ends_with("ff"));
        assert!(is_canonical_batch_digest_token_v1(&token));
    }

    #[test]
    fn only_a_full_lowercase_hex_token_has_the_canonical_shape() {
        assert!(!is_canonical_batch_digest_token_v1(""));
        assert!(!is_canonical_batch_digest_token_v1("batch:lex"));
        assert!(!is_canonical_batch_digest_token_v1(&"a".repeat(63)));
        assert!(!is_canonical_batch_digest_token_v1(&"A".repeat(64)));
        assert!(!is_canonical_batch_digest_token_v1(&"g".repeat(64)));
        assert!(is_canonical_batch_digest_token_v1(&"0".repeat(64)));
    }

    #[test]
    fn every_route_code_is_distinct() {
        let codes = [
            IngestOperationKindV1::SearchCorpus,
            IngestOperationKindV1::History,
            IngestOperationKindV1::Dirty,
            IngestOperationKindV1::RuntimeCatalog,
            IngestOperationKindV1::Structural,
            IngestOperationKindV1::RepoCommitRecency,
            IngestOperationKindV1::RepoTopic,
            IngestOperationKindV1::RepoDescription,
            IngestOperationKindV1::FileOwnership,
            IngestOperationKindV1::FileContributor,
            IngestOperationKindV1::RepoMeta,
        ]
        .map(IngestOperationKindV1::as_code_str);
        let distinct: std::collections::BTreeSet<&str> = codes.iter().copied().collect();
        assert_eq!(distinct.len(), codes.len());
    }
}
