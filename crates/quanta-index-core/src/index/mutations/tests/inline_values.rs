//! Independent logical commitments and canonical inline shape rejection.
use super::*;

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
    let item = row(
        Some(ObjectRef(Digest([255; 32]))),
        Some(ObjectRef(Digest([7; 32]))),
        9,
    );
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
