//! L2 mutation regressions. The source-file owner must be validated before
//! preparing a generation or applying any operation from the request.

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "assertions report regression failures"
)]

use sha2::{Digest as _, Sha256};
use std::error::Error;

use quanta_index_contract::channel::{LexicalChannelOp, ReplaceLexicalScope};
use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalCursor, LqExpr, LqFilter, LqLeaf,
    LqOptions, LqPatternType, LqQuery, LqSelect, LqSpan, ManifestGeneration, QueryConstraintSetV1,
    RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchCorpusSurfaceMutationConflictV1 as MutationConflict, SearchCorpusTombstoneScope,
    SearchScopeSurface, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, SymbolCoverage, SymbolId, source_event_payload_sha256,
    source_file_unit_set_sha256,
};
use quanta_index_core::{
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalPageSpec, LexicalSearcher, RequestBudgetV1,
    SearchCorpusBatchBuildPort, SearchCorpusPreflightPhaseV1,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

fn file_scope(path: &str, marker: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let mut scope = SearchCorpusReplaceScope {
        source_bytes: marker.as_bytes().to_vec(),
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("l2-mutation-repo")?,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new(format!("source-{marker}"))?,
                source_sha256: Sha256::digest(marker.as_bytes()).into(),
            },
            language: LanguageCode::new("rust")?,
            producer_policy_sha256: [8; 32],
            symbol_name_source_policy: quanta_index_contract::SymbolNameSourcePolicyV1::Unspecified,
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

fn code_search_source_owners(
    adapter: &LexicalAdapter,
    batch: &SearchCorpusIngestBatch,
    marker: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let view = adapter.open(
        &batch.repo_id,
        &batch.revision_id,
        batch.generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let mut options = LqOptions::defaults();
    options.pattern_type = LqPatternType::CodeSearch;
    let request = LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::RawString(marker.into())),
        filters: vec![LqFilter::Select {
            dim: LqSelect::File,
        }],
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    };
    Ok(view
        .search_constrained(
            &request,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates
        .into_iter()
        .map(|row| row.source_repo_id.as_str().to_string())
        .collect())
}

#[test]
fn delta_cannot_claim_newer_source_lineage_while_inheriting_an_older_snapshot() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let original = batch(1, None, vec![file_scope("a.rs", "oldmarker")?])?;
    let _stages = adapter.build_batch(&original)?;
    let newer = batch(2, Some(1), vec![file_scope("a.rs", "newmarker")?])?;
    let _stages = adapter.build_batch(&newer)?;

    // The producer knows event-2, but points materialization back at event-1.
    // Applying only b.rs to that snapshot would silently resurrect old a.rs.
    let mut request = batch(3, Some(1), vec![file_scope("b.rs", "othermarker")?])?;
    request.source_event.expected_base_event_id = Some(newer.source_event.event_id.clone());
    request.validate_v1()?;
    request.validate_surface_mutations_v1()?;
    let mut owner = quanta_index_core::PublicationValidationOwner::default();
    for result in [
        adapter.preflight_batch_with_owner(
            &request,
            SearchCorpusPreflightPhaseV1::BeforeIntent,
            &mut owner,
        ),
        adapter.preflight_batch_with_owner(
            &request,
            SearchCorpusPreflightPhaseV1::UnderOperationLock,
            &mut owner,
        ),
        adapter
            .build_batch_with_owner(&request, &mut owner)
            .map(|_stages| ()),
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
        let mut owner = quanta_index_core::PublicationValidationOwner::default();
        for result in [
            adapter.preflight_batch_with_owner(
                &invalid,
                SearchCorpusPreflightPhaseV1::BeforeIntent,
                &mut owner,
            ),
            adapter.preflight_batch_with_owner(
                &invalid,
                SearchCorpusPreflightPhaseV1::UnderOperationLock,
                &mut owner,
            ),
            adapter
                .build_batch_with_owner(&invalid, &mut owner)
                .map(|_stages| ()),
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
    let mut owner = quanta_index_core::PublicationValidationOwner::default();
    adapter.preflight_batch_with_owner(
        &request,
        SearchCorpusPreflightPhaseV1::BeforeIntent,
        &mut owner,
    )?;
    let _stages = adapter.build_batch_with_owner(&request, &mut owner)?;
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
    let view = adapter.open(
        &batch.repo_id,
        &batch.revision_id,
        batch.generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
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
fn oversized_raw_source_is_refused_before_index_commit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let request = batch(1, None, vec![file_scope("a.rs", "needle")?])?;
    let mut oversized = request
        .replace_scopes
        .first()
        .ok_or("missing replacement scope")?
        .clone();
    oversized.source_bytes.resize(8 * 1024 * 1024 + 1, b'x');
    oversized.coverage.source.source_sha256 = Sha256::digest(&oversized.source_bytes).into();
    let raw_op = |scope: &SearchCorpusReplaceScope| -> Result<LexicalChannelOp, Box<dyn Error>> {
        let mut payload = Vec::new();
        ciborium::into_writer(
            &(request.mode, request.base_generation, scope),
            &mut payload,
        )?;
        Ok(LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            generation: request.generation,
            payload,
        }))
    };
    let result = adapter.build(
        &request.repo_id,
        &request.revision_id,
        request.generation,
        &[raw_op(&oversized)?],
    );
    assert!(
        matches!(result, Err(quanta_index_core::CoreError::InvalidContract(ref message)) if message.contains("8 MiB")),
        "{result:?}"
    );
    let target = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &request.repo_id,
        &request.revision_id,
    )
    .generation_dir(dir.path(), request.generation);
    let index = tantivy::Index::open_in_dir(&target)?;
    let reader = index.reader()?;
    assert_eq!(
        reader
            .searcher()
            .search(&tantivy::query::AllQuery, &tantivy::collector::Count)?,
        0,
        "an invalid source must not leave committed candidates"
    );
    adapter.build(
        &request.repo_id,
        &request.revision_id,
        request.generation,
        &[raw_op(
            request
                .replace_scopes
                .first()
                .ok_or("missing replacement scope")?,
        )?],
    )?;
    Ok(())
}

#[test]
fn oversized_published_source_is_refused_before_target_creation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let mut scope = file_scope("a.rs", "needle")?;
    scope.source_bytes.resize(8 * 1024 * 1024 + 1, b'x');
    scope.coverage.source.source_sha256 = Sha256::digest(&scope.source_bytes).into();
    let request = batch(1, None, vec![scope])?;
    for result in [
        adapter.preflight_batch(&request, SearchCorpusPreflightPhaseV1::BeforeIntent),
        adapter.build_batch(&request).map(|_stages| ()),
    ] {
        assert!(
            matches!(result, Err(quanta_index_core::CoreError::InvalidContract(ref message)) if message.contains("8 MiB")),
            "{result:?}"
        );
    }
    let target = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &request.repo_id,
        &request.revision_id,
    )
    .generation_dir(dir.path(), request.generation);
    assert!(!target.exists());
    let mut allowed = file_scope("a.rs", "needle")?;
    allowed.source_bytes.resize(8 * 1024 * 1024, b'x');
    allowed.coverage.source.source_sha256 = Sha256::digest(&allowed.source_bytes).into();
    let admitted = batch(1, None, vec![allowed])?;
    adapter.preflight_batch(&admitted, SearchCorpusPreflightPhaseV1::BeforeIntent)?;
    let _stages = adapter.build_batch(&admitted)?;
    assert_eq!(
        code_search_source_owners(&adapter, &admitted, "needle")?,
        ["l2-mutation-repo"]
    );
    Ok(())
}

#[test]
fn malformed_bundle_is_refused_before_source_publication_or_generation_preparation() -> TestResult {
    for invalid_payload in [&[0xff][..], b"manifest".as_slice()] {
        let dir = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
        let mut request = batch(1, None, vec![file_scope("a.rs", "validmarker")?])?;
        request.bundle_payload = Some(invalid_payload.to_vec());
        request.source_event.payload_sha256 = source_event_payload_sha256(&request)?;
        request.validate_v1()?;
        request.validate_surface_mutations_v1()?;
        for result in [
            adapter.preflight_batch(&request, SearchCorpusPreflightPhaseV1::BeforeIntent),
            adapter.build_batch(&request).map(|_stages| ()),
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
    }
    Ok(())
}

#[test]
fn malformed_bundle_after_a_raw_replacement_refuses_before_any_write() -> TestResult {
    for invalid_payload in [&[0xff][..], b"manifest".as_slice()] {
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
                payload: invalid_payload.to_vec(),
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
    }
    Ok(())
}

#[test]
fn combined_replacement_retires_old_symbols_and_preserves_pinned_view() -> TestResult {
    fn assert_authority_stage_children(
        stages: &quanta_index_contract::LexicalBuildStageDurationsV1,
    ) -> TestResult {
        let preflight = stages
            .prep_file_authority_preflight_ns
            .ok_or("missing file preflight timing")?;
        let coverage = stages
            .prep_coverage_write_ns
            .ok_or("missing coverage write timing")?;
        assert!(
            preflight
                .checked_add(coverage)
                .ok_or("preparation child overflow")?
                <= stages.preparation_ns
        );
        assert!(
            stages
                .file_authority_source_write_ns
                .ok_or("missing source write timing")?
                <= stages.file_authority_ns
        );
        Ok(())
    }
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(1, None, vec![file_scope("a.rs", "oldmarker")?])?;
    let base_stages = adapter
        .build_batch(&base)?
        .ok_or("missing base stage timing")?;
    assert_authority_stage_children(&base_stages)?;
    let pinned = adapter.open(
        &base.repo_id,
        &base.revision_id,
        base.generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "freshmarker")?])?;
    let delta_stages = adapter
        .build_batch(&delta)?
        .ok_or("missing delta stage timing")?;
    assert_authority_stage_children(&delta_stages)?;
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
    let _stages = adapter.build_batch(&base)?;
    let mut delta = batch(2, Some(1), Vec::new())?;
    delta.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    delta.source_event.payload_sha256 = source_event_payload_sha256(&delta)?;
    let _stages = adapter.build_batch(&delta)?;
    assert_units(&adapter, &delta, "retiredmarker", &[], &[])?;
    assert_units(
        &adapter,
        &delta,
        "keptmarker",
        &["chunk-keptmarker"],
        &["symbol-keptmarker"],
    )
}

/// Three distinct source files share one search term, but have independent
/// candidate IDs and source hashes.
///
/// The oracle is the two retained source
/// files, constructed without the replaced/deleted file's indexing history.
fn scored_file_scope(path: &str, marker: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let mut scope = file_scope(path, marker)?;
    let extra_tokens = match path {
        "a.rs" => 47,
        "b.rs" => 73,
        "c.rs" => 109,
        _ => 0,
    };
    let content = format!(
        "{marker} livebm25needle {}",
        std::iter::repeat_n("scoringpadding", extra_tokens)
            .collect::<Vec<_>>()
            .join(" ")
    );
    scope.source_bytes = content.as_bytes().to_vec();
    scope.coverage.source.source_sha256 = Sha256::digest(content.as_bytes()).into();
    let chunk = scope.chunks.first_mut().ok_or("missing scored chunk")?;
    chunk.text = content.into();
    chunk.end_byte = u32::try_from(chunk.text.len())?;
    scope.coverage.unit_set_sha256 = source_file_unit_set_sha256(&scope.chunks, &scope.symbols)?;
    Ok(scope)
}

#[test]
fn tombstone_scoring_uses_only_live_source_docs() -> TestResult {
    struct ScoreStats {
        num_docs: u64,
        max_doc: u64,
        chunk_text_tokens: u64,
        needle_doc_freq: u64,
        segments: Vec<(u32, u32, u64)>,
    }

    fn score_stats(generation_dir: &std::path::Path) -> Result<ScoreStats, Box<dyn Error>> {
        let index = tantivy::Index::open_in_dir(generation_dir)?;
        let chunk_text = index.schema().get_field("chunk_text")?;
        let reader = index.reader()?;
        let searcher = reader.searcher();
        let segments = searcher
            .segment_readers()
            .iter()
            .map(|segment| {
                Ok::<_, tantivy::TantivyError>((
                    segment.num_docs(),
                    segment.max_doc(),
                    segment.inverted_index(chunk_text)?.total_num_tokens(),
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let chunk_text_tokens = segments.iter().map(|row| row.2).sum();
        let max_doc = segments.iter().map(|row| u64::from(row.1)).sum();
        let needle_doc_freq = searcher.doc_freq(&tantivy::Term::from_field_text(
            chunk_text,
            "livebm25needle",
        ))?;
        Ok(ScoreStats {
            num_docs: searcher.num_docs(),
            max_doc,
            chunk_text_tokens,
            needle_doc_freq,
            segments,
        })
    }

    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let retired = scored_file_scope("a.rs", "retiredmarker")?;
    let kept_b = scored_file_scope("b.rs", "keptbmarker")?;
    let kept_c = scored_file_scope("c.rs", "keptcmarker")?;
    let base = batch(1, None, vec![retired, kept_b.clone(), kept_c.clone()])?;
    let _stages = adapter.build_batch(&base)?;
    let changed = batch(
        2,
        Some(1),
        vec![scored_file_scope("a.rs", "replacedmarker")?],
    )?;
    let _stages = adapter.build_batch(&changed)?;
    let mut deleted = batch(3, Some(2), Vec::new())?;
    deleted.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    deleted.source_event.payload_sha256 = source_event_payload_sha256(&deleted)?;
    let target = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &deleted.repo_id,
        &deleted.revision_id,
    )
    .generation_dir(dir.path(), deleted.generation);
    let mut invalid = deleted.clone();
    invalid.source_event.payload_sha256 = [0; 32];
    assert!(adapter.build_batch(&invalid).is_err());
    assert!(
        !target.exists(),
        "invalid delete intent must not create target generation"
    );
    assert_units(
        &adapter,
        &changed,
        "livebm25needle",
        &[
            "chunk-keptbmarker",
            "chunk-keptcmarker",
            "chunk-replacedmarker",
        ],
        &[],
    )?;
    let _stages = adapter.build_batch(&deleted)?;

    let fresh_dir = tempfile::tempdir()?;
    let fresh = LexicalAdapter::with_state_root(fresh_dir.path().to_path_buf());
    let rebuilt = batch(3, None, vec![kept_b, kept_c])?;
    let _stages = fresh.build_batch(&rebuilt)?;
    let budget = RequestBudgetV1::unbounded();
    let live_view = adapter.open(
        &deleted.repo_id,
        &deleted.revision_id,
        deleted.generation,
        &budget,
    )?;
    let fresh_view = fresh.open(
        &rebuilt.repo_id,
        &rebuilt.revision_id,
        rebuilt.generation,
        &budget,
    )?;
    assert_eq!(
        live_view.source_file_coverage(),
        fresh_view.source_file_coverage()
    );
    let live = live_view.search(&query("livebm25needle"), 10, &budget)?;
    let expected = fresh_view.search(&query("livebm25needle"), 10, &budget)?;
    let project = |rows: &[quanta_index_contract::LexicalCandidate]| {
        rows.iter()
            .map(|row| {
                (
                    row.repo_relative_path.as_str().to_string(),
                    row.candidate_id.clone(),
                    row.source.as_ref().map(|source| source.source_sha256),
                    row.score.to_bits(),
                )
            })
            .collect::<Vec<_>>()
    };
    let live_rows = project(&live);
    let expected_rows = project(&expected);
    let live_stats = score_stats(&target)?;
    let rebuilt_target =
        quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
            &rebuilt.repo_id,
            &rebuilt.revision_id,
        )
        .generation_dir(fresh_dir.path(), rebuilt.generation);
    let rebuilt_stats = score_stats(&rebuilt_target)?;
    assert_eq!(
        live_stats.num_docs, 4,
        "two retained files have text and symbol docs"
    );
    assert!(
        live_stats.max_doc > live_stats.num_docs,
        "delta retains base segment bytes"
    );
    assert_eq!(
        live_stats.needle_doc_freq, 3,
        "raw index still counts retired term"
    );
    assert_eq!(live_stats.num_docs, rebuilt_stats.num_docs);
    assert!(live_stats.max_doc > rebuilt_stats.max_doc);
    assert_eq!(
        live_stats.needle_doc_freq,
        rebuilt_stats.needle_doc_freq + 1
    );
    assert_eq!(live_rows.len(), 2);
    assert_eq!(
        live_rows
            .iter()
            .map(|row| row.0.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["b.rs", "c.rs"]),
        "only retained source paths may score"
    );
    assert_eq!(
        live_rows,
        expected_rows,
        "deleted source must not change retained BM25 scores: live chunk_text_tokens={}, segments={:?}; fresh chunk_text_tokens={}, segments={:?}",
        live_stats.chunk_text_tokens,
        live_stats.segments,
        rebuilt_stats.chunk_text_tokens,
        rebuilt_stats.segments,
    );
    let index = tantivy::Index::open_in_dir(&target)?;
    assert_eq!(
        index
            .searchable_segment_metas()?
            .iter()
            .map(tantivy::SegmentMeta::num_deleted_docs)
            .sum::<u32>(),
        2,
        "retired text and symbol documents remain physically present"
    );
    Ok(())
}

/// Compare live scores with an independent full rebuild of the final documents.
///
/// The full rebuild has no tombstone history. Term frequency, field length,
/// duplicate terms, symbol scoring and cursor order all affect the comparison.
fn bm25_oracle_scope(
    path: &str,
    marker: &str,
    needle_repeats: usize,
    padding: usize,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let mut scope = file_scope(path, marker)?;
    let content = format!(
        "livebm25symbol {marker} {} {}",
        std::iter::repeat_n("livebm25needle", needle_repeats)
            .collect::<Vec<_>>()
            .join(" "),
        std::iter::repeat_n("fieldlengthpadding", padding)
            .collect::<Vec<_>>()
            .join(" "),
    );
    scope.source_bytes = content.as_bytes().to_vec();
    scope.coverage.source.source_sha256 = Sha256::digest(content.as_bytes()).into();
    let chunk = scope
        .chunks
        .first_mut()
        .ok_or("missing BM25 oracle chunk")?;
    chunk.text = content.into();
    chunk.end_byte = u32::try_from(chunk.text.len())?;
    let symbol = scope
        .symbols
        .first_mut()
        .ok_or("missing BM25 oracle symbol")?;
    symbol.local_name = "livebm25symbol".into();
    symbol.qualified_name = format!("crate::{marker}::livebm25symbol").into();
    symbol.definition_span.byte_end = u32::try_from("livebm25symbol".len())?;
    scope.coverage.unit_set_sha256 = source_file_unit_set_sha256(&scope.chunks, &scope.symbols)?;
    Ok(scope)
}

fn bm25_text_pages(
    view: &dyn LexicalSearcher,
    generation: ManifestGeneration,
    request: &LqQuery,
) -> Result<Vec<(String, u32)>, Box<dyn Error>> {
    let mut rows = Vec::new();
    let mut after: Option<LexicalCursor> = None;
    loop {
        let page = view.search_constrained(
            request,
            &QueryConstraintSetV1::unconstrained(),
            &LexicalPageSpec { fetch: 2, after },
            &RequestBudgetV1::unbounded(),
        )?;
        let Some(last) = page.candidates.last() else {
            break;
        };
        after = Some(LexicalCursor::at(generation, last.order_key()));
        for candidate in &page.candidates {
            rows.push((candidate.candidate_id.clone(), candidate.score.to_bits()));
        }
        if rows.len() > 4 {
            return Err("BM25 text cursor repeated or emitted extra rows".into());
        }
    }
    Ok(rows)
}

fn bm25_symbol_pages(
    view: &dyn LexicalSearcher,
    generation: ManifestGeneration,
    request: &LqQuery,
) -> Result<Vec<(String, u32)>, Box<dyn Error>> {
    let mut rows = Vec::new();
    let mut after: Option<LexicalCursor> = None;
    loop {
        let page = view.search_symbols_constrained(
            request,
            &QueryConstraintSetV1::unconstrained(),
            &LexicalPageSpec { fetch: 2, after },
            &RequestBudgetV1::unbounded(),
        )?;
        let Some(last) = page.candidates.last() else {
            break;
        };
        after = Some(LexicalCursor::at(generation, last.order_key()));
        for candidate in &page.candidates {
            rows.push((candidate.candidate_id.clone(), candidate.score.to_bits()));
        }
        if rows.len() > 4 {
            return Err("BM25 symbol cursor repeated or emitted extra rows".into());
        }
    }
    Ok(rows)
}

#[test]
fn live_bm25_delete_and_replace_matches_fresh_full_bits_and_pages() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let retired = bm25_oracle_scope("a.rs", "retired", 11, 4096)?;
    let kept_b = bm25_oracle_scope("b.rs", "keptb", 1, 7)?;
    let kept_c = bm25_oracle_scope("c.rs", "keptc", 2, 63)?;
    let old_d = bm25_oracle_scope("d.rs", "oldd", 3, 255)?;
    let new_d = bm25_oracle_scope("d.rs", "newd", 7, 2047)?;
    let kept_e = bm25_oracle_scope("e.rs", "keepte", 5, 1023)?;
    let base = batch(
        1,
        None,
        vec![
            retired,
            kept_b.clone(),
            kept_c.clone(),
            old_d,
            kept_e.clone(),
        ],
    )?;
    let _stages = adapter.build_batch(&base)?;
    let mut delta = batch(2, Some(1), vec![new_d.clone()])?;
    delta.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    delta.source_event.payload_sha256 = source_event_payload_sha256(&delta)?;
    let _stages = adapter.build_batch(&delta)?;

    let fresh_dir = tempfile::tempdir()?;
    let fresh = LexicalAdapter::with_state_root(fresh_dir.path().to_path_buf());
    let rebuilt = batch(2, None, vec![kept_b, kept_c, new_d, kept_e])?;
    let _stages = fresh.build_batch(&rebuilt)?;
    let budget = RequestBudgetV1::unbounded();
    let live = adapter.open(
        &delta.repo_id,
        &delta.revision_id,
        delta.generation,
        &budget,
    )?;
    let expected = fresh.open(
        &rebuilt.repo_id,
        &rebuilt.revision_id,
        rebuilt.generation,
        &budget,
    )?;
    let cold_adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let cold = cold_adapter.open(
        &delta.repo_id,
        &delta.revision_id,
        delta.generation,
        &budget,
    )?;

    let native = tantivy::Index::open_in_dir(
        quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
            &delta.repo_id,
            &delta.revision_id,
        )
        .generation_dir(dir.path(), delta.generation),
    )?;
    assert_eq!(
        native.reader()?.searcher().num_docs(),
        8,
        "four text and four symbol docs"
    );
    let fresh_native = tantivy::Index::open_in_dir(
        quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
            &rebuilt.repo_id,
            &rebuilt.revision_id,
        )
        .generation_dir(fresh_dir.path(), rebuilt.generation),
    )?;
    let fresh_native_reader = fresh_native.reader()?;
    let fresh_native_searcher = fresh_native_reader.searcher();
    assert_eq!(fresh_native_searcher.num_docs(), 8);
    assert_eq!(
        fresh_native_searcher
            .segment_readers()
            .iter()
            .map(|segment| u64::from(segment.max_doc()))
            .sum::<u64>(),
        8,
        "independent native full index must contain no deleted documents",
    );
    // The fixture's ASCII token census is independent of either BM25
    // provider: final text docs have 10+67+2056+1030 tokens, and four symbol
    // snippets contribute one chunk_text token each.
    let chunk_text = fresh_native.schema().get_field("chunk_text")?;
    let symbol_name = fresh_native.schema().get_field("symbol_local_name")?;
    let candidate_id = fresh_native.schema().get_field("candidate_id")?;
    let fresh_chunk_tokens = fresh_native_searcher
        .segment_readers()
        .iter()
        .map(|segment| {
            Ok::<_, tantivy::TantivyError>(segment.inverted_index(chunk_text)?.total_num_tokens())
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum::<u64>();
    assert_eq!(
        fresh_chunk_tokens, 3167,
        "fixed ASCII chunk_text token census"
    );
    // The fixed source census also has four symbol_local_name STRING values,
    // eight candidate_id STRING values, and four indexed numeric
    // text_authority_doc_id values. Native Basic-field token headers can be
    // estimated after a merge; E3's owner-level provider test must compare
    // those live totals with 4, 8, and 4 directly.
    for (field, term, expected_df) in [
        (chunk_text, "livebm25needle", 4),
        (chunk_text, "livebm25symbol", 8),
        (chunk_text, "fieldlengthpadding", 4),
        (chunk_text, "retired", 0),
        (chunk_text, "oldd", 0),
        (symbol_name, "livebm25symbol", 4),
        (candidate_id, "chunk-keptb", 1),
        (candidate_id, "chunk-retired", 0),
    ] {
        assert_eq!(
            fresh_native_searcher.doc_freq(&tantivy::Term::from_field_text(field, term))?,
            expected_df,
            "fixed fresh-native doc frequency for {term}",
        );
    }

    let text_query = query("livebm25needle");
    let symbol_query = query("livebm25symbol");
    let project_text = |view: &dyn LexicalSearcher| -> Result<Vec<(String, u32)>, Box<dyn Error>> {
        Ok(view
            .search(&text_query, 10, &budget)?
            .into_iter()
            .map(|row| (row.candidate_id, row.score.to_bits()))
            .collect())
    };
    let project_symbol =
        |view: &dyn LexicalSearcher| -> Result<Vec<(String, u32)>, Box<dyn Error>> {
            Ok(view
                .search_symbols(&symbol_query, 10, &budget)?
                .into_iter()
                .map(|row| (row.candidate_id, row.score.to_bits()))
                .collect())
        };
    let live_text = project_text(live.as_ref())?;
    let fresh_text = project_text(expected.as_ref())?;
    let live_symbol = project_symbol(live.as_ref())?;
    let fresh_symbol = project_symbol(expected.as_ref())?;
    let text_ids: std::collections::BTreeSet<_> =
        live_text.iter().map(|row| row.0.as_str()).collect();
    let symbol_ids: std::collections::BTreeSet<_> =
        live_symbol.iter().map(|row| row.0.as_str()).collect();
    assert_eq!(
        text_ids,
        std::collections::BTreeSet::from([
            "chunk-keptb",
            "chunk-keptc",
            "chunk-newd",
            "chunk-keepte",
        ])
    );
    assert_eq!(
        symbol_ids,
        std::collections::BTreeSet::from([
            "symbol-keptb",
            "symbol-keptc",
            "symbol-newd",
            "symbol-keepte",
        ])
    );
    assert_eq!(
        live_text, fresh_text,
        "live text BM25 score bits/order differ from fresh full"
    );
    assert_eq!(
        live_symbol, fresh_symbol,
        "live symbol BM25 score bits/order differ from fresh full"
    );
    for view in [live.as_ref(), expected.as_ref(), cold.as_ref()] {
        assert_eq!(project_text(view)?, fresh_text);
        assert_eq!(project_symbol(view)?, fresh_symbol);
        assert_eq!(
            bm25_text_pages(view, delta.generation, &text_query)?,
            fresh_text
        );
        assert_eq!(
            bm25_symbol_pages(view, delta.generation, &symbol_query)?,
            fresh_symbol
        );
    }
    let mut duplicate_phrase = text_query.clone();
    duplicate_phrase.expr = LqExpr::Leaf(LqLeaf::Phrase("livebm25needle livebm25needle".into()));
    let phrase_expected =
        bm25_text_pages(expected.as_ref(), rebuilt.generation, &duplicate_phrase)?;
    assert_eq!(
        phrase_expected
            .iter()
            .map(|row| row.0.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["chunk-keptc", "chunk-newd", "chunk-keepte"]),
    );
    for view in [live.as_ref(), cold.as_ref()] {
        assert_eq!(
            bm25_text_pages(view, delta.generation, &duplicate_phrase)?,
            phrase_expected,
            "repeated-term phrase score bits/order differ from fresh full",
        );
    }
    Ok(())
}

/// Retain earlier corrections and subtract each newly retired document.
///
/// The second delta retains g2's dead-document corrections and subtracts
/// g3's repeated needle term once from df, while subtracting every accepted
/// token from the live field length. A fourth no-op delta exercises unchanged
/// deletion identity; the public delta contract admits an empty mutation.
#[test]
fn live_bm25_consecutive_deletes_and_noop_match_fresh_full() -> TestResult {
    type LiveStatsWire = (
        u32,
        [u8; 32],
        u64,
        Vec<(
            (String, u32, u32, Option<u64>),
            Vec<(u32, u64)>,
            Vec<(u32, Vec<u8>, u64)>,
        )>,
    );
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let retired = bm25_oracle_scope("a.rs", "retired", 11, 4096)?;
    let old_b = bm25_oracle_scope("b.rs", "oldb", 3, 7)?;
    let kept_c = bm25_oracle_scope("c.rs", "keptc", 2, 63)?;
    let old_d = bm25_oracle_scope("d.rs", "oldd", 3, 255)?;
    let new_d = bm25_oracle_scope("d.rs", "newd", 7, 2047)?;
    let kept_e = bm25_oracle_scope("e.rs", "keepte", 5, 1023)?;
    let new_b = bm25_oracle_scope("b.rs", "newb", 9, 127)?;
    let base = batch(
        1,
        None,
        vec![retired, old_b, kept_c.clone(), old_d, kept_e.clone()],
    )?;
    let _stages = adapter.build_batch(&base)?;

    let mut second = batch(2, Some(1), vec![new_d.clone()])?;
    second.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    second.source_event.payload_sha256 = source_event_payload_sha256(&second)?;
    let _stages = adapter.build_batch(&second)?;

    let third = batch(3, Some(2), vec![new_b.clone()])?;
    let _stages = adapter.build_batch(&third)?;
    let fourth = batch(4, Some(3), Vec::new())?;
    let _stages = adapter.build_batch(&fourth)?;

    let fresh_dir = tempfile::tempdir()?;
    let fresh = LexicalAdapter::with_state_root(fresh_dir.path().to_path_buf());
    let rebuilt = batch(3, None, vec![new_b, kept_c, new_d, kept_e])?;
    let _stages = fresh.build_batch(&rebuilt)?;
    let budget = RequestBudgetV1::unbounded();
    let g3 = adapter.open(
        &third.repo_id,
        &third.revision_id,
        third.generation,
        &budget,
    )?;
    let g4 = adapter.open(
        &fourth.repo_id,
        &fourth.revision_id,
        fourth.generation,
        &budget,
    )?;
    let cold_adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let cold_g3 = cold_adapter.open(
        &third.repo_id,
        &third.revision_id,
        third.generation,
        &budget,
    )?;
    let cold_g4 = cold_adapter.open(
        &fourth.repo_id,
        &fourth.revision_id,
        fourth.generation,
        &budget,
    )?;
    let expected = fresh.open(
        &rebuilt.repo_id,
        &rebuilt.revision_id,
        rebuilt.generation,
        &budget,
    )?;

    let g3_native = tantivy::Index::open_in_dir(
        quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
            &third.repo_id,
            &third.revision_id,
        )
        .generation_dir(dir.path(), third.generation),
    )?;
    let g3_segments = g3_native.searchable_segment_metas()?;
    assert_eq!(g3_native.reader()?.searcher().num_docs(), 8);
    assert_eq!(
        g3_segments
            .iter()
            .map(tantivy::SegmentMeta::num_deleted_docs)
            .sum::<u32>(),
        6,
        "g2's two retired scopes and g3's replaced scope must remain as three text/symbol delete pairs",
    );

    let fresh_native = tantivy::Index::open_in_dir(
        quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
            &rebuilt.repo_id,
            &rebuilt.revision_id,
        )
        .generation_dir(fresh_dir.path(), rebuilt.generation),
    )?;
    let fresh_reader = fresh_native.reader()?;
    let fresh_searcher = fresh_reader.searcher();
    assert_eq!(fresh_searcher.num_docs(), 8);
    assert_eq!(
        fresh_searcher
            .segment_readers()
            .iter()
            .map(|segment| u64::from(segment.max_doc()))
            .sum::<u64>(),
        8,
        "fresh full oracle must have no tombstone history",
    );
    let chunk_text = fresh_native.schema().get_field("chunk_text")?;
    let symbol_name = fresh_native.schema().get_field("symbol_local_name")?;
    let candidate_id = fresh_native.schema().get_field("candidate_id")?;
    // Four text scopes contain 138+67+2056+1030 tokens. Four symbol
    // snippets add one chunk_text token each. This census is fixed from the
    // source strings, independent of the live statistics provider.
    assert_eq!(
        fresh_searcher
            .segment_readers()
            .iter()
            .map(|segment| {
                Ok::<_, tantivy::TantivyError>(
                    segment.inverted_index(chunk_text)?.total_num_tokens(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .sum::<u64>(),
        3295,
    );
    for (field, term, df) in [
        (chunk_text, "livebm25needle", 4),
        (chunk_text, "livebm25symbol", 8),
        (chunk_text, "fieldlengthpadding", 4),
        (chunk_text, "retired", 0),
        (chunk_text, "oldd", 0),
        (chunk_text, "oldb", 0),
        (chunk_text, "newb", 1),
        (symbol_name, "livebm25symbol", 4),
        (candidate_id, "chunk-oldb", 0),
        (candidate_id, "chunk-newb", 1),
    ] {
        assert_eq!(
            fresh_searcher.doc_freq(&tantivy::Term::from_field_text(field, term))?,
            df,
            "fixed fresh-native df for {term}",
        );
    }
    let raw = g3_native.reader()?;
    let raw_searcher = raw.searcher();
    assert_eq!(
        raw_searcher.doc_freq(&tantivy::Term::from_field_text(
            chunk_text,
            "livebm25needle"
        ))?,
        7,
        "three retired text documents each had needle, including the g3 doc with three copies",
    );

    // Inspect the committed F14 statistics independently of query scoring.
    // The g2 deaths and the g3 duplicate-token death must all survive in
    // their distinct per-segment correction rows.
    let g2_dir = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &second.repo_id,
        &second.revision_id,
    )
    .generation_dir(dir.path(), second.generation);
    let g2_encoded = std::fs::read(g2_dir.join("search-corpus-live-bm25.cbor"))?;
    let (g2_format, _, g2_live_docs, g2_segments): LiveStatsWire =
        ciborium::from_reader(g2_encoded.as_slice())?;
    assert_eq!(g2_format, 1);
    assert_eq!(g2_live_docs, 8);
    let g3_dir = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &third.repo_id,
        &third.revision_id,
    )
    .generation_dir(dir.path(), third.generation);
    let encoded = std::fs::read(g3_dir.join("search-corpus-live-bm25.cbor"))?;
    let (format, _, live_docs, stats_segments): LiveStatsWire =
        ciborium::from_reader(encoded.as_slice())?;
    assert_eq!(format, 1);
    assert_eq!(live_docs, 8);
    let native_schema = g3_native.schema();
    let fixed_tokens = [
        (native_schema.get_field("chunk_text")?.field_id(), 3295_u64),
        (native_schema.get_field("symbol_local_name")?.field_id(), 4),
        (native_schema.get_field("candidate_id")?.field_id(), 8),
        (
            native_schema.get_field("text_authority_doc_id")?.field_id(),
            4,
        ),
    ];
    for (field, expected_tokens) in fixed_tokens {
        let actual = stats_segments
            .iter()
            .flat_map(|(_, rows, _)| rows)
            .filter(|(id, _)| *id == field)
            .map(|(_, tokens)| *tokens)
            .sum::<u64>();
        assert_eq!(
            actual, expected_tokens,
            "fixed live token census for field {field}"
        );
    }
    let g2_chunk_tokens = g2_segments
        .iter()
        .flat_map(|(_, rows, _)| rows)
        .filter(|(field, _)| *field == chunk_text.field_id())
        .map(|(_, tokens)| *tokens)
        .sum::<u64>();
    assert_eq!(
        g2_chunk_tokens, 3169,
        "g2 has oldb with three needle tokens before replacement"
    );
    let needle_value = tantivy::Term::from_field_text(chunk_text, "livebm25needle")
        .serialized_value_bytes()
        .to_vec();
    let g2_dead_needle_df = g2_segments
        .iter()
        .flat_map(|(_, _, rows)| rows)
        .filter(|(field, value, _)| {
            *field == chunk_text.field_id() && value.as_slice() == needle_value.as_slice()
        })
        .map(|(_, _, count)| *count)
        .sum::<u64>();
    assert_eq!(
        g2_dead_needle_df, 2,
        "g2 must retain both first-generation dead needle documents"
    );
    let dead_needle_df = stats_segments
        .iter()
        .flat_map(|(_, _, rows)| rows)
        .filter(|(field, value, _)| {
            *field == chunk_text.field_id() && value.as_slice() == needle_value.as_slice()
        })
        .map(|(_, _, count)| *count)
        .sum::<u64>();
    assert_eq!(
        dead_needle_df, 3,
        "g2 dead docs plus g3 duplicate-token doc each subtract df once"
    );
    assert_eq!(
        raw_searcher.doc_freq(&tantivy::Term::from_field_text(
            chunk_text,
            "livebm25needle"
        ))? - dead_needle_df,
        4
    );

    let text_query = query("livebm25needle");
    let symbol_query = query("livebm25symbol");
    let text_rows = |view: &dyn LexicalSearcher| -> Result<Vec<(String, u32)>, Box<dyn Error>> {
        Ok(view
            .search(&text_query, 10, &budget)?
            .into_iter()
            .map(|row| (row.candidate_id, row.score.to_bits()))
            .collect())
    };
    let symbol_rows = |view: &dyn LexicalSearcher| -> Result<Vec<(String, u32)>, Box<dyn Error>> {
        Ok(view
            .search_symbols(&symbol_query, 10, &budget)?
            .into_iter()
            .map(|row| (row.candidate_id, row.score.to_bits()))
            .collect())
    };
    let expected_text = text_rows(expected.as_ref())?;
    let expected_symbol = symbol_rows(expected.as_ref())?;
    assert_eq!(
        expected_text
            .iter()
            .map(|row| row.0.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "chunk-keptc",
            "chunk-newd",
            "chunk-keepte",
            "chunk-newb"
        ]),
    );
    assert_eq!(
        expected_symbol
            .iter()
            .map(|row| row.0.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "symbol-keptc",
            "symbol-newd",
            "symbol-keepte",
            "symbol-newb"
        ]),
    );
    for (view, generation) in [
        (g3.as_ref(), third.generation),
        (cold_g3.as_ref(), third.generation),
        (g4.as_ref(), fourth.generation),
        (cold_g4.as_ref(), fourth.generation),
        (expected.as_ref(), rebuilt.generation),
    ] {
        assert_eq!(
            text_rows(view)?,
            expected_text,
            "g2/g3 deletion corrections changed text score bits/order"
        );
        assert_eq!(
            symbol_rows(view)?,
            expected_symbol,
            "g2/g3 deletion corrections changed symbol score bits/order"
        );
        assert_eq!(
            bm25_text_pages(view, generation, &text_query)?,
            expected_text
        );
        assert_eq!(
            bm25_symbol_pages(view, generation, &symbol_query)?,
            expected_symbol
        );
    }
    let mut duplicate_phrase = text_query.clone();
    duplicate_phrase.expr = LqExpr::Leaf(LqLeaf::Phrase("livebm25needle livebm25needle".into()));
    let expected_phrase =
        bm25_text_pages(expected.as_ref(), rebuilt.generation, &duplicate_phrase)?;
    assert_eq!(
        expected_phrase.len(),
        4,
        "all four final text scopes have repeated needle"
    );
    for (view, generation) in [
        (g3.as_ref(), third.generation),
        (cold_g3.as_ref(), third.generation),
        (g4.as_ref(), fourth.generation),
        (cold_g4.as_ref(), fourth.generation),
    ] {
        assert_eq!(
            bm25_text_pages(view, generation, &duplicate_phrase)?,
            expected_phrase
        );
    }
    Ok(())
}

#[test]
fn writer_admission_failure_leaves_only_discardable_unsealed_delta() -> TestResult {
    use quanta_index_core::{
        CoreError, GenerationIdentityValidatePort as _, IncompleteGenerationDiscardOutcomeV1,
        IncompleteGenerationDiscardPort as _, SealedGenerationScanPort as _, WriterAdmissionPort,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RefuseFirstWriterOpen {
        opens: AtomicUsize,
    }

    impl WriterAdmissionPort for RefuseFirstWriterOpen {
        fn admit_writer_open(&self) -> Result<(), CoreError> {
            if self.opens.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(CoreError::Storage(
                    "test: delta writer admission refused".into(),
                ));
            }
            Ok(())
        }
    }

    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let base_adapter = LexicalAdapter::with_state_root(root.clone());
    let kept_b = scored_file_scope("b.rs", "keptbmarker")?;
    let kept_c = scored_file_scope("c.rs", "keptcmarker")?;
    let base = batch(
        1,
        None,
        vec![
            scored_file_scope("a.rs", "retiredmarker")?,
            kept_b.clone(),
            kept_c.clone(),
        ],
    )?;
    let _stages = base_adapter.build_batch(&base)?;
    drop(base_adapter);

    let mut deleted = batch(2, Some(1), Vec::new())?;
    deleted.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    deleted.source_event.payload_sha256 = source_event_payload_sha256(&deleted)?;
    let target = quanta_index_core::domains::generation::GenerationStorageKeyV1::for_repo_revision(
        &deleted.repo_id,
        &deleted.revision_id,
    )
    .generation_dir(&root, deleted.generation);
    let admission = Arc::new(RefuseFirstWriterOpen {
        opens: AtomicUsize::new(0),
    });
    let gate: Arc<dyn WriterAdmissionPort> = admission.clone();
    let failing = LexicalAdapter::with_state_root(root.clone()).with_writer_admission(gate)?;
    let failed = failing.build_batch(&deleted);
    assert!(
        matches!(&failed, Err(CoreError::Storage(message))
            if message.contains("delta writer admission refused")),
        "the failure must occur at delta writer admission: {failed:?}"
    );
    assert_eq!(admission.opens.load(Ordering::SeqCst), 1);
    assert!(
        target.is_dir(),
        "the failed delta has an incomplete directory"
    );
    for seal_file in [
        "search-corpus-generation-manifest.cbor",
        "search-corpus-generation-identity.cbor",
    ] {
        assert!(
            !target.join(seal_file).exists(),
            "writer admission failure cannot publish {seal_file}"
        );
    }
    drop(failing);

    let restarted = LexicalAdapter::with_state_root(root);
    let identity = quanta_index_contract::GenerationSnapshot {
        repo_id: deleted.repo_id.clone(),
        revision_id: deleted.revision_id.clone(),
        track: quanta_index_contract::SearchPlaneTrackKind::Lexical,
        manifest_generation: deleted.generation,
        manifest_digest: deleted.manifest_digest.clone(),
    };
    let inventory = restarted.inventory_sealed_generations()?;
    assert!(
        inventory
            .sealed
            .iter()
            .any(|entry| entry.identity.manifest_generation == base.generation),
        "the base generation remains sealed in the boot inventory"
    );
    assert!(
        inventory
            .sealed
            .iter()
            .all(|entry| entry.identity != identity),
        "an unsealed delta cannot enter the boot sealed inventory"
    );
    assert!(restarted.validate_generation_identity(&identity).is_err());
    assert_eq!(
        restarted.discard_incomplete_generation(&identity)?,
        IncompleteGenerationDiscardOutcomeV1::Discarded
    );
    assert!(!target.exists());

    let _stages = restarted.build_batch(&deleted)?;
    let fresh_dir = tempfile::tempdir()?;
    let fresh = LexicalAdapter::with_state_root(fresh_dir.path().to_path_buf());
    let rebuilt = batch(2, None, vec![kept_b, kept_c])?;
    let _stages = fresh.build_batch(&rebuilt)?;
    let budget = RequestBudgetV1::unbounded();
    let recovered_view = restarted.open(
        &deleted.repo_id,
        &deleted.revision_id,
        deleted.generation,
        &budget,
    )?;
    let fresh_view = fresh.open(
        &rebuilt.repo_id,
        &rebuilt.revision_id,
        rebuilt.generation,
        &budget,
    )?;
    assert_eq!(
        recovered_view.source_file_coverage(),
        fresh_view.source_file_coverage()
    );
    let project = |rows: Vec<quanta_index_contract::LexicalCandidate>| {
        rows.into_iter()
            .map(|row| {
                (
                    row.repo_relative_path.as_str().to_string(),
                    row.candidate_id,
                    row.source.map(|source| source.source_sha256),
                    row.score.to_bits(),
                )
            })
            .collect::<Vec<_>>()
    };
    let recovered = project(recovered_view.search(&query("livebm25needle"), 10, &budget)?);
    let expected = project(fresh_view.search(&query("livebm25needle"), 10, &budget)?);
    assert_eq!(recovered.len(), 2);
    assert_eq!(
        recovered, expected,
        "recovered delta differs from fresh live source"
    );
    Ok(())
}

#[test]
fn same_path_sources_replace_and_tombstone_independently() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let first = file_scope("a.rs", "firstmarker")?;
    let mut other = file_scope("a.rs", "othermarker")?;
    other.coverage.source.file.source_repo_id = RepoId::new("other-source")?;
    let base = batch(1, None, vec![first, other])?;
    let _stages = adapter.build_batch(&base)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "freshmarker")?])?;
    let _stages = adapter.build_batch(&delta)?;
    assert_eq!(
        code_search_source_owners(&adapter, &delta, "firstmarker")?,
        Vec::<String>::new()
    );
    assert_eq!(
        code_search_source_owners(&adapter, &delta, "freshmarker")?,
        vec!["l2-mutation-repo"]
    );
    assert_eq!(
        code_search_source_owners(&adapter, &delta, "othermarker")?,
        vec!["other-source"]
    );
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
    let _stages = adapter.build_batch(&removed)?;
    assert_eq!(
        code_search_source_owners(&adapter, &removed, "freshmarker")?,
        Vec::<String>::new()
    );
    assert_eq!(
        code_search_source_owners(&adapter, &removed, "othermarker")?,
        vec!["other-source"]
    );
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
    let _stages = adapter.build_batch(&base)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "freshmarker")?])?;
    let _stages = adapter.build_batch(&delta)?;
    let view = adapter.open(
        &delta.repo_id,
        &delta.revision_id,
        delta.generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
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
    let full_stages = adapter
        .build_batch(&empty)?
        .ok_or("missing full-build timing")?;
    assert!(full_stages.prep_file_authority_preflight_ns.is_some());
    assert!(full_stages.prep_coverage_write_ns.is_some());
    assert!(full_stages.file_authority_source_write_ns.is_none());
    assert!(full_stages.seal_ns.is_some());
    assert!(full_stages.seal_writer_commit_ns.is_some());
    assert!(full_stages.seal_merge_wait_ns.is_some());
    assert!(full_stages.seal_file_admission_ns.is_some());
    assert!(full_stages.text_authority_collect_ns.is_none());
    assert!(full_stages.text_authority_shard_build_ns.is_none());
    assert!(full_stages.text_authority_publish_ns.is_none());
    let delta = batch(2, Some(1), Vec::new())?;
    let delta_stages = adapter.build_batch(&delta)?.ok_or("missing delta timing")?;
    assert!(delta_stages.prep_file_authority_preflight_ns.is_some());
    assert!(delta_stages.prep_coverage_write_ns.is_some());
    assert!(delta_stages.file_authority_source_write_ns.is_none());
    assert!(delta_stages.seal_file_admission_ns.is_some());
    assert!(delta_stages.text_authority_collect_ns.is_none());
    assert!(delta_stages.text_authority_shard_build_ns.is_none());
    assert!(delta_stages.text_authority_publish_ns.is_none());
    let view = adapter.open(
        &delta.repo_id,
        &delta.revision_id,
        delta.generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
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
fn nonempty_noop_delta_reopens_with_the_same_source_and_units_as_fresh_full() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let scopes = vec![
        file_scope("a.rs", "firstmarker")?,
        file_scope("b.rs", "secondmarker")?,
    ];
    let base = batch(1, None, scopes.clone())?;
    let _base_stages = adapter.build_batch(&base)?;
    let unchanged = batch(2, Some(1), Vec::new())?;
    let _delta_stages = adapter.build_batch(&unchanged)?;

    let fresh_dir = tempfile::tempdir()?;
    let fresh = LexicalAdapter::with_state_root(fresh_dir.path().to_path_buf());
    let rebuilt = batch(2, None, scopes)?;
    let _fresh_stages = fresh.build_batch(&rebuilt)?;

    let reopened = adapter.open(
        &unchanged.repo_id,
        &unchanged.revision_id,
        unchanged.generation,
        &RequestBudgetV1::unbounded(),
    )?;
    let expected = fresh.open(
        &rebuilt.repo_id,
        &rebuilt.revision_id,
        rebuilt.generation,
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(
        reopened.source_file_coverage(),
        expected.source_file_coverage(),
        "no-op delta must inherit the same independently rebuilt source universe"
    );
    assert_eq!(
        reopened.source_publication_event(),
        Some(&unchanged.source_event)
    );
    for marker in ["firstmarker", "secondmarker"] {
        let chunk = format!("chunk-{marker}");
        let symbol = format!("symbol-{marker}");
        assert_units(&adapter, &unchanged, marker, &[&chunk], &[&symbol])?;
        assert_units(&fresh, &rebuilt, marker, &[&chunk], &[&symbol])?;
    }
    assert_units(&adapter, &unchanged, "ghostmarker", &[], &[])?;
    Ok(())
}

#[test]
fn inherited_candidate_collision_refuses_before_target_creation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let base = batch(1, None, vec![file_scope("kept.rs", "keptmarker")?])?;
    let _stages = adapter.build_batch(&base)?;
    let mut replacement = file_scope("different.rs", "newmarker")?;
    replacement
        .chunks
        .first_mut()
        .ok_or("fixture chunk")?
        .chunk_id = ChunkId::new("chunk-keptmarker");
    replacement.coverage.unit_set_sha256 =
        source_file_unit_set_sha256(&replacement.chunks, &replacement.symbols)?;
    let delta = batch(2, Some(1), vec![replacement])?;
    let mut owner = quanta_index_core::PublicationValidationOwner::default();
    assert!(
        adapter
            .preflight_batch_with_owner(
                &delta,
                SearchCorpusPreflightPhaseV1::BeforeIntent,
                &mut owner
            )
            .is_err()
    );
    assert!(
        adapter
            .preflight_batch_with_owner(
                &delta,
                SearchCorpusPreflightPhaseV1::UnderOperationLock,
                &mut owner
            )
            .is_err()
    );
    assert!(adapter.build_batch_with_owner(&delta, &mut owner).is_err());
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
    let _stages = adapter.build_batch(&base)?;
    let request = batch(2, None, vec![file_scope("a.rs", "rebuiltmarker")?])?;
    let _stages = adapter.build_batch(&request)?;
    let mut other_event = request.clone();
    other_event.source_event.event_id = "different-event".into();
    assert!(
        adapter
            .preflight_batch(&other_event, SearchCorpusPreflightPhaseV1::BeforeIntent)
            .is_err()
    );

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
    adapter.preflight_batch(&request, SearchCorpusPreflightPhaseV1::BeforeIntent)?;
    assert_eq!(std::fs::read(&coverage_path)?, corrupt);
    assert!(adapter.build_batch(&request).is_err());
    assert!(
        adapter
            .open(
                &request.repo_id,
                &request.revision_id,
                request.generation,
                &quanta_index_core::RequestBudgetV1::unbounded()
            )
            .is_err()
    );

    let mut wrong_identity = request.clone();
    wrong_identity.manifest_digest = "different-manifest".into();
    assert!(matches!(
        adapter.preflight_batch(&wrong_identity, SearchCorpusPreflightPhaseV1::BeforeIntent),
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
        adapter.preflight_batch(&delta, SearchCorpusPreflightPhaseV1::BeforeIntent),
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
    let _stages = adapter.build_batch(&request)?;
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
    let _stages = adapter.build_batch(&base)?;
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
            .open(
                &base.repo_id,
                &base.revision_id,
                base.generation,
                &quanta_index_core::RequestBudgetV1::unbounded()
            )
            .is_err()
    );
    std::fs::remove_file(&path)?;
    assert!(
        adapter
            .open(
                &base.repo_id,
                &base.revision_id,
                base.generation,
                &quanta_index_core::RequestBudgetV1::unbounded()
            )
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
    let _stages = adapter.build_batch(&base)?;
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
    let pinned_old_reader = adapter.open(
        &base.repo_id,
        &base.revision_id,
        base.generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;

    // Preserve one publication proof owner across all existing refusal boundaries.
    let mut owner = quanta_index_core::PublicationValidationOwner::default();
    adapter.preflight_batch_with_owner(
        &delta,
        SearchCorpusPreflightPhaseV1::BeforeIntent,
        &mut owner,
    )?;
    if after_second_preflight {
        adapter.preflight_batch_with_owner(
            &delta,
            SearchCorpusPreflightPhaseV1::UnderOperationLock,
            &mut owner,
        )?;
    }
    let mut changed = original.clone();
    *changed.last_mut().ok_or("empty base coverage page")? ^= 1;
    std::fs::write(&page, changed)?;
    let refused = if after_second_preflight {
        adapter.build_batch_with_owner(&delta, &mut owner)
    } else {
        adapter
            .preflight_batch_with_owner(
                &delta,
                SearchCorpusPreflightPhaseV1::UnderOperationLock,
                &mut owner,
            )
            .map(|()| None)
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
    adapter.preflight_batch_with_owner(
        &delta,
        SearchCorpusPreflightPhaseV1::BeforeIntent,
        &mut owner,
    )?;
    let _stages = adapter.build_batch_with_owner(&delta, &mut owner)?;
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

fn owned_artifact_change_refuses_before_target(after_lock: bool) -> TestResult {
    for (family, fault) in [
        ("file", "tamper"),
        ("file", "missing"),
        ("file", "symlink"),
        ("file", "orphan"),
        ("text", "tamper"),
        ("text", "missing"),
        ("text", "symlink"),
        ("text", "orphan"),
        ("file-root", "tamper"),
        ("file-root", "missing"),
        ("file-root", "symlink"),
        ("file-root", "orphan"),
        ("text-manifest", "tamper"),
        ("text-manifest", "missing"),
        ("text-manifest", "symlink"),
        ("sealed-manifest", "tamper"),
        ("sealed-manifest", "missing"),
        ("sealed-manifest", "symlink"),
    ] {
        let dir = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
        let base = batch(
            1,
            None,
            vec![
                file_scope("a.rs", "oldmarker")?,
                file_scope("keep.rs", "keepmarker")?,
            ],
        )?;
        let _stages = adapter.build_batch(&base)?;
        let delta = batch(2, Some(1), vec![file_scope("a.rs", "newmarker")?])?;
        let family_key = quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
            &base.repo_id,
            &base.revision_id,
        );
        let base_dir = family_key.generation_dir(dir.path(), base.generation);
        let target = family_key.generation_dir(dir.path(), delta.generation);
        let artifact_dir = match family {
            "file" => base_dir.join("file-authority/objects"),
            "file-root" => base_dir.join("file-authority"),
            "sealed-manifest" => base_dir.clone(),
            "text" | "text-manifest" => base_dir.join("text-authority"),
            _ => return Err("unknown artifact family".into()),
        };
        let artifact = std::fs::read_dir(&artifact_dir)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                match family {
                    "file" => name.ends_with(".bin"),
                    "file-root" => name == "root.cbor",
                    "sealed-manifest" => name == "search-corpus-generation-manifest.cbor",
                    "text" => name.starts_with("shard-"),
                    "text-manifest" => name == "manifest.cbor",
                    _ => false,
                }
            })
            .ok_or("missing committed fixture artifact")?
            .path();
        let original = std::fs::read(&artifact)?;
        let modified = std::fs::metadata(&artifact)?.modified()?;
        let mut owner = quanta_index_core::PublicationValidationOwner::default();
        adapter.preflight_batch_with_owner(
            &delta,
            SearchCorpusPreflightPhaseV1::BeforeIntent,
            &mut owner,
        )?;
        if after_lock {
            adapter.preflight_batch_with_owner(
                &delta,
                SearchCorpusPreflightPhaseV1::UnderOperationLock,
                &mut owner,
            )?;
        }
        let extra = artifact_dir.join("unexpected.bin");
        match fault {
            "tamper" => {
                let mut changed = original.clone();
                *changed.last_mut().ok_or("empty fixture artifact")? ^= 1;
                std::fs::write(&artifact, changed)?;
                // Same inode, length and restored mtime cannot authorize reuse.
                std::fs::File::open(&artifact)?
                    .set_times(std::fs::FileTimes::new().set_modified(modified))?;
            }
            "missing" => std::fs::remove_file(&artifact)?,
            "symlink" => {
                let external = dir.path().join("external-original");
                std::fs::write(&external, &original)?;
                std::fs::remove_file(&artifact)?;
                std::os::unix::fs::symlink(external, &artifact)?;
            }
            "orphan" => std::fs::write(&extra, b"uncommitted")?,
            _ => return Err("unknown artifact fault".into()),
        }
        let refusal = if after_lock {
            adapter
                .build_batch_with_owner(&delta, &mut owner)
                .map(|_stages| ())
        } else {
            adapter.preflight_batch_with_owner(
                &delta,
                SearchCorpusPreflightPhaseV1::UnderOperationLock,
                &mut owner,
            )
        };
        let expected_code = if family == "sealed-manifest" && fault == "missing" {
            quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestMissing
        } else {
            quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt
        };
        assert!(
            matches!(refusal, Err(quanta_index_core::CoreError::Typed { code, .. }) if code == expected_code),
            "{family}/{fault} after_lock={after_lock}: {refusal:?}"
        );
        assert!(
            !target.exists(),
            "{family}/{fault} created a target before refusal"
        );
        assert!(
            adapter
                .open(
                    &base.repo_id,
                    &base.revision_id,
                    base.generation,
                    &RequestBudgetV1::unbounded()
                )
                .is_err(),
            "independent cold open accepted {family}/{fault}"
        );
        if fault == "orphan" {
            std::fs::remove_file(extra)?;
        } else {
            match artifact.symlink_metadata() {
                Ok(_) => std::fs::remove_file(&artifact)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            std::fs::write(&artifact, original)?;
        }
        adapter.preflight_batch_with_owner(
            &delta,
            SearchCorpusPreflightPhaseV1::BeforeIntent,
            &mut owner,
        )?;
        let _stages = adapter.build_batch_with_owner(&delta, &mut owner)?;
        assert_units(&adapter, &delta, "oldmarker", &[], &[])?;
        assert_units(
            &adapter,
            &delta,
            "newmarker",
            &["chunk-newmarker"],
            &["symbol-newmarker"],
        )?;
        assert_units(
            &adapter,
            &delta,
            "keepmarker",
            &["chunk-keepmarker"],
            &["symbol-keepmarker"],
        )?;
    }
    Ok(())
}

#[test]
fn owned_file_and_text_proofs_recheck_current_bytes_after_outer_preflight() -> TestResult {
    owned_artifact_change_refuses_before_target(false)
}

#[test]
fn owned_file_and_text_proofs_recheck_current_bytes_after_lock_and_retry() -> TestResult {
    owned_artifact_change_refuses_before_target(true)
}

#[test]
fn owned_delta_noop_and_delete_match_fresh_full_query_and_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let fresh_dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let fresh = LexicalAdapter::with_state_root(fresh_dir.path().to_path_buf());
    let base = batch(
        1,
        None,
        vec![
            file_scope("a.rs", "oldmarker")?,
            file_scope("keep.rs", "keepmarker")?,
        ],
    )?;
    let _stages = adapter.build_batch(&base)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "newmarker")?])?;
    let mut owner = quanta_index_core::PublicationValidationOwner::default();
    for phase in [
        SearchCorpusPreflightPhaseV1::BeforeIntent,
        SearchCorpusPreflightPhaseV1::UnderOperationLock,
    ] {
        adapter.preflight_batch_with_owner(&delta, phase, &mut owner)?;
    }
    let _stages = adapter.build_batch_with_owner(&delta, &mut owner)?;
    let full = batch(
        1,
        None,
        vec![
            file_scope("a.rs", "newmarker")?,
            file_scope("keep.rs", "keepmarker")?,
        ],
    )?;
    let _stages = fresh.build_batch(&full)?;
    let noop = batch(3, Some(2), Vec::new())?;
    let mut noop_owner = quanta_index_core::PublicationValidationOwner::default();
    assert!(
        adapter
            .preflight_batch_with_owner(
                &noop,
                SearchCorpusPreflightPhaseV1::BeforeIntent,
                &mut owner
            )
            .is_err(),
        "another publication borrowed the delta owner"
    );
    for phase in [
        SearchCorpusPreflightPhaseV1::BeforeIntent,
        SearchCorpusPreflightPhaseV1::UnderOperationLock,
    ] {
        adapter.preflight_batch_with_owner(&noop, phase, &mut noop_owner)?;
    }
    let _stages = adapter.build_batch_with_owner(&noop, &mut noop_owner)?;
    let reopened = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    for marker in ["newmarker", "keepmarker"] {
        let chunk = format!("chunk-{marker}");
        let symbol = format!("symbol-{marker}");
        assert_units(&adapter, &delta, marker, &[&chunk], &[&symbol])?;
        assert_units(&adapter, &noop, marker, &[&chunk], &[&symbol])?;
        assert_units(&reopened, &noop, marker, &[&chunk], &[&symbol])?;
        assert_units(&fresh, &full, marker, &[&chunk], &[&symbol])?;
    }
    assert_units(&reopened, &noop, "oldmarker", &[], &[])?;
    let mut deleted = batch(4, Some(3), Vec::new())?;
    deleted.tombstone_scopes.push(SearchCorpusTombstoneScope {
        file: SourceFileKey {
            source_repo_id: RepoId::new("l2-mutation-repo")?,
            repo_relative_path: RepoRelativePath::new("a.rs"),
        },
    });
    deleted.source_event.payload_sha256 = source_event_payload_sha256(&deleted)?;
    let mut delete_owner = quanta_index_core::PublicationValidationOwner::default();
    for phase in [
        SearchCorpusPreflightPhaseV1::BeforeIntent,
        SearchCorpusPreflightPhaseV1::UnderOperationLock,
    ] {
        adapter.preflight_batch_with_owner(&deleted, phase, &mut delete_owner)?;
    }
    let _stages = adapter.build_batch_with_owner(&deleted, &mut delete_owner)?;
    let fresh_deleted = batch(2, None, vec![file_scope("keep.rs", "keepmarker")?])?;
    let _stages = fresh.build_batch(&fresh_deleted)?;
    let cold_deleted = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    for (opened, generation) in [
        (&adapter, &deleted),
        (&cold_deleted, &deleted),
        (&fresh, &fresh_deleted),
    ] {
        assert_units(opened, generation, "newmarker", &[], &[])?;
        assert_units(opened, generation, "oldmarker", &[], &[])?;
        assert_units(
            opened,
            generation,
            "keepmarker",
            &["chunk-keepmarker"],
            &["symbol-keepmarker"],
        )?;
    }
    Ok(())
}

#[test]
fn repeated_delta_admission_reuses_decode_but_rehashes_all_coverage_pages() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let _stages = adapter.build_batch(&batch(1, None, vec![file_scope("a.rs", "oldmarker")?])?)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "newmarker")?])?;
    let before = adapter.coverage_read_stats()?;
    let before_phases = adapter.coverage_read_by_phase_stats()?;
    adapter.preflight_batch(&delta, SearchCorpusPreflightPhaseV1::BeforeIntent)?;
    let first = adapter.coverage_read_stats()?;
    assert_eq!(first.decodes, before.decodes + 1);
    assert_eq!(first.rows, before.rows + 1);
    adapter.preflight_batch(&delta, SearchCorpusPreflightPhaseV1::UnderOperationLock)?;
    let second = adapter.coverage_read_stats()?;
    assert_eq!(second.decodes, first.decodes);
    assert_eq!(second.rows, first.rows);
    assert_eq!(
        second.page_bytes - first.page_bytes,
        first.page_bytes - before.page_bytes
    );
    assert_eq!(second.pages - first.pages, first.pages - before.pages);
    let _stages = adapter.build_batch(&delta)?;
    let built = adapter.coverage_read_stats()?;
    assert_eq!(built.decodes, first.decodes);
    assert_eq!(built.rows, first.rows);
    assert_eq!(
        built.page_bytes - second.page_bytes,
        first.page_bytes - before.page_bytes
    );
    let phases = adapter.coverage_read_by_phase_stats()?;
    assert_eq!(phases.total, built);
    assert_eq!(
        phases.before_intent.page_bytes - before_phases.before_intent.page_bytes,
        first.page_bytes - before.page_bytes
    );
    assert_eq!(
        phases.under_operation_lock.page_bytes - before_phases.under_operation_lock.page_bytes,
        second.page_bytes - first.page_bytes
    );
    assert_eq!(
        phases.build.page_bytes - before_phases.build.page_bytes,
        built.page_bytes - second.page_bytes
    );
    assert_units(
        &adapter,
        &delta,
        "newmarker",
        &["chunk-newmarker"],
        &["symbol-newmarker"],
    )?;
    let opened = adapter.coverage_read_by_phase_stats()?;
    assert!(opened.open.page_bytes > phases.open.page_bytes);
    assert_eq!(opened.total, adapter.coverage_read_stats()?);
    Ok(())
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
    let _stages = adapter.build_batch(&base)?;
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
    let _stages = adapter.build_batch(&base)?;
    let delta = batch(2, Some(1), vec![file_scope("a.rs", "freshmarker")?])?;
    let _stages = adapter.build_batch(&delta)?;
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
            .open(
                &base.repo_id,
                &base.revision_id,
                base.generation,
                &quanta_index_core::RequestBudgetV1::unbounded()
            )
            .is_err()
    );
    let reopened = adapter.open(
        &delta.repo_id,
        &delta.revision_id,
        delta.generation,
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
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
