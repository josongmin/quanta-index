//! Golden scenario for RT-01: a fixed sequence of `apply_changes` packets
//! exercising buffered, stale-gen reject, buffer-full reject, idempotent
//! re-apply, and TTL sweep — every transition typed, none silent.

use quanta_index_lq_runtime::{
    ApplyOutcome, ApplyTimeMs, BufferConfig, DirtyBuffer, DirtyEntry, DocId, ManifestGeneration,
    PAYLOAD_HASH_LEN, RepoId, RuntimeErrorCode, TenantId, dirty_docs,
};

fn make_entry(t: &TenantId, r: &RepoId, doc: u64, gen_: u64, at: u64, h: u8) -> DirtyEntry {
    DirtyEntry {
        tenant_id: t.clone(),
        repo_id: r.clone(),
        doc_id: DocId(doc),
        generation: ManifestGeneration(gen_),
        applied_at_ms: ApplyTimeMs(at),
        payload_hash: [h; PAYLOAD_HASH_LEN],
    }
}

#[test]
fn golden_apply_changes_sequence() {
    let tenant = match TenantId::new("acme") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let repo = match RepoId::new("infra") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let cfg = match BufferConfig::new(2, 500) {
        Ok(c) => c,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    let active = ManifestGeneration(10);
    let mut buf = DirtyBuffer::new(tenant.clone(), repo.clone(), cfg, active);

    // Packet 1 — fresh entry at active gen, accepted into the buffer.
    let p1 = make_entry(&tenant, &repo, 1, 10, 0, 0xA1);
    match buf.apply(p1, ApplyTimeMs(0)) {
        Ok(ApplyOutcome::Buffered) => (),
        other => {
            assert!(false, "packet 1 expected Buffered, got {other:?}");
            return;
        }
    }

    // Packet 2 — fresh entry, accepted; buffer at capacity (2).
    let p2 = make_entry(&tenant, &repo, 2, 10, 100, 0xA2);
    match buf.apply(p2, ApplyTimeMs(100)) {
        Ok(ApplyOutcome::Buffered) => (),
        other => {
            assert!(false, "packet 2 expected Buffered, got {other:?}");
            return;
        }
    }
    assert_eq!(buf.len(), 2);

    // Packet 3 — stale generation reject. Nothing buffered.
    let p3 = make_entry(&tenant, &repo, 3, 9, 110, 0xA3);
    match buf.apply(p3, ApplyTimeMs(110)) {
        Err(err) => assert_eq!(err.code, RuntimeErrorCode::DirtyStaleGen),
        Ok(o) => {
            assert!(false, "packet 3 expected DirtyStaleGen, got {o:?}");
            return;
        }
    }
    assert_eq!(buf.len(), 2);

    // Packet 4 — buffer-full reject. The buffer is at cap=2 and this is a
    // new doc_id within active gen + ttl window.
    let p4 = make_entry(&tenant, &repo, 4, 10, 120, 0xA4);
    match buf.apply(p4, ApplyTimeMs(120)) {
        Err(err) => assert_eq!(err.code, RuntimeErrorCode::DirtyBufferFull),
        Ok(o) => {
            assert!(false, "packet 4 expected DirtyBufferFull, got {o:?}");
            return;
        }
    }
    assert_eq!(buf.len(), 2);

    // Packet 5 — idempotent re-apply of packet 1.
    let p5 = make_entry(&tenant, &repo, 1, 10, 130, 0xA1);
    match buf.apply(p5, ApplyTimeMs(130)) {
        Ok(ApplyOutcome::Idempotent { matched_hash }) => {
            assert_eq!(matched_hash, [0xA1u8; PAYLOAD_HASH_LEN]);
        }
        other => {
            assert!(false, "packet 5 expected Idempotent, got {other:?}");
            return;
        }
    }
    assert_eq!(buf.len(), 2);

    // TTL sweep at t=700ms. ttl=500ms; entry 1 deadline=500, entry 2
    // deadline=600 — both expired by 700.
    let evicted = buf.sweep_expired(ApplyTimeMs(700));
    assert_eq!(evicted, vec![DocId(1), DocId(2)]);
    assert_eq!(buf.len(), 0);

    // After sweep, `dirty_docs` at the active gen is empty (still pinned).
    match dirty_docs(&buf, ManifestGeneration(10)) {
        Ok(v) => assert!(v.is_empty()),
        Err(e) => assert!(false, "post-sweep dirty_docs failed: {e}"),
    }
}
