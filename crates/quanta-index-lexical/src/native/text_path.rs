//! Source-path term membership for Native Text.
//!
//! Each key is `[u16 big-endian term length][canonical case term][owner]`.
//! The value is the selected SourceRecord object. A phrase is checked against
//! that authenticated record's path; no second path or position copy is kept.

use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapEdit, MapPage, MapRoot, MapValue, MemoryLease,
    MemoryLifetime, ObjectRef, OwnerKey, ScratchAllocation, ScratchRequest, ScratchSpacePort,
    SourceChange, WorkBudget, WorkUsage,
    ports::{MapEditCursor, RootPackingMode, RootPackingPlan},
};
use quanta_index_core::{
    DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};
use quanta_index_lq_text_normalizer::CaseMode;
use quanta_index_scratch_sqlite::{MapEditPage, ScratchConnection};
use rusqlite::types::ValueRef;

use super::text_memory::{
    MAX_TERM_KEY_BYTES, PreparedKey, PreparedRows, decode_case_term_key,
    prepare_indexed_token_cursor,
};
use super::text_statistics::TextStatistics;

mod term_cursor;
pub(super) use term_cursor::PathTermCursor;

pub(super) const PATH_POLICY: &[u8] =
    b"qidx.native.text.path-postings.v2;term-len-be;case-key;owner;source-object;exact-path-phrase;all-source-owners";
pub(super) const LAYERED_PATH_POLICY: &[u8] = b"qidx.native.text.path-postings.layered.v3;term-len-be;case-key;owner;source-digest-inline-no-children;shared-case-postings;exact-path-phrase;all-source-owners";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PathIndex {
    pub(super) postings: MapRoot,
    pub(super) statistics: TextStatistics,
}

impl PathIndex {
    pub(super) fn validate(self, body_docs: u64) -> Result<(), IndexError> {
        let tokens = self.statistics.indexed_tokens;
        if self.statistics.docs < body_docs
            || (self.statistics.docs == 0 && tokens != 0)
            || (self.postings.0.is_none() != (tokens == 0))
        {
            return Err(IndexError::CorruptObject(
                "native text path postings or statistics mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PathPostingValue {
    Object,
    SourceCutDigest,
}

fn posting_value(kind: PathPostingValue, source: ObjectRef) -> Result<MapValue, IndexError> {
    match kind {
        PathPostingValue::Object => Ok(MapValue::Object(source)),
        PathPostingValue::SourceCutDigest => MapValue::inline(&source.0.0, []),
    }
}

fn decode_posting_value(kind: PathPostingValue, value: &MapValue) -> Result<ObjectRef, IndexError> {
    match kind {
        PathPostingValue::Object => Ok(*value.as_object_ref()?),
        PathPostingValue::SourceCutDigest => {
            let MapValue::Inline(inline) = value else {
                return Err(IndexError::CorruptObject(
                    "layered native text path posting requires inline source digest".into(),
                ));
            };
            let data: [u8; 32] = inline.data()?.try_into().map_err(|_error| {
                IndexError::CorruptObject("layered native text path posting digest width".into())
            })?;
            if !inline.children()?.is_empty() {
                return Err(IndexError::CorruptObject(
                    "layered native text path posting has children".into(),
                ));
            }
            Ok(ObjectRef(Digest(data)))
        }
    }
}

const PAGE: u32 = 64;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const SCRATCH_MAX_BYTES: u64 = 4_294_967_296;
const MAX_KEY_BYTES: usize = 2 + 1024 + 32;
const _: () = assert!(MAX_TERM_KEY_BYTES == 1024);
const WRITE_ROWS: usize = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
const INSERT_SQL: &str = "INSERT INTO edits(g,k,p,n) VALUES ";
const FIRST_SQL: &str = "SELECT k,p,n FROM edits WHERE p IS NOT n ORDER BY k LIMIT 64";
const AFTER_SQL: &str = "SELECT k,p,n FROM edits WHERE p IS NOT n AND k>?1 ORDER BY k LIMIT 64";

fn sql(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "native text path scratch")
}

/// Attribute one completed scratch INSERT without changing its Work grant.
/// Off executes the original operation directly, with no trace context work.
fn observe_path_write_batch(
    budget: &mut WorkBudget,
    rows: usize,
    operation: impl FnOnce(&mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let Some(diagnostic) = budget.request().diagnostic_context().cloned() else {
        return operation(budget);
    };
    let Some(span) = crate::causal_profile::enter_span_on_lane(
        &diagnostic,
        DiagnosticStageV1::NativeTextPostings,
        DiagnosticLaneV1(50),
    ) else {
        return operation(budget);
    };
    let before = budget.usage();
    let child = diagnostic.child_context(&span);
    let result = {
        let mut scoped = budget.scoped_diagnostic_context(Some(child));
        let result = operation(&mut scoped);
        scoped.record_diagnostic_usage_delta(before, &span);
        result
    };
    if result.is_ok() {
        diagnostic.counter(&span, DiagnosticCounterV1::SqliteBatches, 1);
        if let Ok(rows) = u64::try_from(rows) {
            diagnostic.counter(&span, DiagnosticCounterV1::SqliteRows, rows);
        } else {
            diagnostic.health(DiagnosticHealthCodeV1::CounterOverflow);
        }
    }
    let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
    result
}

fn owner_bytes(owner: OwnerKey) -> [u8; 32] {
    let mut out = [0; 32];
    let (producer, stable) = out.split_at_mut(16);
    producer.copy_from_slice(&owner.producer.0);
    stable.copy_from_slice(&owner.stable_id.0);
    out
}

fn decode_owner(bytes: &[u8]) -> Result<OwnerKey, IndexError> {
    let raw: &[u8; 32] = bytes
        .try_into()
        .map_err(|_error| IndexError::CorruptObject("native text path owner key width".into()))?;
    let mut producer = [0; 16];
    let mut stable = [0; 16];
    let (producer_wire, stable_wire) = raw.split_at(16);
    producer.copy_from_slice(producer_wire);
    stable.copy_from_slice(stable_wire);
    Ok(OwnerKey {
        producer: quanta_index_core::index::ProducerId(producer),
        stable_id: quanta_index_core::index::StableOwnerId(stable),
    })
}

fn term_prefix(term: &[u8], out: &mut [u8; MAX_KEY_BYTES]) -> Result<usize, IndexError> {
    let _canonical = decode_case_term_key(term)?;
    let length = u16::try_from(term.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let end = 2usize
        .checked_add(term.len())
        .ok_or(IndexError::ResourceLimited)?;
    out.get_mut(..2)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&length.to_be_bytes());
    out.get_mut(2..end)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(term);
    Ok(end)
}

fn validate_page(page: &MapPage, after: &[u8]) -> Result<(), IndexError> {
    if page.entries.is_empty() || page.entries.len() > PAGE as usize {
        return Err(IndexError::BackendViolation(
            "native text path scan page width".into(),
        ));
    }
    if page
        .entries
        .first()
        .is_none_or(|(key, _value)| key.as_slice() <= after)
        || page.entries.windows(2).any(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_none_or(|((left, _), (right, _))| left >= right)
        })
    {
        return Err(IndexError::BackendViolation(
            "native text path scan did not advance in key order".into(),
        ));
    }
    Ok(())
}

struct FundedTerm {
    key: Vec<u8>,
    _memory: Option<MemoryLease>,
}

impl FundedTerm {
    fn new(term: &[u8], owner: OwnerKey, budget: &WorkBudget) -> Result<Self, IndexError> {
        let mut bytes = [0u8; MAX_KEY_BYTES];
        let prefix = term_prefix(term, &mut bytes)?;
        let end = prefix.checked_add(32).ok_or(IndexError::ResourceLimited)?;
        bytes
            .get_mut(prefix..end)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(&owner_bytes(owner));
        let memory = budget.reserve_memory(
            u64::try_from(end).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut key = Vec::new();
        key.try_reserve_exact(end)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if key.capacity() > end {
            return Err(IndexError::ResourceLimited);
        }
        key.extend_from_slice(bytes.get(..end).ok_or(IndexError::ResourceLimited)?);
        Ok(Self {
            key,
            _memory: memory,
        })
    }
}

struct FundedEdit {
    edit: MapEdit,
    _memory: Option<MemoryLease>,
}

fn next_unique<'a>(rows: &'a [FundedTerm], offset: &mut usize) -> Option<&'a FundedTerm> {
    let current = rows.get(*offset)?;
    *offset += 1;
    while rows
        .get(*offset)
        .is_some_and(|following| following.key == current.key)
    {
        *offset += 1;
    }
    Some(current)
}

fn visit_merge(
    prior: &[FundedTerm],
    next: &[FundedTerm],
    prior_source: Option<ObjectRef>,
    next_source: Option<ObjectRef>,
    mut visit: impl FnMut(&[u8], Option<ObjectRef>, Option<ObjectRef>) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let mut prior_offset = 0usize;
    let mut next_offset = 0usize;
    let mut old = next_unique(prior, &mut prior_offset);
    let mut new = next_unique(next, &mut next_offset);
    while old.is_some() || new.is_some() {
        let (key, expected, selected) = match (old, new) {
            (Some(left), Some(right)) => match left.key.cmp(&right.key) {
                std::cmp::Ordering::Less => {
                    old = next_unique(prior, &mut prior_offset);
                    (&left.key, prior_source, None)
                }
                std::cmp::Ordering::Greater => {
                    new = next_unique(next, &mut next_offset);
                    (&right.key, None, next_source)
                }
                std::cmp::Ordering::Equal => {
                    old = next_unique(prior, &mut prior_offset);
                    new = next_unique(next, &mut next_offset);
                    (&left.key, prior_source, next_source)
                }
            },
            (Some(left), None) => {
                old = next_unique(prior, &mut prior_offset);
                (&left.key, prior_source, None)
            }
            (None, Some(right)) => {
                new = next_unique(next, &mut next_offset);
                (&right.key, None, next_source)
            }
            (None, None) => break,
        };
        if expected != selected {
            visit(key, expected, selected)?;
        }
    }
    Ok(())
}

/// One attempt-private bounded sort of path posting changes.
pub(super) struct PathWrites {
    // Connection must close before its namespace lease retires.
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
    docs_delta: i64,
    indexed_delta: i64,
    poisoned: bool,
    value: PathPostingValue,
}

impl PathWrites {
    pub(super) fn new(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::with_value(scratch, PathPostingValue::Object, budget)
    }

    pub(super) fn with_value(
        scratch: &dyn ScratchSpacePort,
        value: PathPostingValue,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let allocation = scratch.allocate(
            ScratchRequest {
                initial_bytes: SCRATCH_INITIAL_BYTES,
                max_bytes: SCRATCH_MAX_BYTES,
            },
            budget,
        )?;
        if allocation.max_bytes() != SCRATCH_MAX_BYTES {
            return Err(IndexError::BackendViolation(
                "native text path scratch envelope changed".into(),
            ));
        }
        let db = ScratchConnection::open(
            &allocation.path().join("text_path.sqlite"),
            allocation.as_ref(),
            budget,
        )?;
        {
            let scoped = db.scoped(budget)?;
            scoped
                .execute_batch(
                    "PRAGMA page_size=4096; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;
                     PRAGMA temp_store=FILE; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;",
                )
                .map_err(|error| sql(&error))?;
            quanta_index_scratch_sqlite::configure(&scoped, allocation.as_ref())?;
            quanta_index_scratch_sqlite::reserve_for_write(
                &scoped,
                allocation.as_ref(),
                8192,
                budget,
            )?;
            scoped
                .execute_batch(
                    "CREATE TABLE edits(g BLOB NOT NULL,k BLOB PRIMARY KEY,p BLOB,n BLOB) WITHOUT ROWID;
                     BEGIN IMMEDIATE;",
                )
                .map_err(|error| sql(&error))?;
        }
        Ok(Self {
            db,
            allocation,
            docs_delta: 0,
            indexed_delta: 0,
            poisoned: false,
            value,
        })
    }

    fn write_batch(
        &mut self,
        rows: &PreparedRows<FundedEdit>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.poisoned {
            return Err(IndexError::BackendViolation(
                "native text path scratch attempt was poisoned".into(),
            ));
        }
        let result = observe_path_write_batch(budget, rows.len(), |budget| {
            if rows.len() == 0 || rows.len() > WRITE_ROWS {
                return Err(IndexError::ResourceLimited);
            }
            let mut reservations = [0u64; WRITE_ROWS];
            for (slot, row) in rows.as_slice().iter().enumerate() {
                *reservations
                    .get_mut(slot)
                    .ok_or(IndexError::ResourceLimited)? = u64::try_from(row.edit.key.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .checked_add(128)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            let scoped = self.db.scoped(budget)?;
            quanta_index_scratch_sqlite::write_map_edit_batch(
                &scoped,
                self.allocation.as_ref(),
                INSERT_SQL,
                4096,
                reservations
                    .get(..rows.len())
                    .ok_or(IndexError::ResourceLimited)?,
                |slot| {
                    let row = rows
                        .as_slice()
                        .get(slot)
                        .ok_or(IndexError::ResourceLimited)?;
                    Ok((ValueRef::Blob(b"path"), &row.edit, 0))
                },
                budget,
            )
        });
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn path_terms(
        value: PathPostingValue,
        path: &str,
        owner: OwnerKey,
        budget: &mut WorkBudget,
    ) -> Result<(PreparedRows<FundedTerm>, u64), IndexError> {
        let mut cursor = prepare_indexed_token_cursor(path, budget)?;
        let mut count = 0u64;
        let maximum = cursor
            .text()
            .len()
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?
            .max(1);
        let mut terms = PreparedRows::new(maximum)?;
        while let Some(run) = cursor.next_run() {
            budget.checkpoint()?;
            if !run.is_indexable() {
                continue;
            }
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                mutations: 1,
                bytes: u64::try_from(run.text.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .checked_mul(4)
                    .and_then(|bytes| bytes.checked_add(128))
                    .ok_or(IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let sensitive = PreparedKey::from_case_term(run.text, CaseMode::Sensitive, budget)?;
            let folded = PreparedKey::from_case_term(run.text, CaseMode::Folded, budget)?;
            let sensitive_body = sensitive
                .as_slice()
                .get(1..)
                .ok_or(IndexError::ResourceLimited)?;
            let folded_body = folded
                .as_slice()
                .get(1..)
                .ok_or(IndexError::ResourceLimited)?;
            let store_folded = value == PathPostingValue::Object || sensitive_body != folded_body;
            terms.push(
                FundedTerm::new(sensitive.as_slice(), owner, budget)?,
                budget,
            )?;
            if store_folded {
                terms.push(FundedTerm::new(folded.as_slice(), owner, budget)?, budget)?;
            }
        }
        terms
            .as_mut_slice()
            .sort_unstable_by(|left, right| left.key.cmp(&right.key));
        Ok((terms, count))
    }

    fn emit_merge(
        &mut self,
        prior: &[FundedTerm],
        next: &[FundedTerm],
        prior_source: Option<ObjectRef>,
        next_source: Option<ObjectRef>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut packet = PreparedRows::<FundedEdit>::with_capacity(WRITE_ROWS, budget)?;
        let value = self.value;
        visit_merge(
            prior,
            next,
            prior_source,
            next_source,
            |key, expected, selected| {
                budget.checkpoint()?;
                let memory = budget.reserve_memory(
                    u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?;
                let mut copied = Vec::new();
                copied
                    .try_reserve_exact(key.len())
                    .map_err(|_error| IndexError::ResourceLimited)?;
                if copied.capacity() > key.len() {
                    return Err(IndexError::ResourceLimited);
                }
                copied.extend_from_slice(key);
                packet.push(
                    FundedEdit {
                        edit: MapEdit {
                            key: copied,
                            expected: expected
                                .map(|source| posting_value(value, source))
                                .transpose()?,
                            next: selected
                                .map(|source| posting_value(value, source))
                                .transpose()?,
                        },
                        _memory: memory,
                    },
                    budget,
                )?;
                if packet.len() == WRITE_ROWS {
                    self.write_batch(&packet, budget)?;
                    packet.clear();
                }
                Ok(())
            },
        )?;
        if packet.len() != 0 {
            self.write_batch(&packet, budget)?;
        }
        Ok(())
    }

    pub(super) fn push_change(
        &mut self,
        change: &SourceChange,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let prior = change
            .prior
            .as_ref()
            .map(|(source, record)| (*source, record));
        let next = change
            .next
            .as_ref()
            .map(|(source, record)| (*source, record));
        let docs_delta = i64::from(next.is_some()) - i64::from(prior.is_some());
        self.docs_delta = self
            .docs_delta
            .checked_add(docs_delta)
            .ok_or(IndexError::ResourceLimited)?;
        if let (Some((prior_source, prior_record)), Some((next_source, next_record))) =
            (prior, next)
            && prior_record.path == next_record.path
        {
            if prior_source != next_source {
                let (terms, _count) =
                    Self::path_terms(self.value, &prior_record.path, change.owner, budget)?;
                self.emit_merge(
                    terms.as_slice(),
                    terms.as_slice(),
                    Some(prior_source),
                    Some(next_source),
                    budget,
                )?;
            }
            return Ok(());
        }
        let mut prior_terms = None;
        if let Some((_source, record)) = prior {
            let (terms, count) = Self::path_terms(self.value, &record.path, change.owner, budget)?;
            prior_terms = Some(terms);
            self.indexed_delta = self
                .indexed_delta
                .checked_sub(i64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
        }
        let mut next_terms = None;
        if let Some((_source, record)) = next {
            let (terms, count) = Self::path_terms(self.value, &record.path, change.owner, budget)?;
            next_terms = Some(terms);
            self.indexed_delta = self
                .indexed_delta
                .checked_add(i64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
        }
        self.emit_merge(
            prior_terms.as_ref().map_or(&[], PreparedRows::as_slice),
            next_terms.as_ref().map_or(&[], PreparedRows::as_slice),
            prior.map(|(source, _record)| source),
            next.map(|(source, _record)| source),
            budget,
        )
    }

    pub(super) fn finish(
        self,
        base: PathIndex,
        context: &mut AttemptContext<'_>,
    ) -> Result<PathIndex, IndexError> {
        if self.poisoned {
            return Err(IndexError::BackendViolation(
                "native text path scratch attempt was poisoned".into(),
            ));
        }
        let apply_delta = |prior: u64, delta: i64| {
            prior.checked_add_signed(delta).ok_or_else(|| {
                IndexError::CorruptObject("native text path statistics outside range".into())
            })
        };
        let statistics = TextStatistics {
            docs: apply_delta(base.statistics.docs, self.docs_delta)?,
            indexed_tokens: apply_delta(base.statistics.indexed_tokens, self.indexed_delta)?,
        };
        let mut edits = PathEditCursor {
            writer: &self,
            after: None,
            page: MapEditPage::default(),
            // Group the cold Tree's source references into fewer physical map
            // objects to reduce the work of registering and confirming them.
            packing_mode: if base.postings.0.is_none()
                && statistics.indexed_tokens > 0
                && self.value == PathPostingValue::Object
            {
                RootPackingMode::Nested
            } else {
                RootPackingMode::None
            },
        };
        let roots = context.apply_stream(&[base.postings], &mut edits)?;
        let root =
            roots.as_slice().first().copied().ok_or_else(|| {
                IndexError::BackendViolation("native text path root missing".into())
            })?;
        Ok(PathIndex {
            postings: root,
            statistics,
        })
    }
}

struct PathEditCursor<'a> {
    writer: &'a PathWrites,
    after: Option<quanta_index_core::index::OrdinalMapEdit>,
    page: MapEditPage,
    packing_mode: RootPackingMode,
}

impl MapEditCursor for PathEditCursor<'_> {
    fn subtree_packing(&self) -> Result<RootPackingPlan, IndexError> {
        let mut packing = RootPackingPlan::default();
        packing.select(0, self.packing_mode)?;
        Ok(packing)
    }

    fn next_edit(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<quanta_index_core::index::OrdinalMapEdit>, IndexError> {
        if let Some(edit) = self.page.pop()? {
            self.after = Some(edit.clone());
            return Ok(Some(edit));
        }
        if self.page.complete() {
            return Ok(None);
        }
        let scoped = self.writer.db.scoped(budget)?;
        let mut statement = scoped
            .prepare_cached(if self.after.is_some() {
                AFTER_SQL
            } else {
                FIRST_SQL
            })
            .map_err(|error| sql(&error))?;
        self.page = if let Some(after) = &self.after {
            MapEditPage::read(
                &mut statement,
                &[&after.key()],
                4096,
                budget,
                |row, budget| {
                    quanta_index_scratch_sqlite::read_map_edit_budgeted(
                        row,
                        MAX_KEY_BYTES,
                        0,
                        budget,
                    )
                },
            )?
        } else {
            MapEditPage::read(&mut statement, &[], 4096, budget, |row, budget| {
                quanta_index_scratch_sqlite::read_map_edit_budgeted(row, MAX_KEY_BYTES, 0, budget)
            })?
        };
        self.page.pop().map(|edit| {
            if let Some(edit) = &edit {
                self.after = Some(edit.clone());
            }
            edit
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::RequestBudgetV1;
    use quanta_index_core::index::{Digest, ProducerId, StableOwnerId, WorkLimits};
    use std::time::{Duration, Instant};

    #[test]
    fn path_write_batch_trace_preserves_sql_edits_and_work() -> Result<(), IndexError> {
        use quanta_index_core::{
            DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
            DiagnosticEventSinkV1, DiagnosticRootKindV1,
        };
        use std::{path::Path, sync::Arc};

        #[derive(Debug)]
        struct Scratch;
        struct Allocation(tempfile::TempDir);
        impl ScratchAllocation for Allocation {
            fn path(&self) -> &Path {
                self.0.path()
            }

            fn max_bytes(&self) -> u64 {
                SCRATCH_MAX_BYTES
            }

            fn reserved_bytes(&self) -> Result<u64, IndexError> {
                Ok(SCRATCH_MAX_BYTES)
            }

            fn reserve_at_least(
                &self,
                bytes: u64,
                budget: &mut WorkBudget,
            ) -> Result<(), IndexError> {
                budget.checkpoint()?;
                if bytes > SCRATCH_MAX_BYTES {
                    return Err(IndexError::ResourceLimited);
                }
                Ok(())
            }
        }
        impl ScratchSpacePort for Scratch {
            fn allocate(
                &self,
                _request: ScratchRequest,
                budget: &mut WorkBudget,
            ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
                budget.checkpoint()?;
                Ok(Box::new(Allocation(tempfile::tempdir().map_err(
                    |error| IndexError::Storage(format!("path test scratch: {error}")),
                )?)))
            }
        }
        #[derive(Debug, Default)]
        struct Clock;
        impl DiagnosticClockV1 for Clock {
            fn now_ns(&self) -> u64 {
                1
            }
        }
        #[derive(Debug, Default)]
        struct Sink;
        impl DiagnosticEventSinkV1 for Sink {
            fn try_emit(&self, _event: DiagnosticEmitV1) -> bool {
                true
            }
        }

        fn run(budget: &mut WorkBudget) -> Result<Vec<(Vec<u8>, Option<MapValue>)>, IndexError> {
            let mut writer =
                PathWrites::with_value(&Scratch, PathPostingValue::SourceCutDigest, budget)?;
            let mut rows = PreparedRows::with_capacity(WRITE_ROWS, budget)?;
            for (key, fill) in [(b"alpha".as_slice(), 1), (b"beta".as_slice(), 2)] {
                rows.push(
                    FundedEdit {
                        edit: MapEdit {
                            key: key.to_vec(),
                            expected: None,
                            next: Some(MapValue::inline(&[fill; 32], [])?),
                        },
                        _memory: None,
                    },
                    budget,
                )?;
            }
            writer.write_batch(&rows, budget)?;
            let mut edits = PathEditCursor {
                writer: &writer,
                after: None,
                page: MapEditPage::default(),
                packing_mode: RootPackingMode::None,
            };
            let mut output = Vec::new();
            while let Some(edit) = edits.next_edit(budget)? {
                if edit.expected()?.is_some() {
                    return Err(IndexError::BackendViolation(
                        "new path edit gained an expected value".into(),
                    ));
                }
                output.push((edit.key().to_vec(), edit.next()?));
            }
            assert_eq!(writer.docs_delta, 0);
            assert_eq!(writer.indexed_delta, 0);
            assert!(matches!(
                writer.write_batch(&PreparedRows::new(WRITE_ROWS)?, budget),
                Err(IndexError::ResourceLimited)
            ));
            assert!(writer.poisoned);
            Ok(output)
        }

        let limits = WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 64 * 1024 * 1024,
            mutations: 10_000,
        };
        let mut off = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(10)),
        );
        let off_edits = run(&mut off)?;
        let trace = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([51; 16]),
            Arc::new(Clock),
            Arc::new(Sink),
        );
        let root = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let mut recorded = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(10))
                .with_diagnostic_context(trace.child_context(&root)),
        );
        let recorded_edits = run(&mut recorded)?;
        root.finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        assert_eq!(recorded_edits, off_edits);
        assert_eq!(recorded.usage(), off.usage());
        assert_eq!(off_edits.len(), 2);
        Ok(())
    }

    #[test]
    fn path_write_batch_trace_preserves_work_and_reports_completed_insert() -> Result<(), IndexError>
    {
        use quanta_index_core::{
            DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
            DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticRootKindV1,
        };
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        };

        #[derive(Debug, Default)]
        struct Clock(AtomicU64);
        impl DiagnosticClockV1 for Clock {
            fn now_ns(&self) -> u64 {
                self.0.fetch_add(1, Ordering::SeqCst)
            }
        }
        #[derive(Debug, Default)]
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

        fn submit(budget: &mut WorkBudget, fail: bool) -> Result<(), IndexError> {
            observe_path_write_batch(budget, if fail { 3 } else { 2 }, |budget| {
                budget.charge(WorkUsage {
                    reads: if fail { 1 } else { 2 },
                    writes: 3,
                    bytes: if fail { 5 } else { 7 },
                    mutations: 1,
                })?;
                if fail {
                    Err(IndexError::BackendViolation("scratch refused".into()))
                } else {
                    Ok(())
                }
            })
        }

        let limits = WorkLimits {
            reads: 10,
            writes: 10,
            bytes: 100,
            mutations: 10,
        };
        let clock = Arc::new(Clock::default());
        let sink = Arc::new(Sink::default());
        let trace =
            DiagnosticContextV1::new(DiagnosticCaptureIdV1([50; 16]), clock.clone(), sink.clone());
        let mut off = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(5)),
        );
        submit(&mut off, false)?;
        assert!(matches!(
            submit(&mut off, true),
            Err(IndexError::BackendViolation(_))
        ));
        assert_eq!(clock.0.load(Ordering::SeqCst), 0);

        let root = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let mut recorded = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(5))
                .with_diagnostic_context(trace.child_context(&root)),
        );
        submit(&mut recorded, false)?;
        assert!(matches!(
            submit(&mut recorded, true),
            Err(IndexError::BackendViolation(_))
        ));
        assert_eq!(recorded.usage(), off.usage());
        root.finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;

        let events = sink
            .0
            .lock()
            .map_err(|_error| IndexError::BackendViolation("path trace lock".into()))?;
        let spans: Vec<_> = events
            .iter()
            .filter_map(|event| match event.kind {
                DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    stage: DiagnosticStageV1::NativeTextPostings,
                    lane: DiagnosticLaneV1(50),
                    ..
                } => Some(span_id),
                _ => None,
            })
            .collect();
        assert_eq!(spans.len(), 2);
        let counters = |span_id, key| {
            events
                .iter()
                .filter_map(|event| match event.kind {
                    DiagnosticEventKindV1::Counter {
                        span_id: owner,
                        key: observed,
                        value,
                    } if owner == span_id && observed == key => Some(value),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(counters(spans[0], DiagnosticCounterV1::SqliteBatches), [1]);
        assert_eq!(counters(spans[0], DiagnosticCounterV1::SqliteRows), [2]);
        assert!(counters(spans[1], DiagnosticCounterV1::SqliteBatches).is_empty());
        assert!(counters(spans[1], DiagnosticCounterV1::SqliteRows).is_empty());
        for (span, reads, writes, bytes, mutations) in
            [(spans[0], 2, 3, 7, 1), (spans[1], 1, 3, 5, 1)]
        {
            for (key, expected) in [
                (DiagnosticCounterV1::WorkReads, reads),
                (DiagnosticCounterV1::WorkWrites, writes),
                (DiagnosticCounterV1::WorkBytes, bytes),
                (DiagnosticCounterV1::WorkMutations, mutations),
            ] {
                assert_eq!(counters(span, key), [expected]);
            }
        }
        assert!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::SpanEnd {
                span_id,
                outcome: DiagnosticTerminalV1::Error { .. },
            } if span_id == spans[1]
        )));
        Ok(())
    }

    #[test]
    fn path_term_key_has_bounded_owner_suffix() -> Result<(), IndexError> {
        let owner = OwnerKey {
            producer: ProducerId([3; 16]),
            stable_id: StableOwnerId([7; 16]),
        };
        let mut encoded = [0u8; MAX_KEY_BYTES];
        let prefix = term_prefix(b"\0src", &mut encoded)?;
        encoded
            .get_mut(prefix..prefix + 32)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(&owner_bytes(owner));
        if encoded.get(..prefix) != Some(b"\0\x04\0src".as_slice())
            || decode_owner(
                encoded
                    .get(prefix..prefix + 32)
                    .ok_or(IndexError::ResourceLimited)?,
            )? != owner
        {
            return Err(IndexError::BackendViolation(
                "path term owner suffix changed".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn path_terms_deduplicate_repeated_runs() {
        let rows = [
            FundedTerm {
                key: b"alpha".to_vec(),
                _memory: None,
            },
            FundedTerm {
                key: b"alpha".to_vec(),
                _memory: None,
            },
            FundedTerm {
                key: b"beta".to_vec(),
                _memory: None,
            },
        ];
        let mut offset = 0;
        assert_eq!(
            next_unique(&rows, &mut offset).map(|row| row.key.as_slice()),
            Some(b"alpha".as_slice())
        );
        assert_eq!(
            next_unique(&rows, &mut offset).map(|row| row.key.as_slice()),
            Some(b"beta".as_slice())
        );
        assert!(next_unique(&rows, &mut offset).is_none());
    }

    #[test]
    fn rename_and_delete_emit_exact_unique_preconditions() -> Result<(), IndexError> {
        let old_source = ObjectRef(Digest([11; 32]));
        let new_source = ObjectRef(Digest([12; 32]));
        let term = |key: &[u8]| FundedTerm {
            key: key.to_vec(),
            _memory: None,
        };
        let old = [term(b"alpha"), term(b"alpha"), term(b"shared")];
        let new = [term(b"shared"), term(b"zeta"), term(b"zeta")];
        let mut rename = Vec::new();
        visit_merge(
            &old,
            &new,
            Some(old_source),
            Some(new_source),
            |key, prior, next| {
                rename.push((key.to_vec(), prior, next));
                Ok(())
            },
        )?;
        assert_eq!(
            rename.as_slice(),
            &[
                (b"alpha".to_vec(), Some(old_source), None),
                (b"shared".to_vec(), Some(old_source), Some(new_source)),
                (b"zeta".to_vec(), None, Some(new_source)),
            ]
        );
        let mut delete = Vec::new();
        visit_merge(&old, &[], Some(old_source), None, |key, prior, next| {
            delete.push((key.to_vec(), prior, next));
            Ok(())
        })?;
        assert_eq!(
            delete.as_slice(),
            &[
                (b"alpha".to_vec(), Some(old_source), None),
                (b"shared".to_vec(), Some(old_source), None),
            ]
        );
        Ok(())
    }

    #[test]
    fn layered_path_value_has_exact_digest_without_child_edges() -> Result<(), IndexError> {
        let source = ObjectRef(Digest([0x5a; 32]));
        let value = posting_value(PathPostingValue::SourceCutDigest, source)?;
        assert_eq!(
            decode_posting_value(PathPostingValue::SourceCutDigest, &value)?,
            source
        );
        assert!(value.children()?.is_empty());
        assert!(decode_posting_value(PathPostingValue::Object, &value).is_err());
        assert!(
            decode_posting_value(PathPostingValue::SourceCutDigest, &MapValue::Object(source))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn layered_path_rejects_wrong_digest_width_or_registered_child() -> Result<(), IndexError> {
        let source = ObjectRef(Digest([0x5a; 32]));
        for data in [&[][..], &[0x5a; 31][..]] {
            let value = MapValue::inline(data, [])?;
            assert!(decode_posting_value(PathPostingValue::SourceCutDigest, &value).is_err());
        }
        let value = MapValue::inline(&source.0.0, [source])?;
        assert!(decode_posting_value(PathPostingValue::SourceCutDigest, &value).is_err());
        Ok(())
    }

    #[test]
    fn layered_path_shares_identical_case_posting_but_keeps_distinct_fold() -> Result<(), IndexError>
    {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 1_000_000,
                mutations: 1_000,
            },
            deadline,
        );
        let owner = OwnerKey {
            producer: ProducerId([3; 16]),
            stable_id: StableOwnerId([7; 16]),
        };
        for (term, layered_rows) in [("alpha", 1), ("Alpha", 2)] {
            let (tree, tree_count) =
                PathWrites::path_terms(PathPostingValue::Object, term, owner, &mut budget)?;
            let (layered, layered_count) = PathWrites::path_terms(
                PathPostingValue::SourceCutDigest,
                term,
                owner,
                &mut budget,
            )?;
            assert_eq!(tree_count, 1);
            assert_eq!(layered_count, tree_count);
            assert_eq!(tree.as_slice().len(), 2);
            assert_eq!(layered.as_slice().len(), layered_rows);
            assert!(layered.as_slice().iter().all(|row| {
                tree.as_slice()
                    .iter()
                    .any(|original| original.key == row.key)
            }));
        }
        Ok(())
    }
}
