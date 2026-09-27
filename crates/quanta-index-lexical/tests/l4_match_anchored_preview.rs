//! L4 source-bound adapter regressions. This exercises sealed read views,
//! indexed/manual selection and preview conversion; it is not SDK/E2E proof.
#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "regressions assert independent source and span oracles"
)]
use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, HighlightSpan, LQ_VERSION_TAG, LqCase, LqExpr, LqLeaf,
    LqOptions, LqQuery, LqSpan, LqYesNoOnly, ManifestGeneration, PreviewKind,
    PreviewUnavailableReason, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, SymbolCoverage, SymbolId, source_event_payload_sha256,
    source_file_unit_set_sha256,
};
use quanta_index_core::{LexicalIndexOpenPort, RequestBudgetV1, SearchCorpusBatchBuildPort};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest, Sha256};
use std::error::Error;
type TestResult = Result<(), Box<dyn Error>>;
fn file_scope(path: &str, marker: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let mut scope = SearchCorpusReplaceScope {
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("l4-preview-repo")?,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new("source-r1")?,
                source_sha256: Sha256::digest(marker.as_bytes()).into(),
            },
            language: LanguageCode::new("rust")?,
            producer_policy_sha256: [8; 32],
            unit_set_sha256: [0; 32],
            text_admitted: true,
            symbols: SymbolCoverage::Complete { symbol_count: 1 },
        },
        chunks: vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-{path}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(marker.len())?,
            start_line: 1,
            end_line: 1,
            text: marker.into(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        symbols: vec![SymbolRecord {
            symbol_id: SymbolId::new(format!("symbol-{path}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            symbol_kind: SymbolKindCode::new("function")?,
            symbol_kind_family: Some(SymbolKindFamily::Callable),
            local_name: "needle".into(),
            qualified_name: "crate::needle".into(),
            signature: None,
            visibility: None,
            definition_span: SymbolSpan {
                path: path.into(),
                byte_start: 0,
                byte_end: u32::try_from(marker.len())?,
                line_start: 1,
                line_end: 1,
            },
            container_qualified_name: None,
            relationship: SymbolRelationship::Def,
        }],
    };
    scope.coverage.unit_set_sha256 = source_file_unit_set_sha256(&scope.chunks, &scope.symbols)?;
    Ok(scope)
}

fn batch(
    generation: u64,
    base: Option<u64>,
    replace_scopes: Vec<SearchCorpusReplaceScope>,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "source-stream".into(),
            event_id: format!("event-{generation}"),
            expected_base_event_id: base.map(|base| format!("event-{base}")),
            payload_sha256: [0; 32],
        },
        repo_id: RepoId::new("l4-preview-repo")?,
        revision_id: RevisionId::new("l4-preview-revision")?,
        generation: ManifestGeneration::new(generation),
        base_generation: base.map(ManifestGeneration::new),
        manifest_digest: format!("l2-manifest-{generation}"),
        // Adapter tests do not exercise the dispatcher's body-digest check.
        batch_digest: "0".repeat(64),
        mode: if base.is_some() {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
    Ok(batch)
}

fn query(marker: &str) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword(marker.into())),
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

#[test]
fn indexed_and_manual_preserve_fixed_original_focus_after_checkout_drift() -> TestResult {
    let fixtures = [
        (LqLeaf::Keyword("needle".into()), "NEEDLE", "NEEDLE"),
        (LqLeaf::Regex("needle[0-9]+".into()), "needle42", "needle42"),
        (LqLeaf::Keyword("café".into()), "cafe\u{301}", "cafe\u{301}"),
        (LqLeaf::RawString("i\u{307}".into()), "İ", "İ"),
        (
            LqLeaf::Phrase("blue whale".into()),
            "blue\r\nwhale",
            "blue\r\nwhale",
        ),
    ];
    for (leaf, ending, expected) in fixtures {
        let dir = tempfile::tempdir()?;
        let checkout = dir.path().join("checkout");
        std::fs::create_dir(&checkout)?;
        let source_file = checkout.join("source.rs");
        let raw = format!("{}{}", "context ".repeat(80), ending);
        std::fs::write(&source_file, &raw)?;
        let adapter = LexicalAdapter::with_state_root(dir.path().join("state"));
        let batch = batch(1, None, vec![file_scope("source.rs", &raw)?])?;
        adapter.build_batch(&batch)?;
        let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
        std::fs::write(
            &source_file,
            "changed checkout has no admitted source bytes",
        )?;
        for index_mode in [None, Some(LqYesNoOnly::No)] {
            let mut q = query("unused");
            q.expr = LqExpr::Leaf(leaf.clone());
            q.options.index_mode = index_mode;
            let request = RequestBudgetV1::unbounded();
            let hits = view.search(&q, 1, &request)?;
            assert_eq!(hits.len(), 1);
            let hit = hits.first().ok_or("selected hit missing")?;
            let preview = hit.preview.as_ref().ok_or("preview missing")?;
            assert_eq!(preview.kind, PreviewKind::SourceChunk);
            assert_eq!(preview.unavailable_reason, None);
            assert_eq!(
                hit.source.as_ref(),
                Some(
                    &batch
                        .replace_scopes
                        .first()
                        .ok_or("scope missing")?
                        .coverage
                        .source
                )
            );
            let focus = preview.original_focus.ok_or("source focus missing")?;
            assert_eq!(
                raw.get(usize::try_from(focus.start)?..usize::try_from(focus.end)?),
                Some(expected)
            );
            assert!(hit.snippet.len() <= 240);
            assert!(hit.highlights.iter().any(|span| {
                let Ok(start) = usize::try_from(span.start) else {
                    return false;
                };
                let Ok(len) = usize::try_from(span.len) else {
                    return false;
                };
                hit.snippet.get(start..start.saturating_add(len)) == Some(expected)
            }));
            hit.validate_source_metadata().map_err(str::to_owned)?;
        }
    }
    Ok(())
}

#[test]
fn indexed_and_manual_preserve_overlapping_raw_witnesses() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let overflow = "a".repeat(35);
    let batch = batch(
        1,
        None,
        vec![
            file_scope("overlap.rs", "ababa")?,
            file_scope("overflow.rs", &overflow)?,
        ],
    )?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    for index_mode in [None, Some(LqYesNoOnly::No)] {
        let mut q = query("unused");
        q.expr = LqExpr::Leaf(LqLeaf::RawString("aba".into()));
        q.options.index_mode = index_mode;
        let request = RequestBudgetV1::unbounded();
        let hits = view.search(&q, 1, &request)?;
        assert_eq!(hits.len(), 1);
        let hit = hits.first().ok_or("overlap hit missing")?;
        assert_eq!(hit.snippet, "ababa");
        assert_eq!(
            hit.highlights,
            [
                HighlightSpan { start: 0, len: 3 },
                HighlightSpan { start: 2, len: 3 },
            ]
        );
        assert_eq!(hit.snippet_hit_offset, Some(0));
        hit.validate_source_metadata().map_err(str::to_owned)?;

        q.expr = LqExpr::Leaf(LqLeaf::RawString("aaa".into()));
        let overflow_hits = view.search(&q, 1, &request)?;
        assert_eq!(overflow_hits.len(), 1, "optional refusal removed the hit");
        let overflow_hit = overflow_hits.first().ok_or("overflow hit missing")?;
        assert_eq!(overflow_hit.repo_relative_path.as_str(), "overflow.rs");
        assert!(overflow_hit.snippet.is_empty());
        assert!(overflow_hit.highlights.is_empty());
        assert_eq!(
            overflow_hit
                .preview
                .as_ref()
                .ok_or("overflow preview missing")?
                .unavailable_reason,
            Some(PreviewUnavailableReason::WorkBudget)
        );
        overflow_hit
            .validate_source_metadata()
            .map_err(str::to_owned)?;
    }
    Ok(())
}

#[test]
fn oversized_focus_and_path_only_keep_admitted_hit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let raw = "x".repeat(241);
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let batch = batch(1, None, vec![file_scope("needle.rs", &raw)?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    for index_mode in [None, Some(LqYesNoOnly::No)] {
        let mut q = query("needle");
        q.options.index_mode = index_mode;
        let request = RequestBudgetV1::unbounded();
        let hits = view.search(&q, 1, &request)?;
        let hit = hits.first().ok_or("path hit missing")?;
        let preview = hit.preview.as_ref().ok_or("preview missing")?;
        assert_eq!(preview.kind, PreviewKind::Path);
        assert!(preview.original_focus.is_none());
        q.expr = LqExpr::Leaf(LqLeaf::RawString(raw.clone()));
        let hits = view.search(&q, 1, &request)?;
        let hit = hits.first().ok_or("long focus hit missing")?;
        assert_eq!(
            hit.preview
                .as_ref()
                .ok_or("preview missing")?
                .unavailable_reason,
            Some(PreviewUnavailableReason::FocusExceedsBudget)
        );
        assert!(hit.snippet.is_empty());
    }
    Ok(())
}

#[test]
fn synthetic_symbol_label_carries_no_source_excerpt_coordinates() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let batch = batch(1, None, vec![file_scope("source.rs", "needle")?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    let request = RequestBudgetV1::unbounded();
    let hits = view.search_symbols(&query("needle"), 1, &request)?;
    let hit = hits.first().ok_or("symbol hit missing")?;
    let preview = hit.preview.as_ref().ok_or("preview missing")?;
    assert_eq!(preview.kind, PreviewKind::SyntheticSymbolLabel);
    assert_eq!(preview.unavailable_reason, None);
    assert!(preview.original_focus.is_none() && preview.normalized_focus.is_none());
    assert!(!preview.normalization_equivalent);
    assert!(hit.snippet.contains("needle"));
    Ok(())
}

#[test]
fn request_output_slot_exhaustion_preserves_hits_and_retained_memory_lifetime() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let batch = batch(1, None, vec![file_scope("source.rs", "needle")?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    let request = RequestBudgetV1::unbounded();
    let ledger = request.lexical_preview_budget(10_000_000, 64 * 1024 * 1024)?;
    for index in 0..257 {
        let hits = view.search(&query("needle"), 1, &request)?;
        assert_eq!(
            hits.len(),
            1,
            "optional retention limits cannot remove a hit"
        );
        let preview = hits
            .first()
            .and_then(|hit| hit.preview.as_ref())
            .ok_or("preview missing")?;
        assert_eq!(
            preview.unavailable_reason,
            (index == 256).then_some(PreviewUnavailableReason::WorkBudget)
        );
    }
    assert!(
        ledger.resident_bytes() > 0,
        "outputs stay charged until request release"
    );
    drop(request);
    assert_eq!(ledger.resident_bytes(), 0);
    Ok(())
}

#[test]
fn l4_preview_admission_skips_empty_regex_pages() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let batch = batch(1, None, vec![file_scope("source.rs", "needle")?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    for index_mode in [None, Some(LqYesNoOnly::No)] {
        let request = RequestBudgetV1::unbounded();
        let ledger = request.lexical_preview_budget(10_000_000, 64 * 1024 * 1024)?;
        let initial_peak = ledger.peak_bytes();
        let mut absent = query("unused");
        absent.expr = LqExpr::Leaf(LqLeaf::Regex("absent[0-9]+".into()));
        absent.options.index_mode = index_mode;
        assert!(view.search(&absent, 1, &request)?.is_empty());
        assert_eq!(ledger.used_work(), 0, "no selected row needs a witness");
        assert_eq!(
            ledger.peak_bytes(),
            initial_peak,
            "no preview executor needed"
        );
    }
    Ok(())
}

#[test]
fn l4_preview_admission_empty_pages_do_not_exhaust_output_slots() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let batch = batch(1, None, vec![file_scope("source.rs", "needle")?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    for index_mode in [None, Some(LqYesNoOnly::No)] {
        let request = RequestBudgetV1::unbounded();
        let mut absent = query("absent");
        absent.options.index_mode = index_mode;
        for _ in 0..256 {
            assert!(view.search(&absent, 1, &request)?.is_empty());
        }
        let mut present = query("needle");
        present.options.index_mode = index_mode;
        let hits = view.search(&present, 1, &request)?;
        assert_eq!(hits.len(), 1);
        let preview = hits
            .first()
            .and_then(|hit| hit.preview.as_ref())
            .ok_or("preview missing")?;
        assert_eq!(
            preview.unavailable_reason, None,
            "empty pages retained no output"
        );
    }
    Ok(())
}

#[test]
fn l4_preview_admission_unavailable_pages_do_not_exhaust_output_slots() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let batch = batch(1, None, vec![file_scope("source.rs", "needle")?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    for index_mode in [None, Some(LqYesNoOnly::No)] {
        let request = RequestBudgetV1::unbounded();
        let mut absence = query("unused");
        absence.expr = LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("absent".into()))));
        absence.options.index_mode = index_mode;
        for _ in 0..256 {
            let hits = view.search(&absence, 1, &request)?;
            assert_eq!(hits.len(), 1);
            assert_eq!(
                hits.first()
                    .and_then(|hit| hit.preview.as_ref())
                    .ok_or("preview missing")?
                    .unavailable_reason,
                Some(PreviewUnavailableReason::NoPositiveWitness)
            );
        }
        let mut present = query("needle");
        present.options.index_mode = index_mode;
        let hits = view.search(&present, 1, &request)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits.first()
                .and_then(|hit| hit.preview.as_ref())
                .ok_or("preview missing")?
                .unavailable_reason,
            None,
            "unavailable previews retained no output"
        );
    }
    Ok(())
}

#[test]
fn l4_preview_admission_skips_oversized_source_before_regex_compile() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let raw = format!("needle{}", " ".repeat(65_536));
    let batch = batch(1, None, vec![file_scope("source.rs", &raw)?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    for index_mode in [None, Some(LqYesNoOnly::No)] {
        let request = RequestBudgetV1::unbounded();
        let ledger = request.lexical_preview_budget(10_000_000, 64 * 1024 * 1024)?;
        let initial_peak = ledger.peak_bytes();
        let mut present = query("unused");
        present.expr = LqExpr::Leaf(LqLeaf::Regex("needle".into()));
        present.options.index_mode = index_mode;
        let hits = view.search(&present, 1, &request)?;
        assert_eq!(hits.len(), 1);
        let preview = hits
            .first()
            .and_then(|hit| hit.preview.as_ref())
            .ok_or("preview missing")?;
        assert_eq!(
            preview.unavailable_reason,
            Some(PreviewUnavailableReason::WorkBudget)
        );
        assert_eq!(ledger.used_work(), 0, "oversized source cannot be rendered");
        assert_eq!(ledger.peak_bytes(), initial_peak);
    }
    Ok(())
}

#[test]
fn l4_preview_admission_oversized_row_does_not_refuse_other_selected_rows() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let oversized = format!("needle{}", " ".repeat(65_536));
    let batch = batch(
        1,
        None,
        vec![
            file_scope("a-large.rs", &oversized)?,
            file_scope("b-small.rs", "needle")?,
        ],
    )?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    for index_mode in [None, Some(LqYesNoOnly::No)] {
        let mut present = query("unused");
        present.expr = LqExpr::Leaf(LqLeaf::Regex("needle".into()));
        present.options.index_mode = index_mode;
        let request = RequestBudgetV1::unbounded();
        let hits = view.search(&present, 2, &request)?;
        assert_eq!(hits.len(), 2);
        for hit in &hits {
            let preview = hit.preview.as_ref().ok_or("preview missing")?;
            if hit.repo_relative_path.as_str() == "a-large.rs" {
                assert_eq!(
                    preview.unavailable_reason,
                    Some(PreviewUnavailableReason::WorkBudget)
                );
                assert!(hit.snippet.is_empty());
            } else {
                assert_eq!(hit.repo_relative_path.as_str(), "b-small.rs");
                assert_eq!(preview.unavailable_reason, None);
                assert_eq!(hit.snippet, "needle");
                assert_eq!(
                    preview.original_focus.map(|range| (range.start, range.end)),
                    Some((0, 6))
                );
            }
        }
    }
    Ok(())
}

#[test]
fn l4_unicode_regex_compilation_is_charged_before_selected_preview() -> TestResult {
    let dir = tempfile::tempdir()?;
    let raw = format!("needle{}", "a".repeat(120));
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let batch = batch(1, None, vec![file_scope("source.rs", &raw)?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;

    let mut present = query("unused");
    present.expr = LqExpr::Leaf(LqLeaf::Regex(r"needle\w{120}".into()));
    present.options.case = Some(LqCase::Sensitive);
    let request = RequestBudgetV1::unbounded();
    let ledger = request.lexical_preview_budget(10_000_000, 64 * 1024 * 1024)?;
    let hits = view.search(&present, 1, &request)?;
    let hit = hits.first().ok_or("selected regex hit missing")?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hit.snippet, raw);
    assert_eq!(
        hit.preview
            .as_ref()
            .ok_or("regex preview missing")?
            .original_focus,
        Some(quanta_index_contract::PreviewByteRange { start: 0, end: 126 })
    );
    // The pinned engine probe exceeded 16 MiB peak for this valid pattern.
    // This asserts the extra charge is admitted before engine compilation;
    // it is not a claim that the logical charge caps process RSS.
    assert!(ledger.peak_bytes() >= 16 * 1024 * 1024 + 95_760 * 256);
    Ok(())
}

#[test]
fn l4_distinct_complex_regex_leaves_refuse_only_optional_preview() -> TestResult {
    let dir = tempfile::tempdir()?;
    let raw = format!("needle{}", "a".repeat(120));
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let batch = batch(1, None, vec![file_scope("source.rs", &raw)?])?;
    adapter.build_batch(&batch)?;
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;

    let first = LqLeaf::Regex(r"needle\w{120}".into());
    let second = LqLeaf::Regex(r"needle\p{L}{120}".into());
    let mut repeated = query("unused");
    repeated.options.case = Some(LqCase::Sensitive);
    repeated.expr = LqExpr::Any(vec![
        LqExpr::Leaf(first.clone()),
        LqExpr::Leaf(first.clone()),
    ]);
    let repeated_request = RequestBudgetV1::unbounded();
    let repeated_ledger = repeated_request.lexical_preview_budget(10_000_000, 64 * 1024 * 1024)?;
    let repeated_hits = view.search(&repeated, 1, &repeated_request)?;
    let repeated_hit = repeated_hits.first().ok_or("repeated regex hit missing")?;
    assert_eq!(repeated_hits.len(), 1);
    assert_eq!(repeated_hit.snippet, raw);
    assert!(repeated_ledger.peak_bytes() < 64 * 1024 * 1024);

    let mut distinct = repeated;
    distinct.expr = LqExpr::Any(vec![LqExpr::Leaf(first), LqExpr::Leaf(second)]);
    let request = RequestBudgetV1::unbounded();
    let hits = view.search(&distinct, 1, &request)?;
    let hit = hits.first().ok_or("distinct regex hit missing")?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hit.candidate_id, repeated_hit.candidate_id);
    assert_eq!(
        hit.preview
            .as_ref()
            .ok_or("preview metadata missing")?
            .unavailable_reason,
        Some(PreviewUnavailableReason::WorkBudget)
    );
    assert!(hit.snippet.is_empty());
    Ok(())
}
