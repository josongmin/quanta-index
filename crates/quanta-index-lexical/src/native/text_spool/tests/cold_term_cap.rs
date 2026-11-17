use super::*;

use quanta_index_core::index::{
    MemoryCredit, MemoryCreditClosePort, MemoryCreditLimits, MemoryLease, MemoryLeaseOwner,
    MemoryLifetime, MemoryScope, MemoryScopePort,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

struct BoundedCreditMeter {
    live: AtomicU64,
    accepted: AtomicU64,
    maximum: u64,
}

impl MemoryLeaseOwner for BoundedCreditMeter {
    fn release(&self, bytes: u64) {
        let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
    }
}

struct BoundedCreditParent {
    meter: Arc<BoundedCreditMeter>,
    offer_credit: bool,
}

impl MemoryScopePort for BoundedCreditParent {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _prior = self
            .meter
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= self.meter.maximum)
            })
            .map_err(|_used| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.meter.clone()))
    }

    fn try_reserve_credit(
        &self,
        limits: MemoryCreditLimits,
    ) -> Result<Option<MemoryCredit>, IndexError> {
        if !self.offer_credit {
            return Ok(None);
        }
        let bytes = limits
            .temporary_data_bytes
            .checked_add(limits.retained_data_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let grant = match self.reserve(bytes, MemoryLifetime::Temporary) {
            Ok(grant) => grant,
            Err(IndexError::ResourceLimited) => return Ok(None),
            Err(error) => return Err(error),
        };
        let _prior = self.meter.accepted.fetch_add(1, Ordering::SeqCst);
        let pool = Arc::new(BoundedCreditPool {
            _grant: grant,
            limits,
            live: Mutex::new((0, 0)),
            closed: AtomicBool::new(false),
        });
        Ok(Some(MemoryCredit::new(
            MemoryScope::new(Arc::new(BoundedCreditChild(pool.clone()))),
            pool,
        )))
    }
}

struct BoundedCreditPool {
    _grant: MemoryLease,
    limits: MemoryCreditLimits,
    live: Mutex<(u64, u32)>,
    closed: AtomicBool,
}

impl MemoryLeaseOwner for BoundedCreditPool {
    fn release(&self, bytes: u64) {
        let mut live = self.live.lock().expect("bounded credit lock poisoned");
        live.0 -= bytes;
        live.1 -= 1;
    }
}

impl MemoryCreditClosePort for BoundedCreditPool {
    fn close(&self) {
        // Serialize close against child admission. Existing leases retain the
        // parent's charge until their last owner and scope have dropped.
        if let Ok(_live) = self.live.lock() {
            self.closed.store(true, Ordering::Release);
        }
    }
}

struct BoundedCreditChild(Arc<BoundedCreditPool>);

impl MemoryScopePort for BoundedCreditChild {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if lifetime != MemoryLifetime::Temporary {
            return Err(IndexError::ResourceLimited);
        }
        let mut live = self
            .0
            .live
            .lock()
            .map_err(|_poison| IndexError::ResourceLimited)?;
        if self.0.closed.load(Ordering::Acquire) {
            return Err(IndexError::BackendViolation(
                "allocation credit has closed".into(),
            ));
        }
        let total = live
            .0
            .checked_add(bytes)
            .filter(|total| *total <= self.0.limits.temporary_data_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let slots = live
            .1
            .checked_add(1)
            .filter(|slots| *slots <= self.0.limits.max_live_temporary_leases)
            .ok_or(IndexError::ResourceLimited)?;
        *live = (total, slots);
        drop(live);
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

/// Fixed valid-key gold: 131,073 distinct terms require 512 MiB plus one
/// 4 KiB SQL row under the old one-row-per-term enumeration alone.
#[test]
fn cold_posting_131073_terms_stage_and_enumerate_with_original_work_cap() -> Result<(), IndexError>
{
    const TERMS: u32 = 131_073;
    const BATCH: usize = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(600))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let reference = ObjectRef(Digest([0x5a; 32]));
    let mut batch = Vec::with_capacity(BATCH);
    for index in 0..TERMS {
        batch.push((
            format!("\0term-{index:06}").into_bytes(),
            MapEdit {
                key: vec![0x2a; 32],
                expected: None,
                next: Some(MapValue::Object(reference)),
            },
        ));
        if batch.len() == BATCH {
            spool.posting_batch(&batch, &mut work)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        spool.posting_batch(&batch, &mut work)?;
    }

    let mut pages = PostingCasePages::new();
    let mut observed = 0_u32;
    loop {
        let terms = pages.next(&spool, &mut work)?;
        if terms.is_empty() {
            break;
        }
        check!(terms.len() <= 64);
        for term in terms.iter() {
            let expected = format!("\0term-{observed:06}");
            check_eq!(term.as_slice(), expected.as_bytes());
            observed = observed.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    check_eq!(observed, TERMS);
    let frozen = work.usage();
    check!(pages.next(&spool, &mut work)?.is_empty());
    check_eq!(work.usage(), frozen);
    Ok(())
}

/// Both an absent-to-absent CAS and an equal-value CAS are real original
/// stream edits. Their expected side is still checked by the public map
/// preparation; the disposable posting stream must not erase either row.
#[test]
fn packed_posting_preserves_noop_cas_rows_for_original_stream_validation() -> Result<(), IndexError>
{
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([0x6b; 32])));
    let rows = [
        (
            b"\0absent".to_vec(),
            MapEdit {
                key: vec![0x11; 32],
                expected: None,
                next: None,
            },
        ),
        (
            b"\0equal".to_vec(),
            MapEdit {
                key: vec![0x22; 32],
                expected: Some(value),
                next: Some(value),
            },
        ),
    ];
    spool.posting_batch(&rows, &mut work)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), 2);
    let mut cursor = spool.posting_cursor(terms);
    for (root, key, expected, next) in [
        (0_u32, vec![0x11; 32], None, None),
        (1, vec![0x22; 32], Some(value), Some(value)),
    ] {
        let actual = cursor
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(actual.root(), root);
        check_eq!(actual.key(), key);
        check_eq!(actual.expected()?, expected);
        check_eq!(actual.next()?, next);
    }
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

/// A page has two sparse roots and sixty-two dense roots. The fixed output
/// stream is small, while a fresh 128 KiB packet read for each dense root
/// exceeds the original 512 MiB work envelope before global EOF.
#[test]
fn cold_posting_mixed_sparse_dense_roots_complete_under_original_work_cap() -> Result<(), IndexError>
{
    const TERMS: u32 = 4096;
    const BATCH: usize = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(600))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([0x5a; 32])));
    let mut batch = Vec::with_capacity(BATCH);
    for index in 0..TERMS {
        let owners = if index % 64 < 2 { 2_u8 } else { 9_u8 };
        for owner in 0..owners {
            batch.push((
                format!("\0term-{index:04}").into_bytes(),
                MapEdit {
                    key: vec![owner; 32],
                    expected: None,
                    next: Some(value),
                },
            ));
            if batch.len() == BATCH {
                spool.posting_batch(&batch, &mut work)?;
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        spool.posting_batch(&batch, &mut work)?;
    }

    let mut pages = PostingCasePages::new();
    let mut observed = 0_u32;
    loop {
        let terms = pages.next(&spool, &mut work)?;
        if terms.is_empty() {
            break;
        }
        check_eq!(terms.len(), 64);
        let deltas = spool.posting_cardinalities(&terms, &mut work)?;
        let mut eligible = [false; 64];
        for (slot, key) in terms.iter().enumerate() {
            let index = observed
                .checked_add(u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
            let expected_term = format!("\0term-{index:04}");
            check_eq!(key.as_slice(), expected_term.as_bytes());
            let owners = if index % 64 < 2 { 2_u64 } else { 9_u64 };
            check_eq!(deltas.df(slot, 0)?, owners);
            eligible[slot] = owners <= 8;
        }
        let mut roots = spool.posting_root_cursors(&terms, &eligible, &mut work)?;
        for (slot, key) in terms.iter().enumerate() {
            let index = observed
                .checked_add(u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
            let owners = if index % 64 < 2 { 2_u8 } else { 9_u8 };
            let mut root = roots.cursor(slot, key.as_slice())?;
            for owner in 0..owners {
                let edit = root
                    .next_edit(&mut work)?
                    .ok_or(IndexError::ResourceLimited)?;
                check_eq!(edit.root(), 0);
                let expected_owner = vec![owner; 32];
                check_eq!(edit.key(), expected_owner);
                check_eq!(edit.expected()?, None);
                check_eq!(edit.next()?, Some(value));
            }
            check!(root.next_edit(&mut work)?.is_none());
        }
        let mut global = spool.posting_cursor(terms);
        for slot in 0..64_u32 {
            let index = observed
                .checked_add(slot)
                .ok_or(IndexError::ResourceLimited)?;
            let owners = if index % 64 < 2 { 2_u8 } else { 9_u8 };
            for owner in 0..owners {
                let edit = global
                    .next_edit(&mut work)?
                    .ok_or(IndexError::ResourceLimited)?;
                check_eq!(edit.root(), slot);
                let expected_owner = vec![owner; 32];
                check_eq!(edit.key(), expected_owner);
                check_eq!(edit.expected()?, None);
                check_eq!(edit.next()?, Some(value));
            }
        }
        check!(global.next_edit(&mut work)?.is_none());
        observed = observed
            .checked_add(64)
            .ok_or(IndexError::ResourceLimited)?;
    }
    check_eq!(observed, TERMS);
    let frozen = work.usage();
    check!(pages.next(&spool, &mut work)?.is_empty());
    check_eq!(work.usage(), frozen);
    Ok(())
}

/// Fixed arithmetic census from the etcd source-only normalizer inventory:
/// 133,221 keys and 611,539 term-owner pairs. This deliberately uses one
/// legal shared ObjectRef to expose repeated-value packet work; it does not
/// assert the reference repetition rate of the real SDK corpus.
#[test]
fn cold_posting_etcd_size_shared_reference_seals_and_enumerates_under_original_work_cap()
-> Result<(), IndexError> {
    const TERMS: u32 = 133_221;
    const FIVE_OWNER_TERMS: u32 = 78_655;
    const OWNERS: u32 = 1_421;
    const EDITS: u32 = 611_539;
    const BATCH: usize = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(600))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let meter = Arc::new(BoundedCreditMeter {
        live: AtomicU64::new(0),
        accepted: AtomicU64::new(0),
        maximum: 64 * 1024 * 1024,
    });
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedCreditParent {
        meter: meter.clone(),
        offer_credit: true,
    }))));
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([0x5a; 32])));
    let mut batch = Vec::with_capacity(BATCH);
    let mut generated = 0_u32;
    for index in 0..TERMS {
        let owner_count = if index < FIVE_OWNER_TERMS {
            5_u32
        } else {
            4_u32
        };
        for slot in 0..owner_count {
            let owner_index = index
                .checked_mul(5)
                .and_then(|number| number.checked_add(slot))
                .ok_or(IndexError::ResourceLimited)?
                % OWNERS;
            let mut owner = [0_u8; 32];
            owner[..2].copy_from_slice(
                &u16::try_from(owner_index)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
            batch.push((
                format!("\0term-{index:06}").into_bytes(),
                MapEdit {
                    key: owner.to_vec(),
                    expected: None,
                    next: Some(value),
                },
            ));
            generated = generated
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if batch.len() == BATCH {
                spool.posting_batch(&batch, &mut work)?;
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        spool.posting_batch(&batch, &mut work)?;
    }
    check_eq!(generated, EDITS);
    let staged = work.usage();
    let mut pages = PostingCasePages::new();
    let mut observed = 0_u32;
    loop {
        let terms = pages.next(&spool, &mut work).inspect_err(|error| {
            eprintln!("etcd census term page refused after {observed} terms: {error}; staged {staged:?}; {work:?}");
        })?;
        if terms.is_empty() {
            break;
        }
        check!(terms.len() <= 64);
        let deltas = spool
            .posting_cardinalities(&terms, &mut work)
            .inspect_err(|error| {
                eprintln!(
                    "etcd census cardinalities refused after {observed} terms: {error}; {work:?}"
                );
            })?;
        for (slot, term) in terms.iter().enumerate() {
            let index = observed
                .checked_add(u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
            let expected_term = format!("\0term-{index:06}");
            check_eq!(term.as_slice(), expected_term.as_bytes());
            let expected_count = if index < FIVE_OWNER_TERMS {
                5_u64
            } else {
                4_u64
            };
            check_eq!(deltas.df(slot, 0)?, expected_count);
        }
        observed = observed
            .checked_add(u32::try_from(terms.len()).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
    }
    check_eq!(observed, TERMS);
    let frozen = work.usage();
    check!(pages.next(&spool, &mut work)?.is_empty());
    check_eq!(work.usage(), frozen);
    check!(meter.accepted.load(Ordering::SeqCst) >= 1);
    drop(pages);
    drop(spool);
    check_eq!(meter.live.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn bounded_credit_parent_and_child_refuse_overbook_and_stale_reuse() -> Result<(), IndexError> {
    let meter = Arc::new(BoundedCreditMeter {
        live: AtomicU64::new(0),
        accepted: AtomicU64::new(0),
        maximum: 1_000,
    });
    let parent = BoundedCreditParent {
        meter: meter.clone(),
        offer_credit: true,
    };
    let limits = MemoryCreditLimits {
        temporary_data_bytes: 400,
        retained_data_bytes: 0,
        max_live_temporary_leases: 2,
        max_live_retained_leases: 0,
    };
    let first = parent
        .try_reserve_credit(limits)?
        .ok_or(IndexError::ResourceLimited)?;
    let second = parent
        .try_reserve_credit(limits)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(meter.live.load(Ordering::SeqCst), 800);
    check!(parent.try_reserve_credit(limits)?.is_none());
    let child = first.scope();
    let live_child = child.reserve(100, MemoryLifetime::Temporary)?;
    check!(matches!(
        child.reserve(301, MemoryLifetime::Temporary),
        Err(IndexError::ResourceLimited)
    ));
    drop(first);
    check!(matches!(
        child.reserve(1, MemoryLifetime::Temporary),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(meter.live.load(Ordering::SeqCst), 800);
    drop(live_child);
    drop(child);
    check_eq!(meter.live.load(Ordering::SeqCst), 400);
    drop(second);
    check_eq!(meter.live.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn denied_direct_credit_keeps_bounded_64_way_fallback_and_exact_eof() -> Result<(), IndexError> {
    const TERMS: u32 = 9_000;
    const BATCH: usize = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
    let meter = Arc::new(BoundedCreditMeter {
        live: AtomicU64::new(0),
        accepted: AtomicU64::new(0),
        maximum: 64 * 1024 * 1024,
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(BoundedCreditParent {
        meter: meter.clone(),
        offer_credit: false,
    }))));
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([0x6b; 32])));
    let mut batch = Vec::with_capacity(BATCH);
    for index in 0..TERMS {
        let mut key = format!("\0long-{index:06}").into_bytes();
        key.resize(1_024, b'x');
        batch.push((
            key,
            MapEdit {
                key: vec![0x2a; 32],
                expected: None,
                next: Some(value),
            },
        ));
        if batch.len() == BATCH {
            spool.posting_batch(&batch, &mut work)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        spool.posting_batch(&batch, &mut work)?;
    }
    let mut pages = PostingCasePages::new();
    let mut observed = 0_u32;
    loop {
        let page = pages.next(&spool, &mut work)?;
        if page.is_empty() {
            break;
        }
        for key in page.iter() {
            let mut expected = format!("\0long-{observed:06}").into_bytes();
            expected.resize(1_024, b'x');
            check_eq!(key.as_slice(), expected.as_slice());
            observed = observed.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    check_eq!(observed, TERMS);
    check_eq!(meter.accepted.load(Ordering::SeqCst), 0);
    drop(pages);
    drop(spool);
    check_eq!(meter.live.load(Ordering::SeqCst), 0);
    Ok(())
}
