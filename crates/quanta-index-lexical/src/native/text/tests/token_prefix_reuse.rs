use super::*;
use crate::native::text_occurrence::block::{PackedBlockView as _, encode_packed};

use super::super::cold_tokens::{
    CanonicalTokenCensus, ColdTokenProgram, ColdTokenProgramView, ColdTokens,
};
use super::super::token_prefix_cache::TokenPrefixCache;

fn prefix_cache(work: &mut WorkBudget) -> Result<Box<TokenPrefixCache>, IndexError> {
    TokenPrefixCache::try_new(TextRecipe::new(Digest([31; 32])), 64, work)?
        .ok_or(IndexError::ResourceLimited)
}

fn prepare_cached(
    cache: &mut TokenPrefixCache,
    body: &str,
    work: &mut WorkBudget,
) -> Result<(Arc<ColdTokenProgram>, u64, u64), IndexError> {
    let cursor = prepare_indexed_token_cursor(body, work)?;
    let before = work.usage().mutations;
    let (cursor, prepared) = super::super::token_prefix_prepare::try_prepare(cache, cursor, work)?;
    let (program, indexed) = prepared.ok_or(IndexError::ResourceLimited)?;
    drop(cursor);
    Ok((program, indexed, work.usage().mutations - before))
}

fn exact_cached_hit(
    cache: &mut TokenPrefixCache,
    body: &str,
    expected_indexed: u64,
    work: &mut WorkBudget,
) -> Result<Arc<ColdTokenProgram>, IndexError> {
    let cursor = prepare_indexed_token_cursor(body, work)?;
    let before_lookup = work.usage();
    let hit = cache
        .lookup(cursor.text(), work)?
        .ok_or(IndexError::Conflict)?;
    check!(hit.exact_body);
    let lookup_bytes = work
        .usage()
        .bytes
        .checked_sub(before_lookup.bytes)
        .ok_or(IndexError::ResourceLimited)?;
    drop(hit);
    let before = work.usage();
    let (cursor, prepared) = super::super::token_prefix_prepare::try_prepare(cache, cursor, work)?;
    let (program, indexed) = prepared.ok_or(IndexError::ResourceLimited)?;
    check_eq!(indexed, expected_indexed);
    check_eq!(program.indexed_run_count()?, expected_indexed);
    check_eq!(work.usage().mutations, before.mutations);
    // Exact equality was already billed by lookup_plan. No second full-body
    // cursor scan or nominal body-length byte charge remains on the hit.
    check_eq!(work.usage().bytes - before.bytes, lookup_bytes);
    drop(cursor);
    Ok(program)
}

#[test]
fn exact_body_hit_reuses_direct_and_flat_program_without_recensus() -> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let mut cache = prefix_cache(&mut work)?;
    let prefix = "a b c d e ";
    let full = format!("{prefix}{} f", "x".repeat(257));
    let (direct, indexed, mutations) = prepare_cached(&mut cache, prefix, &mut work)?;
    check_eq!((indexed, mutations), (5, 5));
    let direct_hit = exact_cached_hit(&mut cache, prefix, 5, &mut work)?;
    check!(Arc::ptr_eq(&direct, &direct_hit));
    let (flat, indexed, mutations) = prepare_cached(&mut cache, &full, &mut work)?;
    check_eq!((indexed, mutations), (6, 1));
    let flat_hit = exact_cached_hit(&mut cache, &full, 6, &mut work)?;
    check!(Arc::ptr_eq(&flat, &flat_hit));
    let view = ColdTokenProgramView::from_program(&flat_hit)?;
    let actual = groups(&view, &mut work)?;
    check_eq!(actual.get(b"\0a".as_slice()), Some(&vec![0]));
    check_eq!(actual.get(b"\0f".as_slice()), Some(&vec![6]));
    check_eq!(actual.get(b"\x01f".as_slice()), Some(&vec![6]));
    check!(actual.keys().all(|key| !key.contains(&b'x')));
    drop(view);
    drop(flat_hit);
    drop(flat);
    drop(direct_hit);
    drop(direct);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

pub(super) fn cold_arena(
    body: &str,
    skip_runs: usize,
    work: &mut WorkBudget,
) -> Result<(ColdTokens, u64), IndexError> {
    let (mut arena, indexed) = fill_arena(body, skip_runs, work)?;
    arena.sort_and_validate(work)?;
    Ok((arena, indexed))
}

fn fill_arena(
    body: &str,
    skip_runs: usize,
    work: &mut WorkBudget,
) -> Result<(ColdTokens, u64), IndexError> {
    let mut cursor = prepare_indexed_token_cursor(body, work)?;
    for _ in 0..skip_runs {
        let _run = cursor.next_run().ok_or_else(|| {
            IndexError::BackendViolation("prefix fixture ended before its boundary".into())
        })?;
    }
    let bound = CanonicalTokenCensus::upper_bound(cursor.remaining_bytes())?
        .ok_or(IndexError::ResourceLimited)?;
    let mut arena = ColdTokens::try_new(&bound, work)?.ok_or(IndexError::ResourceLimited)?;
    let before = work.usage();
    let indexed = arena.fill_from_cursor(&mut cursor, work)?;
    check_eq!(work.usage().mutations - before.mutations, indexed,);
    Ok((arena, indexed))
}

pub(super) fn groups(
    view: &ColdTokenProgramView<'_>,
    work: &mut WorkBudget,
) -> Result<BTreeMap<Vec<u8>, Vec<u32>>, IndexError> {
    let mut result = BTreeMap::new();
    view.visit_groups(8192, work, |key, positions, _work| {
        if result
            .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
            .is_some()
        {
            return Err(IndexError::BackendViolation(
                "prefix view repeated a canonical group".into(),
            ));
        }
        Ok(())
    })?;
    Ok(result)
}

pub(super) fn whole_groups(
    tokens: &ColdTokens,
    work: &mut WorkBudget,
) -> Result<BTreeMap<Vec<u8>, Vec<u32>>, IndexError> {
    let mut result = BTreeMap::new();
    tokens.visit_groups(8192, work, |key, positions, _work| {
        if result
            .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
            .is_some()
        {
            return Err(IndexError::BackendViolation(
                "whole owner repeated a canonical group".into(),
            ));
        }
        Ok(())
    })?;
    Ok(result)
}

#[test]
fn borrowed_prefix_merges_repeated_suffix_term_and_preserves_block_wire() -> Result<(), IndexError>
{
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let prefix_body = "a b c d e ";
    let full_body = "a b c d e a f";
    let (prefix, prefix_runs) = cold_arena(prefix_body, 0, &mut work)?;
    check_eq!(prefix_runs, 5);
    let before_suffix = work.usage();
    let (suffix, suffix_runs) = cold_arena(full_body, 5, &mut work)?;
    check_eq!(suffix_runs, 2);
    check_eq!(work.usage().mutations - before_suffix.mutations, 2);
    let (whole, whole_runs) = cold_arena(full_body, 0, &mut work)?;
    check_eq!(whole_runs, 7);

    let view = ColdTokenProgramView::new(Some((&prefix, 5)), &suffix)?;
    let expected = BTreeMap::from([
        (b"\0a".to_vec(), vec![0, 5]),
        (b"\0b".to_vec(), vec![1]),
        (b"\0c".to_vec(), vec![2]),
        (b"\0d".to_vec(), vec![3]),
        (b"\0e".to_vec(), vec![4]),
        (b"\0f".to_vec(), vec![6]),
        (b"\x01a".to_vec(), vec![0, 5]),
        (b"\x01b".to_vec(), vec![1]),
        (b"\x01c".to_vec(), vec![2]),
        (b"\x01d".to_vec(), vec![3]),
        (b"\x01e".to_vec(), vec![4]),
        (b"\x01f".to_vec(), vec![6]),
    ]);
    check_eq!(groups(&view, &mut work)?, expected);
    check_eq!(whole_groups(&whole, &mut work)?, expected);
    let mut replay = Vec::new();
    view.replay_borrowed_pages(&mut work, |rows, _work| {
        replay.extend(rows.iter().map(|(key, position)| (key.to_vec(), *position)));
        Ok(())
    })?;
    let mut reference_replay = Vec::new();
    whole.replay_borrowed_pages(&mut work, |rows, _work| {
        reference_replay.extend(rows.iter().map(|(key, position)| (key.to_vec(), *position)));
        Ok(())
    })?;
    check_eq!(replay, reference_replay);
    let shape = view.block_shape(&mut work)?;
    check_eq!(shape.term_count, 12);
    check_eq!(shape.object_count, 1);
    let borrowed = view
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let reference = whole
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(borrowed.term_count(), 12);
    let actual_wire = encode_packed(&borrowed, borrowed.encoded_len(), &work)?;
    let reference_wire = encode_packed(&reference, reference.encoded_len(), &work)?;
    check_eq!(actual_wire.bytes, reference_wire.bytes);
    check!(actual_wire.bytes.starts_with(b"QITXOC01"));

    drop(actual_wire);
    drop(reference_wire);
    drop(borrowed);
    drop(reference);
    drop(view);
    drop(whole);
    drop(suffix);
    drop(prefix);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    check!(work.first_denial().is_none());
    Ok(())
}

#[test]
fn borrowed_prefix_keeps_the_gap_from_an_excluded_long_run() -> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let prefix_body = "a b c d e ";
    let full = format!("{prefix_body}{} a f", "x".repeat(257));
    let (prefix, _) = cold_arena(prefix_body, 0, &mut work)?;
    let (suffix, indexed_suffix) = cold_arena(&full, 6, &mut work)?;
    check_eq!(indexed_suffix, 2);
    let (whole, indexed_whole) = cold_arena(&full, 0, &mut work)?;
    check_eq!(indexed_whole, 7);
    let view = ColdTokenProgramView::new(Some((&prefix, 6)), &suffix)?;
    let actual = groups(&view, &mut work)?;
    check_eq!(actual, whole_groups(&whole, &mut work)?);
    check_eq!(actual.get(b"\0a".as_slice()), Some(&vec![0, 6]));
    check_eq!(actual.get(b"\0f".as_slice()), Some(&vec![7]));
    check!(actual.keys().all(|key| !key.contains(&b'x')));
    drop(view);
    drop(whole);
    drop(suffix);
    drop(prefix);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn borrowed_prefix_preserves_nfc_case_groups_and_absolute_suffix_positions()
-> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let (prefix, indexed_prefix) = cold_arena("CAFÉ beta gamma delta epsilon ", 0, &mut work)?;
    check_eq!(indexed_prefix, 5);
    let full = "CAFÉ beta gamma delta epsilon cafe\u{301} zeta";
    let (suffix, indexed_suffix) = cold_arena(full, 5, &mut work)?;
    check_eq!(indexed_suffix, 2);
    let (whole, _) = cold_arena(full, 0, &mut work)?;
    let view = ColdTokenProgramView::new(Some((&prefix, 5)), &suffix)?;
    let actual = groups(&view, &mut work)?;
    check_eq!(actual, whole_groups(&whole, &mut work)?);
    check_eq!(actual.get("\0CAFÉ".as_bytes()), Some(&vec![0]));
    check_eq!(actual.get("\0café".as_bytes()), Some(&vec![5]));
    check_eq!(actual.get("\u{1}café".as_bytes()), Some(&vec![0, 5]));
    check_eq!(actual.get(b"\0zeta".as_slice()), Some(&vec![6]));
    drop(view);
    drop(whole);
    drop(suffix);
    drop(prefix);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn flat_prefix_program_reuses_two_owner_generations_without_changing_groups()
-> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let (prefix, _) = cold_arena("a b c d e ", 0, &mut work)?;
    let prefix = Arc::new(prefix);
    let first = ColdTokenProgram::from_suffix(Some(Arc::clone(&prefix)), None, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let (middle_suffix, indexed) = cold_arena("a b c d e a f", 5, &mut work)?;
    check_eq!(indexed, 2);
    let middle_suffix = Arc::new(middle_suffix);
    let before_middle_bytes = work.usage().bytes;
    let before_middle_live = memory.live.load(Ordering::Relaxed);
    let middle = ColdTokenProgram::from_suffix(
        Some(Arc::clone(&middle_suffix)),
        Some((&first, 5)),
        &mut work,
    )?
    .ok_or(IndexError::ResourceLimited)?;
    // Ten inherited and fourteen final 8-byte locators plus one 4-byte
    // inherited chunk read and one 4-byte final chunk write are billed.
    // Three one-row suffix runs each probe a later boundary once; every
    // probe rereads two 8-byte Rows (48B total), while its key comparison
    // is reused at the following head rather than billed twice.
    check_eq!(
        (
            work.usage().bytes - before_middle_bytes,
            memory.live.load(Ordering::Relaxed) - before_middle_live,
        ),
        (556, 132)
    );
    let before_last_suffix = work.usage();
    let (last_suffix, indexed) = cold_arena("a b c d e a f g", 7, &mut work)?;
    check_eq!(indexed, 1);
    check_eq!(work.usage().mutations - before_last_suffix.mutations, 1);
    let last_suffix = Arc::new(last_suffix);
    let last = ColdTokenProgram::from_suffix(
        Some(Arc::clone(&last_suffix)),
        Some((&middle, 7)),
        &mut work,
    )?
    .ok_or(IndexError::ResourceLimited)?;
    let (whole, indexed) = cold_arena("a b c d e a f g", 0, &mut work)?;
    check_eq!(indexed, 8);
    let view = ColdTokenProgramView::from_program(&last)?;
    let actual = groups(&view, &mut work)?;
    check_eq!(actual, whole_groups(&whole, &mut work)?);
    check_eq!(actual.get(b"\0a".as_slice()), Some(&vec![0, 5]));
    check_eq!(actual.get(b"\0g".as_slice()), Some(&vec![7]));
    let flat_alias = view
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let full_alias = whole
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let flat_wire = encode_packed(&flat_alias, flat_alias.encoded_len(), &work)?;
    let full_wire = encode_packed(&full_alias, full_alias.encoded_len(), &work)?;
    check_eq!(flat_wire.bytes, full_wire.bytes);
    drop(flat_wire);
    drop(full_wire);
    drop(flat_alias);
    drop(full_alias);
    drop(view);
    drop(whole);
    drop(last);
    drop(middle);
    drop(first);
    drop(last_suffix);
    drop(middle_suffix);
    drop(prefix);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn producer_cache_reuses_only_delimited_verified_prefixes_across_three_owners()
-> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let mut cache = prefix_cache(&mut work)?;
    let (first, indexed, charged) = prepare_cached(&mut cache, "a b c d e ", &mut work)?;
    check_eq!((indexed, charged), (5, 5));
    let (second, indexed, charged) = prepare_cached(&mut cache, "a b c d e a f ", &mut work)?;
    check_eq!((indexed, charged), (7, 2));
    let (third, indexed, charged) = prepare_cached(&mut cache, "a b c d e a f g", &mut work)?;
    check_eq!((indexed, charged), (8, 1));
    let (whole, _) = cold_arena("a b c d e a f g", 0, &mut work)?;
    let view = ColdTokenProgramView::from_program(&third)?;
    let actual = groups(&view, &mut work)?;
    check_eq!(actual, whole_groups(&whole, &mut work)?);
    check_eq!(actual.get(b"\0a".as_slice()), Some(&vec![0, 5]));
    check_eq!(actual.get(b"\0g".as_slice()), Some(&vec![7]));
    let borrowed = view
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let reference = whole
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let borrowed_wire = encode_packed(&borrowed, borrowed.encoded_len(), &work)?;
    let reference_wire = encode_packed(&reference, reference.encoded_len(), &work)?;
    check_eq!(borrowed_wire.bytes, reference_wire.bytes);
    drop(borrowed_wire);
    drop(reference_wire);
    drop(borrowed);
    drop(reference);
    drop(view);
    drop(whole);
    drop(third);
    drop(second);
    drop(first);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn producer_cache_reuses_a_token_before_different_ascii_delimiters() -> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let mut cache = prefix_cache(&mut work)?;
    let (first, indexed, charged) =
        prepare_cached(&mut cache, "alpha beta gamma delta epsilon", &mut work)?;
    check_eq!((indexed, charged), (5, 5));
    let current = "alpha beta-gamma delta epsilon";
    let (second, indexed, charged) = prepare_cached(&mut cache, current, &mut work)?;
    check_eq!((indexed, charged), (5, 3));
    let (whole, _) = cold_arena(current, 0, &mut work)?;
    let view = ColdTokenProgramView::from_program(&second)?;
    check_eq!(groups(&view, &mut work)?, whole_groups(&whole, &mut work)?);
    let borrowed = view
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let reference = whole
        .try_packed_alias(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let borrowed_wire = encode_packed(&borrowed, borrowed.encoded_len(), &work)?;
    let reference_wire = encode_packed(&reference, reference.encoded_len(), &work)?;
    check_eq!(borrowed_wire.bytes, reference_wire.bytes);
    drop(borrowed_wire);
    drop(reference_wire);
    drop(borrowed);
    drop(reference);
    drop(view);
    drop(whole);
    drop(second);
    drop(first);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn delimiter_equivalence_keeps_excluded_run_positions_and_rejects_utf8_change()
-> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let mut cache = prefix_cache(&mut work)?;
    let long = "x".repeat(257);
    let source = format!("a {long} b c");
    let current = format!("a {long} b\nc");
    let (first, indexed, charged) = prepare_cached(&mut cache, &source, &mut work)?;
    check_eq!((indexed, charged), (3, 3));
    let (second, indexed, charged) = prepare_cached(&mut cache, &current, &mut work)?;
    check_eq!((indexed, charged), (3, 1));
    let (whole, _) = cold_arena(&current, 0, &mut work)?;
    let actual = groups(&ColdTokenProgramView::from_program(&second)?, &mut work)?;
    check_eq!(actual, whole_groups(&whole, &mut work)?);
    check_eq!(actual.get(b"\0b".as_slice()), Some(&vec![2]));
    drop(whole);
    drop(second);
    drop(first);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);

    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let mut cache = prefix_cache(&mut work)?;
    let (first, _, _) = prepare_cached(&mut cache, "foo\u{a0}bar", &mut work)?;
    let (second, indexed, charged) = prepare_cached(&mut cache, "foo\u{a1}bar", &mut work)?;
    check_eq!((indexed, charged), (2, 2));
    let (whole, _) = cold_arena("foo\u{a1}bar", 0, &mut work)?;
    check_eq!(
        groups(&ColdTokenProgramView::from_program(&second)?, &mut work)?,
        whole_groups(&whole, &mut work)?
    );
    drop(whole);
    drop(second);
    drop(first);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn delimiter_change_does_not_create_a_one_run_locator_program() -> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let mut cache = prefix_cache(&mut work)?;
    let (first, _, _) = prepare_cached(&mut cache, "alpha beta gamma delta epsilon", &mut work)?;
    let current = "alpha-beta gamma delta epsilon";
    let (second, indexed, charged) = prepare_cached(&mut cache, current, &mut work)?;
    check_eq!((indexed, charged), (5, 5));
    let (whole, _) = cold_arena(current, 0, &mut work)?;
    check_eq!(
        groups(&ColdTokenProgramView::from_program(&second)?, &mut work)?,
        whole_groups(&whole, &mut work)?
    );
    drop(whole);
    drop(second);
    drop(first);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn producer_cache_accepts_nfc_equivalence_and_keeps_excluded_run_positions()
-> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let mut cache = prefix_cache(&mut work)?;
    let (first, _, charged) =
        prepare_cached(&mut cache, "CAFÉ beta gamma delta epsilon ", &mut work)?;
    check_eq!(charged, 5);
    let body = "CAFE\u{301} beta gamma delta epsilon cafe\u{301} zeta";
    let (second, indexed, charged) = prepare_cached(&mut cache, body, &mut work)?;
    check_eq!((indexed, charged), (7, 2));
    let view = ColdTokenProgramView::from_program(&second)?;
    let actual = groups(&view, &mut work)?;
    check_eq!(actual.get("\u{1}café".as_bytes()), Some(&vec![0, 5]));
    drop(view);
    drop(second);
    drop(first);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);

    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let mut cache = prefix_cache(&mut work)?;
    let (first, _, _) = prepare_cached(&mut cache, "a b c d e ", &mut work)?;
    let body = format!("a b c d e {} a f", "x".repeat(257));
    let (second, indexed, charged) = prepare_cached(&mut cache, &body, &mut work)?;
    check_eq!((indexed, charged), (7, 2));
    let view = ColdTokenProgramView::from_program(&second)?;
    let actual = groups(&view, &mut work)?;
    check_eq!(actual.get(b"\0a".as_slice()), Some(&vec![0, 6]));
    check_eq!(actual.get(b"\0f".as_slice()), Some(&vec![7]));
    drop(view);
    drop(second);
    drop(first);
    drop(cache);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn producer_cache_rejects_midword_and_partial_utf8_delimiter_matches() -> Result<(), IndexError> {
    for (first_body, second_body, second_tokens) in [
        ("foo ", "foobar baz", 2u64),
        ("foo\u{a0}bar ", "foo\u{a1}baz", 2u64),
    ] {
        let (mut work, memory) = funded_token_budget(1_000_000)?;
        let mut cache = prefix_cache(&mut work)?;
        let (first, _, _) = prepare_cached(&mut cache, first_body, &mut work)?;
        let (second, indexed, charged) = prepare_cached(&mut cache, second_body, &mut work)?;
        check_eq!((indexed, charged), (second_tokens, second_tokens));
        let (whole, _) = cold_arena(second_body, 0, &mut work)?;
        let view = ColdTokenProgramView::from_program(&second)?;
        check_eq!(groups(&view, &mut work)?, whole_groups(&whole, &mut work)?);
        drop(view);
        drop(whole);
        drop(second);
        drop(first);
        drop(cache);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[test]
fn optional_cache_admission_miss_rewinds_before_any_token_mutation() -> Result<(), IndexError> {
    let body = "a b c d e ";
    let (mut probe_work, probe_memory) = funded_token_budget(1_000_000)?;
    let probe_cache = prefix_cache(&mut probe_work)?;
    let probe_cursor = prepare_indexed_token_cursor(body, &mut probe_work)?;
    let bound = CanonicalTokenCensus::upper_bound(probe_cursor.text().len())?
        .ok_or(IndexError::ResourceLimited)?;
    let probe_arena =
        ColdTokens::try_new(&bound, &probe_work)?.ok_or(IndexError::ResourceLimited)?;
    let baseline_live = probe_memory.live.load(Ordering::Relaxed);
    check!(baseline_live > 0);
    drop(probe_arena);
    drop(probe_cursor);
    drop(probe_cache);
    check_eq!(probe_memory.live.load(Ordering::Relaxed), 0);

    let (mut work, memory) = funded_token_budget(baseline_live)?;
    let mut cache = prefix_cache(&mut work)?;
    let cursor = prepare_indexed_token_cursor(body, &mut work)?;
    let before = work.usage().mutations;
    let (mut cursor, prepared) =
        super::super::token_prefix_prepare::try_prepare(&mut cache, cursor, &mut work)?;
    check!(prepared.is_none());
    check_eq!(work.usage().mutations, before);
    check_eq!(cursor.next_run().map(|run| run.text), Some("a"));
    cursor.rewind();
    let mut tokens = ColdTokens::try_new(&bound, &work)?.ok_or(IndexError::ResourceLimited)?;
    check_eq!(tokens.fill_from_cursor(&mut cursor, &mut work)?, 5);
    tokens.sort_and_validate(&work)?;
    drop(cursor);
    drop(cache);
    check_eq!(
        whole_groups(&tokens, &mut work)?.get(b"\0a".as_slice()),
        Some(&vec![0])
    );
    check_eq!(work.usage().mutations - before, 5);
    check!(work.first_denial().is_none());
    drop(tokens);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn verified_prefix_reuses_rows_when_a_second_full_source_arena_cannot_fit() -> Result<(), IndexError>
{
    let prefix = "PrefixWord ".repeat(40);
    let body = format!("{prefix}tail");
    let (mut calibration, calibration_memory) = funded_token_budget(1_000_000)?;
    let mut calibration_cache = prefix_cache(&mut calibration)?;
    let (program, _, _) = prepare_cached(&mut calibration_cache, &prefix, &mut calibration)?;
    let ceiling = calibration_memory
        .live
        .load(Ordering::Relaxed)
        .checked_add(6_000)
        .ok_or(IndexError::ResourceLimited)?;
    drop((program, calibration_cache));

    let (mut work, memory) = funded_token_budget(ceiling)?;
    let mut cache = prefix_cache(&mut work)?;
    let (first, indexed, mutations) = prepare_cached(&mut cache, &prefix, &mut work)?;
    check_eq!((indexed, mutations), (40, 40));
    let full_bound =
        CanonicalTokenCensus::upper_bound(body.len())?.ok_or(IndexError::ResourceLimited)?;
    check!(ColdTokens::try_new(&full_bound, &work)?.is_none());

    let (second, indexed, mutations) = prepare_cached(&mut cache, &body, &mut work)?;
    check_eq!((indexed, mutations), (41, 1));
    let view = ColdTokenProgramView::from_program(&second)?;
    let actual = groups(&view, &mut work)?;
    let (mut reference_work, _) = funded_token_budget(1_000_000)?;
    let (reference, _) = cold_arena(&body, 0, &mut reference_work)?;
    check_eq!(actual, whole_groups(&reference, &mut reference_work)?);
    check!(work.first_denial().is_none());
    drop(view);
    drop((first, second, cache));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn direct_program_selects_a_small_prefix_with_fixed_work_and_exact_rows() -> Result<(), IndexError>
{
    let memory = Arc::new(TokenMemory {
        ceiling: 1_000_000,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 100_000_000,
            mutations: 10_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        TokenMemoryScope(Arc::clone(&memory)),
    ))));
    let body = "a ".repeat(2_048);
    let (mut tokens, indexed) = fill_arena(&body, 0, &mut work)?;
    check_eq!(indexed, 2_048);
    let before_parent = work.usage().bytes;
    let mut grant = ColdTokenProgram::try_reserve_for_suffix(None, 0, 4_096, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    grant.prepare_suffix(&mut tokens, &mut work)?;
    let parent =
        ColdTokenProgram::from_suffix_with_memory(Some(Arc::new(tokens)), None, grant, &mut work)?;
    let parent_bytes = work.usage().bytes - before_parent;
    let before_select = work.usage().bytes;
    let grant = ColdTokenProgram::try_reserve_for_suffix(Some((&parent, 100)), 200, 2, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let selection_bytes = work.usage().bytes - before_select;
    let before_child = work.usage().bytes;
    let child =
        ColdTokenProgram::from_suffix_with_memory(None, Some((&parent, 100)), grant, &mut work)?;
    let child_bytes = work.usage().bytes - before_child;
    let view = ColdTokenProgramView::from_program(&child)?;
    let expected: Vec<u32> = (0..100).collect();
    let actual = groups(&view, &mut work)?;
    check_eq!(actual.get(b"\0a".as_slice()), Some(&expected));
    check_eq!(actual.get(b"\x01a".as_slice()), Some(&expected));
    check_eq!(actual.len(), 2);
    drop(child);
    drop(parent);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    // The direct parent stores one slab pointer and sixteen chunk minima.
    // Selecting the verified prefix inspects only the two intersecting chunks.
    check_eq!(
        (parent_bytes, selection_bytes, child_bytes),
        (72, 5_760, 3_212)
    );
    Ok(())
}

#[test]
fn flat_merge_contiguous_suffix_and_interleaved_keys_keep_canonical_rows() -> Result<(), IndexError>
{
    let mut observed_bytes = Vec::new();
    for (prefix_body, suffix_body) in [
        ("z ".repeat(100), "a ".repeat(2_048)),
        (
            (0..64)
                .step_by(2)
                .map(|index| format!("a{index:03} "))
                .collect::<String>(),
            (1..64)
                .step_by(2)
                .map(|index| format!("a{index:03} "))
                .collect::<String>(),
        ),
    ] {
        let memory = Arc::new(TokenMemory {
            ceiling: 4_000_000,
            ..TokenMemory::default()
        });
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 100_000_000,
                mutations: 10_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            TokenMemoryScope(Arc::clone(&memory)),
        ))));
        let prefix_runs = prefix_body.split_ascii_whitespace().count();
        let full_body = format!("{prefix_body}{suffix_body}");
        let (prefix, counted_prefix) = cold_arena(&prefix_body, 0, &mut work)?;
        check_eq!(
            counted_prefix,
            u64::try_from(prefix_runs).map_err(|_error| IndexError::ResourceLimited)?
        );
        let parent = ColdTokenProgram::from_suffix(Some(Arc::new(prefix)), None, &mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        let (suffix, counted_suffix) = cold_arena(&full_body, prefix_runs, &mut work)?;
        check_eq!(
            counted_suffix,
            u64::try_from(suffix_body.split_ascii_whitespace().count())
                .map_err(|_error| IndexError::ResourceLimited)?
        );
        let before = work.usage().bytes;
        let child = ColdTokenProgram::from_suffix(
            Some(Arc::new(suffix)),
            Some((
                &parent,
                u32::try_from(prefix_runs).map_err(|_error| IndexError::ResourceLimited)?,
            )),
            &mut work,
        )?
        .ok_or(IndexError::ResourceLimited)?;
        let merge_bytes = work.usage().bytes - before;
        let (whole, counted_whole) = cold_arena(&full_body, 0, &mut work)?;
        check_eq!(counted_whole, counted_prefix + counted_suffix);
        check_eq!(
            groups(&ColdTokenProgramView::from_program(&child)?, &mut work)?,
            whole_groups(&whole, &mut work)?
        );
        observed_bytes.push(merge_bytes);
        drop(whole);
        drop(child);
        drop(parent);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
    }
    // The long case has 200 inherited and 4,096 suffix rows. Fixed non-key
    // work is 1,600 selected scan + 3,204 selected materialization + 16 slab
    // refs + 1,600 selected reads + 34,368 output writes + 68 chunk minima
    // = 40,856B. The original row-by-row merge compared 4,096 suffix keys
    // plus 100 intervening prefix keys (16,784B). This bounded search costs
    // 1,264B for key comparisons and independently charged probe Row reads.
    check_eq!(observed_bytes, vec![42_120, 5_374]);
    check!(observed_bytes.first().is_some_and(|bytes| *bytes < 57_640));
    check!(observed_bytes.get(1).is_some_and(|bytes| *bytes > 4_366));
    Ok(())
}

#[test]
fn chunk_edges_unicode_position_gap_and_two_slabs_match_full_canonical_rows()
-> Result<(), IndexError> {
    let memory = Arc::new(TokenMemory {
        ceiling: 4_000_000,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 100_000_000,
            mutations: 10_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        TokenMemoryScope(Arc::clone(&memory)),
    ))));
    let first_128 = "CAFÉ a ".repeat(64);
    let excluded = format!("{} ", "x".repeat(257));
    let prefix_body = format!("{first_128}{excluded}{}", "CAFÉ a ".repeat(76));
    let (first, indexed) = cold_arena(&prefix_body, 0, &mut work)?;
    check_eq!(indexed, 280);
    let first = ColdTokenProgram::from_suffix(Some(Arc::new(first)), None, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let full_body = format!("{prefix_body}{}", "z ".repeat(120));
    let (suffix, suffix_indexed) = cold_arena(&full_body, 281, &mut work)?;
    check_eq!(suffix_indexed, 120);
    let second =
        ColdTokenProgram::from_suffix(Some(Arc::new(suffix)), Some((&first, 281)), &mut work)?
            .ok_or(IndexError::ResourceLimited)?;

    // The excluded run occupies position 128 without producing a row. Cut
    // 129 selects exactly the first 256 case rows at the chunk boundary.
    let edge_grant =
        ColdTokenProgram::try_reserve_for_suffix(Some((&second, 129)), 256, 0, &mut work)?
            .ok_or(IndexError::ResourceLimited)?;
    let edge = ColdTokenProgram::from_suffix_with_memory(
        None,
        Some((&second, 129)),
        edge_grant,
        &mut work,
    )?;
    let (edge_reference, edge_indexed) =
        cold_arena(&format!("{first_128}{excluded}"), 0, &mut work)?;
    check_eq!(edge_indexed, 128);
    check_eq!(
        groups(&ColdTokenProgramView::from_program(&edge)?, &mut work)?,
        whole_groups(&edge_reference, &mut work)?
    );

    // Cut 320 selects 39 rows from the second slab after all 280 indexed
    // runs in the first slab, retaining their original absolute positions.
    let across_grant =
        ColdTokenProgram::try_reserve_for_suffix(Some((&second, 320)), 638, 0, &mut work)?
            .ok_or(IndexError::ResourceLimited)?;
    let across = ColdTokenProgram::from_suffix_with_memory(
        None,
        Some((&second, 320)),
        across_grant,
        &mut work,
    )?;
    let reference_body = format!("{prefix_body}{}", "z ".repeat(39));
    let (reference, reference_indexed) = cold_arena(&reference_body, 0, &mut work)?;
    check_eq!(reference_indexed, 319);
    check_eq!(
        groups(&ColdTokenProgramView::from_program(&across)?, &mut work)?,
        whole_groups(&reference, &mut work)?
    );
    drop(reference);
    drop(across);
    drop(edge_reference);
    drop(edge);
    drop(second);
    drop(first);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn malformed_position_chunk_count_refuses_before_suffix_token_mutation() -> Result<(), IndexError> {
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let (tokens, indexed) = cold_arena("a b c d e ", 0, &mut work)?;
    check_eq!(indexed, 5);
    let mut program = ColdTokenProgram::from_suffix(Some(Arc::new(tokens)), None, &mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    program.drop_last_position_chunk_for_test();
    let before = work.usage().mutations;
    check!(matches!(
        ColdTokenProgram::try_reserve_for_suffix(Some((&program, 2)), 4, 10, &mut work),
        Err(IndexError::BackendViolation(message))
            if message.contains("position chunk count differs")
    ));
    check_eq!(work.usage().mutations, before);
    drop(program);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}
