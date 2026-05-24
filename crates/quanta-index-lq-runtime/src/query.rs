//! Read-side resolver for the `dirty:` filter per RT-01 § 4.6 / § 5.6.
//!
//! [`dirty_docs`] enforces generation pinning at read time: a query against a
//! generation other than the buffer's currently-active one returns a typed
//! [`RuntimeErrorCode::StateNotReady`] / [`StateNotReadyReason::SnapshotUnknown`]
//! instead of an empty `Ok` — no silent fallback.

use crate::buffer::DirtyBuffer;
use crate::errors::{RuntimeError, StateNotReadyReason};
use crate::types::{DocId, ManifestGeneration};

/// Return the dirty [`DocId`] list for the pinned generation in ascending
/// order. Generation mismatch is fail-closed.
pub fn dirty_docs(
    buf: &DirtyBuffer,
    generation: ManifestGeneration,
) -> Result<Vec<DocId>, RuntimeError> {
    if generation != buf.active_generation() {
        return Err(RuntimeError::state_not_ready(
            StateNotReadyReason::SnapshotUnknown,
            "query generation does not match buffer active generation",
        ));
    }
    let mut out: Vec<DocId> = Vec::with_capacity(buf.len());
    for entry in buf.iter_dirty() {
        out.push(entry.doc_id);
    }
    // `iter_dirty` already returns entries in ascending `DocId` order
    // (backed by a `BTreeMap`), so an explicit sort is unnecessary; assert
    // the invariant in tests.
    debug_assert!(
        out.windows(2).all(|w| match (w.first(), w.get(1)) {
            (Some(a), Some(b)) => a <= b,
            _ => true,
        }),
        "dirty_docs must return ascending DocIds",
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::dirty_docs;
    use crate::apply_changes::{DirtyEntry, PAYLOAD_HASH_LEN};
    use crate::buffer::DirtyBuffer;
    use crate::errors::{RuntimeErrorCode, StateNotReadyReason};
    use crate::types::{ApplyTimeMs, BufferConfig, DocId, ManifestGeneration, RepoId, TenantId};

    fn build_buf() -> Result<DirtyBuffer, crate::errors::RuntimeError> {
        let t = TenantId::new("t1")?;
        let r = RepoId::new("r1")?;
        let cfg = BufferConfig::new(4, 1_000)?;
        let mut buf = DirtyBuffer::new(t.clone(), r.clone(), cfg, ManifestGeneration(7));
        let entries = [(2u64, 0xAAu8), (1u64, 0xBBu8), (3u64, 0xCCu8)];
        for (d, h) in entries {
            let e = DirtyEntry {
                tenant_id: t.clone(),
                repo_id: r.clone(),
                doc_id: DocId(d),
                generation: ManifestGeneration(7),
                applied_at_ms: ApplyTimeMs(0),
                payload_hash: [h; PAYLOAD_HASH_LEN],
            };
            let _outcome: crate::apply_changes::ApplyOutcome = buf.apply(e, ApplyTimeMs(0))?;
        }
        Ok(buf)
    }

    #[test]
    fn dirty_docs_returns_sorted_doc_ids() {
        let buf = match build_buf() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match dirty_docs(&buf, ManifestGeneration(7)) {
            Ok(v) => assert_eq!(v, vec![DocId(1), DocId(2), DocId(3)]),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn dirty_docs_rejects_other_generation() {
        let buf = match build_buf() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match dirty_docs(&buf, ManifestGeneration(8)) {
            Err(err) => {
                assert_eq!(err.code, RuntimeErrorCode::StateNotReady);
                assert_eq!(err.state_reason, Some(StateNotReadyReason::SnapshotUnknown));
            }
            Ok(v) => assert!(false, "expected StateNotReady, got {v:?}"),
        }
    }
}
