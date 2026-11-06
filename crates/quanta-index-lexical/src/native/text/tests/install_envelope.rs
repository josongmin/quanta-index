use super::*;

#[derive(serde::Serialize)]
struct OnePosition {
    positions: [u32; 1],
}

fn stage(
    writes: &mut OccurrenceWrites,
    edits: &mut InventoryEdits,
    spool: &TextSpool,
    slot: u16,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let byte = u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
    let key = format!("\0term-{slot:03}");
    let payload = super::super::super::text_memory::encode(
        &OnePosition {
            positions: [u32::from(slot)],
        },
        budget,
    )?;
    writes.push(
        OccurrenceEdit {
            ordinal: u32::from(slot.checked_rem(64).ok_or(IndexError::ResourceLimited)?),
            key: PreparedKey::copy(key.as_bytes(), budget)?,
            prior: None,
            identified: reference(byte),
        },
        payload,
        spool,
        edits,
        budget,
    )
}

#[test]
fn one_256_input_install_promotes_all_ordinals_and_keeps_hot_memo_at_64() -> Result<(), IndexError>
{
    let mut fixture = visitor_fixture()?;
    fixture.objects.install_outputs = Some(
        (0_u16..256)
            .map(|slot| {
                u8::try_from(slot)
                    .map(reference)
                    .map_err(|_error| IndexError::ResourceLimited)
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 4096,
            writes: 4096,
            bytes: 16_000_000,
            mutations: 4096,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(0)?;
    for slot in 0_u16..256 {
        stage(&mut writes, &mut edits, &spool, slot, &mut work)?;
        if slot == 0 {
            let cap = usize::try_from(MAX_OCCURRENCE_STAGED_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?;
            check!(writes.needs_flush(reference(1), cap)?);
            check!(!writes.needs_flush(reference(0), cap)?);
        }
    }
    check_eq!(writes.pending.len(), 256);
    check!(!writes.needs_flush(reference(255), 1)?);
    let mut unknown = [255; 32];
    *unknown.get_mut(0).ok_or(IndexError::ResourceLimited)? = 0;
    check!(writes.needs_flush(ObjectRef(Digest(unknown)), 1)?);
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(64)?;
    writes.flush(
        &mut AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        },
        &spool,
        &mut confirmed,
        &mut edits,
    )?;
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        256
    );
    check_eq!(confirmed.references.len(), 64);
    check!(confirmed.contains_or_spill(reference(255), attempt, &spool, &mut work)?);
    check!(writes.pending.is_empty());
    let mut cursor = spool.inventory_cursor(64)?;
    let mut seen = [false; 256];
    for _ in 0..256 {
        let row = cursor.next_edit(&mut work)?.ok_or_else(|| {
            IndexError::BackendViolation("missing installed occurrence edit".into())
        })?;
        let text = std::str::from_utf8(&row.edit.key)
            .map_err(|_error| IndexError::CorruptObject("invalid fixed key".into()))?;
        let slot: u16 = text
            .strip_prefix("\0term-")
            .ok_or_else(|| IndexError::CorruptObject("invalid fixed term".into()))?
            .parse()
            .map_err(|_error| IndexError::CorruptObject("invalid fixed slot".into()))?;
        let index = usize::from(slot);
        let visited = seen.get_mut(index).ok_or(IndexError::ResourceLimited)?;
        check!(!*visited);
        *visited = true;
        check_eq!(
            row.root,
            u32::from(slot.checked_rem(64).ok_or(IndexError::ResourceLimited)?)
        );
        check_eq!(row.edit.expected, None);
        check_eq!(
            row.edit.next,
            Some(
                reference(u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?).into()
            )
        );
    }
    check!(seen.iter().all(|visited| *visited));
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn wrong_final_output_keeps_all_256_rows_planned() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let mut outputs: Vec<_> = (0_u16..256)
        .map(|slot| {
            u8::try_from(slot)
                .map(reference)
                .map_err(|_error| IndexError::ResourceLimited)
        })
        .collect::<Result<Vec<_>, _>>()?;
    *outputs.last_mut().ok_or(IndexError::ResourceLimited)? = ObjectRef(Digest([42; 32]));
    fixture.objects.install_outputs = Some(outputs);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 4096,
            writes: 4096,
            bytes: 16_000_000,
            mutations: 4096,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(0)?;
    for slot in 0_u16..256 {
        stage(&mut writes, &mut edits, &spool, slot, &mut work)?;
    }
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(64)?;
    check!(matches!(
        writes.flush(
            &mut AttemptContext {
                attempt,
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            },
            &spool,
            &mut confirmed,
            &mut edits,
        ),
        Err(IndexError::BackendViolation(message))
            if message == "native text occurrence identity changed during installation"
    ));
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        256
    );
    check_eq!(writes.pending.len(), 256);
    check!(confirmed.references.is_empty());
    check!(!confirmed.contains_or_spill(reference(0), attempt, &spool, &mut work)?);
    check!(!confirmed.contains_or_spill(reference(255), attempt, &spool, &mut work)?);
    check!(spool.inventory_cursor(64)?.next_edit(&mut work).is_err());
    Ok(())
}
