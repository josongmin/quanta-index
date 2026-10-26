//! Logical producer payload identity commits supplied mutation order.

use super::SearchCorpusIngestBatch;
use crate::source_coverage::hash_canonical_cbor_with_progress_and_work;
use crate::{SourceCoverageError, SourceCoverageProgressError};

/// Hash the complete source mutation payload, not its materialization target.
///
/// Source revisions are carried by file coverage; containing revision/generation,
/// manifest identity, transport digest and seal state are excluded. The event's
/// stream/id/base are separately bound by the durable publication catalog.
pub fn source_event_payload_sha256(
    batch: &SearchCorpusIngestBatch,
) -> Result<[u8; 32], SourceCoverageError> {
    match source_event_payload_sha256_with_progress(batch, &mut || {
        Ok::<_, std::convert::Infallible>(())
    }) {
        Ok(digest) => Ok(digest),
        Err(SourceCoverageProgressError::Invalid(error)) => Err(error),
        Err(SourceCoverageProgressError::Progress(never)) => match never {},
    }
}

/// Hash the same canonical payload with bounded serializer checkpoints.
pub fn source_event_payload_sha256_with_progress<E, F>(
    batch: &SearchCorpusIngestBatch,
    progress: &mut F,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    F: FnMut() -> Result<(), E>,
{
    source_event_payload_sha256_with_work_v3(batch, progress, &mut |_| Ok(()))
}

/// The same canonical event commitment, with the actual domain and encoded
/// hash-write byte counts handed to an Original work control before hashing.
pub fn source_event_payload_sha256_with_work_v3<E, F, W>(
    batch: &SearchCorpusIngestBatch,
    progress: &mut F,
    work: &mut W,
) -> Result<[u8; 32], SourceCoverageProgressError<E>>
where
    F: FnMut() -> Result<(), E>,
    W: FnMut(usize) -> Result<(), E>,
{
    hash_canonical_cbor_with_progress_and_work(
        b"quanta-index:source-event-payload:v1\0",
        &(
            &batch.repo_id,
            batch.mode,
            &batch.bundle_payload,
            &batch.clear_surfaces,
            &batch.replace_scopes,
            &batch.tombstone_scopes,
            &batch.semantic_replace_scopes,
            &batch.semantic_tombstone_scopes,
        ),
        progress,
        work,
    )
}
