//! The point decoder shares the cursor's exact value contract without a scan.
use super::*;

fn point_row(expected: Option<ObjectRef>, next: Option<ObjectRef>) -> OwnerMutation {
    OwnerMutation {
        owner: OwnerKey {
            producer: ProducerId([3; 16]),
            stable_id: StableOwnerId([4; 16]),
        },
        expected,
        next,
    }
}

fn point_object(row: &OwnerMutation) -> Result<(ObjectRef, StoredObject), IndexError> {
    let payload =
        serde_json::to_vec(row).map_err(|error| IndexError::InvalidInput(error.to_string()))?;
    let reference = ObjectRef(Digest(Sha256::digest(&payload).into()));
    let registration = RegisteredObject::unscoped(
        reference,
        u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
        row.next.into_iter().collect(),
    );
    Ok((reference, StoredObject::new(registration, payload, None)))
}

#[test]
fn point_decoder_accepts_inline_upsert_delete_and_zero_expected_stamp() -> Result<(), IndexError> {
    let stamp = ObjectRef(Digest([0; 32]));
    let next = ObjectRef(Digest([8; 32]));
    let objects = Objects(BTreeMap::new());
    for row in [
        point_row(None, Some(next)),
        point_row(Some(stamp), None),
        point_row(Some(stamp), Some(next)),
    ] {
        let mut work = budget()?;
        check_eq!(
            decode_owner_mutation_value(
                row.owner,
                owner_mutation_value(&row)?,
                &objects,
                &mut work
            )?,
            row
        );
        check_eq!(work.usage().mutations, 0);
        check_eq!(work.usage().reads, 0);
        check_eq!(work.usage().bytes, 0);
    }
    Ok(())
}

#[test]
fn point_decoder_rejects_inline_width_and_extra_children() -> Result<(), IndexError> {
    let row = point_row(None, None);
    let objects = Objects(BTreeMap::new());
    for value in [
        MapValue::inline(&[9], [])?,
        MapValue::inline(&[9; 31], [])?,
        MapValue::inline(&[], [ObjectRef(Digest([5; 32])); 2])?,
    ] {
        check!(matches!(
            decode_owner_mutation_value(row.owner, value, &objects, &mut budget()?),
            Err(IndexError::CorruptObject(_))
        ));
    }
    Ok(())
}

#[test]
fn point_decoder_validates_object_owner_canonical_bytes_and_registration() -> Result<(), IndexError>
{
    let row = point_row(
        Some(ObjectRef(Digest([0; 32]))),
        Some(ObjectRef(Digest([7; 32]))),
    );
    let (reference, stored) = point_object(&row)?;
    let canonical_len = stored.payload.len();
    let objects = Objects(BTreeMap::from([(reference, stored.clone())]));
    let mut work = budget()?;
    check_eq!(
        decode_owner_mutation_value(row.owner, MapValue::Object(reference), &objects, &mut work)?,
        row
    );
    check_eq!(work.usage().mutations, 0);
    check_eq!(
        work.usage().bytes,
        u64::try_from(
            canonical_len
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
        )
        .map_err(|_error| IndexError::ResourceLimited)?
    );

    let foreign = OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: row.owner.stable_id,
    };
    check!(matches!(
        decode_owner_mutation_value(
            foreign,
            MapValue::Object(reference),
            &objects,
            &mut budget()?
        ),
        Err(IndexError::CorruptObject(_))
    ));

    let mut noncanonical = stored.payload.clone();
    noncanonical.push(b' ');
    let bad = StoredObject::new(
        RegisteredObject::unscoped(
            reference,
            u64::try_from(noncanonical.len()).map_err(|_error| IndexError::ResourceLimited)?,
            row.next.into_iter().collect(),
        ),
        noncanonical,
        None,
    );
    let bad_objects = Objects(BTreeMap::from([(reference, bad)]));
    check!(matches!(
        decode_owner_mutation_value(
            row.owner,
            MapValue::Object(reference),
            &bad_objects,
            &mut budget()?
        ),
        Err(IndexError::CorruptObject(_))
    ));

    for registration in [
        RegisteredObject::unscoped(
            reference,
            u64::try_from(canonical_len).map_err(|_error| IndexError::ResourceLimited)?,
            vec![],
        ),
        RegisteredObject::unscoped(reference, 0, row.next.into_iter().collect()),
        RegisteredObject::unscoped(
            ObjectRef(Digest([1; 32])),
            u64::try_from(canonical_len).map_err(|_error| IndexError::ResourceLimited)?,
            row.next.into_iter().collect(),
        ),
    ] {
        let bad_objects = Objects(BTreeMap::from([(
            reference,
            StoredObject::new(registration, stored.payload.clone(), None),
        )]));
        check!(matches!(
            decode_owner_mutation_value(
                row.owner,
                MapValue::Object(reference),
                &bad_objects,
                &mut budget()?
            ),
            Err(IndexError::CorruptObject(_))
        ));
    }

    let oversized = vec![
        b' ';
        MAX_OWNER_MUTATION_OBJECT_BYTES
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?
    ];
    let oversized_objects = Objects(BTreeMap::from([(
        reference,
        StoredObject::new(
            RegisteredObject::unscoped(
                reference,
                u64::try_from(oversized.len()).map_err(|_error| IndexError::ResourceLimited)?,
                row.next.into_iter().collect(),
            ),
            oversized,
            None,
        ),
    )]));
    check!(matches!(
        decode_owner_mutation_value(
            row.owner,
            MapValue::Object(reference),
            &oversized_objects,
            &mut budget()?
        ),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn point_decoder_accepts_registered_wire_bytes_beyond_payload() -> Result<(), IndexError> {
    let row = point_row(None, Some(ObjectRef(Digest([7; 32]))));
    let (reference, stored) = point_object(&row)?;
    // The object port owns its wire framing; a consumer must not reinterpret
    // registered wire bytes as a payload-only length.
    let wire_bytes = stored
        .registration
        .bytes
        .checked_add(128)
        .ok_or(IndexError::ResourceLimited)?;
    let framed = StoredObject::new(
        RegisteredObject::unscoped(reference, wire_bytes, row.next.into_iter().collect()),
        stored.payload.clone(),
        None,
    );
    let objects = Objects(BTreeMap::from([(reference, framed)]));
    check_eq!(
        decode_owner_mutation_value(
            row.owner,
            MapValue::Object(reference),
            &objects,
            &mut budget()?,
        )?,
        row
    );
    Ok(())
}
