//! Property: after a sweep at or past `applied_at + ttl`, the entry is
//! evicted and surfaced as part of the returned [`DocId`] list.

use proptest::prelude::*;
use quanta_index_lq_runtime::{
    ApplyOutcome, ApplyTimeMs, BufferConfig, DirtyBuffer, DirtyEntry, DocId, ManifestGeneration,
    PAYLOAD_HASH_LEN, RepoId, RuntimeError, TenantId,
};

fn fresh_buffer(cap: u32, ttl_ms: u64) -> Result<(DirtyBuffer, TenantId, RepoId), RuntimeError> {
    let t = TenantId::new("t")?;
    let r = RepoId::new("r")?;
    let cfg = BufferConfig::new(cap, ttl_ms)?;
    let buf = DirtyBuffer::new(t.clone(), r.clone(), cfg, ManifestGeneration(1));
    Ok((buf, t, r))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    #[test]
    fn sweep_evicts_all_entries_past_ttl(
        docs in proptest::collection::vec(0u64..1024, 1..16),
        ttl_ms in 1u64..1_000,
        applied_at_ms in 0u64..1_000_000,
    ) {
        // Deduplicate doc_ids so each entry occupies its own slot.
        let mut unique: Vec<u64> = docs;
        unique.sort_unstable();
        unique.dedup();
        let cap = match u32::try_from(unique.len()) {
            Ok(v) => v,
            Err(e) => {
                prop_assert!(false, "cap conversion failed: {}", e);
                return Ok(());
            }
        };
        let (mut buf, t, r) = match fresh_buffer(cap, ttl_ms) {
            Ok(v) => v,
            Err(e) => {
                prop_assert!(false, "fresh_buffer failed: {}", e);
                return Ok(());
            }
        };

        for (i, doc) in unique.iter().enumerate() {
            let hash_byte = match u8::try_from(i & 0xFF) {
                Ok(v) => v,
                Err(e) => {
                    prop_assert!(false, "hash_byte conversion failed: {}", e);
                    return Ok(());
                }
            };
            let entry = DirtyEntry {
                tenant_id: t.clone(),
                repo_id: r.clone(),
                doc_id: DocId(*doc),
                generation: ManifestGeneration(1),
                applied_at_ms: ApplyTimeMs(applied_at_ms),
                payload_hash: [hash_byte; PAYLOAD_HASH_LEN],
            };
            match buf.apply(entry, ApplyTimeMs(applied_at_ms)) {
                Ok(ApplyOutcome::Buffered) => (),
                other => {
                    prop_assert!(false, "expected Buffered, got {:?}", other);
                }
            }
        }
        prop_assert_eq!(buf.len(), unique.len());

        // sweep at applied_at + ttl is the boundary: deadline = applied + ttl,
        // and the buffer evicts when `now >= deadline`.
        let now = applied_at_ms.saturating_add(ttl_ms);
        let evicted = buf.sweep_expired(ApplyTimeMs(now));
        // sorted ascending and equal to the sorted unique input.
        let expected: Vec<DocId> = unique.iter().map(|d| DocId(*d)).collect();
        prop_assert_eq!(evicted, expected);
        prop_assert_eq!(buf.len(), 0);

        // sweep before ttl-1 keeps everything (re-apply, then sweep just
        // before deadline).
        if ttl_ms > 1 {
            let (mut buf2, t2, r2) = match fresh_buffer(cap, ttl_ms) {
                Ok(v) => v,
                Err(e) => {
                    prop_assert!(false, "fresh_buffer 2 failed: {}", e);
                    return Ok(());
                }
            };
            for (i, doc) in unique.iter().enumerate() {
                let hash_byte = match u8::try_from(i & 0xFF) {
                Ok(v) => v,
                Err(e) => {
                    prop_assert!(false, "hash_byte conversion failed: {}", e);
                    return Ok(());
                }
            };
                let entry = DirtyEntry {
                    tenant_id: t2.clone(),
                    repo_id: r2.clone(),
                    doc_id: DocId(*doc),
                    generation: ManifestGeneration(1),
                    applied_at_ms: ApplyTimeMs(applied_at_ms),
                    payload_hash: [hash_byte; PAYLOAD_HASH_LEN],
                };
                let _o: ApplyOutcome = match buf2.apply(entry, ApplyTimeMs(applied_at_ms))
                {
                    Ok(v) => v,
                    Err(e) => {
                        prop_assert!(false, "apply failed: {}", e);
                        return Ok(());
                    }
                };
            }
            let pre = applied_at_ms.saturating_add(ttl_ms.saturating_sub(1));
            let evicted_early = buf2.sweep_expired(ApplyTimeMs(pre));
            prop_assert!(evicted_early.is_empty());
            prop_assert_eq!(buf2.len(), unique.len());
        }
    }
}
