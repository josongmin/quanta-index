//! Independent exact-row and refusal oracles for the bounded confirmation receipt.

use super::*;
use quanta_index_core::index::{
    AttemptId, CollectionId, Digest, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
    MemoryScopePort, OperationKey, ProducerId, SlotId, StreamId, TenantId, WorkspaceKey,
    WorktreeId,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use super::super::confirmed_batch::ConfirmedOccurrenceRecord;

fn attempt() -> AttemptRef {
    AttemptRef {
        operation: OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence: 1,
        },
        attempt: AttemptId([7; 16]),
        fence: 1,
    }
}

fn references(count: usize) -> Result<Vec<ObjectRef>, IndexError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for ordinal in 0..count {
        let mut digest = [0u8; 32];
        *digest.first_mut().ok_or(IndexError::ResourceLimited)? = 19;
        digest
            .get_mut(1..9)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(
                &u64::try_from(ordinal)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_le_bytes(),
            );
        result.push(ObjectRef(Digest(digest)));
    }
    Ok(result)
}

fn stored_rows(spool: &TextSpool, budget: &WorkBudget) -> Result<i64, IndexError> {
    spool
        .db
        .scoped(budget)?
        .query_row("SELECT COUNT(*) FROM confirmed_occurrence", [], |row| {
            row.get(0)
        })
        .map_err(|error| sql(&error))
}

fn reserve_record(
    spool: &TextSpool,
    record: &ConfirmedOccurrenceRecord<'_>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    // Production adds the inventory PK demand before this one original reserve.
    let db = spool.db.scoped(budget)?;
    quanta_index_scratch_sqlite::reserve_for_write(
        &db,
        spool.allocation.as_ref(),
        record.scratch_bytes()?,
        budget,
    )
}

#[test]
fn exact_packet_sizes_record_one_row_per_distinct_ref_and_preserve_null_count()
-> Result<(), IndexError> {
    for width in [1usize, 64, 65, 256] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let original = references(width)?;
        let owner = attempt();
        let receipt = ConfirmedOccurrenceRecord::prepare(&spool, &original, owner, &mut work)?;
        check_eq!(
            receipt.scratch_bytes()?,
            u64::try_from(width)
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(128)
                .ok_or(IndexError::ResourceLimited)?
        );
        reserve_record(&spool, &receipt, &mut work)?;
        check_eq!(stored_rows(&spool, &work)?, 0);
        receipt.finish(&work)?;
        check_eq!(
            stored_rows(&spool, &work)?,
            i64::try_from(width).map_err(|_error| IndexError::ResourceLimited)?
        );
        for reference in &original {
            check_eq!(
                spool.confirmed_occurrence_count(*reference, owner, &mut work)?,
                ConfirmedOccurrenceCount::Unvalidated
            );
        }
        let other = references(width + 1)?;
        check_eq!(
            spool.confirmed_occurrence_count(
                *other.get(width).ok_or(IndexError::ResourceLimited)?,
                owner,
                &mut work
            )?,
            ConfirmedOccurrenceCount::Missing
        );
        // A later exact packet may refer to the same immutable object. The
        // original INSERT OR IGNORE semantics retain its one confirmed row.
        let again = ConfirmedOccurrenceRecord::prepare(&spool, &original, owner, &mut work)?;
        reserve_record(&spool, &again, &mut work)?;
        again.finish(&work)?;
        check_eq!(
            stored_rows(&spool, &work)?,
            i64::try_from(width).map_err(|_error| IndexError::ResourceLimited)?
        );
    }
    Ok(())
}

struct TrackedMemory {
    used: Arc<AtomicU64>,
    maximum: u64,
}

impl MemoryScopePort for TrackedMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _before = self
            .used
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(bytes).filter(|sum| *sum <= self.maximum)
            })
            .map_err(|_used| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(
            bytes,
            Arc::new(Self {
                used: Arc::clone(&self.used),
                maximum: self.maximum,
            }),
        ))
    }
}

impl MemoryLeaseOwner for TrackedMemory {
    fn release(&self, bytes: u64) {
        let _before = self.used.fetch_sub(bytes, Ordering::SeqCst);
    }
}

#[test]
fn invalid_duplicate_foreign_and_denied_packets_never_create_a_confirmed_row()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let owner = attempt();
    let all = references(257)?;
    check!(matches!(
        ConfirmedOccurrenceRecord::prepare(&spool, &[], owner, &mut work),
        Err(IndexError::ResourceLimited)
    ));
    check!(matches!(
        ConfirmedOccurrenceRecord::prepare(&spool, &all, owner, &mut work),
        Err(IndexError::ResourceLimited)
    ));
    check!(matches!(
        ConfirmedOccurrenceRecord::prepare(
            &spool,
            &[
                *all.first().ok_or(IndexError::ResourceLimited)?,
                *all.first().ok_or(IndexError::ResourceLimited)?
            ],
            owner,
            &mut work
        ),
        Err(IndexError::BackendViolation(_))
    ));
    let foreign = AttemptRef { fence: 2, ..owner };
    check!(matches!(
        ConfirmedOccurrenceRecord::prepare(
            &spool,
            all.get(..1).ok_or(IndexError::ResourceLimited)?,
            foreign,
            &mut work
        ),
        Err(IndexError::FenceLost)
    ));
    let mut no_writes = WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 0,
            bytes: 1_000_000,
            mutations: 100,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    check!(matches!(
        ConfirmedOccurrenceRecord::prepare(
            &spool,
            all.get(..1).ok_or(IndexError::ResourceLimited)?,
            owner,
            &mut no_writes
        ),
        Err(IndexError::ResourceLimited)
    ));
    let used = Arc::new(AtomicU64::new(0));
    let mut no_memory = budget()?;
    no_memory.bind_memory_scope(Some(MemoryScope::new(Arc::new(TrackedMemory {
        used: Arc::clone(&used),
        maximum: 0,
    }))));
    check!(matches!(
        ConfirmedOccurrenceRecord::prepare(
            &spool,
            all.get(..1).ok_or(IndexError::ResourceLimited)?,
            owner,
            &mut no_memory
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(used.load(Ordering::SeqCst), 0);
    check_eq!(stored_rows(&spool, &work)?, 0);

    let mut admitted = budget()?;
    admitted.bind_memory_scope(Some(MemoryScope::new(Arc::new(TrackedMemory {
        used: Arc::clone(&used),
        maximum: 4_096,
    }))));
    let receipt = ConfirmedOccurrenceRecord::prepare(
        &spool,
        all.get(..65).ok_or(IndexError::ResourceLimited)?,
        owner,
        &mut admitted,
    )?;
    check!(used.load(Ordering::SeqCst) > 0);
    drop(receipt);
    check_eq!(used.load(Ordering::SeqCst), 0);
    check_eq!(stored_rows(&spool, &work)?, 0);
    Ok(())
}

#[test]
fn late_sql_refusal_is_error_and_the_disposable_attempt_may_retain_a_prefix()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let owner = attempt();
    let original = references(65)?;
    let receipt = ConfirmedOccurrenceRecord::prepare(&spool, &original, owner, &mut work)?;
    reserve_record(&spool, &receipt, &mut work)?;
    spool
        .db
        .scoped(&work)?
        .execute_batch(
            "CREATE TRIGGER refuse_second_chunk BEFORE INSERT ON confirmed_occurrence
             WHEN (SELECT COUNT(*) FROM confirmed_occurrence) >= 64
             BEGIN SELECT RAISE(ABORT, 'injected confirmed-row refusal'); END;",
        )
        .map_err(|error| sql(&error))?;
    check!(matches!(receipt.finish(&work), Err(IndexError::Storage(_))));
    // journal_mode=OFF cannot promise atomic rollback of earlier SQL programs.
    // The caller must abandon this scratch attempt after the typed failure.
    check_eq!(stored_rows(&spool, &work)?, 64);
    check_eq!(
        spool.confirmed_occurrence_count(
            *original.get(64).ok_or(IndexError::ResourceLimited)?,
            owner,
            &mut work
        )?,
        ConfirmedOccurrenceCount::Missing
    );
    Ok(())
}
