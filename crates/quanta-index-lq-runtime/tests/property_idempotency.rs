//! Property: idempotent re-apply N times of an equal-hash [`DirtyEntry`]
//! occupies exactly one buffer slot per `(tenant, repo, doc_id)`.

use proptest::prelude::*;
use quanta_index_lq_runtime::{
    ApplyOutcome, ApplyTimeMs, BufferConfig, DirtyBuffer, DirtyEntry, DocId, ManifestGeneration,
    PAYLOAD_HASH_LEN, RepoId, RuntimeError, TenantId,
};

fn fixture(doc: u64, hash_byte: u8) -> Result<(DirtyBuffer, DirtyEntry), RuntimeError> {
    let t = TenantId::new("t")?;
    let r = RepoId::new("r")?;
    let cfg = BufferConfig::new(8, 1_000_000)?;
    let buf = DirtyBuffer::new(t.clone(), r.clone(), cfg, ManifestGeneration(1));
    let e = DirtyEntry {
        tenant_id: t,
        repo_id: r,
        doc_id: DocId(doc),
        generation: ManifestGeneration(1),
        applied_at_ms: ApplyTimeMs(0),
        payload_hash: [hash_byte; PAYLOAD_HASH_LEN],
    };
    Ok((buf, e))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    #[test]
    fn repeated_apply_occupies_one_slot(
        doc in 0u64..1024,
        hash_byte in 0u8..255,
        n in 1usize..32,
    ) {
        let (mut buf, entry) = match fixture(doc, hash_byte) {
            Ok(v) => v,
            Err(e) => {
                prop_assert!(false, "fixture setup failed: {e}");
                return Ok(());
            }
        };
        let mut idempotent_seen = 0usize;
        let mut buffered_seen = 0usize;
        for i in 0..n {
            let raw = match u64::try_from(i) {
                Ok(v) => v,
                Err(e) => {
                    prop_assert!(false, "iteration index out of u64: {}", e);
                    return Ok(());
                }
            };
            let now = ApplyTimeMs(raw);
            match buf.apply(entry.clone(), now) {
                Ok(ApplyOutcome::Buffered) => {
                    buffered_seen = buffered_seen.saturating_add(1);
                }
                Ok(ApplyOutcome::Idempotent { matched_hash }) => {
                    prop_assert_eq!(matched_hash, [hash_byte; PAYLOAD_HASH_LEN]);
                    idempotent_seen = idempotent_seen.saturating_add(1);
                }
                Ok(other) => {
                    prop_assert!(false, "unexpected outcome {:?}", other);
                }
                Err(e) => {
                    prop_assert!(false, "apply failed: {}", e);
                }
            }
            prop_assert_eq!(buf.len(), 1);
        }
        prop_assert_eq!(buffered_seen, 1);
        prop_assert_eq!(idempotent_seen, n.saturating_sub(1));
    }
}
