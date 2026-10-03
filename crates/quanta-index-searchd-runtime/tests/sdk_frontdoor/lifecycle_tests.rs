use super::*;

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
    let fixture = SdkFrontdoorRuntime::start()?;
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

    let expected_active = current_sdk_search_corpus_or_none(client, repo(), revision())?
        .ok_or_else(|| "first composite generation did not become active".to_string())?;
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
        || lexical
            .results
            .iter()
            .any(|candidate| candidate.candidate_id == "alpha")
    {
        return Err(format!(
            "tombstone-only lexical generation retained removed alpha scope: {lexical:?}"
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

    fixture.stop()
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
