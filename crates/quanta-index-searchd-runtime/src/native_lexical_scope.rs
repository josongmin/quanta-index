//! Bounded Text hit eligibility copied under the original selected session.

use quanta_index_contract::native_protocol::{
    NativeOverlayTextResult, NativeOverlayTextSource, NativeTextCoverage, NativeTextResult,
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, NativeTextHitSource, OwnerKey, QueryOutcome, ViewRef,
};
use quanta_index_lexical::native::TextReadResult;
use quanta_index_semantic::native::{SemanticLexicalEligibility, SemanticLexicalEligibleSource};

use crate::{native_overlay::OverlayCorpusSnapshot, native_text_result};

/// Copy the Text hit universe while its producing stage and allocations live.
///
/// The optional corpus was captured before Fast; this resolves no second
/// snapshot or selected view.
pub(super) fn lexical_eligibility_from_raw(
    outcome: &QueryOutcome<TextReadResult>,
    selected_view: ViewRef,
    requested_top_k: u32,
    snapshot: Option<&OverlayCorpusSnapshot>,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<SemanticLexicalEligibility, IndexError> {
    if outcome.view() != selected_view {
        return Err(IndexError::BackendViolation(
            "lexical scope changed selected view".into(),
        ));
    }
    native_text_result::validate_read(outcome, requested_top_k, &reserve)?;
    copy_lexical_eligibility(
        outcome
            .result()
            .hits
            .iter()
            .map(|hit| (hit.owner, hit.source, hit.path.as_str())),
        selected_view,
        snapshot,
        reserve,
    )
}

/// Reuse a Fast result only after its required stage has been emitted. Both
/// wire shapes were validated while their original producing grant was live.
pub(super) fn lexical_eligibility_from_overlay_fast(
    result: &NativeOverlayTextResult,
    selected_view: ViewRef,
    snapshot: &OverlayCorpusSnapshot,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<SemanticLexicalEligibility, IndexError> {
    if result.view != selected_view
        || !result.exhaustive
        || result.corpus.session != snapshot.session.wire
        || result.corpus.identity != snapshot.identity
    {
        return Err(IndexError::BackendViolation(
            "Fast overlay lexical scope changed selected corpus".into(),
        ));
    }
    copy_lexical_eligibility(
        result.hits.iter().map(|hit| {
            let source = match hit.source {
                NativeOverlayTextSource::Durable(source) => NativeTextHitSource::Durable(source),
                NativeOverlayTextSource::Overlay { revision } => {
                    NativeTextHitSource::Overlay { revision }
                }
            };
            (hit.owner, source, hit.path.as_str())
        }),
        selected_view,
        Some(snapshot),
        reserve,
    )
}

pub(super) fn lexical_eligibility_from_durable_fast(
    result: &NativeTextResult,
    selected_view: ViewRef,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<SemanticLexicalEligibility, IndexError> {
    if result.view != selected_view
        || !result.exhaustive
        || result.coverage != NativeTextCoverage::CoreRequired
    {
        return Err(IndexError::BackendViolation(
            "Fast lexical scope changed selected core view".into(),
        ));
    }
    copy_lexical_eligibility(
        result.hits.iter().map(|hit| {
            (
                hit.owner,
                NativeTextHitSource::Durable(hit.source),
                hit.path.as_str(),
            )
        }),
        selected_view,
        None,
        reserve,
    )
}

fn copy_lexical_eligibility<'a>(
    hits: impl ExactSizeIterator<Item = (OwnerKey, NativeTextHitSource, &'a str)>,
    selected_view: ViewRef,
    snapshot: Option<&OverlayCorpusSnapshot>,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<SemanticLexicalEligibility, IndexError> {
    if snapshot.is_some_and(|corpus| corpus.session.workspace != selected_view.workspace) {
        return Err(IndexError::BackendViolation(
            "lexical scope overlay workspace differs".into(),
        ));
    }
    let count = hits.len();
    let bytes = count
        .checked_mul(std::mem::size_of::<SemanticLexicalEligibleSource>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = if bytes == 0 {
        None
    } else {
        reserve(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?
    };
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for (owner, source, path) in hits {
        let captured = snapshot.and_then(|corpus| {
            corpus
                .documents
                .binary_search_by_key(&owner, |document| document.owner)
                .map_or(None, |index| corpus.documents.get(index))
        });
        match (source, snapshot, captured) {
            (NativeTextHitSource::Durable(_), _, Some(document)) if !document.retired => {
                return Err(IndexError::BackendViolation(
                    "lexical durable hit is masked by the captured overlay".into(),
                ));
            }
            (NativeTextHitSource::Durable(_), _, _) => {}
            (NativeTextHitSource::Overlay { revision }, Some(_), Some(document))
                if !document.retired
                    && document.revision == revision
                    && document.text.is_some()
                    && document.path.as_deref() == Some(path) => {}
            (NativeTextHitSource::Overlay { .. }, _, _) => {
                return Err(IndexError::BackendViolation(
                    "lexical overlay hit differs from the captured corpus".into(),
                ));
            }
        }
        entries.push(SemanticLexicalEligibleSource { owner, source });
    }
    SemanticLexicalEligibility::new(selected_view, entries, memory)
}
