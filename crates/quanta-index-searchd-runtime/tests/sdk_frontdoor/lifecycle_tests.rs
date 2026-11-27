use super::*;

use quanta_index_contract::{
    CandidateCountV1, IngestObservationStatus, SymbolId, SymbolQueryResponse, TextQueryResponse,
    TextRankUnit,
};
use quanta_index_core::GenerationStorageKeyV1;

#[test]
fn sdk_search_corpus_frontdoor_promotes_composite_generation_identity() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let initial_status = client.generations().status(repo(), revision())?;
    if initial_status.repo_id != repo()
        || initial_status.revision_id != revision()
        || !initial_status.tracks.is_empty()
    {
        return Err(format!("unexpected initial generation status: {initial_status:?}").into());
    }

    let not_ready = expect_sdk_error(
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Lexical),
        "generation current before activation should fail closed",
    )?;
    expect_remote_code(not_ready, "NOT_READY")?;

    let corpus_batch = lexical_batch()?;
    let composite_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    if composite_active.lexical.repo_id != repo()
        || composite_active.lexical.revision_id != revision()
        || composite_active.lexical.track != SearchPlaneTrackKind::Lexical
        || composite_active.semantic.track != SearchPlaneTrackKind::Semantic
        || composite_active.lexical.manifest_generation != generation()
        || composite_active.lexical.manifest_digest != corpus_batch.manifest_digest()
        || composite_active.semantic.manifest_generation != generation()
        || composite_active.semantic.manifest_digest != corpus_batch.manifest_digest()
    {
        return Err(
            format!("unexpected composite activation identity: {composite_active:?}").into(),
        );
    }

    let lexical_snapshot = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Lexical)
    })?;
    if lexical_snapshot.repo_id != repo()
        || lexical_snapshot.revision_id != revision()
        || lexical_snapshot.track != SearchPlaneTrackKind::Lexical
        || lexical_snapshot.manifest_generation != generation()
        || lexical_snapshot.manifest_digest != corpus_batch.manifest_digest()
    {
        return Err(format!("unexpected lexical generation snapshot: {lexical_snapshot:?}").into());
    }

    let final_status = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client.generations().status(repo(), revision())
    })?;
    if final_status.repo_id != repo() || final_status.revision_id != revision() {
        return Err(format!("unexpected final generation status: {final_status:?}").into());
    }
    match final_status.tracks.as_slice() {
        [lexical, semantic]
            if lexical.track == SearchPlaneTrackKind::Lexical
                && lexical.manifest_digest == corpus_batch.manifest_digest()
                && semantic.track == SearchPlaneTrackKind::Semantic
                && semantic.manifest_digest == corpus_batch.manifest_digest() => {}
        _ => {
            return Err(format!("unexpected final generation track set: {final_status:?}").into());
        }
    }

    fixture.stop()
}

#[test]
fn sdk_tombstone_only_generation_replaces_active_composite_and_removes_both_query_views()
-> TestResult {
    let root = private_tempdir()?;
    let fixture = SdkFrontdoorRuntime::start_at(root.path())?;
    let client = &fixture.client;

    let first_generation = lexical_batch()?;
    let _first_active = publish_and_activate_sdk_search_corpus(client, &first_generation)?;

    let _semantic_before_tombstone = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .semantic()
                .query()
                .text("quartz")
                .active(repo(), revision())
                .top_k(3)
                .execute()
        },
        |response| {
            response.generation == pin()
                && response
                    .results
                    .iter()
                    .any(|candidate| candidate.candidate_id == "alpha")
        },
    )?;
    let expected_active = current_sdk_search_corpus_or_none(client, repo(), revision())?
        .ok_or("first composite generation did not become active")?;
    require_independent_chunk_clear_refusal(client, root.path(), &expected_active)?;

    let removed_scope = SourceFileKey {
        source_repo_id: repo(),
        repo_relative_path: RepoRelativePath::new("src/alpha.rs"),
    };
    let tombstone_only = SearchCorpusBatch::delta(
        repo(),
        revision(),
        generation_two(),
        generation(),
        "manifest:lexical-tombstone-only",
    )
    .source_event(lexical_event(
        "fixture:sdk-lexical-tombstone-v2",
        Some("fixture:sdk-lexical-v1"),
    ))
    .tombstone_scope(removed_scope)
    .tombstone_semantic_scope(
        first_generation
            .semantic_replace_scopes()
            .iter()
            .find(|scope| scope.scope.owner_id == "alpha")
            .ok_or_else(|| "first generation missing alpha semantic source".to_string())?
            .scope
            .clone(),
    );

    let (receipt, activation) = client
        .search_corpus()
        .publish_and_activate(&tombstone_only, Some(expected_active))?;
    if !receipt.sealed
        || receipt.generation != generation_two()
        || receipt.manifest_digest.as_deref() != Some(tombstone_only.manifest_digest())
        || receipt.accepted_replace_scopes != 0
        || receipt.accepted_tombstone_scopes != 1
        || receipt.accepted_semantic_tombstone_scopes != 1
        || activation.active.generation.lexical.manifest_generation != generation_two()
        || activation.active.generation.semantic.manifest_generation != generation_two()
        || activation.active.generation.lexical.manifest_digest != tombstone_only.manifest_digest()
        || activation.active.generation.semantic.manifest_digest != tombstone_only.manifest_digest()
    {
        return Err(format!(
            "unexpected tombstone-only sealed composite promotion: receipt={receipt:?} activation={activation:?}"
        )
        .into());
    }

    let lexical = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .native("sphinx")
                .active(repo(), revision())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin_two(),
    )?;
    if lexical.generation != pin_two()
        || !matches!(lexical.results.as_slice(), [row] if row.candidate_id == "beta")
    {
        return Err(format!(
            "tombstone-only lexical generation must retain only beta for sphinx: {lexical:?}"
        )
        .into());
    }

    let semantic = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .semantic()
                .query()
                .text("quartz")
                .active(repo(), revision())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin_two(),
    )?;
    if semantic.generation != pin_two()
        || semantic
            .results
            .iter()
            .any(|candidate| candidate.candidate_id == "alpha")
    {
        return Err(format!(
            "tombstone-only semantic generation retained removed alpha scope: {semantic:?}"
        )
        .into());
    }

    let retained_text = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        pinned_text_query(client, "sphinx", pin())
    })?;
    let retained_semantic = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .semantic()
            .query()
            .text("quartz")
            .pinned(pin())
            .top_k(3)
            .execute()
    })?;
    let retained_ids = retained_text
        .results
        .iter()
        .map(|row| row.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    if retained_text.generation != pin()
        || retained_text.results.len() != 2
        || retained_ids != BTreeSet::from(["alpha", "beta"])
        || retained_semantic.generation != pin()
        || !retained_semantic
            .results
            .iter()
            .any(|row| row.candidate_id == "alpha")
    {
        return Err(format!(
            "the original pinned view lost alpha: text={retained_text:?} semantic={retained_semantic:?}"
        )
        .into());
    }

    for pinned in [pin_two(), pin()] {
        let symbols = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
            pinned_symbol_query(client, "MySdkSymbol", pinned.clone(), 3)
        })?;
        if symbols.generation != pinned
            || !matches!(symbols.results.as_slice(), [row]
                if row.candidate_id == "sym-sdk" && row.repo_relative_path.as_str() == "src/lib.rs")
        {
            return Err(
                format!("the unrelated symbol must survive in {pinned:?}: {symbols:?}").into(),
            );
        }
    }

    fixture.stop()
}

fn require_independent_chunk_clear_refusal(
    client: &QuantaIndex,
    state_root: &Path,
    active: &SearchCorpusActiveHeadV1,
) -> TestResult {
    let clear = SearchCorpusBatch::delta(
        repo(),
        revision(),
        generation_two(),
        generation(),
        "manifest:lexical-independent-chunk-clear",
    )
    .source_event(lexical_event(
        "fixture:sdk-lexical-independent-chunk-clear-v2",
        Some("fixture:sdk-lexical-v1"),
    ))
    .clear_surface(SearchScopeSurface::Chunk);
    let refusal = expect_sdk_error(
        client.search_corpus().publish_outcome(&clear),
        "independent Chunk clear",
    )?;
    if !matches!(&refusal, SdkError::Remote { code: SearchPlaneErrorCodeV2::InvalidRequest, message, .. }
        if message.contains("forbid independent Chunk/Symbol clear"))
    {
        return Err(format!("unexpected independent Chunk clear refusal: {refusal:?}").into());
    }
    if current_sdk_search_corpus_or_none(client, repo(), revision())?.as_ref() != Some(active) {
        return Err("independent Chunk clear changed the active head".into());
    }

    let state_root = state_root.canonicalize()?;
    let storage_key = GenerationStorageKeyV1::for_repo_revision(&repo(), &revision());
    for track in ["indexes/lexical", "indexes/semantic"] {
        let track_root = state_root.join(track);
        let base = storage_key.generation_dir(&track_root, generation());
        if !base.is_dir() {
            return Err(format!("fixture base directory is missing: {}", base.display()).into());
        }
        let target = storage_key.generation_dir(&track_root, generation_two());
        match target.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => {
                return Err(format!("independent Chunk clear created {}", target.display()).into());
            }
        }
    }
    Ok(())
}

#[test]
fn symbol_content_authority_shapes_fail_closed_on_initial_and_replayed_batches() -> TestResult {
    let root = private_tempdir()?;
    let fixture = SdkFrontdoorRuntime::start_at(root.path())?;
    let client = &fixture.client;
    let batch = symbol_content_source_batch()?;
    let (initial, activation, _timings) = client
        .search_corpus()
        .publish_and_activate_observed(&batch, None)?;
    if !initial.receipt.applied
        || !initial.receipt.sealed
        || initial.receipt.durable_sequence == 0
        || initial.observation.as_ref().map(|value| value.status)
            != Some(IngestObservationStatus::Executed)
    {
        return Err(format!("Symbol fixture did not execute and seal durably: {initial:?}").into());
    }
    verify_symbol_content_shapes(client, "initial publication")?;

    let replay = client.producer().publish_search_corpus_observed(&batch)?;
    if replay.publication != initial.publication
        || replay.receipt != initial.receipt.replayed()
        || replay.observation.as_ref().map(|value| value.status)
            != Some(IngestObservationStatus::Replayed)
        || current_sdk_search_corpus_or_none(client, repo(), revision())?.as_ref()
            != Some(&activation.active)
    {
        return Err(
            format!("Symbol fixture replay changed publication authority: {replay:?}").into(),
        );
    }
    // The initial query populated the resident snapshot. Reopen the replayed
    // publication in a fresh process so that view cannot satisfy the oracle.
    fixture.stop()?;
    let reopened = SdkFrontdoorRuntime::start_at(root.path())?;
    let client = &reopened.client;
    let active = current_sdk_search_corpus_or_none(client, repo(), revision())?;
    if active.as_ref() != Some(&activation.active) {
        return Err(format!(
            "reopening the replayed Symbol publication changed the active head: expected {:?}, got {active:?}",
            activation.active
        )
        .into());
    }
    verify_symbol_content_shapes(client, "cold publication replay")?;
    reopened.stop()
}

fn symbol_content_source_batch() -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let mut batch = lexical_batch()?;
    for (id, path, module) in [
        ("sym-worker", "src/worker.rs", "worker"),
        ("sym-peer", "src/peer.rs", "peer"),
    ] {
        let key = SourceFileKey {
            source_repo_id: repo(),
            repo_relative_path: RepoRelativePath::new(path),
        };
        let mut symbol = symbol_record()?;
        symbol.symbol_id = SymbolId::new(id);
        symbol.repo_relative_path = key.repo_relative_path.clone();
        symbol.definition_span.path = path.into();
        symbol.qualified_name = format!("{module}::MySdkSymbol").into_boxed_str();
        symbol.container_qualified_name = Some(module.into());
        let scope = fixture_source_scope_v1(key, revision(), Vec::new(), vec![symbol])?;
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    Ok(batch)
}

fn verify_symbol_content_shapes(client: &QuantaIndex, phase: &str) -> TestResult {
    let expected_symbols = BTreeSet::from([
        ("sym-sdk", "src/lib.rs"),
        ("sym-worker", "src/worker.rs"),
        ("sym-peer", "src/peer.rs"),
    ]);
    for query in [
        "MySdkSymbol",
        "MySdkSymbol content:MySdkSymbol",
        r#"MySdkSymbol AND repo.has.content("sphinx riddles")"#,
    ] {
        let response = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
            pinned_symbol_query(client, query, pin(), 3)
        })?;
        let symbols = response
            .results
            .iter()
            .map(|row| (row.candidate_id.as_str(), row.repo_relative_path.as_str()))
            .collect::<BTreeSet<_>>();
        if response.generation != pin()
            || symbols != expected_symbols
            || response.window.candidate_count() != CandidateCountV1::Exact(3)
            || response.next_cursor.is_some()
        {
            return Err(
                format!("{phase} {query:?}: unexpected Symbol window: {response:?}").into(),
            );
        }
    }
    // With three matches and top_k=1, the ordinary probe can show two rows.
    // count:all must prove the full total while keeping the page capped.
    for (query, expected_count) in [
        (
            "MySdkSymbol content:MySdkSymbol",
            CandidateCountV1::AtLeast(2),
        ),
        ("count:all MySdkSymbol", CandidateCountV1::Exact(3)),
        (
            "count:all MySdkSymbol content:MySdkSymbol",
            CandidateCountV1::Exact(3),
        ),
    ] {
        let page = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
            pinned_symbol_query(client, query, pin(), 1)
        })?;
        if page.generation != pin()
            || !matches!(page.results.as_slice(), [row] if expected_symbols.contains(
                &(row.candidate_id.as_str(), row.repo_relative_path.as_str())))
            || page.window.candidate_count() != expected_count
            || page.window.has_more() != Some(true)
            || page.next_cursor.is_none()
        {
            return Err(format!(
                "{phase} {query:?}: expected {expected_count:?} in one Symbol page: {page:?}"
            )
            .into());
        }
    }

    let same_file = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        pinned_symbol_query(
            client,
            r#"MySdkSymbol AND file.contains("fn main")"#,
            pin(),
            3,
        )
    })?;
    assert_single_symbol_candidate(&same_file)
        .map_err(|error| format!("{phase} file.contains in the Symbol file: {error}"))?;
    for query in [
        "MySdkSymbol content:absent_content_gate",
        "MySdkSymbol AND repo.has.content(absent_content_gate)",
        r#"MySdkSymbol AND file.contains("sphinx riddles")"#,
    ] {
        let response = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
            pinned_symbol_query(client, query, pin(), 3)
        })?;
        if response.generation != pin()
            || !response.results.is_empty()
            || response.window.candidate_count() != CandidateCountV1::Exact(0)
            || response.next_cursor.is_some()
        {
            return Err(format!(
                "{phase} {query:?}: expected an empty Symbol window: {response:?}"
            )
            .into());
        }
    }
    for projection in ["type:symbol", "select:symbol"] {
        let query = format!("{projection} MySdkSymbol content:MySdkSymbol");
        let response =
            wait_for_sdk_ready(SOCKET_TIMEOUT, || pinned_text_query(client, &query, pin()))?;
        let symbols = response
            .results
            .iter()
            .map(|row| (row.candidate_id.as_str(), row.repo_relative_path.as_str()))
            .collect::<BTreeSet<_>>();
        if response.generation != pin()
            || response.rank_unit != TextRankUnit::Symbol
            || symbols != expected_symbols
            || response.window.candidate_count() != CandidateCountV1::Exact(3)
            || response.next_cursor.is_some()
        {
            return Err(
                format!("{phase} {query:?}: unexpected Symbol projection: {response:?}").into(),
            );
        }
    }

    let unsupported = SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo;
    let parse_fail = SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::ParseFail);
    for leaf in [r#""MySdkSymbol""#, "'MySdkSymbol'", "/MySdkSymbol/"] {
        for query in [
            leaf.to_owned(),
            format!("NOT {leaf}"),
            format!("MySdkSymbol OR {leaf}"),
            format!("MySdkSymbol AND {leaf}"),
            format!("{leaf} AND repo.has.content(absent_content_gate)"),
        ] {
            require_symbol_content_refusal(client, phase, &query, unsupported)?;
        }
        // Public Native LQ only admits a keyword value after content:.
        for expression in [
            "MySdkSymbol",
            "MySdkSymbol AND repo.has.content(absent_content_gate)",
        ] {
            let query = format!("{expression} content:{leaf}");
            require_symbol_content_refusal(client, phase, &query, parse_fail)?;
        }
    }
    for argument in [r#""MySdkSymbol""#, "'MySdkSymbol'"] {
        let query = format!("symbol.has.name({argument})");
        require_symbol_content_refusal(client, phase, &query, unsupported)?;
        let conjunction = format!("MySdkSymbol AND {query}");
        require_symbol_content_refusal(client, phase, &conjunction, unsupported)?;
    }
    for query in [
        "patterntype:regexp MySdkSymbol",
        "patterntype:regexp content:MySdkSymbol",
    ] {
        require_symbol_content_refusal(client, phase, query, unsupported)?;
    }
    Ok(())
}

fn require_symbol_content_refusal(
    client: &QuantaIndex,
    phase: &str,
    query: &str,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    let check = |route: &str, text: &str, result: Result<(), SdkError>| -> TestResult {
        let context = format!("{phase} {route} {text:?}");
        let error = expect_sdk_error(result, &context)?;
        expect_remote_code(error, expected.as_wire_str())
            .map_err(|error| format!("{context}: {error}").into())
    };
    for prefix in ["", "count:all "] {
        let text = format!("{prefix}{query}");
        check(
            "Symbol",
            &text,
            pinned_symbol_query(client, &text, pin(), 3).map(|_| ()),
        )?;
    }
    for projection in ["type:symbol", "select:symbol"] {
        let text = format!("{projection} {query}");
        check(
            "Text",
            &text,
            pinned_text_query(client, &text, pin()).map(|_| ()),
        )?;
    }
    Ok(())
}

fn pinned_symbol_query(
    client: &QuantaIndex,
    query: &str,
    generation: GenerationPin,
    top_k: u32,
) -> Result<SymbolQueryResponse, SdkError> {
    client
        .symbol()
        .query()
        .native(query)
        .pinned(generation)
        .top_k(top_k)
        .execute()
}

fn pinned_text_query(
    client: &QuantaIndex,
    query: &str,
    generation: GenerationPin,
) -> Result<TextQueryResponse, SdkError> {
    client
        .lexical()
        .query()
        .native(query)
        .pinned(generation)
        .top_k(3)
        .execute()
}

#[test]
fn sdk_builder_variant_frontdoors_route_native_inline_vector_and_pinned_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let corpus_batch = lexical_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;

    let lexical_native = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .native("todo")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_native_candidate = lexical_native
        .results
        .first()
        .ok_or_else(|| "missing lexical native candidate".to_string())?;
    if lexical_native.generation != pin()
        || lexical_native_candidate.candidate_id != "chunk-dirty"
        || lexical_native_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        return Err(format!("unexpected lexical native response: {lexical_native:?}").into());
    }

    let runtime_native = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .runtime()
                .query()
                .native("dirty:yes todo")
                .pinned(pin())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let runtime_native_candidate = runtime_native
        .results
        .first()
        .ok_or_else(|| "missing runtime native candidate".to_string())?;
    if runtime_native.generation != pin()
        || runtime_native_candidate.candidate_id != "chunk-dirty"
        || runtime_native_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        return Err(format!("unexpected runtime native response: {runtime_native:?}").into());
    }

    let semantic_inline = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .semantic()
                .query()
                .text("quartz")
                .scope_native("sphinx")
                .scope_top_k(2)
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let semantic_inline_top = semantic_inline
        .results
        .first()
        .ok_or_else(|| "missing semantic inline-vector candidate".to_string())?;
    if semantic_inline.generation != pin()
        || semantic_inline_top.candidate_id != "alpha"
        || semantic_inline.explanation.summary.is_empty()
    {
        return Err(
            format!("unexpected semantic inline-vector response: {semantic_inline:?}").into(),
        );
    }

    let hybrid_inline = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .search()
                .hybrid_seed()
                .native("sphinx")
                .semantic_text("quartz")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.seed_candidates.is_empty(),
    )?;
    let hybrid_inline_top = hybrid_inline
        .seed_candidates
        .first()
        .ok_or_else(|| "missing hybrid inline-vector seed candidate".to_string())?;
    if hybrid_inline.generation != pin()
        || hybrid_inline_top.entity_id != "alpha"
        || hybrid_inline.explanation.summary.is_empty()
    {
        return Err(
            format!("unexpected hybrid inline-vector seed response: {hybrid_inline:?}").into(),
        );
    }

    fixture.stop()
}

#[test]
fn sdk_multi_generation_restart_frontdoor_preserves_pinned_and_flips_active_composite_corpus()
-> TestResult {
    let complex_timeout = Duration::from_secs(30);
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = dir.path().to_path_buf();
    let fixture = SdkFrontdoorRuntime::start_at(&state_root)?;
    let client = &fixture.client;

    let corpus_batch_v1 = lexical_batch()?;
    let _corpus_active_v1 = publish_and_activate_sdk_search_corpus(client, &corpus_batch_v1)?;
    let _structural_receipt_v1 = client.structural().publish(&structural_batch()?)?;

    let lexical_active_v1 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_active_v1_candidate = lexical_active_v1
        .results
        .first()
        .ok_or_else(|| "missing active v1 lexical candidate".to_string())?;
    if lexical_active_v1.generation != pin()
        || lexical_active_v1_candidate.candidate_id != "chunk-dirty"
    {
        return Err(format!("unexpected active v1 lexical response: {lexical_active_v1:?}").into());
    }

    let corpus_batch_v2 = lexical_batch_two()?;
    let _corpus_active_v2 = publish_and_activate_sdk_search_corpus(client, &corpus_batch_v2)?;
    let _structural_receipt_v2 = client.structural().publish(&structural_batch_two()?)?;

    let lexical_pinned_v2 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo_v2")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let lexical_pinned_v2_candidate = lexical_pinned_v2
        .results
        .first()
        .ok_or_else(|| "missing pinned v2 lexical candidate".to_string())?;
    if lexical_pinned_v2.generation != pin_two()
        || lexical_pinned_v2_candidate.candidate_id != "chunk-dirty-v2"
    {
        return Err(format!("unexpected pinned v2 lexical response: {lexical_pinned_v2:?}").into());
    }

    let semantic_pinned_v2 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .semantic()
                .query()
                .text("obsidian")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && !response.results.is_empty(),
    )?;
    let semantic_pinned_v2_top = semantic_pinned_v2
        .results
        .first()
        .ok_or_else(|| "missing pinned v2 semantic candidate".to_string())?;
    if semantic_pinned_v2.generation != pin_two() || semantic_pinned_v2_top.candidate_id != "gamma"
    {
        return Err(
            format!("unexpected pinned v2 semantic response: {semantic_pinned_v2:?}").into(),
        );
    }

    let structural_pinned_v2 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .structural()
                .query()
                .native("match { function_item }")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let structural_pinned_v2_candidate = structural_pinned_v2
        .results
        .first()
        .ok_or_else(|| "missing pinned v2 structural candidate".to_string())?;
    if structural_pinned_v2.generation != pin_two()
        || structural_pinned_v2_candidate.candidate_id != "chunk-tree-v2"
    {
        return Err(
            format!("unexpected pinned v2 structural response: {structural_pinned_v2:?}").into(),
        );
    }

    fixture.stop()?;
    let fixture = SdkFrontdoorRuntime::start_at(&state_root)?;
    let client = &fixture.client;

    let lexical_active_after_restart = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo_v2")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let lexical_active_after_restart_candidate = lexical_active_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted active v1 lexical candidate".to_string())?;
    if lexical_active_after_restart.generation != pin_two()
        || lexical_active_after_restart_candidate.candidate_id != "chunk-dirty-v2"
    {
        return Err(format!(
            "unexpected restarted active v2 lexical response: {lexical_active_after_restart:?}"
        )
        .into());
    }

    let lexical_pinned_v2_after_restart = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo_v2")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let lexical_pinned_v2_after_restart_candidate = lexical_pinned_v2_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted pinned v2 lexical candidate".to_string())?;
    if lexical_pinned_v2_after_restart.generation != pin_two()
        || lexical_pinned_v2_after_restart_candidate.candidate_id != "chunk-dirty-v2"
    {
        return Err(format!(
            "unexpected restarted pinned v2 lexical response: \
             {lexical_pinned_v2_after_restart:?}"
        )
        .into());
    }

    let semantic_pinned_v2_after_restart = wait_for_sdk_observation_with_retry_codes(
        complex_timeout,
        &["NOT_READY"],
        || {
            client
                .semantic()
                .query()
                .text("obsidian")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && !response.results.is_empty(),
    )?;
    let semantic_pinned_v2_after_restart_top = semantic_pinned_v2_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted pinned v2 semantic candidate".to_string())?;
    if semantic_pinned_v2_after_restart.generation != pin_two()
        || semantic_pinned_v2_after_restart_top.candidate_id != "gamma"
    {
        return Err(format!(
            "unexpected restarted pinned v2 semantic response: \
             {semantic_pinned_v2_after_restart:?}"
        )
        .into());
    }

    let structural_pinned_v2_after_restart = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .structural()
                .query()
                .native("match { function_item }")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let structural_pinned_v2_after_restart_candidate = structural_pinned_v2_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted pinned v2 structural candidate".to_string())?;
    if structural_pinned_v2_after_restart.generation != pin_two()
        || structural_pinned_v2_after_restart_candidate.candidate_id != "chunk-tree-v2"
    {
        return Err(format!(
            "unexpected restarted pinned v2 structural response: \
             {structural_pinned_v2_after_restart:?}"
        )
        .into());
    }

    let lexical_snapshot_v2 = wait_for_sdk_ready(complex_timeout, || {
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Lexical)
    })?;
    if lexical_snapshot_v2.manifest_generation != generation_two()
        || lexical_snapshot_v2.manifest_digest != "manifest:lexical-v2"
    {
        return Err(format!(
            "unexpected post-restart lexical generation snapshot: {lexical_snapshot_v2:?}"
        )
        .into());
    }

    let semantic_snapshot_v2 = wait_for_sdk_ready(complex_timeout, || {
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Semantic)
    })?;
    if semantic_snapshot_v2.manifest_generation != lexical_snapshot_v2.manifest_generation
        || semantic_snapshot_v2.manifest_digest != lexical_snapshot_v2.manifest_digest
    {
        return Err(format!(
            "restart split the active composite corpus: lexical={lexical_snapshot_v2:?} semantic={semantic_snapshot_v2:?}"
        )
        .into());
    }

    let lexical_active_v2 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo_v2")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let lexical_active_v2_candidate = lexical_active_v2
        .results
        .first()
        .ok_or_else(|| "missing active v2 lexical candidate".to_string())?;
    if lexical_active_v2.generation != pin_two()
        || lexical_active_v2_candidate.candidate_id != "chunk-dirty-v2"
    {
        return Err(format!("unexpected active v2 lexical response: {lexical_active_v2:?}").into());
    }

    let lexical_pinned_v1_after_flip = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_pinned_v1_after_flip_candidate = lexical_pinned_v1_after_flip
        .results
        .first()
        .ok_or_else(|| "missing pinned v1 lexical candidate after flip".to_string())?;
    if lexical_pinned_v1_after_flip.generation != pin()
        || lexical_pinned_v1_after_flip_candidate.candidate_id != "chunk-dirty"
    {
        return Err(format!(
            "unexpected pinned v1 lexical response after flip: {lexical_pinned_v1_after_flip:?}"
        )
        .into());
    }

    fixture.stop()
}
