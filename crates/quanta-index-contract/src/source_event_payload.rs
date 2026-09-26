//! Logical producer payload identity, independent of target generation and
//! transport idempotency. Order is deliberately committed: changing any supplied
//! mutation order is not an identical event replay.

use std::io::Write;

use sha2::{Digest as _, Sha256};

use crate::{SearchCorpusIngestBatch, SourceCoverageError};

/// Hash the complete source mutation payload, not its materialization target.
/// Source revisions are carried by file coverage; containing revision/generation,
/// manifest identity, transport digest and seal state are excluded. The event's
/// stream/id/base are separately bound by the durable publication catalog.
pub fn source_event_payload_sha256(
    batch: &SearchCorpusIngestBatch,
) -> Result<[u8; 32], SourceCoverageError> {
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    writer.0.update(b"quanta-index:source-event-payload:v1\0");
    ciborium::into_writer(
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
        &mut writer,
    )
    .map_err(|error| SourceCoverageError::Encode(error.to_string()))?;
    Ok(writer.0.finalize().into())
}
