//! The optional path must retain the original packet's fail-closed checks.

use super::*;

#[test]
fn union_adoption_rejects_original_custody_mismatch_before_any_optional_grant()
-> Result<(), IndexError> {
    for invalid in 0..3 {
        let mut fixture = visitor_fixture()?;
        fixture.objects.union_install =
            Some(crate::native::text::tests::prepared_union::Mode::Complete);
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let memory = Arc::new(TokenMemory {
            ceiling: 16 * 1024 * 1024,
            ..TokenMemory::default()
        });
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
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            BatchOutputMemoryScope(Arc::clone(&memory)),
        ))));
        let spool = TextSpool::new(&scratch, &mut work)?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut writes = OccurrenceWrites::new();
        writes.pack_owners = true;
        writes.install_objects = Some(&fixture.objects);
        let mut edits = InventoryEdits::new(0)?;
        let mut confirmed = ConfirmedOccurrences::new(0)?;
        // Exactly one existing physical packet. The next distinct payload
        // crosses the byte boundary but remains within the 256-ref window.
        for ordinal in 0..128u16 {
            let mut bytes = vec![0x5a; 8192];
            bytes
                .get_mut(..2)
                .ok_or(IndexError::ResourceLimited)?
                .copy_from_slice(&ordinal.to_be_bytes());
            let reference = ObjectRef(Digest(Sha256::digest(&bytes).into()));
            let payload = crate::native::text_memory::PreparedPayload::from_staged(
                quanta_index_core::index::StagedPayload::copy(&bytes, &work)?,
            )?;
            writes.push(
                OccurrenceEdit {
                    ordinal: u32::from(ordinal),
                    key: PreparedKey::copy(format!("\0term-{ordinal:03}").as_bytes(), &work)?,
                    prior: None,
                    identified: reference,
                },
                payload,
                &spool,
                &mut edits,
                &mut work,
            )?;
        }
        match invalid {
            0 => drop(writes.payload_memory.pop()),
            1 => writes.payload_memory.push(None),
            _ => {
                writes.confirmation_programs = Some(
                    crate::native::text_spool::ConfirmedOccurrencePrograms::try_new(&work)?
                        .ok_or(IndexError::ResourceLimited)?,
                )
            }
        }
        let before_calls = memory.calls.load(Ordering::Relaxed);
        let before_usage = work.usage();
        let result = writes.flush_before_input(
            ObjectRef(Digest([0xfe; 32])),
            8192,
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
        );
        check!(matches!(result, Err(IndexError::BackendViolation(_))));
        check_eq!(memory.calls.load(Ordering::Relaxed), before_calls);
        check_eq!(work.usage(), before_usage);
        check!(writes.prepared_run.is_none());
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        check_eq!(writes.pending.len(), 128);
        check!(spool.inventory_cursor(256)?.next_edit(&mut work).is_err());
    }
    Ok(())
}
