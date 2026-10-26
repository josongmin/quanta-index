//! Runtime-owned logical encoded-file quota for the lexical object pool.
//!
//! The seed must be inventoried under the held state-root lease before any
//! writer is exposed. This does not reserve filesystem blocks, process RSS,
//! control-file space, or I/O bandwidth.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use quanta_index_core::pool_write_admission::{
    PendingOrphanUnlink, PoolWriteAdmissionPort, PoolWritePermit, PoolWriteUsage,
};
use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE};

pub(super) type PoolFileIdentity = [u8; 32];
type Identity = PoolFileIdentity;

#[derive(Clone, Copy)]
struct PendingCharge {
    object: Identity,
    bytes: u64,
    temp: Option<Identity>,
}

struct LedgerState {
    canonical: BTreeMap<Identity, u64>,
    pending_canonical_unlinks: BTreeMap<Identity, u64>,
    orphans: BTreeMap<Identity, u64>,
    pending_orphan_unlinks: BTreeMap<Identity, PendingOrphanUnlink>,
    pending: BTreeMap<u64, PendingCharge>,
    temp_owners: BTreeMap<Identity, u64>,
    next_token: u64,
    canonical_bytes: u64,
    in_flight_bytes: u64,
    orphan_bytes: u64,
    total_charged_bytes: u64,
    boot_sealed: bool,
    poisoned: bool,
}

pub(super) struct PoolWriteQuota {
    ceiling_bytes: u64,
    max_files: u64,
    state: Arc<Mutex<LedgerState>>,
}

impl Clone for PoolWriteQuota {
    fn clone(&self) -> Self {
        // A cloned quota may be published to another writer. Even if the
        // clone is later dropped, boot inventory must stay closed.
        match self.state.lock() {
            Ok(mut state) => state.boot_sealed = true,
            Err(poisoned) => poisoned.into_inner().poisoned = true,
        }
        Self {
            ceiling_bytes: self.ceiling_bytes,
            max_files: self.max_files,
            state: Arc::clone(&self.state),
        }
    }
}

#[derive(Clone, Copy)]
enum PermitState {
    Existing { object: Identity, bytes: u64 },
    Pending(u64),
    Done,
}

struct QuotaPermit {
    state: Arc<Mutex<LedgerState>>,
    status: PermitState,
}

fn limited(message: &str) -> CoreError {
    CoreError::Typed {
        code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
        message: format!("lexical pool write quota: {message}"),
    }
}

fn damaged(message: &str) -> CoreError {
    CoreError::Storage(format!("lexical pool write quota ledger: {message}"))
}

fn poison(state: &mut LedgerState, message: &str) -> CoreError {
    state.poisoned = true;
    damaged(message)
}

fn lock_state(state: &Mutex<LedgerState>) -> Result<MutexGuard<'_, LedgerState>, CoreError> {
    match state.lock() {
        Ok(guard) if guard.poisoned => Err(damaged("poisoned")),
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            guard.poisoned = true;
            Err(damaged("mutex poisoned"))
        }
    }
}

fn pending(state: &mut LedgerState, token: u64) -> Result<PendingCharge, CoreError> {
    state
        .pending
        .get(&token)
        .copied()
        .map_or_else(|| Err(poison(state, "pending token is absent")), Ok)
}

fn charged_file_count(state: &LedgerState) -> Result<u64, CoreError> {
    let canonical = u64::try_from(state.canonical.len())
        .map_err(|error| limited(&format!("canonical file count overflow: {error}")))?;
    let orphans = u64::try_from(state.orphans.len())
        .map_err(|error| limited(&format!("orphan file count overflow: {error}")))?;
    let pending = u64::try_from(state.pending.len())
        .map_err(|error| limited(&format!("pending file count overflow: {error}")))?;
    canonical
        .checked_add(orphans)
        .and_then(|sum| sum.checked_add(pending))
        .ok_or_else(|| limited("charged file count overflow"))
}

impl PoolWriteQuota {
    /// `canonical` and `orphans` are exact `(identity, encoded-file bytes)`
    /// inventory rows, supplied after startup cleanup or independently
    /// classified on-disk inventory. Duplicate/conflicting rows are refused.
    pub(super) fn new_with_file_limit(
        ceiling_bytes: u64,
        max_files: u64,
        canonical: impl IntoIterator<Item = (Identity, u64)>,
        orphans: impl IntoIterator<Item = (Identity, u64)>,
    ) -> Result<Self, CoreError> {
        if ceiling_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "lexical pool write quota ceiling must be positive".into(),
            ));
        }
        if max_files == 0 {
            return Err(CoreError::InvalidContract(
                "lexical pool write quota file ceiling must be positive".into(),
            ));
        }
        let mut quota = Self {
            ceiling_bytes,
            max_files,
            state: Arc::new(Mutex::new(LedgerState {
                canonical: BTreeMap::new(),
                pending_canonical_unlinks: BTreeMap::new(),
                orphans: BTreeMap::new(),
                pending_orphan_unlinks: BTreeMap::new(),
                pending: BTreeMap::new(),
                temp_owners: BTreeMap::new(),
                next_token: 1,
                canonical_bytes: 0,
                in_flight_bytes: 0,
                orphan_bytes: 0,
                total_charged_bytes: 0,
                boot_sealed: false,
                poisoned: false,
            })),
        };
        for (id, bytes) in canonical {
            quota.record_boot_canonical(id, bytes)?;
        }
        for (id, bytes) in orphans {
            quota.record_boot_orphan(id, bytes)?;
        }
        Ok(quota)
    }

    #[cfg(test)]
    fn new(
        ceiling_bytes: u64,
        canonical: impl IntoIterator<Item = (Identity, u64)>,
        orphans: impl IntoIterator<Item = (Identity, u64)>,
    ) -> Result<Self, CoreError> {
        Self::new_with_file_limit(ceiling_bytes, u64::MAX, canonical, orphans)
    }

    /// Stream one authenticated canonical inventory row while startup still
    /// owns this quota exclusively under the state-root lease.
    pub(super) fn record_boot_canonical(
        &mut self,
        id: Identity,
        bytes: u64,
    ) -> Result<(), CoreError> {
        self.record_boot(id, bytes, false)
    }

    /// Stream one authenticated orphan inventory row. Zero-length crash
    /// temporaries must be unlinked and their directory synced before seed.
    pub(super) fn record_boot_orphan(&mut self, id: Identity, bytes: u64) -> Result<(), CoreError> {
        self.record_boot(id, bytes, true)
    }

    fn record_boot(&mut self, id: Identity, bytes: u64, orphan: bool) -> Result<(), CoreError> {
        let state = Arc::get_mut(&mut self.state)
            .ok_or_else(|| {
                CoreError::InvalidContract("pool quota boot seed already shared".into())
            })?
            .get_mut()
            .map_err(|error| damaged(&format!("pool quota boot seed mutex poisoned: {error}")))?;
        if state.poisoned || state.boot_sealed || !state.pending.is_empty() || state.next_token != 1
        {
            return Err(CoreError::InvalidContract(
                "pool quota boot seed after writer exposure".into(),
            ));
        }
        if bytes == 0 || state.canonical.contains_key(&id) || state.orphans.contains_key(&id) {
            return Err(poison(
                state,
                "duplicate, conflicting, or zero-width boot seed",
            ));
        }
        let next_files = match charged_file_count(state).and_then(|count| {
            count
                .checked_add(1)
                .ok_or_else(|| limited("boot inventory file count overflow"))
        }) {
            Ok(count) => count,
            Err(error) => {
                state.poisoned = true;
                return Err(error);
            }
        };
        if next_files > self.max_files {
            state.poisoned = true;
            return Err(limited("boot inventory exceeds file ceiling"));
        }
        let Some(next_total) = state.total_charged_bytes.checked_add(bytes) else {
            state.poisoned = true;
            return Err(limited("boot inventory bytes overflow"));
        };
        if next_total > self.ceiling_bytes {
            state.poisoned = true;
            return Err(limited("boot inventory exceeds ceiling"));
        }
        if orphan {
            let Some(next_orphan) = state.orphan_bytes.checked_add(bytes) else {
                state.poisoned = true;
                return Err(limited("orphan seed bytes overflow"));
            };
            let _previous = state.orphans.insert(id, bytes);
            state.orphan_bytes = next_orphan;
        } else {
            let Some(next_canonical) = state.canonical_bytes.checked_add(bytes) else {
                state.poisoned = true;
                return Err(limited("canonical seed bytes overflow"));
            };
            let _previous = state.canonical.insert(id, bytes);
            state.canonical_bytes = next_canonical;
        }
        state.total_charged_bytes = next_total;
        Ok(())
    }
}

impl PoolWriteAdmissionPort for PoolWriteQuota {
    fn try_reserve(
        &self,
        object_sha256: Identity,
        encoded_bytes: u64,
    ) -> Result<Box<dyn PoolWritePermit>, CoreError> {
        if encoded_bytes == 0 {
            return Err(CoreError::InvalidContract(
                "lexical pool object encoded width must be positive".into(),
            ));
        }
        let mut state = lock_state(&self.state)?;
        state.boot_sealed = true;
        if state.pending_canonical_unlinks.contains_key(&object_sha256) {
            return Err(damaged("canonical unlink awaits shard directory sync"));
        }
        if let Some(existing) = state.canonical.get(&object_sha256) {
            if *existing != encoded_bytes {
                return Err(poison(&mut state, "canonical digest width conflicts"));
            }
            let permit: Box<dyn PoolWritePermit> = Box::new(QuotaPermit {
                state: Arc::clone(&self.state),
                status: PermitState::Existing {
                    object: object_sha256,
                    bytes: encoded_bytes,
                },
            });
            drop(state);
            return Ok(permit);
        }
        let next_files = charged_file_count(&state)?
            .checked_add(1)
            .ok_or_else(|| limited("admission file count overflow"))?;
        if next_files > self.max_files {
            return Err(limited("encoded object would exceed file ceiling"));
        }
        let next_total = state
            .total_charged_bytes
            .checked_add(encoded_bytes)
            .ok_or_else(|| limited("admission byte count overflow"))?;
        if next_total > self.ceiling_bytes {
            return Err(limited("encoded object would exceed ceiling"));
        }
        let next_in_flight = state
            .in_flight_bytes
            .checked_add(encoded_bytes)
            .ok_or_else(|| limited("in-flight byte count overflow"))?;
        let token = state.next_token;
        let next_token = token
            .checked_add(1)
            .ok_or_else(|| limited("permit identity exhausted"))?;
        state.next_token = next_token;
        state.total_charged_bytes = next_total;
        state.in_flight_bytes = next_in_flight;
        let _previous = state.pending.insert(
            token,
            PendingCharge {
                object: object_sha256,
                bytes: encoded_bytes,
                temp: None,
            },
        );
        let permit: Box<dyn PoolWritePermit> = Box::new(QuotaPermit {
            state: Arc::clone(&self.state),
            status: PermitState::Pending(token),
        });
        drop(state);
        Ok(permit)
    }

    fn canonical_unlinked_pending_sync(
        &self,
        object_sha256: Identity,
        encoded_bytes: u64,
    ) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        if encoded_bytes == 0
            || state.canonical.get(&object_sha256) != Some(&encoded_bytes)
            || state.pending_canonical_unlinks.contains_key(&object_sha256)
        {
            return Err(poison(
                &mut state,
                "pending canonical unlink identity, width, or state differs",
            ));
        }
        let _previous = state
            .pending_canonical_unlinks
            .insert(object_sha256, encoded_bytes);
        drop(state);
        Ok(())
    }

    fn pending_canonical_unlinks(
        &self,
        max_objects: usize,
    ) -> Result<Vec<(Identity, u64)>, CoreError> {
        if max_objects == 0 {
            return Err(CoreError::InvalidContract(
                "pending canonical unlink batch must be positive".into(),
            ));
        }
        let state = lock_state(&self.state)?;
        let mut result = Vec::new();
        result
            .try_reserve(max_objects.min(state.pending_canonical_unlinks.len()))
            .map_err(|error| damaged(&format!("reserve pending canonical unlinks: {error}")))?;
        result.extend(
            state
                .pending_canonical_unlinks
                .iter()
                .take(max_objects)
                .map(|(id, bytes)| (*id, *bytes)),
        );
        drop(state);
        Ok(result)
    }

    fn confirmed_canonical_unlink(
        &self,
        object_sha256: Identity,
        encoded_bytes: u64,
    ) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        if encoded_bytes == 0
            || state.canonical.get(&object_sha256) != Some(&encoded_bytes)
            || state
                .pending_canonical_unlinks
                .get(&object_sha256)
                .is_some_and(|pending| *pending != encoded_bytes)
        {
            return Err(poison(
                &mut state,
                "canonical unlink identity or width differs",
            ));
        }
        let Some(next_canonical) = state.canonical_bytes.checked_sub(encoded_bytes) else {
            return Err(poison(&mut state, "canonical unlink byte underflow"));
        };
        let Some(next_total) = state.total_charged_bytes.checked_sub(encoded_bytes) else {
            return Err(poison(&mut state, "canonical unlink total underflow"));
        };
        let _removed = state.canonical.remove(&object_sha256);
        let _pending = state.pending_canonical_unlinks.remove(&object_sha256);
        state.canonical_bytes = next_canonical;
        state.total_charged_bytes = next_total;
        drop(state);
        Ok(())
    }

    fn confirmed_orphan_unlink(
        &self,
        temp_identity: Identity,
        encoded_bytes: u64,
    ) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        if encoded_bytes == 0
            || state.orphans.get(&temp_identity) != Some(&encoded_bytes)
            || state
                .pending_orphan_unlinks
                .get(&temp_identity)
                .is_some_and(|pending| pending.encoded_bytes != encoded_bytes)
        {
            return Err(poison(
                &mut state,
                "orphan unlink identity or width differs",
            ));
        }
        let Some(next_orphan) = state.orphan_bytes.checked_sub(encoded_bytes) else {
            return Err(poison(&mut state, "orphan unlink byte underflow"));
        };
        let Some(next_total) = state.total_charged_bytes.checked_sub(encoded_bytes) else {
            return Err(poison(&mut state, "orphan unlink total underflow"));
        };
        let _removed = state.orphans.remove(&temp_identity);
        let _pending = state.pending_orphan_unlinks.remove(&temp_identity);
        state.orphan_bytes = next_orphan;
        state.total_charged_bytes = next_total;
        drop(state);
        Ok(())
    }

    fn orphan_unlinked_pending_sync(
        &self,
        temp_identity: Identity,
        encoded_bytes: u64,
        temp_name: &str,
    ) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        if encoded_bytes == 0
            || state.orphans.get(&temp_identity) != Some(&encoded_bytes)
            || state.pending_orphan_unlinks.contains_key(&temp_identity)
            || temp_name.is_empty()
            || temp_name.len() > 128
            || !temp_name
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'/' | b'\\'))
        {
            return Err(poison(
                &mut state,
                "pending orphan unlink identity, width, name, or state differs",
            ));
        }
        let _previous = state.pending_orphan_unlinks.insert(
            temp_identity,
            PendingOrphanUnlink {
                temp_identity,
                encoded_bytes,
                temp_name: temp_name.to_owned(),
            },
        );
        drop(state);
        Ok(())
    }

    fn pending_orphan_unlinks(
        &self,
        max_objects: usize,
    ) -> Result<Vec<PendingOrphanUnlink>, CoreError> {
        if max_objects == 0 {
            return Err(CoreError::InvalidContract(
                "pending orphan unlink batch must be positive".into(),
            ));
        }
        let state = lock_state(&self.state)?;
        let mut result = Vec::new();
        result
            .try_reserve(max_objects.min(state.pending_orphan_unlinks.len()))
            .map_err(|error| damaged(&format!("reserve pending orphan unlinks: {error}")))?;
        result.extend(
            state
                .pending_orphan_unlinks
                .values()
                .take(max_objects)
                .cloned(),
        );
        drop(state);
        Ok(result)
    }

    fn orphan_charge(&self, temp_identity: Identity) -> Result<Option<u64>, CoreError> {
        let state = lock_state(&self.state)?;
        let charge = state.orphans.get(&temp_identity).copied();
        drop(state);
        Ok(charge)
    }

    fn snapshot(&self) -> Result<PoolWriteUsage, CoreError> {
        let mut state = lock_state(&self.state)?;
        if state
            .pending_canonical_unlinks
            .iter()
            .any(|(id, bytes)| state.canonical.get(id) != Some(bytes))
        {
            return Err(poison(
                &mut state,
                "pending canonical unlink differs from charged file",
            ));
        }
        if state.pending_orphan_unlinks.iter().any(|(id, pending)| {
            pending.temp_identity != *id || state.orphans.get(id) != Some(&pending.encoded_bytes)
        }) {
            return Err(poison(
                &mut state,
                "pending orphan unlink differs from charged temporary",
            ));
        }
        let charged_files = charged_file_count(&state)?;
        if charged_files > self.max_files {
            return Err(poison(&mut state, "charged file count invariant differs"));
        }
        if state
            .canonical_bytes
            .checked_add(state.in_flight_bytes)
            .and_then(|sum| sum.checked_add(state.orphan_bytes))
            != Some(state.total_charged_bytes)
        {
            return Err(poison(&mut state, "aggregate byte invariant differs"));
        }
        let usage = PoolWriteUsage {
            file_limit: self.max_files,
            charged_files,
            ceiling_bytes: self.ceiling_bytes,
            canonical_bytes: state.canonical_bytes,
            in_flight_bytes: state.in_flight_bytes,
            orphan_bytes: state.orphan_bytes,
            total_charged_bytes: state.total_charged_bytes,
        };
        drop(state);
        Ok(usage)
    }
}

impl PoolWritePermit for QuotaPermit {
    fn check_new_file_admitted(&self) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        let PermitState::Pending(token) = self.status else {
            return Err(poison(
                &mut state,
                "file creation requires a fresh reservation",
            ));
        };
        if pending(&mut state, token)?.temp.is_some() {
            return Err(poison(
                &mut state,
                "file creation repeats a created temporary",
            ));
        }
        drop(state);
        Ok(())
    }

    fn temp_created(&mut self, temp_identity: Identity) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        let PermitState::Pending(token) = self.status else {
            return Err(poison(&mut state, "temp creation repeated or not reserved"));
        };
        let charge = pending(&mut state, token)?;
        if charge.temp.is_some()
            || state.temp_owners.contains_key(&temp_identity)
            || state.orphans.contains_key(&temp_identity)
            || state.canonical.contains_key(&temp_identity)
        {
            return Err(poison(
                &mut state,
                "temporary identity conflicts or repeats",
            ));
        }
        let Some(row) = state.pending.get_mut(&token) else {
            return Err(poison(&mut state, "pending token disappeared"));
        };
        row.temp = Some(temp_identity);
        let _previous = state.temp_owners.insert(temp_identity, token);
        drop(state);
        Ok(())
    }

    fn canonical_renamed(&mut self) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        let PermitState::Pending(token) = self.status else {
            return Err(poison(
                &mut state,
                "canonical rename repeated or not reserved",
            ));
        };
        let charge = pending(&mut state, token)?;
        let Some(temp_identity) = charge.temp else {
            return Err(poison(
                &mut state,
                "canonical rename has no created temporary",
            ));
        };
        if state.temp_owners.get(&temp_identity) != Some(&token)
            || state.canonical.contains_key(&charge.object)
        {
            return Err(poison(&mut state, "canonical rename identity conflicts"));
        }
        let Some(next_in_flight) = state.in_flight_bytes.checked_sub(charge.bytes) else {
            return Err(poison(&mut state, "canonical rename in-flight underflow"));
        };
        let Some(next_canonical) = state.canonical_bytes.checked_add(charge.bytes) else {
            return Err(poison(&mut state, "canonical rename byte overflow"));
        };
        let _removed = state.pending.remove(&token);
        let _removed_temp = state.temp_owners.remove(&temp_identity);
        let _previous = state.canonical.insert(charge.object, charge.bytes);
        state.in_flight_bytes = next_in_flight;
        state.canonical_bytes = next_canonical;
        self.status = PermitState::Done;
        drop(state);
        Ok(())
    }

    fn temp_unlinked_and_synced(&mut self) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        let PermitState::Pending(token) = self.status else {
            return Err(poison(
                &mut state,
                "temporary cleanup repeated or not reserved",
            ));
        };
        let charge = pending(&mut state, token)?;
        let Some(temp_identity) = charge.temp else {
            return Err(poison(&mut state, "temporary cleanup has no created file"));
        };
        if state.temp_owners.get(&temp_identity) != Some(&token) {
            return Err(poison(&mut state, "temporary cleanup identity differs"));
        }
        let Some(next_in_flight) = state.in_flight_bytes.checked_sub(charge.bytes) else {
            return Err(poison(&mut state, "temporary cleanup in-flight underflow"));
        };
        let Some(next_total) = state.total_charged_bytes.checked_sub(charge.bytes) else {
            return Err(poison(&mut state, "temporary cleanup total underflow"));
        };
        let _removed = state.pending.remove(&token);
        let _removed_temp = state.temp_owners.remove(&temp_identity);
        state.in_flight_bytes = next_in_flight;
        state.total_charged_bytes = next_total;
        self.status = PermitState::Done;
        drop(state);
        Ok(())
    }

    fn reused_verified(&mut self) -> Result<(), CoreError> {
        let mut state = lock_state(&self.state)?;
        match self.status {
            PermitState::Existing { object, bytes } => {
                if state.canonical.get(&object) != Some(&bytes) {
                    return Err(poison(&mut state, "reused canonical identity differs"));
                }
            }
            PermitState::Pending(token) => {
                let charge = pending(&mut state, token)?;
                if charge.temp.is_some()
                    || state.canonical.get(&charge.object) != Some(&charge.bytes)
                {
                    return Err(poison(
                        &mut state,
                        "reused object lacks matching canonical file",
                    ));
                }
                let Some(next_in_flight) = state.in_flight_bytes.checked_sub(charge.bytes) else {
                    return Err(poison(&mut state, "reuse in-flight underflow"));
                };
                let Some(next_total) = state.total_charged_bytes.checked_sub(charge.bytes) else {
                    return Err(poison(&mut state, "reuse total underflow"));
                };
                let _removed = state.pending.remove(&token);
                state.in_flight_bytes = next_in_flight;
                state.total_charged_bytes = next_total;
            }
            PermitState::Done => return Err(poison(&mut state, "reuse callback repeated")),
        }
        self.status = PermitState::Done;
        drop(state);
        Ok(())
    }
}

impl Drop for QuotaPermit {
    fn drop(&mut self) {
        let PermitState::Pending(token) = self.status else {
            return;
        };
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                poisoned.into_inner().poisoned = true;
                return;
            }
        };
        if state.poisoned {
            return;
        }
        let Some(charge) = state.pending.get(&token).copied() else {
            state.poisoned = true;
            return;
        };
        let Some(next_in_flight) = state.in_flight_bytes.checked_sub(charge.bytes) else {
            state.poisoned = true;
            return;
        };
        if let Some(temp_identity) = charge.temp {
            let Some(next_orphan) = state.orphan_bytes.checked_add(charge.bytes) else {
                state.poisoned = true;
                return;
            };
            if state.temp_owners.get(&temp_identity) != Some(&token)
                || state.orphans.contains_key(&temp_identity)
            {
                state.poisoned = true;
                return;
            }
            let _removed = state.pending.remove(&token);
            let _removed_temp = state.temp_owners.remove(&temp_identity);
            let _previous = state.orphans.insert(temp_identity, charge.bytes);
            state.in_flight_bytes = next_in_flight;
            state.orphan_bytes = next_orphan;
        } else {
            let Some(next_total) = state.total_charged_bytes.checked_sub(charge.bytes) else {
                state.poisoned = true;
                return;
            };
            let _removed = state.pending.remove(&token);
            state.in_flight_bytes = next_in_flight;
            state.total_charged_bytes = next_total;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::{MetricSourcePort, MetricValueV1};
    use quanta_index_lexical::LexicalAdapter;

    fn gauge(adapter: &LexicalAdapter, name: &str) -> Option<f64> {
        adapter
            .scrape()
            .expect("healthy lexical scrape")
            .into_iter()
            .find_map(|point| match point.value {
                MetricValueV1::Gauge(value) if point.name == name => Some(value),
                MetricValueV1::Counter(_) | MetricValueV1::Gauge(_) => None,
            })
    }

    #[test]
    fn lexical_pool_metrics_follow_quota_lifecycle_and_refuse_damaged_ledger() {
        let root = tempfile::tempdir().expect("adapter root");
        let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
        assert_eq!(gauge(&adapter, "lexical_pool_quota_enabled"), Some(0.0));
        assert_eq!(gauge(&adapter, "lexical_pool_quota_ceiling_bytes"), None);

        let quota = Arc::new(
            PoolWriteQuota::new_with_file_limit(20, 3, [([1; 32], 10)], [])
                .expect("boot inventory"),
        );
        let port: Arc<dyn PoolWriteAdmissionPort> = quota.clone();
        let adapter = adapter.with_pool_write_admission(port);
        assert_eq!(gauge(&adapter, "lexical_pool_quota_enabled"), Some(1.0));
        assert_eq!(gauge(&adapter, "lexical_pool_quota_file_limit"), Some(3.0));
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_charged_files"),
            Some(1.0)
        );
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_canonical_bytes"),
            Some(10.0)
        );
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_available_bytes"),
            Some(10.0)
        );

        let mut permit = quota.try_reserve([2; 32], 6).expect("fresh object");
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_in_flight_bytes"),
            Some(6.0)
        );
        permit.temp_created([9; 32]).expect("created temporary");
        drop(permit);
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_in_flight_bytes"),
            Some(0.0)
        );
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_orphan_bytes"),
            Some(6.0)
        );
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_charged_bytes"),
            Some(16.0)
        );
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_available_bytes"),
            Some(4.0)
        );

        quota
            .confirmed_orphan_unlink([9; 32], 6)
            .expect("durable orphan cleanup");
        quota
            .confirmed_canonical_unlink([1; 32], 10)
            .expect("durable canonical cleanup");
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_charged_files"),
            Some(0.0)
        );
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_charged_bytes"),
            Some(0.0)
        );
        assert_eq!(
            gauge(&adapter, "lexical_pool_quota_available_bytes"),
            Some(20.0)
        );
        assert!(quota.confirmed_canonical_unlink([1; 32], 10).is_err());
        assert!(adapter.scrape().is_err());
    }

    fn quota(ceiling: u64) -> PoolWriteQuota {
        PoolWriteQuota::new(ceiling, [], []).expect("empty boot inventory")
    }

    fn usage(quota: &PoolWriteQuota) -> PoolWriteUsage {
        quota.snapshot().expect("healthy quota snapshot")
    }

    #[test]
    fn canonical_reuse_and_gc_release_are_identity_bound() {
        let quota = PoolWriteQuota::new(20, [([1; 32], 10)], []).expect("canonical seed");
        let mut reused = quota.try_reserve([1; 32], 10).expect("reuse permit");
        assert_eq!(usage(&quota).total_charged_bytes, 10);
        reused.reused_verified().expect("authenticated reuse");
        drop(reused);
        assert_eq!(usage(&quota).canonical_bytes, 10);
        assert!(quota.confirmed_canonical_unlink([1; 32], 9).is_err());
        assert!(matches!(
            quota.try_reserve([2; 32], 1),
            Err(CoreError::Storage(_))
        ));

        let quota = PoolWriteQuota::new(20, [([1; 32], 10)], []).expect("fresh canonical seed");
        quota
            .confirmed_canonical_unlink([1; 32], 10)
            .expect("verified unlink and sync");
        assert_eq!(usage(&quota).total_charged_bytes, 0);
        assert!(quota.confirmed_canonical_unlink([1; 32], 10).is_err());
        assert!(matches!(quota.snapshot(), Err(CoreError::Storage(_))));
    }

    #[test]
    fn pending_canonical_sync_keeps_charge_and_refuses_reuse_without_poison() {
        let quota = PoolWriteQuota::new(20, [([1; 32], 10), ([2; 32], 4)], [])
            .expect("canonical inventory");
        quota
            .canonical_unlinked_pending_sync([1; 32], 10)
            .expect("GC observed actual unlink");
        quota
            .canonical_unlinked_pending_sync([2; 32], 4)
            .expect("second GC unlink");
        assert_eq!(usage(&quota).canonical_bytes, 14);
        assert_eq!(usage(&quota).charged_files, 2);
        assert_eq!(
            quota
                .pending_canonical_unlinks(1)
                .expect("bounded pending list"),
            vec![([1; 32], 10)]
        );
        assert!(quota.pending_canonical_unlinks(0).is_err());
        assert!(matches!(
            quota.try_reserve([1; 32], 10),
            Err(CoreError::Storage(_))
        ));
        assert!(matches!(
            quota.try_reserve([1; 32], 9),
            Err(CoreError::Storage(_))
        ));
        assert_eq!(usage(&quota).total_charged_bytes, 14);
        assert_eq!(
            quota.pending_canonical_unlinks(2).expect("pending list"),
            vec![([1; 32], 10), ([2; 32], 4)]
        );
        quota
            .confirmed_canonical_unlink([1; 32], 10)
            .expect("GC confirmed later shard sync");
        assert_eq!(
            quota
                .pending_canonical_unlinks(2)
                .expect("remaining pending"),
            vec![([2; 32], 4)]
        );
        quota
            .confirmed_canonical_unlink([2; 32], 4)
            .expect("second shard sync");
        assert!(
            quota
                .pending_canonical_unlinks(2)
                .expect("cleared pending list")
                .is_empty()
        );
        assert_eq!(usage(&quota).canonical_bytes, 0);
        let fresh = quota
            .try_reserve([1; 32], 10)
            .expect("fresh after durable unlink");
        fresh.check_new_file_admitted().expect("new file admitted");
        drop(fresh);
        assert_eq!(usage(&quota).total_charged_bytes, 0);
    }

    #[test]
    fn pending_canonical_wrong_identity_width_and_repeat_fail_closed() {
        let quota = PoolWriteQuota::new(20, [([1; 32], 10)], []).expect("canonical inventory");
        assert!(quota.canonical_unlinked_pending_sync([1; 32], 9).is_err());
        assert!(quota.snapshot().is_err());

        let quota = PoolWriteQuota::new(20, [([1; 32], 10)], []).expect("canonical inventory");
        quota
            .canonical_unlinked_pending_sync([1; 32], 10)
            .expect("first observation");
        assert!(quota.canonical_unlinked_pending_sync([1; 32], 10).is_err());
        assert!(quota.snapshot().is_err());

        let quota = PoolWriteQuota::new(20, [([1; 32], 10)], []).expect("canonical inventory");
        quota
            .canonical_unlinked_pending_sync([1; 32], 10)
            .expect("observed unlink");
        assert!(quota.confirmed_canonical_unlink([2; 32], 10).is_err());
        assert!(quota.snapshot().is_err());
    }

    #[test]
    fn pending_orphan_sync_keeps_charge_and_lists_exact_bounded_names() {
        let quota =
            PoolWriteQuota::new(20, [], [([8; 32], 4), ([9; 32], 10)]).expect("orphan inventory");
        let first_name = format!(".{}.tmp-17-4", "ab".repeat(32));
        let second_name = format!(".{}.tmp-17-5", "cd".repeat(32));
        quota
            .orphan_unlinked_pending_sync([8; 32], 4, &first_name)
            .expect("first actual unlink");
        quota
            .orphan_unlinked_pending_sync([9; 32], 10, &second_name)
            .expect("second actual unlink");
        assert_eq!(usage(&quota).orphan_bytes, 14);
        assert_eq!(usage(&quota).charged_files, 2);
        assert_eq!(quota.orphan_charge([8; 32]).expect("charge"), Some(4));
        assert!(matches!(
            quota.try_reserve([3; 32], 7),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        assert_eq!(
            quota.pending_orphan_unlinks(1).expect("bounded list"),
            vec![PendingOrphanUnlink {
                temp_identity: [8; 32],
                encoded_bytes: 4,
                temp_name: first_name,
            }]
        );
        assert!(quota.pending_orphan_unlinks(0).is_err());
        assert_eq!(
            quota.pending_orphan_unlinks(2).expect("all pending").len(),
            2
        );
        quota
            .confirmed_orphan_unlink([8; 32], 4)
            .expect("first durable absence");
        assert_eq!(usage(&quota).orphan_bytes, 10);
        assert_eq!(quota.pending_orphan_unlinks(2).expect("remaining").len(), 1);
        drop(quota.try_reserve([3; 32], 7).expect("released capacity"));
        quota
            .confirmed_orphan_unlink([9; 32], 10)
            .expect("second durable absence");
        assert!(quota.pending_orphan_unlinks(2).expect("cleared").is_empty());
        assert_eq!(usage(&quota).total_charged_bytes, 0);
    }

    #[test]
    fn pending_orphan_wrong_width_name_and_repeat_fail_closed() {
        let valid_name = format!(".{}.tmp-17-4", "ab".repeat(32));
        let quota = PoolWriteQuota::new(20, [], [([8; 32], 4)]).expect("orphan inventory");
        assert!(
            quota
                .orphan_unlinked_pending_sync([8; 32], 5, &valid_name)
                .is_err()
        );
        assert!(quota.snapshot().is_err());

        let too_long = "x".repeat(129);
        for invalid_name in ["../escape", "bad\0name", "\\escape", "", too_long.as_str()] {
            let quota = PoolWriteQuota::new(20, [], [([8; 32], 4)]).expect("orphan inventory");
            assert!(
                quota
                    .orphan_unlinked_pending_sync([8; 32], 4, invalid_name)
                    .is_err()
            );
            assert!(quota.snapshot().is_err());
        }

        let quota = PoolWriteQuota::new(20, [], [([8; 32], 4)]).expect("orphan inventory");
        quota
            .orphan_unlinked_pending_sync([8; 32], 4, &valid_name)
            .expect("first observation");
        assert!(
            quota
                .orphan_unlinked_pending_sync([8; 32], 4, &valid_name)
                .is_err()
        );
        assert!(quota.snapshot().is_err());

        let quota = PoolWriteQuota::new(20, [], [([8; 32], 4)]).expect("orphan inventory");
        quota
            .orphan_unlinked_pending_sync([8; 32], 4, &valid_name)
            .expect("observed unlink");
        assert!(quota.confirmed_orphan_unlink([9; 32], 4).is_err());
        assert!(quota.snapshot().is_err());
    }

    #[test]
    fn ordinary_write_retains_until_exact_gc_unlink() {
        let quota = quota(10);
        let mut permit = quota.try_reserve([1; 32], 10).expect("exact boundary");
        assert_eq!(usage(&quota).in_flight_bytes, 10);
        assert!(matches!(
            quota.try_reserve([2; 32], 1),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        permit.temp_created([3; 32]).expect("temp created");
        permit.canonical_renamed().expect("canonical rename");
        drop(permit); // A later shard-fsync error must not refund this file.
        assert_eq!(usage(&quota).canonical_bytes, 10);
        assert_eq!(usage(&quota).in_flight_bytes, 0);
        quota
            .confirmed_canonical_unlink([1; 32], 10)
            .expect("GC unlink and directory sync");
        assert_eq!(usage(&quota).total_charged_bytes, 0);
    }

    #[test]
    fn pre_temp_drop_refunds_but_temp_drop_retains_orphan() {
        let quota = quota(10);
        drop(quota.try_reserve([1; 32], 10).expect("pre-temp permit"));
        assert_eq!(usage(&quota).total_charged_bytes, 0);
        let mut permit = quota.try_reserve([1; 32], 10).expect("second permit");
        permit.temp_created([9; 32]).expect("temp created");
        assert_eq!(quota.orphan_charge([9; 32]).expect("lookup"), None);
        drop(permit); // Short-write or temp-fsync failure may leave a file.
        assert_eq!(usage(&quota).orphan_bytes, 10);
        assert_eq!(quota.orphan_charge([9; 32]).expect("lookup"), Some(10));
        assert_eq!(quota.orphan_charge([8; 32]).expect("lookup"), None);
        assert!(matches!(
            quota.try_reserve([2; 32], 1),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        quota
            .confirmed_orphan_unlink([9; 32], 10)
            .expect("orphan unlink and directory sync");
        assert_eq!(usage(&quota).total_charged_bytes, 0);
        assert_eq!(quota.orphan_charge([9; 32]).expect("lookup"), None);
        assert!(quota.confirmed_orphan_unlink([9; 32], 10).is_err());
    }

    #[test]
    fn confirmed_temp_cleanup_refunds_and_repeated_terminal_poison() {
        let cleanup_quota = quota(20);
        let mut permit = cleanup_quota.try_reserve([1; 32], 10).expect("permit");
        permit.temp_created([2; 32]).expect("temp created");
        permit
            .temp_unlinked_and_synced()
            .expect("temp unlink and sync");
        assert_eq!(usage(&cleanup_quota).total_charged_bytes, 0);
        assert!(permit.temp_unlinked_and_synced().is_err());
        assert!(matches!(
            cleanup_quota.try_reserve([3; 32], 1),
            Err(CoreError::Storage(_))
        ));

        let quota = quota(10);
        let mut first = quota.try_reserve([1; 32], 10).expect("first permit");
        first.temp_created([2; 32]).expect("first temp");
        let second = quota.try_reserve([1; 32], 0);
        assert!(matches!(second, Err(CoreError::InvalidContract(_))));
        assert!(first.temp_created([2; 32]).is_err());
        assert!(matches!(quota.snapshot(), Err(CoreError::Storage(_))));
    }

    #[test]
    fn boot_seed_rejects_duplicates_conflicts_and_overflow() {
        let resumed =
            PoolWriteQuota::new(10, [], [([7; 32], 4)]).expect("recovered orphan inventory");
        assert_eq!(usage(&resumed).orphan_bytes, 4);
        assert_eq!(resumed.orphan_charge([7; 32]).expect("lookup"), Some(4));
        resumed
            .confirmed_orphan_unlink([7; 32], 4)
            .expect("recovered orphan cleanup");
        assert_eq!(usage(&resumed).total_charged_bytes, 0);
        assert!(PoolWriteQuota::new(10, [([1; 32], 6), ([1; 32], 4)], []).is_err());
        assert!(PoolWriteQuota::new(10, [([1; 32], 6)], [([1; 32], 4)]).is_err());
        assert!(matches!(
            PoolWriteQuota::new(u64::MAX, [([1; 32], u64::MAX)], [([2; 32], 1)]),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        let quota =
            PoolWriteQuota::new(u64::MAX, [([1; 32], u64::MAX)], []).expect("max width seed");
        assert!(matches!(
            quota.try_reserve([2; 32], 1),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        assert_eq!(usage(&quota).total_charged_bytes, u64::MAX);
    }

    #[test]
    fn streaming_boot_seed_is_bounded_and_closes_before_writer_use() {
        let mut quota = PoolWriteQuota::new_with_file_limit(20, 2, [], []).expect("empty quota");
        quota.record_boot_canonical([1; 32], 4).expect("canonical");
        quota.record_boot_orphan([2; 32], 6).expect("orphan");
        assert_eq!(usage(&quota).total_charged_bytes, 10);
        assert_eq!(quota.orphan_charge([2; 32]).expect("lookup"), Some(6));
        assert!(matches!(
            quota.record_boot_canonical([3; 32], 1),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        assert!(matches!(quota.snapshot(), Err(CoreError::Storage(_))));

        let mut quota = PoolWriteQuota::new_with_file_limit(20, 3, [], []).expect("empty quota");
        quota.record_boot_canonical([1; 32], 4).expect("canonical");
        let published = quota.clone();
        drop(published);
        assert!(matches!(
            quota.record_boot_orphan([2; 32], 6),
            Err(CoreError::InvalidContract(_))
        ));

        let mut quota = PoolWriteQuota::new_with_file_limit(20, 3, [], []).expect("empty quota");
        drop(quota.try_reserve([1; 32], 1).expect("first writer"));
        assert!(matches!(
            quota.record_boot_canonical([2; 32], 1),
            Err(CoreError::InvalidContract(_))
        ));
    }

    #[test]
    fn file_ceiling_counts_distinct_pending_files_even_for_same_digest() {
        assert!(PoolWriteQuota::new_with_file_limit(10, 0, [], []).is_err());
        let quota = PoolWriteQuota::new_with_file_limit(100, 2, [], []).expect("bounded quota");
        let first = quota.try_reserve([1; 32], 10).expect("first pending");
        let second = quota.try_reserve([1; 32], 10).expect("second pending");
        assert!(matches!(
            quota.try_reserve([2; 32], 1),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        drop(first);
        let third = quota.try_reserve([2; 32], 1).expect("released slot");
        assert_eq!(usage(&quota).charged_files, 2);
        drop(second);
        drop(third);
        assert_eq!(usage(&quota).charged_files, 0);
    }

    #[test]
    fn concurrent_distinct_charges_cannot_cross_ceiling() {
        let quota = quota(80);
        let mut workers = Vec::new();
        for number in 1_u8..=8 {
            let quota = quota.clone();
            workers.push(std::thread::spawn(move || {
                quota
                    .try_reserve([number; 32], 10)
                    .expect("concurrent permit")
            }));
        }
        let permits = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker joined"))
            .collect::<Vec<_>>();
        assert_eq!(usage(&quota).in_flight_bytes, 80);
        assert!(matches!(
            quota.try_reserve([9; 32], 1),
            Err(CoreError::Typed {
                code: INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                ..
            })
        ));
        drop(permits);
        assert_eq!(usage(&quota).total_charged_bytes, 0);
    }
}
