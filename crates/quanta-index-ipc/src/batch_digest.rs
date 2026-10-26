//! The canonical batch digest (QI-BB-032): one computation for producers,
//! the SDK and the search plane.
//!
//! The contract fixes the formula
//! ([`quanta_index_contract::INGEST_BATCH_DIGEST_DOMAIN_V1`]); this module
//! owns the encoding step, because the canonical body encoding is the IPC
//! wire encoding and this crate is the codec's owner. The digest is hashed
//! over the batch with its root `batch_digest` field cleared. A serializer
//! decorator substitutes that one field without mutating or cloning the body.

mod borrowed;

use quanta_index_contract::{
    BATCH_DIGEST_TOKEN_LEN_V1, INGEST_BATCH_DIGEST_DOMAIN_V1,
    INGEST_BATCH_DIGEST_FIELD_SEPARATOR_V1, batch_digest_token_v1,
};
use quanta_index_core::IngestBatchBodyV1;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::convert::Infallible;

use crate::error::IpcError;
use borrowed::ClearedRootDigest;

struct HashWriter<'a, E, F> {
    hasher: Sha256,
    progress: &'a mut F,
    interrupted: Option<E>,
}

impl<E, F: FnMut(usize) -> Result<(), E>> HashWriter<'_, E, F> {
    fn checkpoint(&mut self, bytes: usize) -> Result<(), E> {
        (self.progress)(bytes)
    }

    fn hash_header(&mut self, bytes: &[u8]) -> Result<(), E> {
        self.checkpoint(bytes.len())?;
        self.hasher.update(bytes);
        Ok(())
    }
}

impl<E, F: FnMut(usize) -> Result<(), E>> std::io::Write for HashWriter<'_, E, F> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.interrupted.is_some() {
            return Err(std::io::ErrorKind::Other.into());
        }
        if let Err(cause) = self.checkpoint(bytes.len()) {
            self.interrupted = Some(cause);
            return Err(std::io::ErrorKind::Other.into());
        }
        self.hasher.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Encoding defect or the original execution-owner refusal.
#[derive(Debug)]
pub enum CanonicalBatchDigestProgressError<E> {
    Encode(IpcError),
    Progress(E),
}

impl<E: std::fmt::Display> std::fmt::Display for CanonicalBatchDigestProgressError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(error) => {
                write!(formatter, "canonical batch digest encoding failed: {error}")
            }
            Self::Progress(error) => {
                write!(formatter, "canonical batch digest interrupted: {error}")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CanonicalBatchDigestProgressError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(error) => Some(error),
            Self::Progress(error) => Some(error),
        }
    }
}

fn canonical_batch_digest_body<B, E, F>(
    body: &B,
    progress: &mut F,
) -> Result<[u8; 32], CanonicalBatchDigestProgressError<E>>
where
    B: IngestBatchBodyV1 + Serialize,
    F: FnMut(usize) -> Result<(), E>,
{
    progress(0).map_err(CanonicalBatchDigestProgressError::Progress)?;
    let mut writer = HashWriter {
        hasher: Sha256::new(),
        progress,
        interrupted: None,
    };
    writer
        .hash_header(INGEST_BATCH_DIGEST_DOMAIN_V1)
        .map_err(CanonicalBatchDigestProgressError::Progress)?;
    writer
        .hash_header(B::OPERATION.as_code_str().as_bytes())
        .map_err(CanonicalBatchDigestProgressError::Progress)?;
    writer
        .hash_header(INGEST_BATCH_DIGEST_FIELD_SEPARATOR_V1)
        .map_err(CanonicalBatchDigestProgressError::Progress)?;

    let encoded = ciborium::into_writer(&ClearedRootDigest(body), &mut writer);
    if let Some(cause) = writer.interrupted.take() {
        return Err(CanonicalBatchDigestProgressError::Progress(cause));
    }
    encoded.map_err(|error| {
        CanonicalBatchDigestProgressError::Encode(IpcError::Encode(error.to_string()))
    })?;
    writer
        .checkpoint(0)
        .map_err(CanonicalBatchDigestProgressError::Progress)?;
    Ok(writer.hasher.finalize().into())
}

/// Canonical digest with a work charge before every prefix and encoded hash
/// write.
///
/// `progress` receives the exact byte count for that write, plus a
/// zero-byte entry and exit check. The callback's original error is returned
/// without wrapping it in an encoder error. Ciborium streams the normal
/// struct/sequence/map path, but a body-specific `Serialize` implementation
/// may allocate or compute before its first write; that work needs its own
/// admission and cooperative checkpoints.
pub fn canonical_batch_digest_with_progress_v1<B, E, F>(
    body: &B,
    progress: &mut F,
) -> Result<[u8; 32], CanonicalBatchDigestProgressError<E>>
where
    B: IngestBatchBodyV1 + Serialize,
    F: FnMut(usize) -> Result<(), E>,
{
    canonical_batch_digest_body(body, progress)
}

/// The canonical digest of `body`'s route and content, as raw bytes.
///
/// Fails only when the body cannot be encoded, which the caller treats as
/// a contract defect of the batch, never as "no digest".
pub fn canonical_batch_digest_v1<B: IngestBatchBodyV1 + Serialize>(
    body: &B,
) -> Result<[u8; 32], IpcError> {
    match canonical_batch_digest_body(body, &mut |_bytes| Ok::<(), Infallible>(())) {
        Ok(digest) => Ok(digest),
        Err(CanonicalBatchDigestProgressError::Encode(error)) => Err(error),
        Err(CanonicalBatchDigestProgressError::Progress(never)) => match never {},
    }
}

/// Compare a carried lowercase hexadecimal token with raw digest bytes
/// without rendering a temporary `String`.
#[must_use]
pub fn matches_batch_digest_token_v1(carried: &str, digest: &[u8; 32]) -> bool {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let token = carried.as_bytes();
    if token.len() != BATCH_DIGEST_TOKEN_LEN_V1 {
        return false;
    }
    digest
        .iter()
        .zip(token.chunks_exact(2))
        .all(|(byte, pair)| {
            let high = HEX.get(usize::from(byte >> 4)).copied();
            let low = HEX.get(usize::from(byte & 0x0f)).copied();
            pair.first().copied() == high && pair.get(1).copied() == low
        })
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
    body: &B,
) -> Result<BatchDigestVerdictV1, IpcError> {
    let digest = canonical_batch_digest_v1(body)?;
    if matches_batch_digest_token_v1(body.batch_digest(), &digest) {
        return Ok(BatchDigestVerdictV1::Verified(digest));
    }
    Ok(BatchDigestVerdictV1::Mismatch {
        carried: body.batch_digest().to_string(),
        expected: batch_digest_token_v1(&digest),
    })
}

#[cfg(test)]
mod tests {
    use serde::ser::{Error as _, SerializeSeq as _, SerializeStruct as _};
    use serde::{Serialize, Serializer};

    use quanta_index_contract::lex::DirtyRecord;
    use quanta_index_contract::{
        ChunkId, DirtyIngestBatch, DirtyMutation, ManifestGeneration, RepoId, RevisionId,
        RuntimeCatalogIngestBatch, is_canonical_batch_digest_token_v1,
    };
    use quanta_index_core::IngestBatchBodyV1 as _;

    use super::{
        BatchDigestVerdictV1, CanonicalBatchDigestProgressError, canonical_batch_digest_v1,
        canonical_batch_digest_with_progress_v1, matches_batch_digest_token_v1,
        stamp_batch_digest_v1, verify_batch_digest_v1,
    };
    use quanta_index_contract::{
        INGEST_BATCH_DIGEST_DOMAIN_V1, INGEST_BATCH_DIGEST_FIELD_SEPARATOR_V1,
    };

    #[derive(Clone, Copy)]
    enum ProbeShape {
        Struct,
        Missing,
        Duplicate,
        Sequence,
        Error,
        Unwind,
    }

    struct ProbeBatch {
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
        batch_digest: String,
        shape: ProbeShape,
    }

    #[derive(Serialize)]
    struct NestedDigest {
        batch_digest: &'static str,
    }

    impl Serialize for ProbeBatch {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self.shape {
                ProbeShape::Sequence => serializer.serialize_seq(Some(0))?.end(),
                ProbeShape::Error => Err(S::Error::custom("probe serialization error")),
                ProbeShape::Unwind => std::panic::resume_unwind(Box::new("probe unwind")),
                ProbeShape::Struct | ProbeShape::Missing | ProbeShape::Duplicate => {
                    let len = if matches!(self.shape, ProbeShape::Duplicate) {
                        3
                    } else if matches!(self.shape, ProbeShape::Missing) {
                        1
                    } else {
                        2
                    };
                    let mut root = serializer.serialize_struct("ProbeBatch", len)?;
                    if !matches!(self.shape, ProbeShape::Missing) {
                        root.serialize_field("batch_digest", &self.batch_digest)?;
                    }
                    root.serialize_field(
                        "nested",
                        &NestedDigest {
                            batch_digest: "inside",
                        },
                    )?;
                    if matches!(self.shape, ProbeShape::Duplicate) {
                        root.serialize_field("batch_digest", &self.batch_digest)?;
                    }
                    root.end()
                }
            }
        }
    }

    impl quanta_index_core::IngestBatchBodyV1 for ProbeBatch {
        const OPERATION: quanta_index_contract::IngestOperationKindV1 =
            quanta_index_contract::IngestOperationKindV1::Dirty;

        fn repo_id(&self) -> &RepoId {
            &self.repo_id
        }
        fn revision_id(&self) -> &RevisionId {
            &self.revision_id
        }
        fn generation(&self) -> ManifestGeneration {
            self.generation
        }
        fn batch_digest(&self) -> &str {
            &self.batch_digest
        }
        fn batch_digest_mut(&mut self) -> &mut String {
            &mut self.batch_digest
        }
    }

    fn probe(shape: ProbeShape) -> ProbeBatch {
        let fixture = dirty_batch("src/a.rs");
        ProbeBatch {
            repo_id: fixture.repo_id,
            revision_id: fixture.revision_id,
            generation: fixture.generation,
            batch_digest: "carried".to_owned(),
            shape,
        }
    }

    #[test]
    fn borrowed_root_decorator_preserves_fixed_cbor_and_nested_digest()
    -> Result<(), Box<dyn std::error::Error>> {
        use sha2::{Digest as _, Sha256};
        const CLEARED_CBOR: &[u8] =
            b"\xa2\x6cbatch_digest\x60\x66nested\xa1\x6cbatch_digest\x66inside";
        let batch = probe(ProbeShape::Struct);
        let mut hash = Sha256::new();
        hash.update(b"quanta-index:ingest-batch-digest:v1\0");
        hash.update(b"dirty");
        hash.update(b"\x1f");
        hash.update(CLEARED_CBOR);
        let expected: [u8; 32] = hash.finalize().into();
        if canonical_batch_digest_v1(&batch)? != expected || batch.batch_digest != "carried" {
            return Err("borrowed root digest diverged from fixed CBOR preimage".into());
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed preimage length, checkpoint count, and digest equality are independent test oracles"
    )]
    fn progress_charges_fixed_preimage_bytes_and_matches_ordinary_digest()
    -> Result<(), Box<dyn std::error::Error>> {
        const CLEARED_CBOR: &[u8] =
            b"\xa2\x6cbatch_digest\x60\x66nested\xa1\x6cbatch_digest\x66inside";
        let batch = probe(ProbeShape::Struct);
        let mut charged = 0usize;
        let mut checkpoints = 0usize;
        let with_progress = canonical_batch_digest_with_progress_v1(&batch, &mut |bytes| {
            charged += bytes;
            checkpoints += 1;
            Ok::<_, std::io::Error>(())
        })?;
        let expected_bytes = INGEST_BATCH_DIGEST_DOMAIN_V1.len()
            + ProbeBatch::OPERATION.as_code_str().len()
            + INGEST_BATCH_DIGEST_FIELD_SEPARATOR_V1.len()
            + CLEARED_CBOR.len();
        assert_eq!(charged, expected_bytes);
        assert!(checkpoints >= 5);
        assert_eq!(with_progress, canonical_batch_digest_v1(&batch)?);
        Ok(())
    }

    #[test]
    fn progress_refusal_precedes_hash_and_preserves_typed_error() {
        #[derive(Debug, Eq, PartialEq)]
        struct Refused(&'static str);

        // The serializer panics if reached. Refusing at entry must return the
        // original typed cause before any header/hash/serialization work.
        let batch = probe(ProbeShape::Unwind);
        let first = canonical_batch_digest_with_progress_v1(&batch, &mut |_bytes| {
            Err::<(), _>(Refused("entry"))
        });
        assert!(matches!(
            first,
            Err(CanonicalBatchDigestProgressError::Progress(Refused(
                "entry"
            )))
        ));

        let mut calls = 0usize;
        let header = canonical_batch_digest_with_progress_v1(&batch, &mut |_bytes| {
            calls += 1;
            if calls == 2 {
                Err(Refused("header"))
            } else {
                Ok(())
            }
        });
        assert!(matches!(
            header,
            Err(CanonicalBatchDigestProgressError::Progress(Refused(
                "header"
            )))
        ));
        assert_eq!(calls, 2);

        let valid = probe(ProbeShape::Struct);
        let mut calls = 0usize;
        let encoded_refusal = canonical_batch_digest_with_progress_v1(&valid, &mut |_bytes| {
            calls += 1;
            if calls == 5 {
                Err(Refused("encoded"))
            } else {
                Ok(())
            }
        });
        assert!(matches!(
            encoded_refusal,
            Err(CanonicalBatchDigestProgressError::Progress(Refused(
                "encoded"
            )))
        ));

        let invalid = probe(ProbeShape::Missing);
        let encoded =
            canonical_batch_digest_with_progress_v1(&invalid, &mut |_bytes| Ok::<(), Refused>(()));
        assert!(matches!(
            encoded,
            Err(CanonicalBatchDigestProgressError::Encode(_))
        ));
    }

    #[test]
    fn unsupported_root_and_serializer_failures_do_not_mutate_the_batch()
    -> Result<(), Box<dyn std::error::Error>> {
        for shape in [
            ProbeShape::Missing,
            ProbeShape::Duplicate,
            ProbeShape::Sequence,
            ProbeShape::Error,
        ] {
            let batch = probe(shape);
            let original_ptr = batch.batch_digest.as_ptr();
            if canonical_batch_digest_v1(&batch).is_ok()
                || batch.batch_digest != "carried"
                || batch.batch_digest.as_ptr() != original_ptr
            {
                return Err("failed digest changed the carried field".into());
            }
        }
        let batch = probe(ProbeShape::Unwind);
        let original_ptr = batch.batch_digest.as_ptr();
        if std::panic::catch_unwind(|| canonical_batch_digest_v1(&batch)).is_ok()
            || batch.batch_digest != "carried"
            || batch.batch_digest.as_ptr() != original_ptr
        {
            return Err("serializer unwind changed the carried field".into());
        }
        Ok(())
    }

    #[test]
    fn borrowed_token_match_refuses_malformed_or_changed_hex() {
        let digest = [0xab; 32];
        let expected = "ab".repeat(32);
        assert!(matches_batch_digest_token_v1(&expected, &digest));
        assert!(!matches_batch_digest_token_v1(
            &expected.to_uppercase(),
            &digest
        ));
        assert!(!matches_batch_digest_token_v1("ab", &digest));
        assert!(!matches_batch_digest_token_v1(&"ac".repeat(32), &digest));
    }

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
        let first = dirty_batch("src/a.rs");
        let mut second = dirty_batch("src/a.rs");
        second.batch_digest = "a different token".to_string();
        let left = canonical_batch_digest_v1(&first).expect("encodes");
        let right = canonical_batch_digest_v1(&second).expect("encodes");
        assert_eq!(left, right, "the carried token is not part of the digest");
        assert_eq!(first.batch_digest, "whatever the producer wrote");
        assert_eq!(second.batch_digest, "a different token");
    }

    #[test]
    fn streaming_digest_preserves_the_published_cbor_preimage_formula() {
        use sha2::{Digest as _, Sha256};
        let batch = dirty_batch("src/a.rs");
        let mut reference = batch.clone();
        reference.batch_digest.clear();
        let encoded = crate::encode_cbor_payload(&reference).expect("reference CBOR");
        // Literal protocol domain and operation code are independent of the
        // streaming implementation. Existing persisted commitments must match.
        let mut digest = Sha256::new();
        digest.update(b"quanta-index:ingest-batch-digest:v1\0");
        digest.update(b"dirty");
        digest.update(b"\x1f");
        digest.update(encoded);
        let expected: [u8; 32] = digest.finalize().into();
        assert_eq!(
            canonical_batch_digest_v1(&batch).expect("streamed digest"),
            expected
        );
    }

    #[test]
    fn one_changed_byte_changes_the_digest() {
        let base = dirty_batch("src/a.rs");
        let changed = dirty_batch("src/b.rs");
        assert_ne!(
            canonical_batch_digest_v1(&base).expect("encodes"),
            canonical_batch_digest_v1(&changed).expect("encodes")
        );
    }

    #[test]
    fn the_route_is_part_of_the_domain() {
        let dirty = dirty_batch("src/a.rs");
        let runtime = RuntimeCatalogIngestBatch {
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
            canonical_batch_digest_v1(&dirty).expect("encodes"),
            canonical_batch_digest_v1(&runtime).expect("encodes")
        );
    }

    #[test]
    fn a_stamped_batch_verifies_and_a_mutated_one_does_not() {
        let mut batch = dirty_batch("src/a.rs");
        stamp_batch_digest_v1(&mut batch).expect("stamps");
        assert!(is_canonical_batch_digest_token_v1(batch.batch_digest()));
        let expected = canonical_batch_digest_v1(&batch).expect("encodes");
        assert_eq!(
            verify_batch_digest_v1(&batch).expect("verifies"),
            BatchDigestVerdictV1::Verified(expected)
        );
        let stamped = batch.batch_digest.clone();
        batch.overlay_epoch_ms = 8;
        match verify_batch_digest_v1(&batch).expect("verifies") {
            BatchDigestVerdictV1::Mismatch { carried, expected } => {
                assert_eq!(carried, stamped);
                assert_ne!(expected, stamped);
            }
            BatchDigestVerdictV1::Verified(_) => panic!("a mutated body must not verify"),
        }
        assert_eq!(
            batch.batch_digest, stamped,
            "verification leaves the batch as it was"
        );
    }
}
