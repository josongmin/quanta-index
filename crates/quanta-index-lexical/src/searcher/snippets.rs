//! Snippet windows and highlights, and reading stored fields back.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::{SNIPPET_LEAD_BYTES, SNIPPET_WINDOW_BYTES};
use quanta_index_contract::{HighlightSpan, LqLeaf};

use crate::searcher::match_sets::PositiveWitness;
use core::ops::Range;
use quanta_index_contract::{
    PreviewByteRange, PreviewKind, PreviewMetadata, PreviewUnavailableReason, SourceFileRevision,
};
use quanta_index_core::{CoreError, LexicalMemoryReservation};
use quanta_index_lq_regex::RegexExecutor;
use quanta_index_lq_text_normalizer::{self as normalize, CaseMode, MappedText};
use sha2::{Digest, Sha256};

const OUTPUT_LEASE_OVERHEAD_BYTES: usize =
    core::mem::size_of::<LexicalMemoryReservation>().saturating_mul(4);

pub(crate) use super::preview_types::{
    PreviewResult, PreviewStop, RenderedPreview, SelectedSnippetSource, SnippetContext,
    SnippetLimits, integrity, token_allocation_bound,
};

/// Render only a final selected row.
///
/// Regexes are caller-prepared request-local executors, and metadata/structural truth comes from the canonical matcher.
/// This function never opens a filesystem path or changes eligibility/ranking.
pub(crate) fn render_selected<'a>(
    context: &SnippetContext<'_>,
    source: &SelectedSnippetSource<'_>,
    regex_executor: &dyn Fn(&str) -> Result<&'a RegexExecutor, CoreError>,
    predicate_truth: &dyn Fn(&LqLeaf) -> Result<bool, CoreError>,
) -> Result<RenderedPreview, CoreError> {
    context.request.checkpoint("lexical:preview")?;
    match render_selected_inner(context, source, regex_executor, predicate_truth) {
        Ok(result) => Ok(result),
        Err(PreviewStop::Mandatory(error)) => Err(error),
        Err(PreviewStop::Unavailable(reason)) => {
            // Cancellation must remain mandatory even if an optional limit is
            // encountered at the same checkpoint.
            context.request.checkpoint("lexical:preview-unavailable")?;
            Ok(RenderedPreview {
                snippet: String::new(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
                preview: PreviewMetadata::unavailable(source.kind, reason, None),
                reservation: None,
            })
        }
    }
}

fn render_selected_inner<'a>(
    context: &SnippetContext<'_>,
    source: &SelectedSnippetSource<'_>,
    regex_executor: &dyn Fn(&str) -> Result<&'a RegexExecutor, CoreError>,
    predicate_truth: &dyn Fn(&LqLeaf) -> Result<bool, CoreError>,
) -> PreviewResult<RenderedPreview> {
    let bound = source
        .source
        .ok_or_else(|| PreviewStop::Mandatory(integrity("selected source identity missing")))?;
    bound
        .validate()
        .map_err(|message| PreviewStop::Mandatory(integrity(message)))?;
    if bound.file.repo_relative_path.as_str() != source.path {
        return Err(PreviewStop::Mandatory(integrity(
            "selected source path mismatch",
        )));
    }
    if source.kind == PreviewKind::Path {
        return render_path(context, source);
    }
    let raw = source
        .raw
        .ok_or_else(|| PreviewStop::Mandatory(integrity("required immutable bytes missing")))?;
    let indexed = source
        .indexed_nfc
        .ok_or_else(|| PreviewStop::Mandatory(integrity("indexed NFC authority missing")))?;
    if source.kind == PreviewKind::SourceChunk && source.expected_raw_sha256.is_none() {
        return Err(PreviewStop::Mandatory(integrity(
            "immutable chunk digest missing",
        )));
    }
    if raw.len() > context.limits.source_bytes || indexed.len() > context.limits.transformed_bytes {
        return Err(PreviewStop::Unavailable(
            PreviewUnavailableReason::WorkBudget,
        ));
    }
    context.charge(raw.len())?;
    if let Some(expected) = source.expected_raw_sha256 {
        let actual: [u8; 32] = Sha256::digest(raw.as_bytes()).into();
        if actual != expected {
            return Err(PreviewStop::Mandatory(integrity(
                "immutable chunk digest mismatch",
            )));
        }
    }
    context.checkpoint()?;
    if context.limits.witnesses == 0 {
        return Err(PreviewStop::Unavailable(
            PreviewUnavailableReason::WorkBudget,
        ));
    }
    let entries = raw
        .len()
        .saturating_add(indexed.len())
        .saturating_mul(8)
        .min(context.limits.map_entries);
    let map_bytes = MappedText::allocation_bound(context.limits.transformed_bytes, entries).ok_or(
        PreviewStop::Unavailable(PreviewUnavailableReason::WorkBudget),
    )?;
    // Two maps cover NFC coordinates (tokens/regex) and folded coordinates
    // (raw substrings). Tokenization is source-bounded too, including vector
    // spare capacity, folded term strings and its owned NFC string.
    let token_bytes = token_allocation_bound(indexed.len())?;
    let working_bytes = map_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(token_bytes))
        .and_then(|bytes| {
            bytes.checked_add(
                context
                    .limits
                    .witnesses
                    .saturating_mul(2)
                    .max(4)
                    .saturating_mul(core::mem::size_of::<PositiveWitness>()),
            )
        })
        .ok_or(PreviewStop::Unavailable(
            PreviewUnavailableReason::WorkBudget,
        ))?;
    let _working = context.reserve(working_bytes)?;
    context.charge(raw.len().saturating_add(indexed.len()).saturating_mul(4))?;
    let interrupted = || context.request.interruption().is_some();
    let nfc = MappedText::new(
        raw,
        indexed,
        CaseMode::Sensitive,
        context.limits.transformed_bytes,
        entries,
        &interrupted,
    )
    .map_err(|error| context.mapping_error(error))?;
    let folded = MappedText::new(
        raw,
        indexed,
        context.options.case_mode(),
        context.limits.transformed_bytes,
        entries,
        &interrupted,
    )
    .map_err(|error| context.mapping_error(error))?;
    let tokenized = normalize::tokenize(indexed, context.options.case_mode());
    let document: Vec<_> = tokenized.indexable().cloned().collect();
    context.checkpoint()?;
    let (matched, path_match, mut witnesses) = crate::searcher::match_sets::selected_witnesses(
        context,
        source,
        &nfc,
        &folded,
        &document,
        regex_executor,
        predicate_truth,
    )?;
    if !matched {
        return Err(PreviewStop::Mandatory(integrity(
            "selected row does not satisfy prepared expression",
        )));
    }
    if witnesses.is_empty() {
        return if path_match {
            render_path(context, source)
        } else {
            Err(PreviewStop::Unavailable(
                PreviewUnavailableReason::NoPositiveWitness,
            ))
        };
    }
    witnesses.sort_unstable_by_key(|witness| {
        (
            witness.original.start,
            witness.original.end,
            witness.normalized.start,
            witness.normalized.end,
        )
    });
    witnesses.dedup_by(|left, right| {
        left.original == right.original && left.normalized == right.normalized
    });
    let focus = witnesses
        .first()
        .ok_or_else(|| PreviewStop::Mandatory(integrity("lost positive witness")))?;
    let focus_len = focus.original.end.saturating_sub(focus.original.start);
    if focus_len > SNIPPET_WINDOW_BYTES {
        return Err(PreviewStop::Unavailable(
            PreviewUnavailableReason::FocusExceedsBudget,
        ));
    }
    let lead = SNIPPET_LEAD_BYTES.min(SNIPPET_WINDOW_BYTES.saturating_sub(focus_len));
    let mut start = if raw.len() <= SNIPPET_WINDOW_BYTES {
        0
    } else {
        focus.original.start.saturating_sub(lead)
    };
    while !raw.is_char_boundary(start) {
        start = start.saturating_add(1);
    }
    let end = floor_char_boundary(raw, start.saturating_add(SNIPPET_WINDOW_BYTES));
    if start > focus.original.start || end < focus.original.end {
        return Err(PreviewStop::Mandatory(integrity(
            "window clipped a fitting focus",
        )));
    }
    let reservation = context.reserve(
        SNIPPET_WINDOW_BYTES
            .saturating_add(OUTPUT_LEASE_OVERHEAD_BYTES)
            .saturating_add(
                context
                    .limits
                    .witnesses
                    .saturating_mul(2)
                    .max(4)
                    .saturating_mul(core::mem::size_of::<HighlightSpan>()),
            )
            .saturating_add(binding_bytes(bound)),
    )?;
    let snippet = raw
        .get(start..end)
        .ok_or_else(|| PreviewStop::Mandatory(integrity("window is not UTF-8 aligned")))?
        .to_string();
    let mut highlights: Vec<_> = witnesses
        .iter()
        .filter(|witness| witness.original.start >= start && witness.original.end <= end)
        .map(|witness| HighlightSpan {
            start: snippet_offset_u32(witness.original.start.saturating_sub(start)),
            len: snippet_offset_u32(witness.original.end.saturating_sub(witness.original.start)),
        })
        .collect();
    highlights.sort_unstable_by_key(|span| (span.start, span.len));
    highlights.dedup();
    let is_source = source.kind == PreviewKind::SourceChunk;
    let preview = PreviewMetadata {
        kind: source.kind,
        source: Some(bound.clone()),
        chunk_start_byte: if is_source {
            source.chunk_start_byte
        } else {
            None
        },
        original_focus: if is_source {
            Some(wire_range(focus.original.clone())?)
        } else {
            None
        },
        original_context: if is_source {
            Some(wire_range(start..end)?)
        } else {
            None
        },
        normalized_focus: if is_source {
            Some(wire_range(focus.normalized.clone())?)
        } else {
            None
        },
        normalization_equivalent: is_source
            && raw.get(focus.original.clone()) != indexed.get(focus.normalized.clone()),
        unavailable_reason: None,
    };
    preview
        .validate()
        .map_err(|message| PreviewStop::Mandatory(integrity(message)))?;
    context.checkpoint()?;
    Ok(RenderedPreview {
        snippet_hit_offset: highlights.first().map(|span| span.start),
        snippet,
        highlights,
        preview,
        reservation: Some(reservation),
    })
}

fn binding_bytes(source: &SourceFileRevision) -> usize {
    source
        .file
        .source_repo_id
        .as_str()
        .len()
        .saturating_add(source.file.repo_relative_path.as_str().len())
        .saturating_add(source.revision_id.as_str().len())
        .saturating_add(core::mem::size_of::<PreviewMetadata>())
}

fn wire_range(range: Range<usize>) -> PreviewResult<PreviewByteRange> {
    let start = u64::try_from(range.start)
        .map_err(|_overflow| PreviewStop::Mandatory(integrity("span overflow")))?;
    let end = u64::try_from(range.end)
        .map_err(|_overflow| PreviewStop::Mandatory(integrity("span overflow")))?;
    Ok(PreviewByteRange { start, end })
}

fn render_path(
    context: &SnippetContext<'_>,
    source: &SelectedSnippetSource<'_>,
) -> PreviewResult<RenderedPreview> {
    if source.path.len() > SNIPPET_WINDOW_BYTES {
        return Err(PreviewStop::Unavailable(
            PreviewUnavailableReason::FocusExceedsBudget,
        ));
    }
    let bound = source
        .source
        .ok_or_else(|| PreviewStop::Mandatory(integrity("path source identity missing")))?;
    let reservation = context.reserve(
        source
            .path
            .len()
            .saturating_add(binding_bytes(bound))
            .saturating_add(OUTPUT_LEASE_OVERHEAD_BYTES),
    )?;
    let preview = PreviewMetadata {
        kind: PreviewKind::Path,
        source: Some(bound.clone()),
        chunk_start_byte: None,
        original_focus: None,
        original_context: None,
        normalized_focus: None,
        normalization_equivalent: false,
        unavailable_reason: None,
    };
    preview
        .validate()
        .map_err(|message| PreviewStop::Mandatory(integrity(message)))?;
    context.checkpoint()?;
    Ok(RenderedPreview {
        snippet: source.path.to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
        preview,
        reservation: Some(reservation),
    })
}

/// Step `index` down to the nearest UTF-8 char boundary at or below it.
///
/// `str::floor_char_boundary` is unstable, so this is a stable hand-rolled
/// equivalent. `index` is always clamped into `0..=len` by callers.
pub(crate) fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut i = index.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i = i.saturating_sub(1);
    }
    i
}

/// Narrow a within-snippet byte offset to `u32` for the candidate field.
///
/// The offset is always bounded by [`SNIPPET_WINDOW_BYTES`] (≤ 240) in the
/// truncated case, or by the short snippet's own length otherwise, so it is far
/// below `u32::MAX` and the narrowing is exact.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "the offset is always into the emitted text, which is at most SNIPPET_WINDOW_BYTES (240) bytes — the full stored snippet when it is <= 240 bytes, otherwise a windowed excerpt — so the usize->u32 narrowing is exact"
)]
pub(crate) fn snippet_offset_u32(within: usize) -> u32 {
    within as u32
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    clippy::expect_used,
    reason = "owner regressions assert fixed byte/source oracles and propagate renderer failures"
)]
mod l4_selected_preview_regressions {
    use super::*;
    use quanta_index_contract::{
        LqExpr, LqOptions, RepoId, RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2,
        SourceFileKey,
    };
    use quanta_index_core::{LexicalCollectionBudget, RequestBudgetV1};
    use std::collections::BTreeMap;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn binding(raw: &str) -> SourceFileRevision {
        SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("l4-source").expect("fixture repo"),
                repo_relative_path: RepoRelativePath::new("needle.rs"),
            },
            revision_id: RevisionId::new("immutable-r1").expect("fixture revision"),
            source_sha256: Sha256::digest(raw.as_bytes()).into(),
        }
    }

    fn executors(
        expr: &LqExpr,
        options: &LqOptions,
        out: &mut BTreeMap<String, RegexExecutor>,
    ) -> Result<(), CoreError> {
        match expr {
            LqExpr::Leaf(LqLeaf::Regex(text)) => {
                let pattern = crate::TantivySearcher::regex_source_for_options(text, options);
                let executor = RegexExecutor::compile(&pattern)
                    .map_err(|error| integrity(&error.to_string()))?;
                drop(out.insert(text.clone(), executor));
            }
            LqExpr::Not(inner) => executors(inner, options, out)?,
            LqExpr::All(children) | LqExpr::Any(children) => {
                for child in children {
                    executors(child, options, out)?;
                }
            }
            LqExpr::Leaf(
                LqLeaf::Keyword(_)
                | LqLeaf::RawString(_)
                | LqLeaf::Phrase(_)
                | LqLeaf::StructuralBlock(_)
                | LqLeaf::Predicate { .. },
            )
            | LqExpr::Empty => {}
        }
        Ok(())
    }

    pub(super) fn render(expr: &LqExpr, raw: &str) -> Result<RenderedPreview, CoreError> {
        render_with_limits(expr, raw, SnippetLimits::default())
    }

    fn render_with_limits(
        expr: &LqExpr,
        raw: &str,
        limits: SnippetLimits,
    ) -> Result<RenderedPreview, CoreError> {
        let options = LqOptions::defaults();
        let ledger = LexicalCollectionBudget::new(10_000_000, 64 * 1024 * 1024)?;
        let request = RequestBudgetV1::unbounded();
        let context = SnippetContext {
            expr,
            filters: &[],
            options: &options,
            limits,
            ledger: &ledger,
            request: &request,
        };
        let bound = binding(raw);
        let indexed = normalize::nfc(raw);
        let source = SelectedSnippetSource {
            raw: Some(raw),
            indexed_nfc: Some(&indexed),
            path: "needle.rs",
            kind: PreviewKind::SourceChunk,
            source: Some(&bound),
            chunk_start_byte: Some(400),
            expected_raw_sha256: Some(Sha256::digest(raw.as_bytes()).into()),
        };
        let mut regexes = BTreeMap::new();
        executors(expr, &options, &mut regexes)?;
        render_selected(
            &context,
            &source,
            &|pattern| {
                regexes
                    .get(pattern)
                    .ok_or_else(|| integrity("fixture executor missing"))
            },
            &|_| Ok(false),
        )
    }

    #[test]
    fn false_and_branch_and_not_have_no_positive_witness() -> TestResult {
        let leaf = |text: &str| LqExpr::Leaf(LqLeaf::Keyword(text.into()));
        let expr = LqExpr::Any(vec![
            LqExpr::All(vec![leaf("blocked"), leaf("missing")]),
            leaf("allow"),
        ]);
        let result = render(&expr, &format!("blocked {}allow", "context ".repeat(80)))?;
        assert!(result.snippet.contains("allow"));
        assert!(!result.snippet.contains("blocked"));
        let result = render(&LqExpr::Not(Box::new(leaf("missing"))), "allowed")?;
        assert_eq!(
            result.preview.unavailable_reason,
            Some(PreviewUnavailableReason::NoPositiveWitness)
        );
        assert!(result.highlights.is_empty());
        assert!(result.snippet.is_empty());
        let expr = LqExpr::Any(vec![
            LqExpr::All(vec![leaf("one"), leaf("two"), leaf("missing")]),
            leaf("allow"),
        ]);
        let result = render_with_limits(
            &expr,
            &format!("one two {}allow", "context ".repeat(80)),
            SnippetLimits {
                witnesses: 1,
                ..SnippetLimits::default()
            },
        )?;
        assert!(
            result.preview.unavailable_reason.is_none(),
            "discarded false-branch evidence cannot consume witness count"
        );
        assert!(result.snippet.contains("allow"));
        Ok(())
    }

    #[test]
    fn path_only_preview_never_claims_source_excerpt() -> TestResult {
        let result = render(
            &LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
            "unrelated content",
        )?;
        assert_eq!(result.preview.kind, PreviewKind::Path);
        assert_eq!(result.snippet, "needle.rs");
        assert!(result.highlights.is_empty());
        assert!(result.preview.original_focus.is_none());
        assert!(result.preview.original_context.is_none());
        assert!(result.preview.normalized_focus.is_none());
        Ok(())
    }

    #[test]
    fn oversized_and_zero_width_focus_are_explicitly_unavailable() -> TestResult {
        let text = "n".repeat(241);
        let result = render(&LqExpr::Leaf(LqLeaf::RawString(text.clone())), &text)?;
        assert_eq!(
            result.preview.unavailable_reason,
            Some(PreviewUnavailableReason::FocusExceedsBudget)
        );
        assert!(result.snippet.is_empty());
        let result = render(&LqExpr::Leaf(LqLeaf::Regex("^".into())), "needle")?;
        assert_eq!(
            result.preview.unavailable_reason,
            Some(PreviewUnavailableReason::UnsupportedRange)
        );
        // A zero-width leaf in a false NOT branch cannot poison the true arm.
        let expr = LqExpr::Any(vec![
            LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Regex("^".into())))),
            LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
        ]);
        let result = render(&expr, "needle")?;
        assert_eq!(result.snippet, "needle");
        assert!(result.preview.unavailable_reason.is_none());
        Ok(())
    }

    #[test]
    fn original_context_and_focus_are_source_bound_and_utf8_aligned() -> TestResult {
        let raw = format!("{}cafe\u{301} tail", "🙂".repeat(70));
        let result = render(&LqExpr::Leaf(LqLeaf::Keyword("café".into())), &raw)?;
        let focus = result.preview.original_focus.ok_or("missing focus")?;
        let context = result.preview.original_context.ok_or("missing context")?;
        let normalized = result
            .preview
            .normalized_focus
            .ok_or("missing normalized focus")?;
        assert_eq!((focus.start, focus.end), (280, 286));
        assert_eq!((normalized.start, normalized.end), (280, 285));
        assert!(result.preview.normalization_equivalent);
        assert_eq!(
            raw.get(usize::try_from(context.start)?..usize::try_from(context.end)?),
            Some(result.snippet.as_str())
        );
        assert!(result.snippet.len() <= 240);
        assert_eq!(result.preview.chunk_start_byte, Some(400));
        assert!(result.reservation.is_some());
        Ok(())
    }

    #[test]
    fn integrity_and_optional_budget_and_mandatory_cancellation_are_separate() -> TestResult {
        let raw = "needle";
        let expr = LqExpr::Leaf(LqLeaf::Keyword(raw.into()));
        let options = LqOptions::defaults();
        let bound = binding(raw);
        let request = RequestBudgetV1::unbounded();
        let ledger = LexicalCollectionBudget::new(1_000_000, 16_000_000)?;
        let mut context = SnippetContext {
            expr: &expr,
            filters: &[],
            options: &options,
            limits: SnippetLimits::default(),
            ledger: &ledger,
            request: &request,
        };
        let mut source = SelectedSnippetSource {
            raw: Some(raw),
            indexed_nfc: Some(raw),
            path: "needle.rs",
            kind: PreviewKind::SourceChunk,
            source: Some(&bound),
            chunk_start_byte: None,
            expected_raw_sha256: Some([0; 32]),
        };
        let absent_regex =
            |_: &str| -> Result<&RegexExecutor, CoreError> { Err(integrity("unexpected regex")) };
        let false_predicate = |_: &LqLeaf| Ok(false);
        assert!(matches!(
            render_selected(&context, &source, &absent_regex, &false_predicate),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchPreviewIntegrity,
                ..
            })
        ));
        source.expected_raw_sha256 = Some(Sha256::digest(raw.as_bytes()).into());
        source.indexed_nfc = Some("wrong");
        assert!(matches!(
            render_selected(&context, &source, &absent_regex, &false_predicate),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::SearchPreviewIntegrity,
                ..
            })
        ));
        source.raw = Some("cafe\u{301}");
        source.indexed_nfc = Some("café");
        source.expected_raw_sha256 = Some(Sha256::digest("café".as_bytes()).into());
        assert!(
            matches!(
                render_selected(&context, &source, &absent_regex, &false_predicate),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::SearchPreviewIntegrity,
                    ..
                })
            ),
            "NFC-equivalent raw-byte tamper must still fail digest binding"
        );
        source.raw = Some(raw);
        source.indexed_nfc = Some(raw);
        source.expected_raw_sha256 = Some(Sha256::digest(raw.as_bytes()).into());
        let tiny = LexicalCollectionBudget::new(1, 1)?;
        context.ledger = &tiny;
        let unavailable = render_selected(&context, &source, &absent_regex, &false_predicate)?;
        assert_eq!(
            unavailable.preview.unavailable_reason,
            Some(PreviewUnavailableReason::WorkBudget)
        );
        assert!(unavailable.snippet.is_empty());
        assert!(
            ledger.failure().is_none(),
            "separate preview budget must not poison mandatory work"
        );
        request.cancel_handle().cancel();
        assert!(matches!(
            render_selected(&context, &source, &absent_regex, &false_predicate),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            })
        ));
        let past = std::time::Instant::now()
            .checked_sub(core::time::Duration::from_secs(1))
            .ok_or("fixture monotonic clock cannot represent past deadline")?;
        let expired = RequestBudgetV1::until(past);
        context.request = &expired;
        assert!(matches!(
            render_selected(&context, &source, &absent_regex, &false_predicate),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn content_filter_witness_retains_result_memory_until_drop() -> TestResult {
        let raw = "needle";
        let expr = LqExpr::Empty;
        let filters = [quanta_index_contract::LqFilter::Content {
            leaf: LqLeaf::Keyword(raw.into()),
        }];
        let options = LqOptions::defaults();
        let bound = binding(raw);
        let request = RequestBudgetV1::unbounded();
        let ledger = LexicalCollectionBudget::new(1_000_000, 16_000_000)?;
        let context = SnippetContext {
            expr: &expr,
            filters: &filters,
            options: &options,
            limits: SnippetLimits::default(),
            ledger: &ledger,
            request: &request,
        };
        let source = SelectedSnippetSource {
            raw: Some(raw),
            indexed_nfc: Some(raw),
            path: "needle.rs",
            kind: PreviewKind::SourceChunk,
            source: Some(&bound),
            chunk_start_byte: None,
            expected_raw_sha256: Some(Sha256::digest(raw.as_bytes()).into()),
        };
        let result = render_selected(
            &context,
            &source,
            &|_| Err(integrity("unexpected regex")),
            &|_| Ok(false),
        )?;
        assert_eq!(result.snippet_hit_offset, Some(0));
        assert!(ledger.resident_bytes() >= u64::try_from(result.snippet.len())?);
        assert!(ledger.peak_bytes() > ledger.resident_bytes());
        drop(result);
        assert_eq!(ledger.resident_bytes(), 0);
        Ok(())
    }
}

#[cfg(test)]
mod l4_witness_regressions {
    use super::RenderedPreview;
    use super::l4_selected_preview_regressions::render;
    use quanta_index_contract::{LqExpr, LqLeaf, PreviewUnavailableReason};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[expect(
        clippy::expect_used,
        reason = "a fixed admitted regression fixture must render"
    )]
    fn emitted(expr: &LqExpr, source: &str) -> RenderedPreview {
        render(expr, source).expect("admitted fixed fixture must render")
    }

    fn assert_focus(expr: LqExpr, source: &str, expected: &str) {
        let result = emitted(&expr, source);
        let snippet = &result.snippet;
        let primary = result.snippet_hit_offset;
        let highlights = &result.highlights;
        assert!(snippet.len() <= 240);
        assert!(
            snippet.contains(expected),
            "missing focus {expected:?}: {snippet:?}"
        );
        assert_eq!(primary, highlights.first().map(|span| span.start));
        assert!(
            highlights.iter().any(|span| {
                let (Ok(start), Ok(len)) = (usize::try_from(span.start), usize::try_from(span.len))
                else {
                    return false;
                };
                let end = start.saturating_add(len);
                snippet.get(start..end) == Some(expected)
            }),
            "no exact original-byte highlight for {expected:?}: {highlights:?}"
        );
    }

    fn distant(text: &str) -> String {
        format!("{}{text}", "context ".repeat(80))
    }

    #[test]
    fn ordinary_literal_positive_control() {
        assert_focus(
            LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
            &distant("needle"),
            "needle",
        );
    }

    #[test]
    fn default_case_fold_anchors_original_uppercase() {
        assert_focus(
            LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
            &distant("NEEDLE"),
            "NEEDLE",
        );
    }

    #[test]
    fn regex_anchors_executor_match() {
        assert_focus(
            LqExpr::Leaf(LqLeaf::Regex("needle[0-9]+".into())),
            &distant("needle42"),
            "needle42",
        );
    }

    #[test]
    fn whole_token_does_not_anchor_token_prefix() {
        let source = format!("needlework {}needle", "context ".repeat(80));
        assert_focus(
            LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
            &source,
            "needle",
        );
        let result = emitted(&LqExpr::Leaf(LqLeaf::Keyword("needle".into())), &source);
        assert!(!result.snippet.contains("needlework"));
    }

    #[test]
    fn nfc_match_maps_to_decomposed_source_interval() {
        assert_focus(
            LqExpr::Leaf(LqLeaf::Keyword("café".into())),
            &distant("cafe\u{301}"),
            "cafe\u{301}",
        );
    }

    #[test]
    fn expanding_lowercase_maps_to_one_original_scalar() {
        assert_focus(
            LqExpr::Leaf(LqLeaf::RawString("i\u{307}".into())),
            &distant("İ"),
            "İ",
        );
    }

    #[test]
    fn false_negated_or_branch_never_highlights_forbidden_literal() {
        let expr = LqExpr::Any(vec![
            LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("blocked".into())))),
            LqExpr::Leaf(LqLeaf::Keyword("allow".into())),
        ]);
        assert_focus(
            expr,
            &format!("blocked {}allow", "context ".repeat(80)),
            "allow",
        );
    }

    #[test]
    fn phrase_uses_token_positions_across_crlf() {
        assert_focus(
            LqExpr::Leaf(LqLeaf::Phrase("blue whale".into())),
            &distant("blue\r\nwhale"),
            "blue\r\nwhale",
        );
    }

    #[test]
    fn fitting_two_hundred_byte_focus_kept_before_context() {
        let focus = "n".repeat(200);
        assert_focus(
            LqExpr::Leaf(LqLeaf::RawString(focus.clone())),
            &distant(&focus),
            &focus,
        );
    }

    #[test]
    fn every_positive_match_in_the_emitted_window_has_a_typed_span() -> TestResult {
        let cases = [
            (
                LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
                "needle needle needle",
                vec![(0, 6), (7, 6), (14, 6)],
            ),
            (
                LqExpr::Leaf(LqLeaf::RawString("ab".into())),
                "ab ab ab",
                vec![(0, 2), (3, 2), (6, 2)],
            ),
            (
                LqExpr::Leaf(LqLeaf::RawString("aa".into())),
                "aaa",
                vec![(0, 2), (1, 2)],
            ),
            (
                LqExpr::Leaf(LqLeaf::Regex("needle[0-9]+".into())),
                "needle1 needle22",
                vec![(0, 7), (8, 8)],
            ),
            (
                LqExpr::Leaf(LqLeaf::Phrase("blue whale".into())),
                "blue whale blue whale",
                vec![(0, 10), (11, 10)],
            ),
        ];
        for (expr, raw, expected) in cases {
            let rendered = render(&expr, raw)?;
            assert_eq!(rendered.snippet, raw);
            let actual = rendered
                .highlights
                .iter()
                .map(|span| (span.start, span.len))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{expr:?}");
            assert_eq!(
                rendered.snippet_hit_offset,
                expected.first().map(|span| span.0)
            );
        }

        let saturated = render(&LqExpr::Leaf(LqLeaf::Keyword("x".into())), &"x ".repeat(33))?;
        assert_eq!(
            saturated.preview.unavailable_reason,
            Some(PreviewUnavailableReason::WorkBudget)
        );
        assert!(saturated.snippet.is_empty());
        assert!(saturated.highlights.is_empty());

        // Thirty-five `a` scalars contain thirty-three overlapping `aaa`
        // occurrences. This three-byte needle is admitted by the public raw
        // substring planner; a non-overlapping iterator would miss the
        // overflow and publish an incomplete highlight set as complete.
        let overlapping = render(
            &LqExpr::Leaf(LqLeaf::RawString("aaa".into())),
            &"a".repeat(35),
        )?;
        assert_eq!(
            overlapping.preview.unavailable_reason,
            Some(PreviewUnavailableReason::WorkBudget)
        );
        assert!(overlapping.snippet.is_empty());
        assert!(overlapping.highlights.is_empty());
        Ok(())
    }
}
