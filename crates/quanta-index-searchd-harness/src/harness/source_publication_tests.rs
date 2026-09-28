// These test assertions intentionally panic to preserve the failing fixture context.
#![allow(
    clippy::panic_in_result_fn,
    clippy::indexing_slicing,
    reason = "fixture assertions intentionally fail by panic"
)]

use super::*;
use quanta_index_contract::lex::{SymbolKindCode, SymbolRelationship, SymbolSpan};
use quanta_index_contract::{BatchPublishReceipt, ChunkId, RepoRelativePath, SymbolId};

fn repo() -> Result<RepoId> {
    Ok(RepoId::new("fixture-repo")?)
}
fn chunk(id: &str, text: &str, source_repo: Option<RepoId>) -> Result<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new("src/a.py"),
        language: LanguageCode::new("python").map_err(anyhow::Error::msg)?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 1,
        text: text.into(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: source_repo,
    })
}
fn symbol(id: &str) -> Result<SymbolRecord> {
    Ok(SymbolRecord {
        symbol_id: SymbolId::new(id),
        repo_relative_path: RepoRelativePath::new("src/a.py"),
        language: LanguageCode::new("python").map_err(anyhow::Error::msg)?,
        symbol_kind: SymbolKindCode::new("function").map_err(anyhow::Error::msg)?,
        symbol_kind_family: None,
        local_name: "func".into(),
        qualified_name: "pkg.func".into(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: "src/a.py".into(),
            byte_start: 0,
            byte_end: 4,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: None,
        relationship: SymbolRelationship::Def,
    })
}
fn batch(state: &CorpusPublicationState, generation: u64) -> Result<SearchCorpusIngestBatch> {
    state.build_batch(
        repo()?,
        RevisionId::new("fixture-revision")?,
        ManifestGeneration::new(generation),
        if generation == 1 {
            BatchIngestMode::ReplaceGeneration
        } else {
            BatchIngestMode::Delta
        },
        (generation > 1).then(|| ManifestGeneration::new(generation.saturating_sub(1))),
        format!("fixture-event-{generation}"),
    )
}
fn receipt(batch: &SearchCorpusIngestBatch) -> Result<SearchCorpusPublishOutcome> {
    Ok(SearchCorpusPublishOutcome {
        publication: SourcePublicationBinding::for_batch(batch),
        observation: None,
        receipt: BatchPublishReceipt {
            generation: batch.generation,
            manifest_digest: Some(batch.manifest_digest.clone()),
            batch_digest: batch.batch_digest.clone(),
            accepted_replace_scopes: u32::try_from(batch.replace_scopes.len())?,
            accepted_tombstone_scopes: u32::try_from(batch.tombstone_scopes.len())?,
            accepted_semantic_replace_scopes: u32::try_from(batch.semantic_replace_scopes.len())?,
            accepted_semantic_tombstone_scopes: u32::try_from(
                batch.semantic_tombstone_scopes.len(),
            )?,
            accepted_clear_surfaces: 0,
            sealed: true,
            applied: true,
            durable_sequence: 1,
            semantic_content: None,
        },
    })
}

#[test]
fn file_coverage_keeps_all_chunks_foreign_sources_and_appended_symbols() -> Result<()> {
    let mut state = CorpusPublicationState::default();
    state.replace_chunks(
        &repo()?,
        vec![
            chunk("one", "alpha", None)?,
            chunk("two", "beta", None)?,
            chunk("foreign", "other", Some(RepoId::new("foreign-repo")?))?,
        ],
    )?;
    state.add_symbol(repo()?, symbol("symbol-one")?)?;
    let batch = batch(&state, 1)?;
    batch.validate_v1()?;
    batch.validate_surface_mutations_v1()?;
    assert_eq!(batch.replace_scopes.len(), 2);
    let own_repo = repo()?;
    let own = batch
        .replace_scopes
        .iter()
        .find(|scope| scope.coverage.source.file.source_repo_id == own_repo)
        .ok_or_else(|| anyhow!("own source missing"))?;
    assert_eq!(own.chunks.len(), 2);
    assert_eq!(own.symbols.len(), 1);
    assert_eq!(
        own.coverage.symbols,
        SymbolCoverage::Complete { symbol_count: 1 }
    );
    assert_eq!(
        own.coverage.source.source_sha256,
        <[u8; 32]>::from(Sha256::digest(b"alpha\nbeta\nfunc\n"))
    );
    assert_eq!((own.chunks[0].start_byte, own.chunks[0].end_byte), (0, 5));
    assert_eq!((own.chunks[1].start_byte, own.chunks[1].end_byte), (6, 10));
    assert!(
        batch
            .replace_scopes
            .iter()
            .any(|scope| scope.coverage.symbols == SymbolCoverage::NotRequested)
    );
    Ok(())
}

#[test]
fn symbol_staged_before_text_survives_file_replacement() -> Result<()> {
    let mut state = CorpusPublicationState::default();
    state.add_symbol(repo()?, symbol("symbol-first")?)?;
    state.replace_chunks(&repo()?, vec![chunk("chunk-later", "alpha", None)?])?;
    let publication = batch(&state, 1)?;
    publication.validate_v1()?;
    publication.validate_surface_mutations_v1()?;
    let file = publication
        .replace_scopes
        .first()
        .ok_or_else(|| anyhow!("missing file"))?;
    assert_eq!(file.chunks.len(), 1);
    assert_eq!(file.symbols.len(), 1);
    assert_eq!(file.symbols[0].symbol_id, SymbolId::new("symbol-first"));
    Ok(())
}

#[test]
fn symbol_after_explicit_delete_does_not_restore_committed_text() -> Result<()> {
    let mut state = CorpusPublicationState::default();
    state.replace_chunks(&repo()?, vec![chunk("old-chunk", "alpha", None)?])?;
    let first = batch(&state, 1)?;
    state.accept(&first, &receipt(&first)?)?;
    state.finish_frozen();
    state.activate(
        &first.repo_id,
        &first.revision_id,
        first.generation,
        &first.manifest_digest,
    )?;
    state.delete_path(repo()?, "src/a.py")?;
    state.add_symbol(repo()?, symbol("new-symbol")?)?;
    let second = batch(&state, 2)?;
    second.validate_v1()?;
    second.validate_surface_mutations_v1()?;
    let file = second
        .replace_scopes
        .first()
        .ok_or_else(|| anyhow!("missing successor file"))?;
    assert!(file.chunks.is_empty());
    assert_eq!(file.symbols.len(), 1);
    assert_eq!(second.semantic_tombstone_scopes.len(), 1);
    Ok(())
}

#[test]
fn seal_failure_keeps_original_event_and_activation_alone_advances_source_base() -> Result<()> {
    let mut state = CorpusPublicationState::default();
    state.replace_chunks(&repo()?, vec![chunk("one", "alpha", None)?])?;
    let original = batch(&state, 1)?;
    state.freeze(original.clone());
    assert!(state.set_metadata(vec![1]).is_err());
    assert_eq!(state.frozen(), Some(&original));
    let mut wrong = receipt(&original)?;
    wrong.receipt.accepted_replace_scopes += 1;
    assert!(state.accept(&original, &wrong).is_err());
    assert!(state.sealed_events.is_empty());
    assert_eq!(state.frozen(), Some(&original));
    state.accept(&original, &receipt(&original)?)?;
    state.finish_frozen();
    assert!(state.active_events.is_empty());
    state.observe_accepted_activation(
        &RepoId::new("foreign-container")?,
        &original.revision_id,
        original.generation,
        &original.manifest_digest,
    );
    assert!(
        state.active_events.is_empty(),
        "a matching generation/manifest in a foreign repository is not this source event"
    );
    assert!(
        state
            .activate(
                &original.repo_id,
                &original.revision_id,
                original.generation,
                "wrong-manifest"
            )
            .is_err()
    );
    state.activate(
        &original.repo_id,
        &original.revision_id,
        original.generation,
        &original.manifest_digest,
    )?;
    let successor = batch(&state, 2)?;
    assert_eq!(
        successor.source_event.expected_base_event_id.as_deref(),
        Some("fixture-event-1")
    );
    assert_ne!(
        successor.source_event.event_id,
        original.source_event.event_id
    );
    Ok(())
}

#[test]
fn deleting_a_path_retracts_each_source_and_each_old_semantic_unit() -> Result<()> {
    let mut state = CorpusPublicationState::default();
    state.replace_chunks(
        &repo()?,
        vec![
            chunk("one", "alpha", None)?,
            chunk("two", "beta", None)?,
            chunk("foreign", "other", Some(RepoId::new("foreign-repo")?))?,
        ],
    )?;
    let original = batch(&state, 1)?;
    state.accept(&original, &receipt(&original)?)?;
    state.finish_frozen();
    state.activate(
        &original.repo_id,
        &original.revision_id,
        original.generation,
        &original.manifest_digest,
    )?;
    state.delete_path(repo()?, "src/a.py")?;
    let successor = batch(&state, 2)?;
    assert!(successor.replace_scopes.is_empty());
    assert_eq!(successor.tombstone_scopes.len(), 2);
    assert_eq!(successor.semantic_tombstone_scopes.len(), 3);
    successor.validate_v1()?;
    successor.validate_surface_mutations_v1()?;
    Ok(())
}
#[test]
fn standalone_delta_preserves_prior_semantic_removal_evidence() -> Result<()> {
    let mut state = CorpusPublicationState::default();
    state.replace_chunks(
        &repo()?,
        vec![
            chunk("old-one", "old", None)?,
            chunk("old-two", "also old", None)?,
        ],
    )?;
    let first = batch(&state, 1)?;
    state.accept(&first, &receipt(&first)?)?;
    state.finish_frozen();
    state.activate(
        &first.repo_id,
        &first.revision_id,
        first.generation,
        &first.manifest_digest,
    )?;
    let mut standalone = state.empty_successor();
    standalone.replace_chunks(&repo()?, vec![chunk("new-one", "new", None)?])?;
    let replacement = batch(&standalone, 2)?;
    assert_eq!(replacement.replace_scopes.len(), 1);
    assert_eq!(replacement.semantic_replace_scopes.len(), 1);
    assert_eq!(replacement.semantic_tombstone_scopes.len(), 2);
    assert_eq!(
        replacement.source_event.expected_base_event_id,
        Some(first.source_event.event_id)
    );
    replacement.validate_surface_mutations_v1()?;
    Ok(())
}

#[test]
fn retargeted_replay_keeps_original_publication_and_cannot_rewind_source_parent() -> Result<()> {
    let mut state = CorpusPublicationState::default();
    state.replace_chunks(&repo()?, vec![chunk("one", "alpha", None)?])?;
    let first = batch(&state, 1)?;
    state.accept(&first, &receipt(&first)?)?;
    state.finish_frozen();
    state.activate(
        &first.repo_id,
        &first.revision_id,
        first.generation,
        &first.manifest_digest,
    )?;
    state.replace_chunks(&repo()?, vec![chunk("two", "beta", None)?])?;
    let second = batch(&state, 2)?;
    state.accept(&second, &receipt(&second)?)?;
    state.finish_frozen();
    state.activate(
        &second.repo_id,
        &second.revision_id,
        second.generation,
        &second.manifest_digest,
    )?;
    let mut requested = first.clone();
    requested.generation = ManifestGeneration::new(7);
    requested.manifest_digest = "retargeted-original-event".to_string();
    quanta_index_ipc::stamp_batch_digest_v1(&mut requested)?;
    let mut replay = receipt(&first)?;
    replay.receipt.applied = false;
    state.accept(&requested, &replay)?;
    assert!(!state.sealed_events.contains_key(&(
        requested.repo_id.clone(),
        requested.revision_id.clone(),
        requested.generation
    )));
    state.observe_accepted_activation(
        &first.repo_id,
        &first.revision_id,
        first.generation,
        &first.manifest_digest,
    );
    state.activate(
        &first.repo_id,
        &first.revision_id,
        first.generation,
        &first.manifest_digest,
    )?;
    assert_eq!(
        batch(&state, 3)?.source_event.expected_base_event_id,
        Some(second.source_event.event_id)
    );
    Ok(())
}

#[test]
fn transport_stamping_does_not_repair_a_mutated_source_event() -> Result<()> {
    let mut state = CorpusPublicationState::default();
    state.replace_chunks(&repo()?, vec![chunk("one", "alpha", None)?])?;
    let mut changed = batch(&state, 1)?;
    let original_event = changed.source_event.clone();
    changed.replace_scopes[0].chunks[0].text = "tampered".into();
    let request = super::super::stamped_ingest_request(
        quanta_index_contract::SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(changed),
    )?;
    let quanta_index_contract::SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(changed) =
        request
    else {
        return Err(anyhow!("unexpected request variant"));
    };
    assert_eq!(changed.source_event, original_event);
    assert!(changed.validate_v1().is_err());
    Ok(())
}

#[test]
fn accepted_raw_publication_supplies_later_delta_removal_baseline() -> Result<()> {
    let mut producer = CorpusPublicationState::default();
    producer.replace_chunks(
        &repo()?,
        vec![
            chunk("old-a", "alpha", None)?,
            chunk("old-b", "beta", None)?,
        ],
    )?;
    let original = batch(&producer, 1)?;
    let mut observer = CorpusPublicationState::default();
    observer.ensure_publishable(&original)?;
    observer.accept(&original, &receipt(&original)?)?;
    observer.activate(
        &original.repo_id,
        &original.revision_id,
        original.generation,
        &original.manifest_digest,
    )?;
    observer.replace_chunks(&repo()?, vec![chunk("new", "new", None)?])?;
    let delta = batch(&observer, 2)?;
    assert_eq!(delta.semantic_tombstone_scopes.len(), 2);
    assert_eq!(
        delta.source_event.expected_base_event_id,
        Some(original.source_event.event_id.clone())
    );
    assert!(
        observer.ensure_publishable(&delta).is_err(),
        "raw publish cannot overwrite the staged delta"
    );
    observer.freeze(delta.clone());
    observer.ensure_publishable(&delta)?;
    assert!(
        observer.ensure_publishable(&original).is_err(),
        "a frozen failed generation cannot publish a different event"
    );
    Ok(())
}

#[test]
fn explicit_semantic_fixture_is_preserved_and_owns_complete_pending_corpus() -> Result<()> {
    let chunks = vec![chunk("one", "alpha", None)?];
    let mut semantic = super::super::semantic_source_scopes_for_chunk_records(&chunks);
    semantic[0].scope_digest = "explicit-test-owner".into();
    let mut state = CorpusPublicationState::default();
    state.stage_fixture(&repo()?, chunks.clone(), Vec::new(), semantic.clone())?;
    assert!(state.replace_chunks(&repo()?, chunks.clone()).is_err());
    assert!(
        state
            .stage_fixture(&repo()?, chunks, Vec::new(), Vec::new())
            .is_err()
    );
    state.set_metadata(vec![1, 2])?;
    let issued = batch(&state, 1)?;
    assert_eq!(issued.semantic_replace_scopes, semantic);
    assert_eq!(issued.bundle_payload, Some(vec![1, 2]));
    Ok(())
}
