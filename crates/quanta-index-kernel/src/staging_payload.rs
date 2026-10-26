//! Canonical admitted staging is shared with Semantic through Core.

pub(crate) use quanta_index_core::index::{
    StagedObjectBatch, StagedPayload, encode_staged_json as encode,
};

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{
        Digest, MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits,
    };

    use super::*;
    use quanta_index_core::index::{
        IndexError, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectRef, WorkBudget,
    };

    pub(crate) struct Ledger {
        cap: u64,
        live: AtomicU64,
    }

    impl Ledger {
        pub(crate) fn live(&self) -> u64 {
            self.live.load(Ordering::SeqCst)
        }
    }

    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            let _previous = self.live.fetch_sub(bytes, Ordering::SeqCst);
        }
    }

    struct Scope(Arc<Ledger>);

    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Temporary {
                return Err(IndexError::BackendViolation(
                    "staging preparation escaped as retained data".into(),
                ));
            }
            let next = self
                .0
                .live()
                .checked_add(bytes)
                .filter(|next| *next <= self.0.cap)
                .ok_or(IndexError::ResourceLimited)?;
            self.0.live.store(next, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    pub(crate) fn scoped_budget(cap: u64) -> Result<(WorkBudget, Arc<Ledger>), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let ledger = Arc::new(Ledger {
            cap,
            live: AtomicU64::new(0),
        });
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        Ok((budget, ledger))
    }

    pub(crate) fn require(condition: bool) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "staged batch independent custody oracle mismatch".into(),
            ))
        }
    }

    #[test]
    fn batch_refuses_array_floor_and_overflow_before_allocating() -> Result<(), IndexError> {
        let (budget, ledger) = scoped_budget(0)?;
        require(matches!(
            StagedObjectBatch::new(1, &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(matches!(
            StagedObjectBatch::new(65, &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.live() == 0)?;
        let empty = StagedObjectBatch::new(0, &budget)?;
        require(empty.inputs().is_empty() && ledger.live() == 0)
    }

    #[test]
    fn child_refusal_refunds_new_payload_and_preserves_admitted_empty_batch()
    -> Result<(), IndexError> {
        let array_bytes = std::mem::size_of::<ObjectBatchInput>()
            .checked_add(std::mem::size_of::<(
                Option<MemoryLease>,
                Option<MemoryLease>,
            )>())
            .ok_or(IndexError::ResourceLimited)?;
        let array_bytes =
            u64::try_from(array_bytes).map_err(|_error| IndexError::ResourceLimited)?;
        let (budget, ledger) = scoped_budget(
            array_bytes
                .checked_add(3)
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        let mut batch = StagedObjectBatch::new(1, &budget)?;
        let payload = StagedPayload::copy(b"abc", &budget)?;
        require(matches!(
            batch.push(payload, &[ObjectRef(Digest([9; 32]))], &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(batch.inputs().is_empty() && ledger.live() == array_bytes)?;
        drop(batch);
        require(ledger.live() == 0)
    }

    #[test]
    fn batch_preserves_opaque_bytes_edges_and_original_cancellation() -> Result<(), IndexError> {
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let mut batch = StagedObjectBatch::new(2, &budget)?;
        let children = [ObjectRef(Digest([1; 32])), ObjectRef(Digest([3; 32]))];
        batch.push(
            StagedPayload::copy(b"a\0\xff", &budget)?,
            &children,
            &budget,
        )?;
        let input = batch.inputs().first().ok_or(IndexError::Conflict)?;
        require(input.payload == b"a\0\xff" && input.children == children)?;
        let live = ledger.live();
        let payload = StagedPayload::copy(b"second", &budget)?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            batch.push(payload, &[], &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(batch.inputs().len() == 1 && ledger.live() == live)?;
        drop(batch);
        require(ledger.live() == 0)
    }

    #[test]
    fn full_batch_refuses_extra_input_and_releases_only_its_new_payload() -> Result<(), IndexError>
    {
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let mut batch = StagedObjectBatch::new(1, &budget)?;
        batch.push(StagedPayload::copy(b"first", &budget)?, &[], &budget)?;
        let live = ledger.live();
        require(matches!(
            batch.push(StagedPayload::copy(b"extra", &budget)?, &[], &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(batch.inputs().len() == 1 && ledger.live() == live)?;
        drop(batch);
        require(ledger.live() == 0)
    }
}
