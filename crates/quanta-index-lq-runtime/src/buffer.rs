//! Per-(tenant, repo) bounded dirty buffer per RT-01 § 4.6.
//!
//! [`DirtyBuffer`] holds at most `config.capacity` [`DirtyEntry`] rows pinned
//! to a single active [`ManifestGeneration`]. Every reject is typed; every
//! TTL eviction returns the explicit list of evicted [`DocId`]s for the
//! caller's observability rail (no silent drop).
//!
//! Time is caller-supplied via [`ApplyTimeMs`]; the buffer never reads the
//! wall clock. The write coordinator (RT-01 § 4.4) gates concurrent calls
//! through the LEX-04 advisory lock so this type is single-writer at the
//! tenant-repo scope.

use std::collections::BTreeMap;

use crate::apply_changes::{ApplyOutcome, DirtyEntry};
use crate::errors::{RuntimeError, RuntimeErrorCode};
use crate::types::{ApplyTimeMs, BufferConfig, DocId, ManifestGeneration, RepoId, TenantId};

/// The dirty buffer for one `(tenant, repo)` scope at one active generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtyBuffer {
    tenant_id: TenantId,
    repo_id: RepoId,
    config: BufferConfig,
    entries: BTreeMap<DocId, DirtyEntry>,
    active_gen: ManifestGeneration,
}

impl DirtyBuffer {
    /// Build an empty buffer pinned to `active_gen`.
    #[must_use]
    pub fn new(
        tenant_id: TenantId,
        repo_id: RepoId,
        config: BufferConfig,
        active_gen: ManifestGeneration,
    ) -> Self {
        Self {
            tenant_id,
            repo_id,
            config,
            entries: BTreeMap::new(),
            active_gen,
        }
    }

    /// Borrow the tenant scope.
    #[must_use]
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    /// Borrow the repo scope.
    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        &self.repo_id
    }

    /// The currently-pinned active generation.
    #[must_use]
    pub fn active_generation(&self) -> ManifestGeneration {
        self.active_gen
    }

    /// Current entry count (post-sweep state is the caller's concern).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True iff no entries are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Borrow the buffer config.
    #[must_use]
    pub fn config(&self) -> BufferConfig {
        self.config
    }

    /// Apply one inbound `apply_changes` record.
    ///
    /// Order of checks (every reject is typed):
    /// 1. tenant or repo mismatch -> [`RuntimeErrorCode::DirtyBadIdentity`]
    /// 2. `entry.generation` older than the pinned active generation ->
    ///    [`RuntimeErrorCode::DirtyStaleGen`]
    /// 3. sweep expired entries against `now_ms`
    /// 4. existing slot with equal `payload_hash` ->
    ///    [`ApplyOutcome::Idempotent`]
    /// 5. capacity reached after sweep ->
    ///    [`RuntimeErrorCode::DirtyBufferFull`]
    /// 6. otherwise insert and return [`ApplyOutcome::Buffered`]
    pub fn apply(
        &mut self,
        entry: DirtyEntry,
        now_ms: ApplyTimeMs,
    ) -> Result<ApplyOutcome, RuntimeError> {
        if entry.tenant_id != self.tenant_id || entry.repo_id != self.repo_id {
            return Err(RuntimeError::new(
                RuntimeErrorCode::DirtyBadIdentity,
                "entry identity does not match buffer scope",
            ));
        }
        if entry.generation < self.active_gen {
            return Err(RuntimeError::new(
                RuntimeErrorCode::DirtyStaleGen,
                "entry generation older than active generation",
            ));
        }
        let _evicted = self.sweep_expired(now_ms);

        if let Some(existing) = self.entries.get(&entry.doc_id)
            && existing.payload_hash == entry.payload_hash
        {
            return Ok(ApplyOutcome::Idempotent {
                matched_hash: existing.payload_hash,
            });
        }

        let cap_usize = usize::try_from(self.config.capacity()).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorCode::InvalidBufferConfig,
                format!("capacity overflows usize: {err}"),
            )
        })?;
        if !self.entries.contains_key(&entry.doc_id) && self.entries.len() >= cap_usize {
            return Err(RuntimeError::new(
                RuntimeErrorCode::DirtyBufferFull,
                "dirty buffer at capacity",
            ));
        }

        let _prior = self.entries.insert(entry.doc_id, entry);
        Ok(ApplyOutcome::Buffered)
    }

    /// Sweep entries whose `applied_at_ms + ttl_ms` is at or before `now_ms`.
    /// Returns the explicit list of evicted [`DocId`]s in ascending order so
    /// the caller can emit a typed `runtime.dirty.evicted` observability
    /// event — no silent drop.
    pub fn sweep_expired(&mut self, now_ms: ApplyTimeMs) -> Vec<DocId> {
        let ttl = self.config.ttl_ms();
        let now = now_ms.0;
        let mut expired: Vec<DocId> = Vec::new();
        for (doc, entry) in &self.entries {
            let applied = entry.applied_at_ms.0;
            // Compute deadline saturating-add to avoid overflow.
            let deadline = applied.saturating_add(ttl);
            if now >= deadline {
                expired.push(*doc);
            }
        }
        for doc in &expired {
            let _removed = self.entries.remove(doc);
        }
        expired
    }

    /// Iterate over all currently-buffered entries (insertion order is the
    /// sorted [`DocId`] order because the backing store is a [`BTreeMap`]).
    pub fn iter_dirty(&self) -> impl Iterator<Item = &DirtyEntry> {
        self.entries.values()
    }
}

#[cfg(test)]
mod tests {
    use super::DirtyBuffer;
    use crate::apply_changes::{ApplyOutcome, DirtyEntry, PAYLOAD_HASH_LEN};
    use crate::errors::{RuntimeError, RuntimeErrorCode};
    use crate::types::{ApplyTimeMs, BufferConfig, DocId, ManifestGeneration, RepoId, TenantId};

    fn ids() -> Result<(TenantId, RepoId), RuntimeError> {
        Ok((TenantId::new("t1")?, RepoId::new("r1")?))
    }

    fn entry(tenant: &TenantId, repo: &RepoId, doc: u64, gen_: u64, at: u64, h: u8) -> DirtyEntry {
        DirtyEntry {
            tenant_id: tenant.clone(),
            repo_id: repo.clone(),
            doc_id: DocId(doc),
            generation: ManifestGeneration(gen_),
            applied_at_ms: ApplyTimeMs(at),
            payload_hash: [h; PAYLOAD_HASH_LEN],
        }
    }

    fn small_cfg() -> Result<BufferConfig, RuntimeError> {
        BufferConfig::new(2, 100)
    }

    #[test]
    fn apply_buffers_fresh_entry() {
        let (t, r) = match ids() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let cfg = match small_cfg() {
            Ok(c) => c,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf = DirtyBuffer::new(t.clone(), r.clone(), cfg, ManifestGeneration(1));
        let e = entry(&t, &r, 10, 1, 0, 0xAA);
        match buf.apply(e, ApplyTimeMs(1)) {
            Ok(ApplyOutcome::Buffered) => (),
            other => assert!(false, "expected Buffered, got {other:?}"),
        }
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn apply_rejects_bad_identity() {
        let (t, r) = match ids() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let cfg = match small_cfg() {
            Ok(c) => c,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf = DirtyBuffer::new(t, r, cfg, ManifestGeneration(1));
        let other_t = match TenantId::new("t2") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let other_r = match RepoId::new("r2") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let e = entry(&other_t, &other_r, 1, 1, 0, 0);
        match buf.apply(e, ApplyTimeMs(0)) {
            Err(err) => assert_eq!(err.code, RuntimeErrorCode::DirtyBadIdentity),
            Ok(o) => assert!(false, "expected DirtyBadIdentity, got {o:?}"),
        }
    }

    #[test]
    fn apply_rejects_stale_gen() {
        let (t, r) = match ids() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let cfg = match small_cfg() {
            Ok(c) => c,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf = DirtyBuffer::new(t.clone(), r.clone(), cfg, ManifestGeneration(5));
        let e = entry(&t, &r, 1, 4, 0, 0);
        match buf.apply(e, ApplyTimeMs(0)) {
            Err(err) => assert_eq!(err.code, RuntimeErrorCode::DirtyStaleGen),
            Ok(o) => assert!(false, "expected DirtyStaleGen, got {o:?}"),
        }
    }

    #[test]
    fn apply_idempotent_on_equal_hash() {
        let (t, r) = match ids() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let cfg = match small_cfg() {
            Ok(c) => c,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf = DirtyBuffer::new(t.clone(), r.clone(), cfg, ManifestGeneration(1));
        let e = entry(&t, &r, 1, 1, 0, 0xAB);
        match buf.apply(e.clone(), ApplyTimeMs(1)) {
            Ok(ApplyOutcome::Buffered) => (),
            o => {
                assert!(false, "expected Buffered, got {o:?}");
                return;
            }
        }
        match buf.apply(e, ApplyTimeMs(2)) {
            Ok(ApplyOutcome::Idempotent { matched_hash }) => {
                assert_eq!(matched_hash, [0xABu8; PAYLOAD_HASH_LEN]);
            }
            o => assert!(false, "expected Idempotent, got {o:?}"),
        }
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn apply_full_after_sweep() {
        let (t, r) = match ids() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let cfg = match BufferConfig::new(2, 10_000) {
            Ok(c) => c,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf = DirtyBuffer::new(t.clone(), r.clone(), cfg, ManifestGeneration(1));
        let _o1: ApplyOutcome = match buf.apply(entry(&t, &r, 1, 1, 0, 1), ApplyTimeMs(0)) {
            Ok(o) => o,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let _o2: ApplyOutcome = match buf.apply(entry(&t, &r, 2, 1, 0, 2), ApplyTimeMs(0)) {
            Ok(o) => o,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match buf.apply(entry(&t, &r, 3, 1, 0, 3), ApplyTimeMs(0)) {
            Err(err) => assert_eq!(err.code, RuntimeErrorCode::DirtyBufferFull),
            Ok(o) => assert!(false, "expected DirtyBufferFull, got {o:?}"),
        }
    }

    #[test]
    fn sweep_returns_evicted_doc_ids() {
        let (t, r) = match ids() {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let cfg = match BufferConfig::new(4, 100) {
            Ok(c) => c,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let mut buf = DirtyBuffer::new(t.clone(), r.clone(), cfg, ManifestGeneration(1));
        let _o1: ApplyOutcome = match buf.apply(entry(&t, &r, 1, 1, 0, 1), ApplyTimeMs(0)) {
            Ok(o) => o,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let _o2: ApplyOutcome = match buf.apply(entry(&t, &r, 2, 1, 50, 2), ApplyTimeMs(50)) {
            Ok(o) => o,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        // doc 1 expires at 0+100=100; doc 2 expires at 50+100=150
        // sweep at now=120 evicts doc 1 only.
        let evicted = buf.sweep_expired(ApplyTimeMs(120));
        assert_eq!(evicted, vec![DocId(1)]);
        assert_eq!(buf.len(), 1);
    }
}
