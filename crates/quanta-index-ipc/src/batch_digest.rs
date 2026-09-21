//! The canonical batch digest (QI-BB-032): one computation for producers,
//! the SDK and the search plane.
//!
//! The contract fixes the formula
//! ([`quanta_index_contract::INGEST_BATCH_DIGEST_DOMAIN_V1`]); this module
//! owns the encoding step, because the canonical body encoding is the IPC
//! wire encoding and this crate is the codec's owner. The digest is hashed
//! over the batch with its `batch_digest` field cleared, so the field is
//! taken out, the body encoded, and the field put back — the batch is
//! unchanged afterwards whichever way the computation ended.

use quanta_index_contract::{
    INGEST_BATCH_DIGEST_DOMAIN_V1, INGEST_BATCH_DIGEST_FIELD_SEPARATOR_V1, batch_digest_token_v1,
};
use quanta_index_core::IngestBatchBodyV1;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::codec::{IpcError, encode_cbor_payload};

/// The canonical digest of `body`'s route and content, as raw bytes.
///
/// Fails only when the body cannot be encoded, which the caller treats as
/// a contract defect of the batch, never as "no digest".
pub fn canonical_batch_digest_v1<B: IngestBatchBodyV1 + Serialize>(
    body: &mut B,
) -> Result<[u8; 32], IpcError> {
    let carried = std::mem::take(body.batch_digest_mut());
    let encoded = encode_cbor_payload(&*body);
    *body.batch_digest_mut() = carried;
    let encoded = encoded?;
    let mut hasher = Sha256::new();
    hasher.update(INGEST_BATCH_DIGEST_DOMAIN_V1);
    hasher.update(B::OPERATION.as_code_str().as_bytes());
    hasher.update(INGEST_BATCH_DIGEST_FIELD_SEPARATOR_V1);
    hasher.update(&encoded);
    Ok(hasher.finalize().into())
}

/// Set `body.batch_digest` to its canonical token, whatever it carried.
///
/// This is what a producer does last, after the body is final: any later
/// change to the body invalidates the digest and the search plane refuses
/// the batch.
pub fn stamp_batch_digest_v1<B: IngestBatchBodyV1 + Serialize>(
    body: &mut B,
) -> Result<(), IpcError> {
    let digest = canonical_batch_digest_v1(body)?;
    *body.batch_digest_mut() = batch_digest_token_v1(&digest);
    Ok(())
}

/// What the search plane found when it recomputed a carried digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BatchDigestVerdictV1 {
    /// The carried token is the canonical digest; the bytes are the digest.
    Verified([u8; 32]),
    /// The carried token is not the canonical digest of this body.
    Mismatch { carried: String, expected: String },
}

/// Recompute `body`'s digest and compare it with the token it carries.
pub fn verify_batch_digest_v1<B: IngestBatchBodyV1 + Serialize>(
    body: &mut B,
) -> Result<BatchDigestVerdictV1, IpcError> {
    let digest = canonical_batch_digest_v1(body)?;
    let expected = batch_digest_token_v1(&digest);
    if body.batch_digest() == expected {
        return Ok(BatchDigestVerdictV1::Verified(digest));
    }
    Ok(BatchDigestVerdictV1::Mismatch {
        carried: body.batch_digest().to_string(),
        expected,
    })
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::lex::DirtyRecord;
    use quanta_index_contract::{
        ChunkId, DirtyIngestBatch, DirtyMutation, ManifestGeneration, RepoId, RevisionId,
        RuntimeCatalogIngestBatch, is_canonical_batch_digest_token_v1,
    };
    use quanta_index_core::IngestBatchBodyV1 as _;

    use super::{
        BatchDigestVerdictV1, canonical_batch_digest_v1, stamp_batch_digest_v1,
        verify_batch_digest_v1,
    };

    fn dirty_batch(doc: &str) -> DirtyIngestBatch {
        DirtyIngestBatch {
            repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev")
                .expect("static fixture ID satisfies canonical policy"),
            generation: ManifestGeneration::new(3),
            overlay_epoch_ms: 7,
            batch_digest: "whatever the producer wrote".to_string(),
            entries: vec![DirtyMutation::Upsert(DirtyRecord {
                wire_version: 1,
                doc_id: ChunkId::new(doc),
                applied_at_ms: 7,
                payload_hash: [3; 32],
            })],
        }
    }

    #[test]
    fn the_digest_ignores_the_carried_token_and_restores_it() {
        let mut first = dirty_batch("src/a.rs");
        let mut second = dirty_batch("src/a.rs");
        second.batch_digest = "a different token".to_string();
        let left = canonical_batch_digest_v1(&mut first).expect("encodes");
        let right = canonical_batch_digest_v1(&mut second).expect("encodes");
        assert_eq!(left, right, "the carried token is not part of the digest");
        assert_eq!(first.batch_digest, "whatever the producer wrote");
        assert_eq!(second.batch_digest, "a different token");
    }

    #[test]
    fn one_changed_byte_changes_the_digest() {
        let mut base = dirty_batch("src/a.rs");
        let mut changed = dirty_batch("src/b.rs");
        assert_ne!(
            canonical_batch_digest_v1(&mut base).expect("encodes"),
            canonical_batch_digest_v1(&mut changed).expect("encodes")
        );
    }

    #[test]
    fn the_route_is_part_of_the_domain() {
        let mut dirty = dirty_batch("src/a.rs");
        let mut runtime = RuntimeCatalogIngestBatch {
            repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev")
                .expect("static fixture ID satisfies canonical policy"),
            generation: ManifestGeneration::new(3),
            overlay_epoch_ms: 7,
            batch_digest: String::new(),
            producer_head_applied_at_ms: 0,
            generation_materialized_at_ms: 0,
            changed_entries: Vec::new(),
            facet_entries: Vec::new(),
            snapshot_entries: Vec::new(),
            affected_entries: Vec::new(),
            invalidated_by_entries: Vec::new(),
        };
        // Different bodies anyway; what matters is that two routes never
        // hash under one domain, which the code string in the preimage
        // guarantees even for byte-identical encodings.
        assert_ne!(
            canonical_batch_digest_v1(&mut dirty).expect("encodes"),
            canonical_batch_digest_v1(&mut runtime).expect("encodes")
        );
    }

    #[test]
    fn a_stamped_batch_verifies_and_a_mutated_one_does_not() {
        let mut batch = dirty_batch("src/a.rs");
        stamp_batch_digest_v1(&mut batch).expect("stamps");
        assert!(is_canonical_batch_digest_token_v1(batch.batch_digest()));
        let expected = canonical_batch_digest_v1(&mut batch).expect("encodes");
        assert_eq!(
            verify_batch_digest_v1(&mut batch).expect("verifies"),
            BatchDigestVerdictV1::Verified(expected)
        );
        let stamped = batch.batch_digest.clone();
        batch.overlay_epoch_ms = 8;
        match verify_batch_digest_v1(&mut batch).expect("verifies") {
            BatchDigestVerdictV1::Mismatch { carried, expected } => {
                assert_eq!(carried, stamped);
                assert_ne!(expected, stamped);
            }
            BatchDigestVerdictV1::Verified(_) => panic!("a mutated body must not verify"),
        }
        assert_eq!(batch.batch_digest, stamped, "verification leaves the batch as it was");
    }
}
