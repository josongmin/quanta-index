//! Render selected body ranges from the ranker's prepared matchers and truth.

use core::{cell::RefCell, ops::Range};
use std::borrow::Cow;

use quanta_index_contract::{
    PreviewByteRange, PreviewUnavailableReason,
    native_protocol::{NativeTextExcerpt, NativeTextPreviewEvidence},
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, SourceRecordOwner, WorkBudget, WorkUsage,
    ports::ImmutableObjectPort,
};
use quanta_index_lq_regex::executor::RegexRangeError;
use quanta_index_lq_text_normalizer::{
    self as normalize, BoundedNfcError, CaseMode, MappedText, MappingError, Token,
};

use crate::native::text_memory::decode_case_term_key;
use crate::preview::{
    PositiveWitness, SnippetLimits, WindowError, selected_content_window,
    token_allocation_bound_bytes,
};

use super::{
    Clause, ClauseTruth, HeapHit, MAX_CONTENT_LEAVES, MAX_QUERY_NODES, NativeRegexProgram,
    NativeTextRankedHit, TextPostingCursor, TextReadContext,
};

pub(in crate::native) type PreviewVisitor<'a> =
    dyn FnMut(&PreviewSelection<'_>, &mut TextReadContext<'_>) -> Result<(), IndexError> + 'a;

pub(in crate::native) struct PreviewSelection<'a> {
    pub(super) hits: &'a [HeapHit<ClauseTruth>],
    pub(super) program: &'a [Clause],
    pub(super) term_remap: &'a [usize],
    pub(super) cursors: &'a [TextPostingCursor],
    pub(super) regex: &'a NativeRegexProgram,
    pub(super) case: CaseMode,
}

impl PreviewSelection<'_> {
    pub(in crate::native) fn ranked(
        &self,
    ) -> impl ExactSizeIterator<Item = NativeTextRankedHit> + '_ {
        self.hits.iter().map(|row| row.0)
    }

    pub(in crate::native) fn prepared(&self) -> impl Iterator<Item = PreparedPreview<'_>> + '_ {
        self.hits.iter().map(|row| PreparedPreview {
            program: self.program,
            truth: row.1,
            term_remap: self.term_remap,
            cursors: self.cursors,
            regex: self.regex,
            case: self.case,
            path_only: row.2,
        })
    }
}

pub(in crate::native) struct PreparedPreview<'a> {
    program: &'a [Clause],
    truth: ClauseTruth,
    term_remap: &'a [usize],
    cursors: &'a [TextPostingCursor],
    regex: &'a NativeRegexProgram,
    case: CaseMode,
    path_only: bool,
}

impl PreparedPreview<'_> {
    pub(in crate::native) const fn is_path_only(&self) -> bool {
        self.path_only
    }
}

#[derive(Clone, Copy)]
pub(in crate::native) struct PreviewSource {
    reference: ObjectRef,
    byte_len: u64,
}

impl PreviewSource {
    /// Keep the locator from the authenticated record after its path is moved.
    pub(in crate::native) fn from_record(record: &SourceRecordOwner) -> Self {
        Self {
            reference: record.bytes,
            byte_len: record.byte_len,
        }
    }
}

enum Stop {
    Unavailable(PreviewUnavailableReason),
    Failure(IndexError),
}

impl From<IndexError> for Stop {
    fn from(error: IndexError) -> Self {
        Self::Failure(error)
    }
}

type Projection = (NativeTextPreviewEvidence, Option<MemoryLease>);

enum Coordinates<'a> {
    Identity(&'a str),
    Mapped {
        coordinates: MappedText<'a>,
        _memory: MemoryLease,
    },
}

impl Coordinates<'_> {
    fn witness(&self, range: Range<usize>) -> Result<PositiveWitness, MappingError> {
        match self {
            Self::Identity(raw) if raw.get(range.clone()).is_some() => Ok(PositiveWitness {
                original: range.clone(),
                normalized: range,
            }),
            Self::Identity(_) => Err(MappingError::InvalidSpan),
            Self::Mapped { coordinates, .. } => Ok(PositiveWitness {
                original: coordinates.source_range(range.clone())?,
                normalized: coordinates.normalized_range(range)?,
            }),
        }
    }
}

fn unavailable(reason: PreviewUnavailableReason) -> Projection {
    (NativeTextPreviewEvidence::Unavailable(reason), None)
}

fn optional_work() -> Stop {
    Stop::Unavailable(PreviewUnavailableReason::WorkBudget)
}

fn interrupted(budget: &WorkBudget) -> Stop {
    Stop::Failure(budget.checkpoint().err().unwrap_or_else(|| {
        IndexError::BackendViolation("native preview reported an unobserved interruption".into())
    }))
}

fn corrupt() -> IndexError {
    IndexError::CorruptObject("native preview source differs from prepared hit evidence".into())
}

fn pay(budget: &mut WorkBudget, bytes: usize) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })
}

fn loan(budget: &WorkBudget, bytes: usize, lifetime: MemoryLifetime) -> Result<MemoryLease, Stop> {
    budget
        .try_reserve_memory_for_optimization(
            u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
            lifetime,
        )?
        .ok_or_else(optional_work)
}

fn map_error(error: MappingError, budget: &WorkBudget) -> Stop {
    match error {
        MappingError::ByteLimit | MappingError::EntryLimit | MappingError::AllocationRefused => {
            optional_work()
        }
        MappingError::Interrupted => interrupted(budget),
        MappingError::SourceMismatch | MappingError::InvalidSpan => Stop::Failure(corrupt()),
    }
}

impl PreparedPreview<'_> {
    pub(in crate::native) fn render(
        &self,
        source: PreviewSource,
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<Projection, IndexError> {
        let limits = SnippetLimits::default();
        pay(budget, self.program.len())?;
        let mut clauses = [None; MAX_QUERY_NODES];
        let mut count = 0usize;
        self.truth.visit_positive_body(self.program, |clause| {
            *clauses.get_mut(count).ok_or(IndexError::ResourceLimited)? = Some(clause);
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            Ok(())
        })?;
        if count == 0 {
            return Ok(unavailable(PreviewUnavailableReason::NoPositiveWitness));
        }
        if usize::try_from(source.byte_len).map_or(true, |len| len > limits.source_bytes) {
            return Ok(unavailable(PreviewUnavailableReason::WorkBudget));
        }
        let mut projected = None;
        objects
            .visit_utf8_leaf(
                source.reference,
                source.byte_len,
                budget,
                &mut |raw, budget| {
                    if projected.is_some() {
                        return Err(IndexError::BackendViolation(
                            "native preview leaf visited more than once".into(),
                        ));
                    }
                    projected = Some(match self.render_body(raw, &clauses, limits, budget) {
                        Ok(result) => result,
                        Err(Stop::Unavailable(reason)) => unavailable(reason),
                        Err(Stop::Failure(error)) => return Err(error),
                    });
                    budget.checkpoint()
                },
            )
            .map_err(|error| {
                if matches!(error, IndexError::InvalidInput(_)) {
                    corrupt()
                } else {
                    error
                }
            })?;
        projected.ok_or_else(|| {
            IndexError::BackendViolation("native preview leaf was not visited".into())
        })
    }

    fn render_body(
        &self,
        raw: &str,
        clauses: &[Option<Clause>],
        limits: SnippetLimits,
        budget: &mut WorkBudget,
    ) -> Result<Projection, Stop> {
        let (_nfc_loan, indexed) = {
            // Work checkpoints and scratch births run sequentially and use
            // the same selected request's budget for the duration of each call.
            let admission = RefCell::new(&mut *budget);
            let scratch_cap = limits.transformed_bytes.saturating_mul(8);
            let (indexed, memory) = normalize::bounded_nfc_with_checkpoint(
                raw,
                limits.transformed_bytes,
                scratch_cap,
                |units| {
                    admission
                        .borrow_mut()
                        .charge(WorkUsage {
                            bytes: units,
                            ..WorkUsage::default()
                        })
                        .map_err(Stop::Failure)
                },
                |bytes| loan(&admission.borrow(), bytes, MemoryLifetime::Temporary),
            )
            .map_err(|error| match error.into_checkpoint() {
                Ok(cause) => cause,
                Err(BoundedNfcError::LengthChanged) => Stop::Failure(corrupt()),
                Err(_error) => optional_work(),
            })?;
            (memory, indexed)
        };
        let coordinates = match &indexed {
            // The normalizer only borrows when its NFC output is identical
            // to the original. UTF-8 ranges already use both coordinates.
            Cow::Borrowed(raw) => Coordinates::Identity(raw),
            Cow::Owned(indexed) => {
                let entries = raw
                    .len()
                    .saturating_add(indexed.len())
                    .saturating_mul(8)
                    .min(limits.map_entries);
                let mapped_bytes = raw.len().max(indexed.len());
                let bytes = MappedText::allocation_bound(mapped_bytes, entries)
                    .ok_or_else(optional_work)?;
                let memory = loan(budget, bytes, MemoryLifetime::Temporary)?;
                pay(
                    budget,
                    raw.len().saturating_add(indexed.len()).saturating_mul(4),
                )?;
                let map = MappedText::new(
                    raw,
                    indexed,
                    CaseMode::Sensitive,
                    mapped_bytes,
                    entries,
                    &|| budget.checkpoint().is_err(),
                )
                .map_err(|error| map_error(error, budget))?;
                Coordinates::Mapped {
                    coordinates: map,
                    _memory: memory,
                }
            }
        };
        let has_terms = clauses
            .iter()
            .flatten()
            .any(|clause| matches!(clause, Clause::Term(_) | Clause::Phrase { .. }));
        let token_bytes = if has_terms {
            token_allocation_bound_bytes(indexed.len()).ok_or_else(optional_work)?
        } else {
            0
        };
        let witness_bytes = limits
            .witnesses
            .saturating_add(1)
            .saturating_mul(4)
            .saturating_mul(core::mem::size_of::<PositiveWitness>());
        let _working = loan(
            budget,
            token_bytes.saturating_add(witness_bytes),
            MemoryLifetime::Temporary,
        )?;
        let mut tokens = if has_terms {
            pay(budget, indexed.len())?;
            normalize::tokenize(&indexed, self.case).tokens
        } else {
            Vec::new()
        };
        tokens.retain(Token::is_indexable);
        let mut witnesses = Vec::new();
        witnesses
            .try_reserve_exact(limits.witnesses)
            .map_err(|_error| optional_work())?;
        for &clause in clauses.iter().flatten() {
            budget.checkpoint()?;
            match clause {
                Clause::Term(index) => self.append_terms(
                    &coordinates,
                    &tokens,
                    &[self.term(index)?],
                    limits,
                    &mut witnesses,
                    budget,
                )?,
                Clause::Phrase { first, count, .. } => {
                    let positions = self
                        .term_remap
                        .get(first..first.checked_add(count).ok_or_else(corrupt)?)
                        .ok_or_else(corrupt)?;
                    let mut keys = [""; MAX_CONTENT_LEAVES];
                    let keys = keys.get_mut(..count).ok_or_else(corrupt)?;
                    for (key, &index) in keys.iter_mut().zip(positions) {
                        *key = self.term(index)?;
                    }
                    self.append_terms(&coordinates, &tokens, keys, limits, &mut witnesses, budget)?;
                }
                Clause::Regex(index) => {
                    let (matcher, states) = self.regex.content_executor(index)?;
                    let bytes = u64::try_from(indexed.len().max(1))
                        .map_err(|_width| IndexError::ResourceLimited)?;
                    budget.charge(WorkUsage {
                        bytes: bytes
                            .checked_mul(states)
                            .ok_or(IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let result = matcher
                        .find_ranges_bounded(
                            indexed.as_bytes(),
                            limits.transformed_bytes,
                            limits.witnesses.saturating_add(1),
                            &|| budget.checkpoint().is_err(),
                        )
                        .map_err(|error| match error {
                            RegexRangeError::SourceByteLimit
                            | RegexRangeError::AllocationRefused => optional_work(),
                            RegexRangeError::Interrupted => interrupted(budget),
                        })?;
                    if !result.exhausted {
                        return Err(optional_work());
                    }
                    append_ranges(&coordinates, result.ranges, limits, &mut witnesses, budget)?;
                }
                Clause::Contains(_)
                | Clause::Repo(_)
                | Clause::All(_)
                | Clause::Any(_)
                | Clause::Not => {
                    return Err(IndexError::BackendViolation(
                        "non-body clause reached native preview".into(),
                    )
                    .into());
                }
            }
        }
        let window =
            selected_content_window(raw, &indexed, &witnesses).map_err(|error| match error {
                WindowError::FocusExceedsBudget => {
                    Stop::Unavailable(PreviewUnavailableReason::FocusExceedsBudget)
                }
                WindowError::Integrity(_) => Stop::Failure(corrupt()),
            })?;
        let output_bytes = window.text.len().saturating_add(
            limits
                .witnesses
                .saturating_mul(2)
                .saturating_mul(core::mem::size_of::<quanta_index_contract::HighlightSpan>()),
        );
        let retained = loan(budget, output_bytes, MemoryLifetime::Retained)?;
        pay(budget, output_bytes)?;
        let excerpt = NativeTextExcerpt {
            text: window.text.to_owned(),
            original_focus: wire_range(&window.focus.original)?,
            original_context: wire_range(&window.context)?,
            normalized_focus: wire_range(&window.focus.normalized)?,
            highlights: window.highlights().map_err(|_error| corrupt())?,
        };
        Ok((NativeTextPreviewEvidence::Excerpt(excerpt), Some(retained)))
    }

    fn term(&self, index: usize) -> Result<&str, IndexError> {
        let key = self
            .cursors
            .get(index)
            .map(TextPostingCursor::key)
            .ok_or_else(corrupt)?;
        let (case, term) = decode_case_term_key(key)?;
        if case != self.case {
            return Err(IndexError::CorruptObject(
                "native preview term key case disagrees with query".into(),
            ));
        }
        Ok(term)
    }

    fn append_terms(
        &self,
        coordinates: &Coordinates<'_>,
        tokens: &[Token],
        keys: &[&str],
        limits: SnippetLimits,
        witnesses: &mut Vec<PositiveWitness>,
        budget: &mut WorkBudget,
    ) -> Result<(), Stop> {
        pay(budget, tokens.len().saturating_mul(keys.len()))?;
        append_ranges(
            coordinates,
            normalize::phrase_ranges_by(tokens, keys, |token, key| token.text.as_str() == *key),
            limits,
            witnesses,
            budget,
        )
    }
}

fn wire_range(range: &Range<usize>) -> Result<PreviewByteRange, IndexError> {
    Ok(PreviewByteRange {
        start: u64::try_from(range.start).map_err(|_width| IndexError::ResourceLimited)?,
        end: u64::try_from(range.end).map_err(|_width| IndexError::ResourceLimited)?,
    })
}

fn append_ranges(
    coordinates: &Coordinates<'_>,
    ranges: impl IntoIterator<Item = Range<usize>>,
    limits: SnippetLimits,
    witnesses: &mut Vec<PositiveWitness>,
    budget: &WorkBudget,
) -> Result<(), Stop> {
    let mut matched = false;
    for range in ranges {
        budget.checkpoint()?;
        matched = true;
        if range.is_empty() {
            return Err(Stop::Unavailable(
                PreviewUnavailableReason::UnsupportedRange,
            ));
        }
        let witness = coordinates
            .witness(range)
            .map_err(|error| map_error(error, budget))?;
        if !witnesses.contains(&witness) {
            if witnesses.len() == limits.witnesses {
                return Err(optional_work());
            }
            witnesses.push(witness);
        }
    }
    if !matched {
        return Err(corrupt().into());
    }
    Ok(())
}
