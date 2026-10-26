//! Stream mutation data directly into the existing authenticated map value.
//! The map installer owns child-first reservation, sync and confirmation.

use quanta_index_core::index::{
    Digest, IndexError, OrdinalMapEdit, OwnerKey, OwnerMutation, OwnerMutationDigest, WorkBudget,
    WorkUsage, owner_map_key, owner_mutation_value, ports::MapEditCursor,
};

pub(crate) struct StageMutationCursor<I> {
    input: I,
    digest: OwnerMutationDigest,
    previous: Option<OwnerKey>,
    exhausted: bool,
    failed: bool,
}

impl<I: Iterator<Item = Result<OwnerMutation, IndexError>>> StageMutationCursor<I> {
    #[must_use]
    pub(crate) fn new(input: I) -> Self {
        Self {
            input,
            digest: OwnerMutationDigest::default(),
            previous: None,
            exhausted: false,
            failed: false,
        }
    }

    pub(crate) fn sealed_digest(&self) -> Result<(u64, Digest), IndexError> {
        if self.failed || !self.exhausted {
            return Err(IndexError::BackendViolation(
                "mutation staging did not reach complete EOF".into(),
            ));
        }
        Ok((self.digest.count(), self.digest.finish()))
    }

    fn next(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if self.exhausted {
            return Ok(None);
        }
        let Some(mutation) = self.input.next().transpose()? else {
            self.exhausted = true;
            return Ok(None);
        };
        if self
            .previous
            .is_some_and(|previous| previous >= mutation.owner)
        {
            return Err(IndexError::InvalidInput(
                "owner mutation stream must be sorted and unique".into(),
            ));
        }
        let key = owner_map_key(mutation.owner);
        let value = owner_mutation_value(&mutation)?;
        budget.charge(WorkUsage {
            bytes: OrdinalMapEdit::allocation_bytes(key.len())?,
            ..WorkUsage::default()
        })?;
        let edit = OrdinalMapEdit::copy_key(0, &key, None, Some(value), budget)?;
        self.digest.push_budgeted(&mutation, budget)?;
        self.previous = Some(mutation.owner);
        Ok(Some(edit))
    }
}

impl<I: Iterator<Item = Result<OwnerMutation, IndexError>>> MapEditCursor
    for StageMutationCursor<I>
{
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        if self.failed {
            return Err(IndexError::BackendViolation(
                "failed mutation cursor cannot resume".into(),
            ));
        }
        let result = self.next(budget);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !($condition) {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        };
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {
            check_eq!($left, $right, "assertion failed: left == right");
        };
        ($left:expr, $right:expr, $message:expr $(,)?) => {
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!("{}; left: {left:?}, right: {right:?}", $message).into());
                }
                _ => {}
            }
        };
    }

    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{
        MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort, ObjectRef,
        ProducerId, StableOwnerId, WorkLimits,
    };

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    #[derive(Default)]
    struct Charges {
        live: AtomicU64,
        peak: AtomicU64,
        invalid_refund: AtomicBool,
        ceiling: u64,
    }

    impl MemoryLeaseOwner for Charges {
        fn release(&self, bytes: u64) {
            if self
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_sub(bytes)
                })
                .is_err()
            {
                self.invalid_refund.store(true, Ordering::Relaxed);
            }
        }
    }

    struct Scope(Arc<Charges>);
    impl MemoryScopePort for Scope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let prior = self
                .0
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= self.0.ceiling)
                })
                .map_err(|_error| IndexError::ResourceLimited)?;
            let current = prior
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?;
            let _previous_peak = self.0.peak.fetch_max(current, Ordering::Relaxed);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn budget(ceiling: u64) -> TestResult<(WorkBudget, Arc<Charges>)> {
        let charges = Arc::new(Charges {
            ceiling,
            ..Charges::default()
        });
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 64 * 1024 * 1024,
                mutations: 100_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or("test deadline overflow")?,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(charges.clone())))));
        Ok((budget, charges))
    }

    fn mutation(id: u16) -> OwnerMutation {
        let [high, low] = id.to_be_bytes();
        let stable_id = [high, low, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        OwnerMutation {
            owner: OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId(stable_id),
            },
            expected: Some(ObjectRef(Digest([7; 32]))),
            next: None,
        }
    }

    #[test]
    fn inline_stream_matches_independent_json_commitment_without_wrapper_io() -> TestResult {
        use sha2::{Digest as _, Sha256};
        let (mut work, charges) = budget(1_048_576)?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-owner-mutation-stream-v1\0");
        let rows: Vec<_> = (1..=129)
            .map(|id| {
                let mut row = mutation(id);
                if id % 2 == 0 {
                    row.expected = None;
                }
                if id % 3 == 0 {
                    row.next = Some(quanta_index_core::index::ObjectRef(Digest([9; 32])));
                }
                row
            })
            .collect();
        for row in &rows {
            let payload = serde_json::to_vec(row)?;
            hash.update(owner_map_key(row.owner));
            hash.update(u64::try_from(payload.len())?.to_be_bytes());
            hash.update(payload);
        }
        hash.update(129_u64.to_be_bytes());
        let gold = Digest(hash.finalize().into());
        let mut cursor = StageMutationCursor::new(rows.iter().cloned().map(Ok));
        for row in &rows {
            check!(cursor.sealed_digest().is_err());
            let edit = cursor
                .next_edit(&mut work)?
                .ok_or("missing inline mutation")?;
            check_eq!(edit.root, 0);
            check_eq!(edit.edit.key, owner_map_key(row.owner));
            check_eq!(edit.edit.expected, None);
            let Some(quanta_index_core::index::MapValue::Inline(value)) = edit.edit.next else {
                return Err("mutation wrapper remains".into());
            };
            check_eq!(
                value.data()?,
                row.expected
                    .as_ref()
                    .map_or(&[][..], |reference| reference.0.0.as_slice())
            );
            check_eq!(value.children()?, row.next.as_slice());
            // Exactly one escaping key/record allocation; no batch or JSON buffer.
            check_eq!(
                charges.live.load(Ordering::Relaxed),
                OrdinalMapEdit::allocation_bytes(38)?
            );
            drop(edit);
            check_eq!(charges.live.load(Ordering::Relaxed), 0);
        }
        check!(cursor.next_edit(&mut work)?.is_none());
        check_eq!(cursor.sealed_digest()?, (129, gold));
        check!(cursor.next_edit(&mut work)?.is_none());
        check_eq!(work.usage().writes, 0);
        check_eq!(work.usage().reads, 0);
        check!(!charges.invalid_refund.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn inline_staging_requires_eof_and_latches_input_order_or_cancel_failure() -> TestResult {
        for case in 0..3 {
            let (mut work, charges) = budget(1_048_576)?;
            let tail = if case == 0 {
                Err(IndexError::Cancelled)
            } else {
                Ok(mutation(1))
            };
            let input = std::iter::once(Ok(mutation(1))).chain(std::iter::once(tail));
            let mut cursor = StageMutationCursor::new(input);
            let first = cursor.next_edit(&mut work)?.ok_or("first edit absent")?;
            check!(cursor.sealed_digest().is_err());
            if case == 2 {
                work.request().cancel_handle().cancel();
            }
            check!(matches!(
                cursor.next_edit(&mut work),
                Err(IndexError::Cancelled | IndexError::InvalidInput(_))
            ));
            check!(cursor.sealed_digest().is_err());
            check!(matches!(
                cursor.next_edit(&mut work),
                Err(IndexError::BackendViolation(_))
            ));
            drop(cursor);
            check_eq!(
                charges.live.load(Ordering::Relaxed),
                OrdinalMapEdit::allocation_bytes(38)?
            );
            drop(first);
            check_eq!(charges.live.load(Ordering::Relaxed), 0);
        }
        Ok(())
    }

    #[test]
    fn empty_stream_needs_no_memory_and_retained_denial_cannot_seal() -> TestResult {
        let (mut work, charges) = budget(0)?;
        let mut empty = StageMutationCursor::new(std::iter::empty());
        check!(empty.next_edit(&mut work)?.is_none());
        check_eq!(
            empty.sealed_digest()?,
            (0, OwnerMutationDigest::default().finish())
        );
        let mut denied = StageMutationCursor::new(std::iter::once(Ok(mutation(1))));
        check!(matches!(
            denied.next_edit(&mut work),
            Err(IndexError::ResourceLimited)
        ));
        check!(denied.sealed_digest().is_err());
        check_eq!(charges.live.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn canonical_byte_denial_refunds_admitted_key_before_any_escape() -> TestResult {
        let (mut work, charges) = budget(1_048_576)?;
        work.charge(WorkUsage {
            bytes: 64 * 1024 * 1024 - OrdinalMapEdit::allocation_bytes(38)?,
            ..WorkUsage::default()
        })?;
        let mut cursor = StageMutationCursor::new(std::iter::once(Ok(mutation(1))));
        check!(matches!(
            cursor.next_edit(&mut work),
            Err(IndexError::ResourceLimited)
        ));
        check!(cursor.sealed_digest().is_err());
        check_eq!(charges.live.load(Ordering::Relaxed), 0);
        check!(!charges.invalid_refund.load(Ordering::Relaxed));
        Ok(())
    }
}
