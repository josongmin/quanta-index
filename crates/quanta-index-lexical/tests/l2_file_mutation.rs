//! L2 mutation regressions. The source-file owner must be validated before
//! preparing a generation or applying any operation from the request.

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "assertions report regression failures"
)]

use std::error::Error;

use quanta_index_contract::channel::{LexicalChannelOp, ReplaceLexicalScope};
use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchCorpusSurfaceMutationConflictV1 as MutationConflict,
    SearchCorpusTombstoneScope, SearchScopeSurface, SourceFileCoverage, SourceFileKey,
    SourceFileRevision, SourcePublicationEvent, SymbolCoverage, SymbolId,
    source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{
    LexicalIndexBuildPort, LexicalIndexOpenPort, RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

fn file_scope(path: &str, marker: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let mut scope = SearchCorpusReplaceScope {
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("l2-mutation-repo")?,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new(format!("source-{marker}"))?,
                source_sha256: [7; 32],
            },
            language: LanguageCode::new("rust")?,
            producer_policy_sha256: [8; 32],
            unit_set_sha256: [0; 32],
            text_admitted: true,
            symbols: SymbolCoverage::Complete { symbol_count: 1 },
        },
        chunks: vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-{marker}")),
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
            symbol_id: SymbolId::new(format!("symbol-{marker}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            symbol_kind: SymbolKindCode::new("function")?,
            symbol_kind_family: Some(SymbolKindFamily::Callable),
            local_name: marker.into(),
            qualified_name: format!("crate::{marker}").into(),
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
        repo_id: RepoId::new("l2-mutation-repo")?,
        revision_id: RevisionId::new("l2-mutation-revision")?,
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
fn delta_cannot_claim_newer_source_lineage_while_inheriting_an_older_snapshot() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let original = batch(1, None, vec![file_scope("a.rs", "oldmarker")?])?;
    adapter.build_batch(&original)?;
    let newer = batch(2, Some(1), vec![file_scope("a.rs", "newmarker")?])?;
    adapter.build_batch(&newer)?;

    // The producer knows event-2, but points materialization back at event-1.
    // Applying only b.rs to that snapshot would silently resurrect old a.rs.
    let mut request = batch(3, Some(1), vec![file_scope("b.rs", "othermarker")?])?;
    request.source_event.expected_base_event_id = Some(newer.source_event.event_id.clone());
    request.validate_v1()?;
    request.validate_surface_mutations_v1()?;
    for result in [
        adapter.preflight_batch(&request),
        adapter.build_batch(&request),
    ] {
        assert!(
            matches!(
                result,
                Err(quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseConflict,
                    ..
                })
            ),
            "stale physical base must refuse before any target mutation: {result:?}"
        );
    }
    let target = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &request.repo_id,
        &request.revision_id,
    )
    .generation_dir(dir.path(), request.generation);
    assert!(!target.exists());

    request.base_generation = Some(newer.generation);
    for mutate in [
        |batch: &mut SearchCorpusIngestBatch| {
            batch.source_event.expected_base_event_id = None;
        },
        |batch: &mut SearchCorpusIngestBatch| {
            batch.source_event.stream_id = "other-stream".into();
        },
    ] {
        let mut invalid = request.clone();
        mutate(&mut invalid);
        invalid.validate_v1()?;
        for result in [
            adapter.preflight_batch(&invalid),
            adapter.build_batch(&invalid),
        ] {
            assert!(matches!(
                result,
                Err(quanta_index_core::CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseConflict,
                    ..
                })
            ));
        }
        assert!(!target.exists());
    }
    adapter.preflight_batch(&request)?;
    adapter.build_batch(&request)?;
    assert_units(&adapter, &request, "oldmarker", &[], &[])?;
    assert_units(
        &adapter,
        &request,
        "newmarker",
        &["chunk-newmarker"],
        &["symbol-newmarker"],
    )?;
    assert_units(
        &adapter,
        &request,
        "othermarker",
        &["chunk-othermarker"],
        &["symbol-othermarker"],
    )
}

fn assert_units(
    adapter: &LexicalAdapter,
    batch: &SearchCorpusIngestBatch,
    marker: &str,
    expected_text: &[&str],
    expected_symbols: &[&str],
) -> TestResult {
    let view = adapter.open(&batch.repo_id, &batch.revision_id, batch.generation)?;
    let budget = RequestBudgetV1::unbounded();
    let mut text: Vec<_> = view
        .search(&query(marker), 20, &budget)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    let mut symbols: Vec<_> = view
        .search_symbols(&query(marker), 20, &budget)?
        .into_iter()
        .map(|hit| hit.candidate_id)
        .collect();
    text.sort();
    symbols.sort();
    assert_eq!(text, expected_text);
    assert_eq!(symbols, expected_symbols);
    Ok(())
}

/// The request is invalid before storage access. Even an empty target
/// generation or a partially applied first scope would violate this oracle.
fn assert_refused_without_mutation(
    batch: &SearchCorpusIngestBatch,
    expected: MutationConflict,
) -> TestResult {
    let mut batch = batch.clone();
    // Mutation fixtures must retain a valid envelope so a stale event digest
    // cannot stand in for the file-owner/conflict validation under test.
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
    batch.validate_v1()?;
    assert_eq!(batch.validate_surface_mutations_v1(), Err(expected));
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let result = adapter.build_batch(&batch);
    let entries = std::fs::read_dir(dir.path())?.collect::<Result<Vec<_>, _>>()?;
    assert!(
        result.is_err() && entries.is_empty(),
        "invalid file mutation must fail before writing: result={result:?}, root_entries={:?}",
        entries
            .iter()
            .map(std::fs::DirEntry::file_name)
            .collect::<Vec<_>>()
    );
    Ok(())
}

fn aliases(symbol_first: bool) -> Result<Vec<SearchCorpusReplaceScope>, Box<dyn Error>> {
    let mut text = file_scope("a.rs", "freshmarker")?;
    text.symbols.clear();
    text.coverage.symbols = SymbolCoverage::NotRequested;
    text.coverage.unit_set_sha256 = source_file_unit_set_sha256(&text.chunks, &text.symbols)?;
    let mut symbols = file_scope("a.rs", "freshmarker")?;
    symbols.chunks.clear();
    symbols.coverage.unit_set_sha256 =
        source_file_unit_set_sha256(&symbols.chunks, &symbols.symbols)?;
    Ok(if symbol_first {
        vec![symbols, text]
    } else {
        vec![text, symbols]
    })
}

#[test]
fn chunk_then_symbol_aliases_are_refused_before_mutation() -> TestResult {
    assert_refused_without_mutation(
        &batch(1, None, aliases(false)?)?,
        MutationConflict::DuplicateReplaceScope(SearchScopeSurface::Chunk),
    )
}

#[test]
fn symbol_then_chunk_aliases_are_refused_before_mutation() -> TestResult {
    assert_refused_without_mutation(
        &batch(1, None, aliases(true)?)?,
        MutationConflict::DuplicateReplaceScope(SearchScopeSurface::Chunk),
    )
}

#[test]
fn chunk_path_mismatch_is_refused_before_any_scope_applies() -> TestResult {
    let mut invalid = file_scope("a.rs", "invalidmarker")?;
    invalid
        .chunks
        .first_mut()
        .ok_or("missing fixture chunk")?
        .repo_relative_path = RepoRelativePath::new("b.rs");
    invalid.coverage.unit_set_sha256 =
        source_file_unit_set_sha256(&invalid.chunks, &invalid.symbols)?;
    assert_refused_without_mutation(
        &batch(
            1,
            None,
            vec![file_scope("valid.rs", "validmarker")?, invalid],
        )?,
        MutationConflict::RecordPathMismatch(SearchScopeSurface::Chunk),
    )
}

#[test]
fn symbol_path_mismatch_is_refused_before_any_scope_applies() -> TestResult {
    let mut invalid = file_scope("a.rs", "invalidmarker")?;
    invalid
        .symbols
        .first_mut()
        .ok_or("missing fixture symbol")?
        .repo_relative_path = RepoRelativePath::new("b.rs");
    invalid.coverage.unit_set_sha256 =
        source_file_unit_set_sha256(&invalid.chunks, &invalid.symbols)?;
    assert_refused_without_mutation(
        &batch(1, None, vec![invalid])?,
        MutationConflict::RecordPathMismatch(SearchScopeSurface::Symbol),
    )
}

#[test]
fn definition_path_mismatch_is_refused_before_mutation() -> TestResult {
    let mut invalid = file_scope("a.rs", "invalidmarker")?;
    invalid
        .symbols
        .first_mut()
        .ok_or("missing fixture symbol")?
        .definition_span
        .path = "b.rs".into();
    invalid.coverage.unit_set_sha256 =
        source_file_unit_set_sha256(&invalid.chunks, &invalid.symbols)?;
    assert_refused_without_mutation(
        &batch(1, None, vec![invalid])?,
        MutationConflict::RecordPathMismatch(SearchScopeSurface::Symbol),
    )
}

#[test]
fn cross_kind_candidate_collision_is_refused_before_mutation() -> TestResult {
    let mut invalid = file_scope("a.rs", "invalidmarker")?;
    invalid
        .symbols
        .first_mut()
        .ok_or("missing fixture symbol")?
        .symbol_id = SymbolId::new("chunk-invalidmarker");
    // The unit-set encoder independently refuses duplicate IDs. The exact
    // conflict assertion prevents that digest refusal masking the ID validator.
    assert_refused_without_mutation(
        &batch(1, None, vec![invalid])?,
        MutationConflict::DuplicateCandidateId,
    )
}

#[test]
fn clear_and_file_replacement_overlap_is_refused_before_mutation() -> TestResult {
    for surface in [SearchScopeSurface::Chunk, SearchScopeSurface::Symbol] {
        let mut request = batch(1, None, vec![file_scope("a.rs", "invalidmarker")?])?;
        request.clear_surfaces.push(surface);
        assert_refused_without_mutation(&request, MutationConflict::ClearAndReplace(surface))?;
    }
    Ok(())
}

#[test]
fn replace_and_tombstone_surface_aliases_are_refused_before_mutation() -> TestResult {
    let mut request = batch(1, None, vec![file_scope("a.rs", "invalidmarker")?])?;
    request.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    assert_refused_without_mutation(
        &request,
        MutationConflict::ReplaceAndTombstone(SearchScopeSurface::Chunk),
    )
}

#[test]
fn raw_channel_aliases_are_refused_before_generation_preparation() -> TestResult {
    let request = batch(1, None, aliases(false)?)?;
    let mut ops = Vec::new();
    for scope in &request.replace_scopes {
        let mut payload = Vec::new();
        ciborium::into_writer(
            &(request.mode, request.base_generation, scope),
            &mut payload,
        )?;
        ops.push(LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            generation: request.generation,
            payload,
        }));
    }
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let result = adapter.build(
        &request.repo_id,
        &request.revision_id,
        request.generation,
        &ops,
    );
    let entries = std::fs::read_dir(dir.path())?.collect::<Result<Vec<_>, _>>()?;
    assert!(
        result.is_err() && entries.is_empty(),
        "result={result:?}, entries={entries:?}"
    );
    Ok(())
}

#[test]
fn malformed_bundle_is_refused_before_source_publication_or_generation_preparation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let mut request = batch(1, None, vec![file_scope("a.rs", "validmarker")?])?;
    request.bundle_payload = Some(vec![0xff]);
    request.source_event.payload_sha256 = source_event_payload_sha256(&request)?;
    request.validate_v1()?;
    request.validate_surface_mutations_v1()?;
    for result in [
        adapter.preflight_batch(&request),
        adapter.build_batch(&request),
    ] {
        assert!(
            matches!(
                result,
                Err(quanta_index_core::CoreError::InvalidContract(_))
            ),
            "invalid metadata must be an admission error: {result:?}"
        );
    }
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}

#[test]
fn malformed_bundle_after_a_raw_replacement_refuses_before_any_write() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let request = batch(1, None, vec![file_scope("a.rs", "validmarker")?])?;
    let mut payload = Vec::new();
    ciborium::into_writer(
        &(
            request.mode,
            request.base_generation,
            request
                .replace_scopes
                .first()
                .ok_or("replacement scope missing")?,
        ),
        &mut payload,
    )?;
    let ops = [
        LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            generation: request.generation,
            payload,
        }),
        LexicalChannelOp::FullBundle(quanta_index_contract::LexicalFullBundle {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            generation: request.generation,
            payload: vec![0xff],
        }),
    ];
    assert!(matches!(
        adapter.build(
            &request.repo_id,
            &request.revision_id,
            request.generation,
            &ops
        ),
        Err(quanta_index_core::CoreError::InvalidContract(_))
    ));
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}

#[test]
fn combined_replacement_retires_old_symbols_and_preserves_pinned_view() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(1, None, vec![file_scope("a.rs", "oldmarker")?])?;
    adapter.build_batch(&base)?;
    let pinned = adapter.open(&base.repo_id, &base.revision_id, base.generation)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "freshmarker")?])?;
    adapter.build_batch(&delta)?;
    assert_units(&adapter, &delta, "oldmarker", &[], &[])?;
    assert_units(
        &adapter,
        &delta,
        "freshmarker",
        &["chunk-freshmarker"],
        &["symbol-freshmarker"],
    )?;
    let old_symbols =
        pinned.search_symbols(&query("oldmarker"), 20, &RequestBudgetV1::unbounded())?;
    assert_eq!(old_symbols.len(), 1);
    assert_eq!(
        old_symbols
            .first()
            .ok_or("missing pinned symbol")?
            .candidate_id,
        "symbol-oldmarker"
    );
    Ok(())
}

#[test]
fn tombstone_removes_its_file_and_inherits_other_file_units() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(
        1,
        None,
        vec![
            file_scope("a.rs", "retiredmarker")?,
            file_scope("b.rs", "keptmarker")?,
        ],
    )?;
    adapter.build_batch(&base)?;
    let mut delta = batch(2, Some(1), Vec::new())?;
    delta.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    delta.source_event.payload_sha256 = source_event_payload_sha256(&delta)?;
    adapter.build_batch(&delta)?;
    assert_units(&adapter, &delta, "retiredmarker", &[], &[])?;
    assert_units(
        &adapter,
        &delta,
        "keptmarker",
        &["chunk-keptmarker"],
        &["symbol-keptmarker"],
    )
}

#[test]
fn same_path_sources_replace_and_tombstone_independently() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let first = file_scope("a.rs", "firstmarker")?;
    let mut other = file_scope("a.rs", "othermarker")?;
    other.coverage.source.file.source_repo_id = RepoId::new("other-source")?;
    let base = batch(1, None, vec![first, other])?;
    adapter.build_batch(&base)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "freshmarker")?])?;
    adapter.build_batch(&delta)?;
    assert_units(
        &adapter,
        &delta,
        "othermarker",
        &["chunk-othermarker"],
        &["symbol-othermarker"],
    )?;
    assert_units(&adapter, &delta, "firstmarker", &[], &[])?;
    let mut removed = batch(3, Some(2), Vec::new())?;
    removed.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    removed.source_event.payload_sha256 = source_event_payload_sha256(&removed)?;
    adapter.build_batch(&removed)?;
    assert_units(
        &adapter,
        &removed,
        "othermarker",
        &["chunk-othermarker"],
        &["symbol-othermarker"],
    )?;
    assert_units(&adapter, &removed, "freshmarker", &[], &[])
}

#[test]
fn admitted_empty_failed_file_gates_broad_symbol_query_and_survives_delta() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let mut failed = file_scope("z.rs", "unusedmarker")?;
    failed.chunks.clear();
    failed.symbols.clear();
    failed.coverage.symbols = SymbolCoverage::ParseFailed;
    failed.coverage.unit_set_sha256 = source_file_unit_set_sha256(&[], &[])?;
    let base = batch(1, None, vec![file_scope("a.rs", "oldmarker")?, failed])?;
    adapter.build_batch(&base)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "freshmarker")?])?;
    adapter.build_batch(&delta)?;
    let view = adapter.open(&delta.repo_id, &delta.revision_id, delta.generation)?;
    assert_eq!(
        view.source_file_coverage()
            .ok_or("missing admitted universe")?
            .len(),
        2
    );
    assert!(matches!(
        view.search_symbols(&query("nohitsmarker"), 20, &RequestBudgetV1::unbounded()),
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SymbolCoverageIncomplete,
            ..
        })
    ));
    let mut narrow = query("freshmarker");
    narrow.filters.push(quanta_index_contract::LqFilter::File {
        pattern: "a[.]rs".into(),
        scope: quanta_index_contract::LqFileScope::PathOnly,
    });
    assert_eq!(
        view.search_symbols(&narrow, 20, &RequestBudgetV1::unbounded())?
            .len(),
        1
    );
    assert_eq!(
        view.search(&query("freshmarker"), 20, &RequestBudgetV1::unbounded())?
            .len(),
        1
    );
    Ok(())
}

#[test]
fn empty_full_generation_and_empty_delta_retain_admission() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let empty = batch(1, None, Vec::new())?;
    adapter.build_batch(&empty)?;
    let delta = batch(2, Some(1), Vec::new())?;
    adapter.build_batch(&delta)?;
    let view = adapter.open(&delta.repo_id, &delta.revision_id, delta.generation)?;
    assert!(
        view.source_file_coverage()
            .ok_or("missing admitted empty universe")?
            .is_empty()
    );
    assert!(
        view.search_symbols(&query("anything"), 20, &RequestBudgetV1::unbounded())?
            .is_empty()
    );
    assert_eq!(view.source_publication_event(), Some(&delta.source_event));
    Ok(())
}

#[test]
fn inherited_candidate_collision_refuses_before_target_creation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(1, None, vec![file_scope("kept.rs", "keptmarker")?])?;
    adapter.build_batch(&base)?;
    let mut replacement = file_scope("different.rs", "newmarker")?;
    replacement
        .chunks
        .first_mut()
        .ok_or("fixture chunk")?
        .chunk_id = ChunkId::new("chunk-keptmarker");
    replacement.coverage.unit_set_sha256 =
        source_file_unit_set_sha256(&replacement.chunks, &replacement.symbols)?;
    let delta = batch(2, Some(1), vec![replacement])?;
    assert!(adapter.preflight_batch(&delta).is_err());
    assert!(adapter.build_batch(&delta).is_err());
    let target = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &delta.repo_id,
        &delta.revision_id,
    )
    .generation_dir(dir.path(), delta.generation);
    assert!(
        !target.exists(),
        "ownership refusal must precede target materialization"
    );
    assert_units(
        &adapter,
        &base,
        "keptmarker",
        &["chunk-keptmarker"],
        &["symbol-keptmarker"],
    )
}

#[test]
fn full_rebuild_preflight_reaches_repair_without_admitting_corrupt_content() -> TestResult {
    use quanta_index_core::{
        GenerationIdentityValidatePort as _, SealedGenerationReclaimPort as _,
    };

    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(1, None, vec![file_scope("base.rs", "basemarker")?])?;
    adapter.build_batch(&base)?;
    let request = batch(2, None, vec![file_scope("a.rs", "rebuiltmarker")?])?;
    adapter.build_batch(&request)?;
    let mut other_event = request.clone();
    other_event.source_event.event_id = "different-event".into();
    assert!(adapter.preflight_batch(&other_event).is_err());

    let identity = quanta_index_contract::GenerationSnapshot {
        repo_id: request.repo_id.clone(),
        revision_id: request.revision_id.clone(),
        track: quanta_index_contract::SearchPlaneTrackKind::Lexical,
        manifest_generation: request.generation,
        manifest_digest: request.manifest_digest.clone(),
    };
    let target = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &request.repo_id,
        &request.revision_id,
    )
    .generation_dir(dir.path(), request.generation);
    let coverage_path = target.join("source-file-coverage.cbor");
    let mut corrupt = std::fs::read(&coverage_path)?;
    *corrupt.last_mut().ok_or("empty coverage fixture")? ^= 1;
    std::fs::write(&coverage_path, &corrupt)?;
    assert!(adapter.validate_generation_identity(&identity).is_err());
    adapter.preflight_batch(&request)?;
    assert_eq!(std::fs::read(&coverage_path)?, corrupt);
    assert!(adapter.build_batch(&request).is_err());
    assert!(
        adapter
            .open(&request.repo_id, &request.revision_id, request.generation)
            .is_err()
    );

    let mut wrong_identity = request.clone();
    wrong_identity.manifest_digest = "different-manifest".into();
    assert!(matches!(
        adapter.preflight_batch(&wrong_identity),
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
            ..
        })
    ));
    let mut delta = request.clone();
    delta.mode = BatchIngestMode::Delta;
    delta.base_generation = Some(base.generation);
    delta.source_event.payload_sha256 = source_event_payload_sha256(&delta)?;
    delta.validate_v1()?;
    assert!(matches!(
        adapter.preflight_batch(&delta),
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        })
    ));

    // The materializer owns journal binding and registry fencing. At the
    // adapter boundary only explicit identity-checked reclamation permits
    // the full payload to rebuild; preflight itself did not change storage.
    assert!(matches!(
        adapter.reclaim_sealed_generation(&identity)?,
        quanta_index_core::SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
    ));
    adapter.build_batch(&request)?;
    adapter.validate_generation_identity(&identity)?;
    assert_units(
        &adapter,
        &request,
        "rebuiltmarker",
        &["chunk-rebuiltmarker"],
        &["symbol-rebuiltmarker"],
    )
}

#[test]
fn actual_generation_open_refuses_missing_or_tampered_coverage() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(1, None, vec![file_scope("a.rs", "marker")?])?;
    adapter.build_batch(&base)?;
    let generation =
        quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
            &base.repo_id,
            &base.revision_id,
        )
        .generation_dir(dir.path(), base.generation);
    let path = generation.join("source-file-coverage.cbor");
    let original = std::fs::read(&path)?;
    let mut corrupted = original.clone();
    *corrupted.last_mut().ok_or("empty committed coverage")? ^= 1;
    std::fs::write(&path, corrupted)?;
    assert!(
        adapter
            .open(&base.repo_id, &base.revision_id, base.generation)
            .is_err()
    );
    std::fs::remove_file(&path)?;
    assert!(
        adapter
            .open(&base.repo_id, &base.revision_id, base.generation)
            .is_err()
    );
    std::fs::write(path, original)?;
    assert_units(
        &adapter,
        &base,
        "marker",
        &["chunk-marker"],
        &["symbol-marker"],
    )
}

fn changed_base_page_between_phases_is_refused(after_second_preflight: bool) -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(1, None, vec![file_scope("a.rs", "oldmarker")?])?;
    adapter.build_batch(&base)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "newmarker")?])?;
    let base_dir =
        quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
            &base.repo_id,
            &base.revision_id,
        )
        .generation_dir(dir.path(), base.generation);
    let target = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &delta.repo_id,
        &delta.revision_id,
    )
    .generation_dir(dir.path(), delta.generation);
    let page = std::fs::read_dir(&base_dir)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("source-file-coverage-page-")
        })
        .ok_or("missing base coverage page")?
        .path();
    let original = std::fs::read(&page)?;
    let pinned_old_reader = adapter.open(&base.repo_id, &base.revision_id, base.generation)?;

    // The outer and lock-held calls currently have the same lexical port.
    adapter.preflight_batch(&delta)?;
    if after_second_preflight {
        adapter.preflight_batch(&delta)?;
    }
    let mut changed = original.clone();
    *changed.last_mut().ok_or("empty base coverage page")? ^= 1;
    std::fs::write(&page, changed)?;
    let refused = if after_second_preflight {
        adapter.build_batch(&delta)
    } else {
        adapter.preflight_batch(&delta)
    };
    assert!(
        matches!(
            refused,
            Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ),
        "phase recheck admitted a changed base coverage page: {refused:?}"
    );
    assert!(!target.exists(), "refusal created the delta target");
    assert_eq!(
        pinned_old_reader
            .search_symbols(&query("oldmarker"), 20, &RequestBudgetV1::unbounded())?
            .len(),
        1,
        "a pinned reader lost its previously authenticated generation"
    );

    std::fs::write(page, original)?;
    adapter.preflight_batch(&delta)?;
    adapter.build_batch(&delta)?;
    assert_units(
        &adapter,
        &delta,
        "newmarker",
        &["chunk-newmarker"],
        &["symbol-newmarker"],
    )
}

#[test]
fn lock_phase_rechecks_base_page_after_outer_preflight() -> TestResult {
    changed_base_page_between_phases_is_refused(false)
}

#[test]
fn build_rechecks_base_page_after_lock_phase_and_retry_succeeds() -> TestResult {
    changed_base_page_between_phases_is_refused(true)
}

#[test]
fn foreign_source_owner_refuses_before_target_creation() -> TestResult {
    let mut replacement = file_scope("a.rs", "marker")?;
    replacement
        .chunks
        .first_mut()
        .ok_or("fixture chunk")?
        .source_repo_id = Some(RepoId::new("foreign-source")?);
    replacement.coverage.unit_set_sha256 =
        source_file_unit_set_sha256(&replacement.chunks, &replacement.symbols)?;
    assert_refused_without_mutation(
        &batch(1, None, vec![replacement])?,
        MutationConflict::RecordSourceMismatch,
    )
}

#[test]
fn raw_delta_cannot_inherit_coverage_without_a_source_publication() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(1, None, vec![file_scope("a.rs", "oldmarker")?])?;
    adapter.build_batch(&base)?;
    let replacement = file_scope("a.rs", "freshmarker")?;
    let mut payload = Vec::new();
    ciborium::into_writer(
        &(BatchIngestMode::Delta, Some(base.generation), replacement),
        &mut payload,
    )?;
    let target = ManifestGeneration::new(2);
    let operations = [LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
        repo_id: base.repo_id.clone(),
        revision_id: base.revision_id.clone(),
        generation: target,
        payload,
    })];
    let result = adapter.build(&base.repo_id, &base.revision_id, target, &operations);
    assert!(
        matches!(result, Err(quanta_index_core::CoreError::InvalidContract(ref message)) if message.contains("coverage-bound"))
    );
    let target_path =
        quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
            &base.repo_id,
            &base.revision_id,
        )
        .generation_dir(dir.path(), target);
    assert!(!target_path.exists());
    assert_units(
        &adapter,
        &base,
        "oldmarker",
        &["chunk-oldmarker"],
        &["symbol-oldmarker"],
    )
}

#[test]
fn reclaiming_base_preserves_delta_coverage_and_refuses_old_open() -> TestResult {
    use quanta_index_core::SealedGenerationReclaimPort as _;
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(
        1,
        None,
        vec![
            file_scope("a.rs", "oldmarker")?,
            file_scope("b.rs", "keptmarker")?,
        ],
    )?;
    adapter.build_batch(&base)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "freshmarker")?])?;
    adapter.build_batch(&delta)?;
    let retired = quanta_index_contract::GenerationSnapshot {
        repo_id: base.repo_id.clone(),
        revision_id: base.revision_id.clone(),
        track: quanta_index_contract::SearchPlaneTrackKind::Lexical,
        manifest_generation: base.generation,
        manifest_digest: base.manifest_digest.clone(),
    };
    assert!(matches!(
        adapter.reclaim_sealed_generation(&retired)?,
        quanta_index_core::SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
    ));
    assert!(
        adapter
            .open(&base.repo_id, &base.revision_id, base.generation)
            .is_err()
    );
    let reopened = adapter.open(&delta.repo_id, &delta.revision_id, delta.generation)?;
    assert_eq!(
        reopened
            .source_file_coverage()
            .ok_or("delta coverage missing")?
            .len(),
        2
    );
    assert_eq!(
        reopened.source_publication_event(),
        Some(&delta.source_event)
    );
    assert_units(
        &adapter,
        &delta,
        "keptmarker",
        &["chunk-keptmarker"],
        &["symbol-keptmarker"],
    )?;
    assert_units(
        &adapter,
        &delta,
        "freshmarker",
        &["chunk-freshmarker"],
        &["symbol-freshmarker"],
    )
}
