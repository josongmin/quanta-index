use super::*;

fn observed_token_groups(
    spool: &TextSpool,
    budget: &mut WorkBudget,
) -> Result<BTreeMap<Vec<u8>, Vec<u32>>, IndexError> {
    let mut observed = BTreeMap::new();
    spool.visit_token_groups(8192, budget, |key, positions, _budget| {
        if observed
            .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
            .is_some()
        {
            return Err(IndexError::BackendViolation(
                "ordered token visitor repeated a group".into(),
            ));
        }
        Ok(())
    })?;
    Ok(observed)
}

fn funded_group_reader_budget(ceiling: u64) -> Result<(WorkBudget, Arc<TokenMemory>), IndexError> {
    let memory = Arc::new(TokenMemory {
        ceiling,
        ..TokenMemory::default()
    });
    // 128 groups cost 1 + 2*128 = 257 reads. The generic 100-read test
    // budget is too small for this bounded-memory oracle.
    let mut work = packet_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        TokenMemoryScope(Arc::clone(&memory)),
    ))));
    Ok((work, memory))
}

#[test]
fn actual_token_writer_and_group_reader_preserve_fixed_unicode_and_packet_boundaries()
-> Result<(), IndexError> {
    for runs in [1usize, 32, 33] {
        let scratch = TestScratch::default();
        let mut work = packet_budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let body = "a ".repeat(runs);
        check_eq!(
            visit_indexed_token_batches(&body, &mut work, |batch, budget| {
                spool.token_batch(batch, budget)
            })?,
            u64::try_from(runs).map_err(|_error| IndexError::ResourceLimited)?
        );
        let positions = (0..u32::try_from(runs).map_err(|_error| IndexError::ResourceLimited)?)
            .collect::<Vec<_>>();
        let expected = BTreeMap::from([
            (b"\0a".to_vec(), positions.clone()),
            (b"\x01a".to_vec(), positions),
        ]);
        check_eq!(observed_token_groups(&spool, &mut work)?, expected);
    }

    let scratch = TestScratch::default();
    let mut work = packet_budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    check_eq!(
        visit_indexed_token_batches("A İ ΟΔΟΣ ß CAFÉ", &mut work, |batch, budget| {
            spool.token_batch(batch, budget)
        })?,
        5
    );
    let expected = BTreeMap::from([
        (b"\0A".to_vec(), vec![0]),
        (b"\x01a".to_vec(), vec![0]),
        ("\0İ".as_bytes().to_vec(), vec![1]),
        ("\x01i\u{307}".as_bytes().to_vec(), vec![1]),
        ("\0ΟΔΟΣ".as_bytes().to_vec(), vec![2]),
        ("\x01οδοσ".as_bytes().to_vec(), vec![2]),
        ("\0ß".as_bytes().to_vec(), vec![3]),
        ("\x01ß".as_bytes().to_vec(), vec![3]),
        ("\0CAFÉ".as_bytes().to_vec(), vec![4]),
        ("\x01café".as_bytes().to_vec(), vec![4]),
    ]);
    check_eq!(observed_token_groups(&spool, &mut work)?, expected);
    Ok(())
}

#[test]
fn token_sql_memory_refusal_precedes_insert_and_refunds_temporary_backing() -> Result<(), IndexError>
{
    let scratch = TestScratch::default();
    let (mut work, memory) = funded_token_budget(
        token_row_backing()?
            .checked_add(4)
            .ok_or(IndexError::ResourceLimited)?,
    )?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut batch = TokenBatch::new(&work)?;
    batch.prepare("a", &one_source_run("a", 0), &work)?;
    let live_before = memory.live.load(Ordering::Relaxed);
    let calls_before = memory.calls.load(Ordering::Relaxed);
    check!(matches!(
        spool.token_batch(&batch, &mut work),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(memory.live.load(Ordering::Relaxed), live_before);
    check_eq!(
        memory.calls.load(Ordering::Relaxed),
        calls_before
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?
    );
    let mut inspect = packet_budget()?;
    check!(observed_token_groups(&spool, &mut inspect)?.is_empty());
    drop(batch);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn late_duplicate_token_pk_refuses_and_discards_the_disposable_attempt() -> Result<(), IndexError> {
    let scratch = TestScratch::default();
    let (mut work, memory) = funded_token_budget(65_536)?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let body = "a ".repeat(32);
    let mut runs = [SourceRun::default(); TOKEN_RUN_BATCH];
    for (slot, run) in runs.iter_mut().enumerate() {
        *run = SourceRun {
            start: slot.checked_mul(2).ok_or(IndexError::ResourceLimited)?,
            end: slot
                .checked_mul(2)
                .and_then(|start| start.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?,
            position: if slot == 31 {
                0
            } else {
                u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?
            },
        };
    }
    let mut batch = TokenBatch::new(&work)?;
    batch.prepare(&body, &runs, &work)?;
    check!(matches!(
        spool.token_batch(&batch, &mut work),
        Err(IndexError::Storage(_))
    ));
    // journal_mode=OFF does not promise a clean statement rollback. This
    // disposable spool is abandoned; no map/object publication path is called.
    drop(batch);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn ordered_token_visitor_refuses_reentry_and_mutation_then_allows_reset() -> Result<(), IndexError>
{
    let scratch = TestScratch::default();
    let mut work = packet_budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    check_eq!(
        visit_indexed_token_batches("a b c", &mut work, |batch, budget| {
            spool.token_batch(batch, budget)
        })?,
        3
    );
    let mut extra = TokenBatch::new(&work)?;
    extra.prepare("d", &one_source_run("d", 4), &work)?;
    let mut visited = 0usize;
    spool.visit_token_groups(8192, &mut work, |_key, _positions, budget| {
        visited = visited.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        let used = budget.usage();
        check!(matches!(
            spool.visit_token_groups(8192, budget, |_, _, _| Ok(())),
            Err(IndexError::BackendViolation(_))
        ));
        check!(matches!(
            spool.reset_tokens(budget),
            Err(IndexError::BackendViolation(_))
        ));
        check!(matches!(
            spool.token_batch(&extra, budget),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(budget.usage(), used);
        Ok(())
    })?;
    check_eq!(visited, 6);
    spool.reset_tokens(&mut work)?;
    check!(observed_token_groups(&spool, &mut work)?.is_empty());
    Ok(())
}

#[test]
fn ordered_token_visitor_releases_lookahead_on_cancel_and_callback_error() -> Result<(), IndexError>
{
    let scratch = TestScratch::default();
    let mut setup = packet_budget()?;
    let spool = TextSpool::new(&scratch, &mut setup)?;
    let words = (b'a'..=b'c')
        .flat_map(|left| {
            (b'a'..=b'z').map(move |right| {
                let left = char::from(left);
                let right = char::from(right);
                format!("t{left}{right}")
            })
        })
        .take(64)
        .collect::<Vec<_>>();
    let body = words.join(" ");
    check_eq!(
        visit_indexed_token_batches(&body, &mut setup, |batch, budget| {
            spool.token_batch(batch, budget)
        })?,
        64
    );

    let (mut read, memory) = funded_group_reader_budget(96)?;
    let mut seen = 0usize;
    spool.visit_token_groups(8192, &mut read, |key, positions, _budget| {
        let position = seen % 64;
        check_eq!(
            positions.as_slice(),
            &[u32::try_from(position).map_err(|_error| IndexError::ResourceLimited)?]
        );
        let surface = if seen < 64 { '\0' } else { '\x01' };
        let word = words.get(position).ok_or(IndexError::ResourceLimited)?;
        check_eq!(key.as_slice(), format!("{surface}{word}").as_bytes());
        seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        check!(memory.live.load(Ordering::Relaxed) > 0);
        Ok(())
    })?;
    check_eq!(seen, 128);
    check!(memory.peak.load(Ordering::Relaxed) <= 96);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));

    let (mut cancelled, cancelled_memory) = funded_group_reader_budget(96)?;
    let mut visits = 0usize;
    let result = spool.visit_token_groups(8192, &mut cancelled, |_, _, budget| {
        visits = visits.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        budget.request().cancel_handle().cancel();
        Ok(())
    });
    check!(matches!(result, Err(IndexError::Cancelled)));
    check_eq!(visits, 1);
    check_eq!(cancelled_memory.live.load(Ordering::Relaxed), 0);
    check!(!cancelled_memory.invalid_release.load(Ordering::Relaxed));

    let (mut failed, failed_memory) = funded_group_reader_budget(96)?;
    let mut visits = 0usize;
    let result = spool.visit_token_groups(8192, &mut failed, |_, _, _| {
        visits = visits.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        Err(IndexError::BackendViolation(
            "fixed callback failure".into(),
        ))
    });
    check!(matches!(result, Err(IndexError::BackendViolation(_))));
    check_eq!(visits, 1);
    check_eq!(failed_memory.live.load(Ordering::Relaxed), 0);
    check!(!failed_memory.invalid_release.load(Ordering::Relaxed));
    check_eq!(
        observed_token_groups(&spool, &mut packet_budget()?)?.len(),
        128
    );
    Ok(())
}

#[test]
fn widened_cold_tokens_match_sql_groups_and_packed_wire() -> Result<(), IndexError> {
    use super::super::super::text_occurrence::block::{BlockDraft, BlockPush, SealedBlock};

    for body_bytes in [4097_usize, 8192, 16_383] {
        let scratch = TestScratch::default();
        let (mut work, memory) = funded_group_reader_budget(2_000_000)?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut body = "Ab Cd ".repeat(64);
        body.extend(std::iter::repeat_n(' ', body_bytes - body.len()));
        check_eq!(body.len(), body_bytes);
        check!(body.is_ascii());
        let mut cold = super::super::cold_tokens::ColdTokens::try_new(body.len(), &work)?
            .ok_or(IndexError::ResourceLimited)?;
        let indexed = visit_indexed_token_batches(&body, &mut work, |batch, budget| {
            spool.token_batch(batch, budget)?;
            cold.push_batch(batch, budget)
        })?;
        check_eq!(indexed, 128);
        cold.sort_and_validate(&work)?;

        let mut sql_groups = Vec::new();
        let mut sql_draft = BlockDraft::new()?;
        spool.visit_token_groups(8192, &mut work, |key, positions, budget| {
            sql_groups.push((key.as_slice().to_vec(), positions.as_slice().to_vec()));
            check!(matches!(
                sql_draft.try_push(key, positions, budget)?,
                BlockPush::Added
            ));
            Ok(())
        })?;
        let SealedBlock::Packed {
            payload: sql_wire, ..
        } = sql_draft.finish(&work)?
        else {
            return Err(IndexError::BackendViolation(
                "SQL groups did not pack".into(),
            ));
        };

        let mut cold_groups = Vec::new();
        let mut cold_draft = BlockDraft::new()?;
        cold.visit_groups(8192, &mut work, |key, positions, budget| {
            cold_groups.push((key.as_slice().to_vec(), positions.as_slice().to_vec()));
            check!(matches!(
                cold_draft.try_push(key, positions, budget)?,
                BlockPush::Added
            ));
            Ok(())
        })?;
        let SealedBlock::Packed {
            payload: cold_wire, ..
        } = cold_draft.finish(&work)?
        else {
            return Err(IndexError::BackendViolation(
                "cold groups did not pack".into(),
            ));
        };
        check_eq!(cold_groups, sql_groups);
        check_eq!(cold_wire.bytes, sql_wire.bytes);
        check_eq!(cold_groups.len(), 4);
        drop(cold_wire);
        drop(sql_wire);
        drop(cold);
        drop(spool);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
        check!(work.first_denial().is_none());
    }
    Ok(())
}

#[test]
fn cold_tokens_beyond_u16_key_arena_match_sql_and_packed_wire() -> Result<(), IndexError> {
    use super::super::super::text_occurrence::block::{
        BlockDraft, BlockPush, PackedBlockView, SealedBlock, encode_packed,
    };

    let memory = Arc::new(TokenMemory {
        ceiling: 5_000_000,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 100_000_000,
            mutations: 1_000_000,
        },
        std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        TokenMemoryScope(Arc::clone(&memory)),
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut body = String::new();
    for _ in 0..210 {
        for suffix in b'a'..=b'e' {
            body.push_str(&"A".repeat(30));
            body.push(char::from(suffix));
            body.push(' ');
        }
    }
    body.extend(std::iter::repeat_n(' ', 102_400 - body.len()));
    check_eq!(body.len(), 102_400);
    let mut cold = super::super::cold_tokens::ColdTokens::try_new(body.len(), &work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(
        visit_indexed_token_batches(&body, &mut work, |batch, budget| {
            spool.token_batch(batch, budget)?;
            cold.push_batch(batch, budget)
        })?,
        1_050
    );
    // Each source token emits two 32-byte keys: 1,050 * 2 * 32 = 67,200.
    cold.sort_and_validate(&work)?;
    let mut sql_groups = BTreeMap::new();
    let mut sql_draft = BlockDraft::new()?;
    spool.visit_token_groups(8192, &mut work, |key, positions, budget| {
        check!(
            sql_groups
                .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
                .is_none()
        );
        check!(matches!(
            sql_draft.try_push(key, positions, budget)?,
            BlockPush::Added
        ));
        Ok(())
    })?;
    let SealedBlock::Packed {
        payload: sql_wire, ..
    } = sql_draft.finish(&work)?
    else {
        return Err(IndexError::BackendViolation(
            "SQL parity draft did not pack".into(),
        ));
    };
    let mut cold_groups = BTreeMap::new();
    cold.visit_groups(8192, &mut work, |key, positions, _budget| {
        check!(
            cold_groups
                .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
                .is_none()
        );
        Ok(())
    })?;
    check_eq!(cold_groups, sql_groups);
    check_eq!(cold_groups.len(), 10);
    let alias = cold
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(alias.term_count(), 10);
    check_eq!(alias.vector_count(), 5);
    let direct = encode_packed(&alias, alias.encoded_len(), &work)?;
    check_eq!(direct.bytes, sql_wire.bytes);
    drop(direct);
    drop(sql_wire);
    drop(alias);
    drop(cold);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    check!(work.first_denial().is_none());
    Ok(())
}

#[test]
fn widened_cold_optional_memory_denial_leaves_sql_fallback_fundable() -> Result<(), IndexError> {
    let body = "Ab Cd";
    let (mut work, memory) = funded_group_reader_budget(100_000)?;
    let occupied =
        work.reserve_memory(40_000, quanta_index_core::index::MemoryLifetime::Temporary)?;
    check!(super::super::cold_tokens::ColdTokens::try_new(8192, &work)?.is_none());
    check!(work.first_denial().is_none());
    drop(occupied);
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    check_eq!(
        visit_indexed_token_batches(body, &mut work, |batch, budget| {
            spool.token_batch(batch, budget)
        })?,
        2
    );
    check_eq!(observed_token_groups(&spool, &mut work)?.len(), 4);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(work.first_denial().is_none());
    Ok(())
}
