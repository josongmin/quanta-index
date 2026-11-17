use super::*;
use crate::native::text_occurrence::block::PackedBlockView as _;

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

fn exact_canonical_census(
    body: &str,
    budget: &mut WorkBudget,
) -> Result<super::super::cold_tokens::CanonicalTokenCensus, IndexError> {
    let mut cursor = TokenCursor::new(body);
    super::super::cold_tokens::CanonicalTokenCensus::count(&mut cursor, body.len(), budget)?
        .ok_or(IndexError::ResourceLimited)
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

fn funded_output_budget(ceiling: u64) -> Result<(WorkBudget, Arc<TokenMemory>), IndexError> {
    use quanta_index_core::index::MemoryScope;

    let memory = Arc::new(TokenMemory {
        ceiling,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(BatchOutputMemoryScope(
        Arc::clone(&memory),
    )))));
    Ok((work, memory))
}

#[derive(Default)]
struct TestCreditUsage {
    temporary_bytes: u64,
    retained_bytes: u64,
    temporary_slots: u32,
    retained_slots: u32,
}

struct TestCreditPool {
    limits: quanta_index_core::index::MemoryCreditLimits,
    usage: Mutex<TestCreditUsage>,
    // The parent charge lives until the last escaped output lease is dropped.
    _grant: quanta_index_core::index::MemoryLease,
}

struct TestCreditScope(Arc<TestCreditPool>);

struct TestCreditLease {
    pool: Arc<TestCreditPool>,
    lifetime: quanta_index_core::index::MemoryLifetime,
}

impl quanta_index_core::index::MemoryLeaseOwner for TestCreditLease {
    fn release(&self, bytes: u64) {
        let mut usage = self.pool.usage.lock().expect("test credit mutex poisoned");
        match self.lifetime {
            quanta_index_core::index::MemoryLifetime::Temporary => {
                usage.temporary_bytes -= bytes;
                usage.temporary_slots -= 1;
            }
            quanta_index_core::index::MemoryLifetime::Retained => {
                usage.retained_bytes -= bytes;
                usage.retained_slots -= 1;
            }
        }
    }
}

impl quanta_index_core::index::MemoryScopePort for TestCreditScope {
    fn reserve(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        let mut usage = self
            .0
            .usage
            .lock()
            .map_err(|_error| IndexError::ResourceLimited)?;
        match lifetime {
            quanta_index_core::index::MemoryLifetime::Temporary => {
                let next_bytes = usage
                    .temporary_bytes
                    .checked_add(bytes)
                    .filter(|next| *next <= self.0.limits.temporary_data_bytes)
                    .ok_or(IndexError::ResourceLimited)?;
                let next_slots = usage
                    .temporary_slots
                    .checked_add(1)
                    .filter(|next| *next <= self.0.limits.max_live_temporary_leases)
                    .ok_or(IndexError::ResourceLimited)?;
                usage.temporary_bytes = next_bytes;
                usage.temporary_slots = next_slots;
            }
            quanta_index_core::index::MemoryLifetime::Retained => {
                let next_bytes = usage
                    .retained_bytes
                    .checked_add(bytes)
                    .filter(|next| *next <= self.0.limits.retained_data_bytes)
                    .ok_or(IndexError::ResourceLimited)?;
                let next_slots = usage
                    .retained_slots
                    .checked_add(1)
                    .filter(|next| *next <= self.0.limits.max_live_retained_leases)
                    .ok_or(IndexError::ResourceLimited)?;
                usage.retained_bytes = next_bytes;
                usage.retained_slots = next_slots;
            }
        }
        drop(usage);
        Ok(quanta_index_core::index::MemoryLease::new(
            bytes,
            Arc::new(TestCreditLease {
                pool: Arc::clone(&self.0),
                lifetime,
            }),
        ))
    }
}

impl quanta_index_core::index::MemoryCreditClosePort for TestCreditScope {
    fn close(&self) {}
}

pub(super) struct CreditOutputMemoryScope {
    pub(super) memory: Arc<TokenMemory>,
    pub(super) attempts: Arc<AtomicUsize>,
}

impl quanta_index_core::index::MemoryScopePort for CreditOutputMemoryScope {
    fn reserve(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        if lifetime == quanta_index_core::index::MemoryLifetime::Retained {
            let _previous_calls = self.memory.retained_calls.fetch_add(1, Ordering::Relaxed);
        }
        token_memory_grant(&self.memory, bytes)
    }

    fn try_reserve_credit(
        &self,
        limits: quanta_index_core::index::MemoryCreditLimits,
    ) -> Result<Option<quanta_index_core::index::MemoryCredit>, IndexError> {
        let _previous_attempts = self.attempts.fetch_add(1, Ordering::Relaxed);
        let bytes = limits
            .temporary_data_bytes
            .checked_add(limits.retained_data_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let grant = match token_memory_grant(&self.memory, bytes) {
            Ok(grant) => grant,
            Err(IndexError::ResourceLimited) => return Ok(None),
            Err(error) => return Err(error),
        };
        let pool = Arc::new(TestCreditPool {
            limits,
            usage: Mutex::new(TestCreditUsage::default()),
            _grant: grant,
        });
        Ok(Some(quanta_index_core::index::MemoryCredit::new(
            quanta_index_core::index::MemoryScope::new(Arc::new(TestCreditScope(Arc::clone(
                &pool,
            )))),
            Arc::new(TestCreditScope(pool)),
        )))
    }
}

fn funded_credit_output_budget(
    ceiling: u64,
) -> Result<(WorkBudget, Arc<TokenMemory>, Arc<AtomicUsize>), IndexError> {
    let (mut work, memory) = funded_output_budget(ceiling)?;
    let attempts = Arc::new(AtomicUsize::new(0));
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::clone(&attempts),
        },
    ))));
    Ok((work, memory, attempts))
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
fn token_sql_insert_uses_only_the_funded_input_backing() -> Result<(), IndexError> {
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
    let usage_before = work.usage();
    spool.token_batch(&batch, &mut work)?;
    check_eq!(memory.live.load(Ordering::Relaxed), live_before);
    check_eq!(memory.calls.load(Ordering::Relaxed), calls_before);
    check!(work.first_denial().is_none());
    check_eq!(work.usage().writes - usage_before.writes, 2);
    check_eq!(work.usage().bytes - usage_before.bytes, 132);
    let mut inspect = packet_budget()?;
    check_eq!(
        observed_token_groups(&spool, &mut inspect)?,
        BTreeMap::from([(b"\0a".to_vec(), vec![0]), (b"\x01a".to_vec(), vec![0])])
    );
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
        let census = exact_canonical_census(&body, &mut work)?;
        let mut cold = super::super::cold_tokens::ColdTokens::try_new(census, &work)?
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
    body.extend(std::iter::repeat_n(' ', 102_401 - body.len()));
    check_eq!(body.len(), 102_401);
    let census = exact_canonical_census(&body, &mut work)?;
    // Fixed independent layout: 1,050 runs, two 32-byte keys and two
    // 8-byte rows per run. The old body bound requested 2,867,228 bytes.
    check_eq!(census.arena_bytes()?, 84_000);
    check_eq!(body.len() * 28, 2_867_228);
    let mut cold = super::super::cold_tokens::ColdTokens::try_new(census, &work)?
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
    // The exact arena needs 19,968 bytes while only 19,000 remain. Keep the
    // SQL control within packet_budget's independent 1,024-row work bounds.
    let body = "AbCdEfGhIjKlMnOpQrStUvWxYz0123 ".repeat(256);
    let (mut work, memory) = funded_group_reader_budget(100_000)?;
    let occupied =
        work.reserve_memory(81_000, quanta_index_core::index::MemoryLifetime::Temporary)?;
    let census = exact_canonical_census(&body, &mut work)?;
    check_eq!(census.arena_bytes()?, 19_968);
    check!(super::super::cold_tokens::ColdTokens::try_new(census, &work)?.is_none());
    check!(work.first_denial().is_none());
    drop(occupied);
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    check_eq!(
        visit_indexed_token_batches(&body, &mut work, |batch, budget| {
            spool.token_batch(batch, budget)
        })?,
        256
    );
    check_eq!(observed_token_groups(&spool, &mut work)?.len(), 2);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(work.first_denial().is_none());
    Ok(())
}

#[test]
fn cold_selection_keeps_mandatory_token_packet_fundable() -> Result<(), IndexError> {
    use quanta_index_core::index::MemoryScope;

    let body = format!("a{}", " ".repeat(99));
    let ceiling = 2_800_u64;
    let bind = |work: &mut WorkBudget| {
        let memory = Arc::new(TokenMemory {
            ceiling,
            ..TokenMemory::default()
        });
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(TokenMemoryScope(
            Arc::clone(&memory),
        )))));
        memory
    };

    // The prior worst-case 28 * 100-byte grant would consume the ceiling
    // before the mandatory TokenBatch row packet.
    let mut old_order = packet_budget()?;
    let old_memory = bind(&mut old_order);
    let old_grant = old_order.reserve_memory(
        28 * u64::try_from(body.len()).map_err(|_error| IndexError::ResourceLimited)?,
        quanta_index_core::index::MemoryLifetime::Temporary,
    )?;
    check!(matches!(
        visit_indexed_token_batches(&body, &mut old_order, |_batch, _budget| Ok(())),
        Err(IndexError::ResourceLimited)
    ));
    drop(old_grant);
    check_eq!(old_memory.live.load(Ordering::Relaxed), 0);

    // The ordinary SQL producer accepts the same body and memory ceiling.
    let scratch = TestScratch::default();
    let mut sql_work = packet_budget()?;
    let sql_spool = TextSpool::new(&scratch, &mut sql_work)?;
    let sql_memory = bind(&mut sql_work);
    check_eq!(
        visit_indexed_token_batches(&body, &mut sql_work, |batch, budget| {
            sql_spool.token_batch(batch, budget)
        })?,
        1
    );
    let expected = observed_token_groups(&sql_spool, &mut packet_budget()?)?;
    check_eq!(expected.len(), 2);
    drop(sql_spool);
    check_eq!(sql_memory.live.load(Ordering::Relaxed), 0);

    // The new caller funds the cursor and packet first. Its exact arena fits
    // the same ceiling and must preserve the SQL token groups.
    let scratch = TestScratch::default();
    let mut selected_work = packet_budget()?;
    let selected_spool = TextSpool::new(&scratch, &mut selected_work)?;
    let selected_memory = bind(&mut selected_work);
    let (mut cursor, mut token_rows) = prepare_indexed_token_input(&body, &mut selected_work)?;
    let census = super::super::cold_tokens::CanonicalTokenCensus::count(
        &mut cursor,
        body.len(),
        &mut selected_work,
    )?
    .ok_or(IndexError::ResourceLimited)?;
    check!(token_rows.try_prepare_cold_keys(census.packet_key_bytes(), &selected_work)?);
    let mut cold = super::super::cold_tokens::ColdTokens::try_new(census, &selected_work)?
        .ok_or(IndexError::ResourceLimited)?;
    check!(selected_work.first_denial().is_none());
    check_eq!(
        visit_prepared_token_batches(
            &mut cursor,
            &mut token_rows,
            &mut selected_work,
            |batch, budget| cold.push_batch(batch, budget),
        )?,
        1
    );
    cold.sort_and_validate(&selected_work)?;
    let mut selected = BTreeMap::new();
    cold.visit_groups(8192, &mut selected_work, |key, positions, _budget| {
        let _previous = selected.insert(key.as_slice().to_vec(), positions.as_slice().to_vec());
        Ok(())
    })?;
    check_eq!(selected, expected);
    drop(cold);
    drop(token_rows);
    drop(cursor);
    drop(selected_spool);
    check_eq!(selected_memory.live.load(Ordering::Relaxed), 0);
    check!(selected_work.first_denial().is_none());
    Ok(())
}

#[test]
fn cold_arena_does_not_starve_mandatory_packed_output() -> Result<(), IndexError> {
    // Five sensitive/folded pairs satisfy the nine-term alias minimum.
    let body = "a b c d e ".repeat(100);
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;

    // Measure the exact live cold input and alias metadata, then leave room
    // for the first owned key but one byte short of the encoded block. This
    // ceiling is derived from canonical output, not an arbitrary memory size.
    let (mut calibration, calibration_memory) = funded_output_budget(1_000_000)?;
    let (mut cursor, mut packet) = prepare_indexed_token_input(&body, &mut calibration)?;
    let census = super::super::cold_tokens::CanonicalTokenCensus::count(
        &mut cursor,
        body.len(),
        &mut calibration,
    )?
    .ok_or(IndexError::ResourceLimited)?;
    check!(packet.try_prepare_cold_keys(census.packet_key_bytes(), &calibration)?);
    let mut cold = super::super::cold_tokens::ColdTokens::try_new(census, &calibration)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(
        visit_prepared_token_batches(
            &mut cursor,
            &mut packet,
            &mut calibration,
            |batch, budget| { cold.push_batch(batch, budget) }
        )?,
        500
    );
    cold.sort_and_validate(&calibration)?;
    drop(packet);
    drop(cursor);
    let alias = cold.try_packed_alias(&mut calibration)?.ok_or_else(|| {
        IndexError::BackendViolation("alias memory fixture has no complete packed candidate".into())
    })?;
    check_eq!(alias.term_count(), 10);
    let key_bytes =
        u64::try_from(alias.first_key()?.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let output_bytes =
        u64::try_from(alias.encoded_len()).map_err(|_error| IndexError::ResourceLimited)?;
    let ceiling = calibration_memory
        .live
        .load(Ordering::Relaxed)
        .checked_add(key_bytes)
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .and_then(|bytes| bytes.checked_sub(1))
        .ok_or(IndexError::ResourceLimited)?;
    drop(alias);
    drop(cold);
    check_eq!(calibration_memory.live.load(Ordering::Relaxed), 0);

    // The original SQL producer finishes the same source at this ceiling.
    let (mut baseline_work, baseline_memory) = funded_output_budget(ceiling)?;
    let baseline_scratch = TestScratch::default();
    let baseline_spool = TextSpool::new(&baseline_scratch, &mut baseline_work)?;
    baseline_spool.reset_tokens(&mut baseline_work)?;
    check_eq!(
        visit_indexed_token_batches(&body, &mut baseline_work, |batch, budget| {
            baseline_spool.token_batch(batch, budget)
        })?,
        500
    );
    let baseline_objects = TestObjects::default();
    let baseline_maps = TestMaps::default();
    let baseline_custody = TestCustody;
    let mut baseline_edits = InventoryEdits::new(0)?;
    let mut baseline_writes = OccurrenceWrites::new();
    baseline_writes.pack_owners = true;
    let mut baseline_confirmed = ConfirmedOccurrences::new(0)?;
    let mut baseline_context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &baseline_objects,
        maps: &baseline_maps,
        custody: &baseline_custody,
        scratch: &baseline_scratch,
        budget: &mut baseline_work,
    };
    let mut no_cold_tokens = None;
    let (baseline_terms, _baseline_alias) = super::super::occurrence_pack::prepare_owner_blocks(
        0,
        &mut baseline_context,
        &baseline_spool,
        &mut no_cold_tokens,
        &mut baseline_confirmed,
        &mut baseline_edits,
        &mut baseline_writes,
        None,
    )?;
    check_eq!(baseline_terms, 10);
    check_eq!(baseline_writes.pending.len(), 1);
    let baseline_payload = baseline_writes
        .inputs
        .first()
        .ok_or(IndexError::ResourceLimited)?
        .payload
        .clone();
    check!(baseline_context.budget.first_denial().is_none());
    drop((
        baseline_context,
        baseline_writes,
        baseline_edits,
        baseline_spool,
    ));
    check_eq!(baseline_memory.live.load(Ordering::Relaxed), 0);

    // The selected cold path must also finish, even when its optional arena
    // would otherwise consume the mandatory packed-output headroom.
    let (mut selected_work, selected_memory) = funded_output_budget(ceiling)?;
    let selected_scratch = TestScratch::default();
    let selected_spool = TextSpool::new(&selected_scratch, &mut selected_work)?;
    let selected_objects = TestObjects::default();
    let selected_maps = TestMaps::default();
    let selected_custody = TestCustody;
    let mut selected_edits = InventoryEdits::new(0)?;
    let mut selected_writes = OccurrenceWrites::new();
    selected_writes.pack_owners = true;
    let mut selected_confirmed = ConfirmedOccurrences::new(0)?;
    let mut selected_context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &selected_objects,
        maps: &selected_maps,
        custody: &selected_custody,
        scratch: &selected_scratch,
        budget: &mut selected_work,
    };
    let prepared = prepare_inventory(
        &body,
        &EntryMap::default(),
        0,
        &mut selected_context,
        &selected_spool,
        &mut selected_confirmed,
        &mut selected_edits,
        &mut selected_writes,
    )?;
    check_eq!(prepared.indexed_tokens, 500);
    check_eq!(prepared.term_count, baseline_terms);
    check_eq!(selected_writes.pending.len(), 1);
    check!(
        !selected_spool
            .token_term_page(None, selected_context.budget)?
            .is_empty()
    );
    check_eq!(selected_objects.installs.load(Ordering::Relaxed), 0);
    check_eq!(selected_objects.install_batches.load(Ordering::Relaxed), 0);
    check_eq!(
        selected_writes
            .inputs
            .first()
            .ok_or(IndexError::ResourceLimited)?
            .payload,
        baseline_payload
    );
    check!(selected_context.budget.first_denial().is_none());
    drop((
        selected_context,
        selected_writes,
        selected_edits,
        selected_spool,
    ));
    check_eq!(selected_memory.live.load(Ordering::Relaxed), 0);

    Ok(())
}

#[test]
fn nonalias_cold_output_keeps_sql_baseline_admissible_at_same_memory_ceiling()
-> Result<(), IndexError> {
    let mut body = String::new();
    for _repeat in 0..20 {
        for ordinal in 0..257_u16 {
            body.push_str(&format!("t{ordinal:03} "));
        }
    }
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let (mut census_work, _census_memory) = funded_output_budget(1_000_000)?;
    let census = exact_canonical_census(&body, &mut census_work)?;
    let packet_bytes =
        u64::try_from(census.packet_key_bytes()).map_err(|_error| IndexError::ResourceLimited)?;
    let arena_bytes =
        u64::try_from(census.arena_bytes()?).map_err(|_error| IndexError::ResourceLimited)?;
    // All lane-34 input grants fit; only 4 KiB remains while the optional
    // arena is live. The same ceiling must fund the original SQL output.
    let ceiling = token_row_backing()?
        .checked_add(packet_bytes)
        .and_then(|bytes| bytes.checked_add(arena_bytes))
        .and_then(|bytes| bytes.checked_add(4_096))
        .ok_or(IndexError::ResourceLimited)?;

    let (mut baseline_work, baseline_memory) = funded_output_budget(ceiling)?;
    let baseline_scratch = TestScratch::default();
    let baseline_spool = TextSpool::new(&baseline_scratch, &mut baseline_work)?;
    baseline_spool.reset_tokens(&mut baseline_work)?;
    check_eq!(
        visit_indexed_token_batches(&body, &mut baseline_work, |batch, budget| {
            baseline_spool.token_batch(batch, budget)
        })?,
        5_140
    );
    let baseline_objects = TestObjects::default();
    let baseline_maps = TestMaps::default();
    let baseline_custody = TestCustody;
    let mut baseline_edits = InventoryEdits::new(0)?;
    let mut baseline_writes = OccurrenceWrites::new();
    baseline_writes.pack_owners = true;
    let mut baseline_confirmed = ConfirmedOccurrences::new(0)?;
    let mut baseline_context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &baseline_objects,
        maps: &baseline_maps,
        custody: &baseline_custody,
        scratch: &baseline_scratch,
        budget: &mut baseline_work,
    };
    let mut no_cold_tokens = None;
    let (baseline_terms, baseline_alias) = super::super::occurrence_pack::prepare_owner_blocks(
        0,
        &mut baseline_context,
        &baseline_spool,
        &mut no_cold_tokens,
        &mut baseline_confirmed,
        &mut baseline_edits,
        &mut baseline_writes,
        None,
    )?;
    check_eq!(baseline_terms, 514);
    check!(baseline_alias.is_none());
    check!(baseline_writes.pending.len() > 1);
    let baseline_payloads: Vec<_> = baseline_writes
        .inputs
        .iter()
        .map(|input| input.payload.clone())
        .collect();
    check!(baseline_context.budget.first_denial().is_none());
    drop((
        baseline_context,
        baseline_writes,
        baseline_edits,
        baseline_spool,
    ));
    check_eq!(baseline_memory.live.load(Ordering::Relaxed), 0);

    let (mut selected_work, selected_memory, credit_attempts) =
        funded_credit_output_budget(ceiling)?;
    let selected_scratch = TestScratch::default();
    let selected_spool = TextSpool::new(&selected_scratch, &mut selected_work)?;
    let selected_objects = TestObjects {
        bounded_install_memory: true,
        ..TestObjects::default()
    };
    let selected_maps = TestMaps::default();
    let selected_custody = TestCustody;
    let mut selected_edits = InventoryEdits::new(0)?;
    let mut selected_writes = OccurrenceWrites::new();
    selected_writes.pack_owners = true;
    let mut selected_confirmed = ConfirmedOccurrences::new(0)?;
    let mut selected_context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &selected_objects,
        maps: &selected_maps,
        custody: &selected_custody,
        scratch: &selected_scratch,
        budget: &mut selected_work,
    };
    let prepared = prepare_inventory(
        &body,
        &EntryMap::default(),
        0,
        &mut selected_context,
        &selected_spool,
        &mut selected_confirmed,
        &mut selected_edits,
        &mut selected_writes,
    )?;
    check_eq!(prepared.indexed_tokens, 5_140);
    check_eq!(prepared.term_count, baseline_terms);
    check!(prepared.alias.is_none());
    check!(credit_attempts.load(Ordering::Relaxed) >= 1);
    check!(
        !selected_spool
            .token_term_page(None, selected_context.budget)?
            .is_empty()
    );
    check_eq!(
        selected_writes
            .inputs
            .iter()
            .map(|input| input.payload.clone())
            .collect::<Vec<_>>(),
        baseline_payloads
    );
    check!(selected_context.budget.first_denial().is_none());
    drop((
        selected_context,
        selected_writes,
        selected_edits,
        selected_spool,
    ));
    check_eq!(selected_memory.live.load(Ordering::Relaxed), 0);
    // With an admitted credit, the same canonical outputs stay on the cold
    // path and the disposable token SQL table remains untouched.
    let high_ceiling = ceiling
        .checked_add(16 * 1024 * 1024)
        .ok_or(IndexError::ResourceLimited)?;
    let (mut cold_work, cold_memory, cold_credit_attempts) =
        funded_credit_output_budget(high_ceiling)?;
    let cold_scratch = TestScratch::default();
    let cold_spool = TextSpool::new(&cold_scratch, &mut cold_work)?;
    let cold_objects = TestObjects {
        bounded_install_memory: true,
        ..TestObjects::default()
    };
    let cold_maps = TestMaps::default();
    let cold_custody = TestCustody;
    let mut cold_edits = InventoryEdits::new(0)?;
    let mut cold_writes = OccurrenceWrites::new();
    cold_writes.pack_owners = true;
    let mut cold_confirmed = ConfirmedOccurrences::new(0)?;
    let mut cold_context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &cold_objects,
        maps: &cold_maps,
        custody: &cold_custody,
        scratch: &cold_scratch,
        budget: &mut cold_work,
    };
    let cold_prepared = prepare_inventory(
        &body,
        &EntryMap::default(),
        0,
        &mut cold_context,
        &cold_spool,
        &mut cold_confirmed,
        &mut cold_edits,
        &mut cold_writes,
    )?;
    check_eq!(cold_prepared.term_count, baseline_terms);
    check!(cold_credit_attempts.load(Ordering::Relaxed) >= 1);
    check!(
        cold_spool
            .token_term_page(None, cold_context.budget)?
            .is_empty()
    );
    check_eq!(
        cold_writes
            .inputs
            .iter()
            .map(|input| input.payload.clone())
            .collect::<Vec<_>>(),
        baseline_payloads,
    );
    check!(cold_context.budget.first_denial().is_none());
    drop((cold_context, cold_writes, cold_edits, cold_spool));
    check_eq!(cold_memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn token_lane_records_sql_writes_without_changing_off_mode_work_or_groups() -> Result<(), IndexError>
{
    #[derive(Default, Debug)]
    struct Clock(AtomicU64);
    impl DiagnosticClockV1 for Clock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }
    #[derive(Default, Debug)]
    struct Sink(Mutex<Vec<DiagnosticEmitV1>>);
    impl DiagnosticEventSinkV1 for Sink {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            let Ok(mut events) = self.0.try_lock() else {
                return false;
            };
            events.push(event);
            true
        }
    }

    let body = "Ab Cd ".repeat(32);
    let run = |record: bool,
               use_cold: bool|
     -> Result<(BTreeMap<Vec<u8>, Vec<u32>>, WorkUsage, Option<u64>), IndexError> {
        let fixture = visitor_fixture()?;
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let sink = Arc::new(Sink::default());
        let diagnostic = record.then(|| {
            DiagnosticContextV1::new(
                DiagnosticCaptureIdV1([122; 16]),
                Arc::new(Clock::default()),
                sink.clone(),
            )
        });
        let mut root = None;
        let child = if let Some(trace) = diagnostic.as_ref() {
            let operation = trace
                .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
            let child = trace.child_context(&operation);
            root = Some(operation);
            Some(child)
        } else {
            None
        };
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let request = if let Some(trace) = child.as_ref() {
            request.with_diagnostic_context(trace.clone())
        } else {
            request
        };
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 100_000_000,
                mutations: 1_000_000,
            },
            request,
        );
        let memory = Arc::new(TokenMemory {
            ceiling: 1_000_000,
            ..TokenMemory::default()
        });
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            TokenMemoryScope(Arc::clone(&memory)),
        ))));
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut context = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut cold = observe_attempt_on_lane(
            &mut context,
            child.as_ref(),
            DiagnosticStageV1::NativeTextInventoryPrepare,
            DiagnosticLaneV1(34),
            |_child, context| {
                if use_cold {
                    let (mut cursor, mut batch) =
                        prepare_indexed_token_input(&body, context.budget)?;
                    let census = super::super::cold_tokens::CanonicalTokenCensus::count(
                        &mut cursor,
                        body.len(),
                        context.budget,
                    )?
                    .ok_or(IndexError::ResourceLimited)?;
                    check!(batch.try_prepare_cold_keys(census.packet_key_bytes(), context.budget)?);
                    let mut tokens =
                        super::super::cold_tokens::ColdTokens::try_new(census, context.budget)?
                            .ok_or(IndexError::ResourceLimited)?;
                    let _indexed = visit_prepared_token_batches(
                        &mut cursor,
                        &mut batch,
                        context.budget,
                        |rows, budget| tokens.push_batch(rows, budget),
                    )?;
                    tokens.sort_and_validate(context.budget)?;
                    Ok(Some(tokens))
                } else {
                    spool.reset_tokens(context.budget)?;
                    let _indexed =
                        visit_indexed_token_batches(&body, context.budget, |rows, budget| {
                            spool.token_batch(rows, budget)
                        })?;
                    Ok(None)
                }
            },
        )?;
        let groups = if let Some(tokens) = cold.as_mut() {
            let mut groups = BTreeMap::new();
            tokens.visit_groups(8192, &mut work, |key, positions, _budget| {
                let _previous =
                    groups.insert(key.as_slice().to_vec(), positions.as_slice().to_vec());
                Ok(())
            })?;
            groups
        } else {
            observed_token_groups(&spool, &mut work)?
        };
        drop(cold);
        drop(spool);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        let usage = work.usage();
        if let Some(operation) = root {
            operation
                .finish(DiagnosticTerminalV1::Ok)
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        }
        let events = sink
            .0
            .lock()
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let lane_span = events.iter().find_map(|event| match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::NativeTextInventoryPrepare,
                lane: DiagnosticLaneV1(34),
                ..
            } => Some(span_id),
            _ => None,
        });
        let lane_writes = events.iter().find_map(|event| match event.kind {
            DiagnosticEventKindV1::Counter {
                span_id,
                key: quanta_index_core::DiagnosticCounterV1::WorkWrites,
                value,
            } if Some(span_id) == lane_span => Some(value),
            _ => None,
        });
        Ok((groups, usage, lane_writes))
    };

    for use_cold in [false, true] {
        let (off_groups, off_usage, off_writes) = run(false, use_cold)?;
        let (record_groups, record_usage, lane_writes) = run(true, use_cold)?;
        check_eq!(off_groups, record_groups);
        check_eq!(off_usage, record_usage);
        check_eq!(off_writes, None);
        check_eq!(lane_writes, Some(if use_cold { 0 } else { 129 }));
    }
    Ok(())
}

#[test]
fn unicode_cold_groups_and_packed_wire_match_fixed_goldens_and_sql() -> Result<(), IndexError> {
    use super::super::super::text_occurrence::block::{BlockDraft, BlockPush, SealedBlock};

    let body = "A İ ΟΣ ß cafe\u{301} नमस्ते";
    let (mut work, memory) = funded_group_reader_budget(1_000_000)?;
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    let (mut cursor, mut packet) = prepare_indexed_token_input(body, &mut work)?;
    check_eq!(cursor.text(), "A İ ΟΣ ß café नमस्ते");
    let normalized = cursor.text().as_ptr();
    let census =
        super::super::cold_tokens::CanonicalTokenCensus::count(&mut cursor, body.len(), &mut work)?
            .ok_or(IndexError::ResourceLimited)?;
    // Six runs emit 12 eight-byte rows. The fixed case keys contain 77 UTF-8 bytes.
    check_eq!(census.arena_bytes()?, 173);
    check_eq!(cursor.text().as_ptr(), normalized);
    check!(packet.try_prepare_cold_keys(census.packet_key_bytes(), &work)?);
    let mut cold = super::super::cold_tokens::ColdTokens::try_new(census, &work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(
        visit_prepared_token_batches(&mut cursor, &mut packet, &mut work, |rows, budget| {
            cold.push_batch(rows, budget)
        })?,
        6
    );
    cold.sort_and_validate(&work)?;
    drop(packet);
    drop(cursor);

    let expected = BTreeMap::from([
        (b"\0A".to_vec(), vec![0]),
        (b"\x01a".to_vec(), vec![0]),
        ("\0İ".as_bytes().to_vec(), vec![1]),
        ("\x01i\u{307}".as_bytes().to_vec(), vec![1]),
        ("\0ΟΣ".as_bytes().to_vec(), vec![2]),
        ("\x01οσ".as_bytes().to_vec(), vec![2]),
        ("\0ß".as_bytes().to_vec(), vec![3]),
        ("\x01ß".as_bytes().to_vec(), vec![3]),
        ("\0café".as_bytes().to_vec(), vec![4]),
        ("\x01café".as_bytes().to_vec(), vec![4]),
        ("\0नमस्ते".as_bytes().to_vec(), vec![5]),
        ("\x01नमस्ते".as_bytes().to_vec(), vec![5]),
    ]);
    let mut cold_groups = BTreeMap::new();
    let mut cold_draft = BlockDraft::new()?;
    cold.visit_groups(8192, &mut work, |key, positions, budget| {
        check!(
            cold_groups
                .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
                .is_none()
        );
        check!(matches!(
            cold_draft.try_push(key, positions, budget)?,
            BlockPush::Added
        ));
        Ok(())
    })?;
    check_eq!(cold_groups, expected);
    let SealedBlock::Packed {
        payload: cold_wire, ..
    } = cold_draft.finish(&work)?
    else {
        return Err(IndexError::BackendViolation(
            "Unicode cold groups did not pack".into(),
        ));
    };

    spool.reset_tokens(&mut work)?;
    check_eq!(
        visit_indexed_token_batches(body, &mut work, |rows, budget| {
            spool.token_batch(rows, budget)
        })?,
        6
    );
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
    check_eq!(sql_groups, expected);
    let SealedBlock::Packed {
        payload: sql_wire, ..
    } = sql_draft.finish(&work)?
    else {
        return Err(IndexError::BackendViolation(
            "Unicode SQL groups did not pack".into(),
        ));
    };
    check_eq!(cold_wire.bytes, sql_wire.bytes);
    drop(cold_wire);
    drop(sql_wire);
    drop(cold);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(work.first_denial().is_none());
    Ok(())
}

#[test]
fn unicode_optional_arena_denial_uses_original_sql_without_terminal_denial()
-> Result<(), IndexError> {
    // 401 canonical runs require 802 SQL token writes plus one reset. The
    // 1,024-write packet budget can therefore validate the original path.
    let body = format!("{}cafe\u{301}", "İ ".repeat(400));
    let census = exact_canonical_census(&body, &mut packet_budget()?)?;
    let packet_bytes =
        u64::try_from(census.packet_key_bytes()).map_err(|_error| IndexError::ResourceLimited)?;
    let arena_bytes =
        u64::try_from(census.arena_bytes()?).map_err(|_error| IndexError::ResourceLimited)?;
    let ceiling = token_row_backing()?
        .checked_add(packet_bytes)
        .and_then(|bytes| bytes.checked_add(arena_bytes))
        .and_then(|bytes| bytes.checked_sub(1))
        .ok_or(IndexError::ResourceLimited)?;

    // Establish the independent ordinary SQL result at the same memory and
    // work limits before asserting the optional cold selection behavior.
    let (mut baseline_work, baseline_memory) = funded_group_reader_budget(ceiling)?;
    let baseline_scratch = TestScratch::default();
    let baseline_spool = TextSpool::new(&baseline_scratch, &mut baseline_work)?;
    let writes_before = baseline_work.usage().writes;
    baseline_spool.reset_tokens(&mut baseline_work)?;
    let baseline_indexed =
        visit_indexed_token_batches(&body, &mut baseline_work, |rows, budget| {
            baseline_spool.token_batch(rows, budget)
        });
    let baseline_indexed = baseline_indexed.map_err(|error| {
        IndexError::BackendViolation(format!(
            "original Unicode SQL refused: {error:?}, first={:?}, usage={:?}, live={}",
            baseline_work.first_denial(),
            baseline_work.usage(),
            baseline_memory.live.load(Ordering::Relaxed),
        ))
    })?;
    check_eq!(baseline_indexed, 401);
    check_eq!(baseline_work.usage().writes - writes_before, 803);
    let expected = observed_token_groups(&baseline_spool, &mut baseline_work)?;
    check_eq!(expected.len(), 4);
    drop(baseline_spool);
    check_eq!(baseline_memory.live.load(Ordering::Relaxed), 0);
    check!(baseline_work.first_denial().is_none());

    let (mut work, memory) = funded_group_reader_budget(ceiling)?;
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    let (mut cursor, mut packet) = prepare_indexed_token_input(&body, &mut work)?;
    let census =
        super::super::cold_tokens::CanonicalTokenCensus::count(&mut cursor, body.len(), &mut work)?
            .ok_or(IndexError::ResourceLimited)?;
    check!(packet.try_prepare_cold_keys(census.packet_key_bytes(), &work)?);
    check!(super::super::cold_tokens::ColdTokens::try_new(census, &work)?.is_none());
    check!(work.first_denial().is_none());
    packet.discard_cold_keys();
    spool.reset_tokens(&mut work)?;
    check_eq!(
        visit_prepared_token_batches(&mut cursor, &mut packet, &mut work, |rows, budget| {
            spool.token_batch(rows, budget)
        })?,
        401
    );
    drop(packet);
    drop(cursor);
    check_eq!(observed_token_groups(&spool, &mut work)?, expected);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(work.first_denial().is_none());
    Ok(())
}
