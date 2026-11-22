//! Prepare canonical token programs from a verified NFC prefix and new suffix.

use std::sync::Arc;

use quanta_index_core::index::{AttemptContext, IndexError, WorkBudget, WorkUsage};

use super::IndexedTokenCursor;
use super::cold_tokens::{CanonicalTokenCensus, ColdTokenProgram, ColdTokens};
use super::token_prefix_cache::{CacheLookupPlan, TokenPrefixCache};
use quanta_index_core::index::SourceRecord;

/// A run ends at a proved common delimiter, or at two differing one-byte
/// delimiters already classified by the cache's charged byte comparison.
/// Matching a partial UTF-8 character or only a token spelling is insufficient.
fn prefix_boundary(
    cursor: &mut IndexedTokenCursor<'_>,
    shared_bytes: usize,
    equivalent_delimiter: bool,
    budget: &mut WorkBudget,
) -> Result<(u32, u64), IndexError> {
    cursor.rewind();
    let mut cut = 0u32;
    let mut indexed = 0u64;
    let mut inspected = 0usize;
    loop {
        let mut special_consumed = false;
        let Some(_run) = cursor.try_next_run_if(|run, text| {
            budget.checkpoint()?;
            let delimiter_end = text
                .get(run.end..)
                .and_then(|rest| rest.chars().next())
                .and_then(|ch| run.end.checked_add(ch.len_utf8()));
            let scanned = delimiter_end.unwrap_or(run.end);
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    scanned
                        .checked_sub(inspected)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            inspected = scanned;
            let exact_delimiter = delimiter_end.is_some_and(|boundary| boundary <= shared_bytes);
            let equivalent_boundary = delimiter_end.is_some_and(|boundary| {
                equivalent_delimiter
                    // Avoid turning a direct single-slab program into a
                    // locator merge merely to borrow one short run.
                    && indexed != 0
                    && run.is_indexable()
                    && run.end == shared_bytes
                    && Some(boundary) == shared_bytes.checked_add(1)
            });
            if !exact_delimiter && !equivalent_boundary {
                return Ok(false);
            }
            cut = u32::try_from(
                run.position
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?;
            indexed = indexed
                .checked_add(u64::from(run.is_indexable()))
                .ok_or(IndexError::ResourceLimited)?;
            special_consumed = equivalent_boundary;
            Ok(true)
        })?
        else {
            break;
        };
        if special_consumed {
            break;
        }
    }
    Ok((cut, indexed))
}

/// Exclusive cache ownership keeps the search position tied to this exact
/// immutable normalized source until its program is inserted.
struct PreparedCacheEntry<'cache, 'source> {
    cursor: IndexedTokenCursor<'source>,
    lookup: CacheLookupPlan<'cache>,
}

type PreparedTokenProgram<'source> = (
    IndexedTokenCursor<'source>,
    Option<(Arc<ColdTokenProgram>, u64)>,
);

impl<'cache, 'source> PreparedCacheEntry<'cache, 'source> {
    fn new(
        cache: &'cache mut TokenPrefixCache,
        cursor: IndexedTokenCursor<'source>,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let lookup = cache.lookup_plan(cursor.text(), budget)?;
        Ok(Self { cursor, lookup })
    }

    fn rewind(mut self) -> IndexedTokenCursor<'source> {
        self.cursor.rewind();
        self.cursor
    }
}

/// A missing optional grant returns a rewound cursor for the original path.
/// After the first suffix mutation, all failures are terminal.
pub(super) fn try_prepare<'source>(
    cache: &mut TokenPrefixCache,
    cursor: IndexedTokenCursor<'source>,
    budget: &mut WorkBudget,
) -> Result<PreparedTokenProgram<'source>, IndexError> {
    let mut entry = PreparedCacheEntry::new(cache, cursor, budget)?;
    let hit = entry.lookup.hit.as_ref();
    if let Some(hit) = hit.as_ref().filter(|hit| hit.exact_body) {
        entry.cursor.rewind();
        let indexed = hit.program.indexed_run_count()?;
        return Ok((entry.cursor, Some((Arc::clone(&hit.program), indexed))));
    }
    let (cut, prefix_indexed) = if let Some(hit) = hit.as_ref() {
        prefix_boundary(
            &mut entry.cursor,
            hit.raw_lcp_bytes,
            hit.equivalent_delimiter,
            budget,
        )?
    } else {
        (0, 0)
    };
    let inherited = hit
        .as_ref()
        .filter(|_hit| prefix_indexed != 0)
        .map(|hit| (&*hit.program, cut));
    if inherited.is_none() {
        entry.cursor.rewind();
    }
    // The cursor committed only exact common runs, including their delimiter.
    // The same bound now applies to bytes that can still produce new rows.
    let remaining = entry.cursor.remaining_bytes();
    if remaining == 0 && inherited.is_none() {
        return Ok((entry.rewind(), None));
    }
    let Some(mut grant) = entry.lookup.try_admit_entry(entry.cursor.text(), budget)? else {
        return Ok((entry.rewind(), None));
    };
    let (mut arena, max_suffix_rows) = if remaining == 0 {
        (None, 0)
    } else {
        let Some(bound) = CanonicalTokenCensus::upper_bound(remaining)? else {
            return Ok((entry.rewind(), None));
        };
        let Some(arena) = ColdTokens::try_new(&bound, budget)? else {
            return Ok((entry.rewind(), None));
        };
        (Some(arena), bound.row_bound())
    };
    let Some(mut memory) = ColdTokenProgram::try_reserve_for_suffix(
        inherited,
        usize::try_from(prefix_indexed)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?,
        max_suffix_rows,
        budget,
    )?
    else {
        return Ok((entry.rewind(), None));
    };
    let suffix_indexed = if let Some(arena) = arena.as_mut() {
        arena.fill_from_cursor(&mut entry.cursor, budget)?
    } else {
        0
    };
    let total = prefix_indexed
        .checked_add(suffix_indexed)
        .ok_or(IndexError::ResourceLimited)?;
    if total == 0 {
        return Ok((entry.rewind(), None));
    }
    let suffix = arena
        .filter(|_| suffix_indexed != 0)
        .map(|mut arena| {
            memory.prepare_suffix(&mut arena, budget)?;
            grant.wrap_suffix(arena, budget)
        })
        .transpose()?;
    let program = ColdTokenProgram::from_suffix_with_memory(suffix, inherited, memory, budget)?;
    if program.indexed_run_count()? != total {
        return Err(IndexError::BackendViolation(
            "cold program row count differs from indexed source".into(),
        ));
    }
    let program = grant.wrap_program(program, budget)?;
    grant.copy_key(entry.cursor.text(), budget)?;
    entry.lookup.insert(grant, Arc::clone(&program), budget)?;
    Ok((entry.cursor, Some((program, total))))
}

/// Release all inactive cached programs before a mandatory output allocation
/// is refused. The currently selected program retains its own Original grants.
pub(super) fn with_discardable_cache<T>(
    cache: &mut Option<Box<TokenPrefixCache>>,
    context: &mut AttemptContext<'_>,
    operation: impl FnOnce(&mut AttemptContext<'_>) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let Some(state) = cache.take() else {
        return operation(context);
    };
    let attempt = context.attempt;
    let objects = context.objects;
    let maps = context.maps;
    let custody = context.custody;
    let scratch = context.scratch;
    let (result, remaining) = context.budget.with_discardable_memory(state, |budget| {
        operation(&mut AttemptContext {
            attempt,
            objects,
            maps,
            custody,
            scratch,
            budget,
        })
    });
    *cache = remaining;
    result
}

/// The source callback restores the cache after the port has funded the
/// original UTF-8 body. No source copy or additional read is introduced.
pub(super) fn with_source_text<T>(
    record: &SourceRecord,
    context: &mut AttemptContext<'_>,
    cache: &mut Option<Box<TokenPrefixCache>>,
    visit: impl FnOnce(
        &str,
        &mut AttemptContext<'_>,
        &mut Option<Box<TokenPrefixCache>>,
    ) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let Some(state) = cache.take() else {
        return super::with_source_text(record, context, |body, context| {
            visit(body, context, cache)
        });
    };
    let attempt = context.attempt;
    let objects = context.objects;
    let maps = context.maps;
    let custody = context.custody;
    let scratch = context.scratch;
    let mut entered = false;
    let (result, remaining) = context.budget.with_discardable_memory(state, |budget| {
        let mut context = AttemptContext {
            attempt,
            objects,
            maps,
            custody,
            scratch,
            budget,
        };
        super::with_source_text(record, &mut context, |body, context| {
            *cache = context.budget.take_discardable_memory::<TokenPrefixCache>();
            entered = true;
            visit(body, context, cache)
        })
    });
    if !entered {
        *cache = remaining;
    }
    result
}
