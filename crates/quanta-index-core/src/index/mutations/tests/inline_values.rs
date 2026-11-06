//! Independent logical commitments and canonical inline shape rejection.
use super::*;
use crate::index::WorkBudgetDenial;
use crate::{BudgetInterruptionV1, RequestBudgetV1, RequestExecutionObserverV1};

#[derive(Debug)]
struct CommitmentObserver {
    calls: AtomicUsize,
    cancel_at: AtomicUsize,
}

impl CommitmentObserver {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            cancel_at: AtomicUsize::new(usize::MAX),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl RequestExecutionObserverV1 for CommitmentObserver {
    fn interruption(&self) -> Option<BudgetInterruptionV1> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        (call >= self.cancel_at.load(Ordering::SeqCst)).then_some(BudgetInterruptionV1::Cancelled)
    }
}

fn observed_budget(observer: Arc<CommitmentObserver>) -> Result<WorkBudget, IndexError> {
    let backing = Arc::new(RecordMemory {
        capacity: 1_000_000,
        live: AtomicU64::new(0),
        invalid: AtomicBool::new(false),
    });
    let memory = MemoryScope::new(Arc::new(RecordScope(backing)));
    let request = RequestBudgetV1::observing_execution_admitted(observer, &memory)?;
    Ok(WorkBudget::with_request(
        WorkLimits {
            reads: 1_000,
            writes: 0,
            bytes: 100_000,
            mutations: 1_000,
        },
        request,
    ))
}

fn long_row() -> OwnerMutation {
    row(
        Some(ObjectRef(Digest([255; 32]))),
        Some(ObjectRef(Digest([7; 32]))),
        9,
    )
}

fn row(expected: Option<ObjectRef>, next: Option<ObjectRef>, id: u8) -> OwnerMutation {
    OwnerMutation {
        owner: OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([id; 16]),
        },
        expected,
        next,
    }
}
fn commitment(rows: &[OwnerMutation]) -> Result<Digest, IndexError> {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index-owner-mutation-stream-v1\0");
    for row in rows {
        let bytes =
            serde_json::to_vec(row).map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        hash.update(owner_map_key(row.owner));
        hash.update(
            u64::try_from(bytes.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        hash.update(bytes);
    }
    hash.update(
        u64::try_from(rows.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    Ok(Digest(hash.finalize().into()))
}

#[test]
fn four_mutation_shapes_keep_original_commitment_and_only_next_edges() -> Result<(), IndexError> {
    let stamp = ObjectRef(Digest([0; 32]));
    let next = ObjectRef(Digest([2; 32]));
    let rows = [
        row(None, None, 1),
        row(None, Some(next), 2),
        row(Some(stamp), None, 3),
        row(Some(stamp), Some(next), 4),
    ];
    let mut maps = empty_maps();
    maps.forced = Some(MapPage::new(
        rows.iter()
            .map(|row| {
                Ok((
                    owner_map_key(row.owner).to_vec(),
                    owner_mutation_value(row)?,
                ))
            })
            .collect::<Result<_, IndexError>>()?,
        false,
        Vec::new(),
        None,
    ));
    // A get on this empty object port fails: inline replay cannot hide an IO.
    let objects = Objects(BTreeMap::new());
    let set = OwnerMutationSet {
        root: MapRoot(None),
        count: 4,
        digest: commitment(&rows)?,
    };
    let mut cursor = OwnerMutationCursor::new(set, &maps, &objects);
    let mut work = budget()?;
    for row in &rows {
        let value = owner_mutation_value(row)?;
        check_eq!(value.children()?, row.next.as_slice());
        check_eq!(cursor.next_mutation(&mut work)?, Some(row.clone()));
        check!(cursor.sealed_set().is_none());
    }
    check!(cursor.next_mutation(&mut work)?.is_none());
    check_eq!(cursor.sealed_set(), Some(set));
    check!(cursor.next_mutation(&mut work)?.is_none());
    check_eq!(maps.scans.load(Ordering::Relaxed), 1);
    check_eq!(work.usage().reads, 0);
    check_eq!(work.usage().mutations, 4);
    Ok(())
}

#[test]
fn malformed_inline_stamp_or_edges_never_skip_to_a_valid_suffix() -> Result<(), IndexError> {
    let item = row(None, Some(ObjectRef(Digest([8; 32]))), 1);
    for bad in [
        MapValue::inline(&[0], [])?,
        MapValue::inline(&[0; 31], [])?,
        MapValue::inline(&[], [ObjectRef(Digest([8; 32])); 2])?,
    ] {
        let mut maps = empty_maps();
        maps.forced = Some(MapPage::new(
            vec![
                (owner_map_key(item.owner).to_vec(), bad),
                (
                    owner_map_key(row(None, None, 2).owner).to_vec(),
                    MapValue::inline(&[], [])?,
                ),
            ],
            false,
            Vec::new(),
            None,
        ));
        let objects = Objects(BTreeMap::new());
        let mut cursor = OwnerMutationCursor::unsealed(MapRoot(None), &maps, &objects);
        let first = cursor
            .next_mutation(&mut budget()?)
            .expect_err("malformed mutation accepted");
        check!(matches!(first, IndexError::CorruptObject(_)));
        check_eq!(cursor.next_mutation(&mut budget()?), Err(first));
        check!(cursor.sealed_set().is_none());
    }
    Ok(())
}

#[test]
fn inline_digest_mismatch_and_original_cancellation_stay_terminal() -> Result<(), IndexError> {
    let item = row(None, None, 1);
    let mut maps = empty_maps();
    maps.forced = Some(MapPage::new(
        vec![(
            owner_map_key(item.owner).to_vec(),
            owner_mutation_value(&item)?,
        )],
        false,
        Vec::new(),
        None,
    ));
    let objects = Objects(BTreeMap::new());
    let mut cursor = OwnerMutationCursor::new(
        OwnerMutationSet {
            root: MapRoot(None),
            count: 1,
            digest: Digest([0; 32]),
        },
        &maps,
        &objects,
    );
    check_eq!(cursor.next_mutation(&mut budget()?)?, Some(item));
    let first = cursor
        .next_mutation(&mut budget()?)
        .expect_err("foreign digest accepted");
    check!(matches!(first, IndexError::CorruptObject(_)));
    check_eq!(cursor.next_mutation(&mut budget()?), Err(first));
    check!(cursor.sealed_set().is_none());
    let mut cancelled = budget()?;
    cancelled.request().cancel_handle().cancel();
    let mut cursor = OwnerMutationCursor::unsealed(MapRoot(None), &maps, &objects);
    check_eq!(
        cursor.next_mutation(&mut cancelled),
        Err(IndexError::Cancelled)
    );
    check_eq!(
        cursor.next_mutation(&mut budget()?),
        Err(IndexError::Cancelled)
    );
    Ok(())
}

#[test]
fn budgeted_commitment_refusal_keeps_original_digest_and_canonical_matching_is_exact()
-> Result<(), IndexError> {
    let item = long_row();
    let bytes =
        serde_json::to_vec(&item).map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let mut digest = OwnerMutationDigest::default();
    let original = digest.finish();
    let mut work = budget()?;
    work.charge(WorkUsage {
        bytes: 100_000,
        ..WorkUsage::default()
    })?;
    check_eq!(
        digest.push_budgeted(&item, &mut work),
        Err(IndexError::ResourceLimited)
    );
    check_eq!(digest.finish(), original);
    check_eq!(digest.count(), 0);
    check_eq!(work.first_denial(), Some(WorkBudgetDenial::Bytes));
    let mut work = budget()?;
    digest.push_budgeted(&item, &mut work)?;
    check_eq!(digest.finish(), commitment(std::slice::from_ref(&item))?);
    check_eq!(
        work.usage().bytes,
        u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?
    );
    check!(canonical::matches(&item, &bytes, &work)?);
    check!(!canonical::matches(
        &item,
        bytes
            .get(..bytes.len() - 1)
            .ok_or(IndexError::ResourceLimited)?,
        &work
    )?);
    let mut longer = bytes;
    longer.push(b' ');
    check!(!canonical::matches(&item, &longer, &work)?);
    Ok(())
}

#[test]
fn canonical_commitment_polls_by_bounded_bytes_not_json_fragments() -> Result<(), IndexError> {
    struct Fragments(usize);
    impl std::io::Write for Fragments {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += 1;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let item = long_row();
    let bytes =
        serde_json::to_vec(&item).map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    check!(bytes.len() > 256);
    let mut fragments = Fragments(0);
    serde_json::to_writer(&mut fragments, &item)
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let observer = Arc::new(CommitmentObserver::new());
    let mut work = observed_budget(observer.clone())?;
    let before = observer.calls();
    let mut digest = OwnerMutationDigest::default();
    digest.push_budgeted(&item, &mut work)?;
    let polls = observer.calls() - before;
    let per_pass = 2 + bytes.len() / 256;
    check!(polls <= 2 * per_pass + 1); // count pass, charge, hash pass
    check!(polls >= 5); // initial and completion on both passes, plus charge
    check!(polls < fragments.0);
    check_eq!(digest.finish(), commitment(std::slice::from_ref(&item))?);
    check_eq!(
        work.usage().bytes,
        u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?
    );
    Ok(())
}

#[test]
fn canonical_commitment_cancellation_at_start_mid_and_completion_keeps_digest()
-> Result<(), IndexError> {
    let item = long_row();
    let bytes =
        serde_json::to_vec(&item).map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    check!(bytes.len() > 256);
    let per_pass = 2 + bytes.len() / 256;
    for (cancel_on_relative_call, charged) in [
        (1, false),
        (per_pass + 3, true), // after charge, at the first hash byte boundary
        (2 * per_pass + 1, true), // final hash checkpoint, before digest commit
    ] {
        let observer = Arc::new(CommitmentObserver::new());
        let mut work = observed_budget(observer.clone())?;
        let mut digest = OwnerMutationDigest::default();
        digest.push(&row(None, None, 1))?;
        let original = digest.finish();
        let before = observer.calls();
        observer
            .cancel_at
            .store(before + cancel_on_relative_call, Ordering::SeqCst);
        check_eq!(
            digest.push_budgeted(&item, &mut work),
            Err(IndexError::Cancelled)
        );
        check_eq!(digest.finish(), original);
        check_eq!(digest.count(), 1);
        check_eq!(work.first_denial(), Some(WorkBudgetDenial::Cancelled));
        check_eq!(
            work.usage().bytes,
            if charged {
                u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?
            } else {
                0
            }
        );
    }
    Ok(())
}

#[test]
fn canonical_commitment_expired_deadline_refuses_before_charge_or_digest_change()
-> Result<(), IndexError> {
    let item = long_row();
    let mut digest = OwnerMutationDigest::default();
    digest.push(&row(None, None, 1))?;
    let original = digest.finish();
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000,
            writes: 0,
            bytes: 100_000,
            mutations: 1_000,
        },
        Instant::now(),
    );
    check_eq!(
        digest.push_budgeted(&item, &mut work),
        Err(IndexError::ResourceLimited)
    );
    check_eq!(work.first_denial(), Some(WorkBudgetDenial::Deadline));
    check_eq!(work.usage().bytes, 0);
    check_eq!(digest.finish(), original);
    check_eq!(digest.count(), 1);
    Ok(())
}

#[test]
fn existing_object_value_and_inline_value_share_one_logical_stream() -> Result<(), IndexError> {
    let first = row(
        Some(ObjectRef(Digest([6; 32]))),
        Some(ObjectRef(Digest([7; 32]))),
        1,
    );
    let second = row(None, None, 2);
    let payload =
        serde_json::to_vec(&first).map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let reference = ObjectRef(Digest(Sha256::digest(&payload).into()));
    let registration = RegisteredObject::unscoped(
        reference,
        u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
        first.next.into_iter().collect(),
    );
    let mut objects = Objects(BTreeMap::from([(
        reference,
        StoredObject::new(registration, payload, None),
    )]));
    let mut maps = empty_maps();
    maps.forced = Some(MapPage::new(
        vec![
            (owner_map_key(first.owner).to_vec(), reference.into()),
            (
                owner_map_key(second.owner).to_vec(),
                owner_mutation_value(&second)?,
            ),
        ],
        false,
        Vec::new(),
        None,
    ));
    let set = OwnerMutationSet {
        root: MapRoot(None),
        count: 2,
        digest: commitment(&[first.clone(), second.clone()])?,
    };
    {
        let mut cursor = OwnerMutationCursor::new(set, &maps, &objects);
        check_eq!(cursor.next_mutation(&mut budget()?)?, Some(first));
        check_eq!(cursor.next_mutation(&mut budget()?)?, Some(second));
        check!(cursor.next_mutation(&mut budget()?)?.is_none());
        check_eq!(cursor.sealed_set(), Some(set));
    }
    let stored = objects
        .0
        .get_mut(&reference)
        .ok_or(IndexError::ResourceLimited)?;
    let mut noncanonical = stored.payload.clone();
    noncanonical.push(b' ');
    *stored = StoredObject::new(stored.registration.clone(), noncanonical, None);
    let mut cursor = OwnerMutationCursor::new(set, &maps, &objects);
    let first = cursor
        .next_mutation(&mut budget()?)
        .expect_err("noncanonical object mutation accepted");
    check!(matches!(first, IndexError::CorruptObject(_)));
    check_eq!(cursor.next_mutation(&mut budget()?), Err(first));
    Ok(())
}
