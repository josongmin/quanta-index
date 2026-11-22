use super::super::cold_tokens::CanonicalTokenCensus;
use super::*;
use quanta_index_core::index::{
    Digest, MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits,
};
use quanta_index_lq_text_normalizer::TokenCursor;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Meter {
    live: AtomicU64,
    ceiling: AtomicU64,
}

impl MemoryLeaseOwner for Meter {
    fn release(&self, bytes: u64) {
        let previous = self.live.fetch_sub(bytes, AtomicOrdering::Relaxed);
        assert!(previous >= bytes);
    }
}

struct Scope(Arc<Meter>);

impl MemoryScopePort for Scope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self
            .0
            .live
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |live| {
                live.checked_add(bytes)
                    .filter(|next| *next <= self.0.ceiling.load(AtomicOrdering::Relaxed))
            })
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn budget() -> Result<(WorkBudget, Arc<Meter>), IndexError> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or(IndexError::ResourceLimited)?;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 100_000_000,
            mutations: 1_000_000,
        },
        deadline,
    );
    let meter = Arc::new(Meter::default());
    meter.ceiling.store(1_000_000, AtomicOrdering::Relaxed);
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(&meter))))));
    Ok((work, meter))
}

fn insert_body(
    cache: &mut TokenPrefixCache,
    body: &str,
    work: &mut WorkBudget,
) -> Result<(), IndexError> {
    let plan = cache.lookup_plan(body, work)?;
    let mut grant = plan
        .try_admit_entry(body, work)?
        .ok_or(IndexError::ResourceLimited)?;
    let census =
        CanonicalTokenCensus::upper_bound(body.len())?.ok_or(IndexError::ResourceLimited)?;
    let mut program_memory = ColdTokenProgram::try_reserve_for_suffix(
        None,
        0,
        body.len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?,
        work,
    )?
    .ok_or(IndexError::ResourceLimited)?;
    let mut tokens = ColdTokens::try_new(&census, work)?.ok_or(IndexError::ResourceLimited)?;
    let mut cursor: TokenCursor<'_, Option<MemoryLease>> = TokenCursor::try_new_bounded(
        body,
        body.len(),
        body.len()
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?,
        |_units| work.checkpoint(),
        |bytes| {
            work.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )
        },
    )
    .map_err(|_error| IndexError::ResourceLimited)?;
    let _indexed = tokens.fill_from_cursor(&mut cursor, work)?;
    program_memory.prepare_suffix(&mut tokens, work)?;
    let suffix = grant.wrap_suffix(tokens, work)?;
    let program =
        ColdTokenProgram::from_suffix_with_memory(Some(suffix), None, program_memory, work)?;
    let program = grant.wrap_program(program, work)?;
    grant.copy_key(body, work)?;
    plan.insert(grant, program, work)
}

fn independent_lcp(left: &str, right: &str) -> usize {
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .take_while(|(a, b)| a == b)
        .count()
}

#[test]
fn compared_carries_delimiter_proof_without_another_byte_pass() -> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let before = work.usage().bytes;
    assert_eq!(
        compared(b"alpha beta", b"alpha-beta", 0, &mut work)?,
        (Ordering::Less, 5, true)
    );
    assert_eq!(work.usage().bytes.checked_sub(before), Some(6));
    let before = work.usage().bytes;
    assert_eq!(
        compared(b"alpha beta", b"alpha-beta", 5, &mut work)?,
        (Ordering::Less, 5, true)
    );
    assert_eq!(work.usage().bytes.checked_sub(before), Some(1));
    assert!(!compared(b"foo_bar", b"foo-bar", 0, &mut work)?.2);
    assert!(
        !compared(
            "foo\u{a0}bar".as_bytes(),
            "foo\u{a1}bar".as_bytes(),
            0,
            &mut work
        )?
        .2
    );
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn sorted_neighbors_match_exhaustive_byte_prefix_oracle() -> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let mut cache = TokenPrefixCache::try_new(TextRecipe::new(Digest([7; 32])), 8, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let bodies = [
        "alpha one",
        "alpha two",
        "alpine x",
        "beta one",
        "delta x",
        "écho z",
    ];
    for body in bodies {
        insert_body(&mut cache, body, &mut work)?;
    }
    for query in [
        "alpha three",
        "alphz",
        "alpine xy",
        "beta one",
        "charlie",
        "étude",
    ] {
        let expected = bodies
            .iter()
            .map(|body| independent_lcp(body, query))
            .max()
            .ok_or(IndexError::ResourceLimited)?;
        let actual = cache
            .lookup(query, &mut work)?
            .map_or(0, |hit| hit.raw_lcp_bytes);
        assert_eq!(actual, expected, "query: {query}");
    }
    drop(cache);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn lower_bound_skips_only_both_neighbors_proven_prefix() -> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let mut cache = TokenPrefixCache::try_new(TextRecipe::new(Digest([14; 32])), 8, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    for number in (0..80).step_by(10) {
        insert_body(&mut cache, &format!("shared-prefix-{number:03}"), &mut work)?;
    }
    // The third probe lies between two cached neighbors that each proved
    // 15 common bytes. It inspects only its two remaining bytes: 16+16+2.
    for (query, position, existing, lcp, exact, inspected_fee) in [
        ("shared-prefix-035", 4, false, 16, false, 77),
        ("shared-prefix-030", 3, true, 17, true, 77),
        ("shared-prefix-03", 3, false, 16, false, 76),
        // A single upper neighbor cannot prove the prefix of a lower key.
        ("shared-prefix-!35", 0, false, 14, false, 112),
    ] {
        let before = work.usage().bytes;
        let plan = cache.lookup_plan(query, &mut work)?;
        assert_eq!(plan.position, position, "query: {query}");
        assert_eq!(plan.existing, existing, "query: {query}");
        let hit = plan.hit.as_ref().ok_or(IndexError::ResourceLimited)?;
        assert_eq!(hit.raw_lcp_bytes, lcp, "query: {query}");
        assert_eq!(hit.exact_body, exact, "query: {query}");
        assert!(!hit.equivalent_delimiter, "query: {query}");
        assert_eq!(work.usage().bytes - before, inspected_fee, "query: {query}");
    }
    drop(cache);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn lru_eviction_keeps_touched_program_and_active_arc_funded() -> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let mut cache = TokenPrefixCache::try_new(TextRecipe::new(Digest([8; 32])), 2, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    insert_body(&mut cache, "alpha one", &mut work)?;
    insert_body(&mut cache, "beta two", &mut work)?;
    let held = cache
        .lookup("alpha one", &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    insert_body(&mut cache, "gamma three", &mut work)?;
    let keys = (0..cache.len)
        .map(|position| cache.entry(position).map(|entry| entry.key.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(keys, [b"alpha one".to_vec(), b"gamma three".to_vec()]);
    drop(cache);
    assert!(meter.live.load(AtomicOrdering::Relaxed) > 0);
    drop(held);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn optional_entry_miss_does_not_change_work_or_memory() -> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let cache = TokenPrefixCache::try_new(TextRecipe::new(Digest([9; 32])), 2, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let live = meter.live.load(AtomicOrdering::Relaxed);
    meter.ceiling.store(live, AtomicOrdering::Relaxed);
    let before = work.usage();
    assert!(cache.try_admit_entry("alpha one", &work)?.is_none());
    assert_eq!(work.usage(), before);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), live);
    drop(cache);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn selected_prefix_grant_checks_rows_and_optional_capacity_before_suffix_mutation()
-> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let mut cache = TokenPrefixCache::try_new(TextRecipe::new(Digest([10; 32])), 2, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    insert_body(&mut cache, "alpha one", &mut work)?;
    let hit = cache
        .lookup("alpha new", &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let before = work.usage().mutations;
    assert!(matches!(
        ColdTokenProgram::try_reserve_for_suffix(Some((&hit.program, 1)), 0, 10, &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(work.usage().mutations, before);
    let live = meter.live.load(AtomicOrdering::Relaxed);
    meter.ceiling.store(live, AtomicOrdering::Relaxed);
    assert!(
        ColdTokenProgram::try_reserve_for_suffix(Some((&hit.program, 1)), 2, 10, &mut work)?
            .is_none()
    );
    assert_eq!(work.usage().mutations, before);
    assert_eq!(work.first_denial(), None);
    drop(hit);
    drop(cache);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn lookup_reuses_binary_search_neighbor_bytes_and_exact_slot() -> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let mut cache = TokenPrefixCache::try_new(TextRecipe::new(Digest([11; 32])), 2, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    insert_body(&mut cache, "alpha", &mut work)?;
    insert_body(&mut cache, "beta", &mut work)?;

    let before = work.usage().bytes;
    let mut plan = cache.lookup_plan("alpha", &mut work)?;
    assert_eq!(plan.position, 0);
    assert!(plan.existing);
    let hit = plan.hit.take().ok_or(IndexError::ResourceLimited)?;
    assert_eq!(hit.raw_lcp_bytes, 5);
    assert!(hit.exact_body);
    // Two binary probes: one byte (beta/alpha), five bytes (alpha/alpha),
    // each with one ordered slot and one index read; one hit touch/Arc copy.
    assert_eq!(work.usage().bytes - before, 2 * (1 + 8) + 1 + 5 + 8 + 8);
    drop(hit);
    drop(plan);
    drop(cache);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn eviction_before_insert_position_and_duplicate_replace_keep_sorted_unique_keys()
-> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let mut cache = TokenPrefixCache::try_new(TextRecipe::new(Digest([12; 32])), 2, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    insert_body(&mut cache, "alpha", &mut work)?;
    insert_body(&mut cache, "beta", &mut work)?;
    drop(cache.lookup("beta", &mut work)?);
    insert_body(&mut cache, "gamma", &mut work)?;
    assert_eq!(cache.len, 2);
    assert_eq!(cache.entry(0)?.key.as_slice(), b"beta");
    assert_eq!(cache.entry(1)?.key.as_slice(), b"gamma");

    insert_body(&mut cache, "gamma", &mut work)?;
    assert_eq!(cache.len, 2);
    assert_eq!(cache.entry(0)?.key.as_slice(), b"beta");
    assert_eq!(cache.entry(1)?.key.as_slice(), b"gamma");
    drop(cache);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}

#[test]
fn punctuation_owner_keeps_cache_empty_without_key_copy_or_mutation() -> Result<(), IndexError> {
    let (mut work, meter) = budget()?;
    let mut cache = TokenPrefixCache::try_new(TextRecipe::new(Digest([13; 32])), 2, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let cursor = super::super::prepare_indexed_token_cursor("!!!", &mut work)?;
    let before = work.usage().mutations;
    let (cursor, prepared) =
        super::super::token_prefix_prepare::try_prepare(&mut cache, cursor, &mut work)?;
    assert!(prepared.is_none());
    assert_eq!(work.usage().mutations, before);
    assert_eq!(cache.len, 0);
    drop(cursor);
    drop(cache);
    assert_eq!(meter.live.load(AtomicOrdering::Relaxed), 0);
    Ok(())
}
