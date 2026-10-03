use super::*;

#[test]
fn sdk_dsl_frontdoor_fail_closed_timeout_and_recovery_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    publish_sdk_search_corpus_ready(client)?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

    let lexical_timeout = expect_sdk_error(
        client
            .lexical()
            .query()
            .sourcegraph(r"timeout:0ms /todo!/")
            .active(repo(), revision())
            .top_k(2)
            .execute(),
        "Sourcegraph lexical timeout must fail closed",
    )?;
    match lexical_timeout {
        SdkError::Remote { code, message, .. }
            if code.as_wire_str() == "QUERY_TIMEOUT"
                && (message.contains("timeout") || message.contains("timed out")) => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
            return Err(format!("unexpected lexical timeout error: {other:?}").into());
        }
    }

    let lexical_follow_up = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("todo")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_follow_up_candidate = lexical_follow_up
        .results
        .first()
        .ok_or_else(|| "missing lexical follow-up candidate".to_string())?;
    if lexical_follow_up.generation != pin()
        || lexical_follow_up_candidate.candidate_id != "chunk-dirty"
    {
        return Err(
            format!("unexpected lexical follow-up after timeout: {lexical_follow_up:?}").into(),
        );
    }

    let structural_timeout = expect_sdk_error(
        client
            .structural()
            .query()
            .sourcegraph(r#"timeout:0ms patterntype:structural "function_item""#)
            .pinned(pin())
            .top_k(2)
            .execute(),
        "Sourcegraph structural timeout must fail closed",
    )?;
    match structural_timeout {
        SdkError::Remote { code, message, .. }
            if code.as_wire_str() == "STR_INVALID_REQUEST"
                && message.contains("timeout option") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
            return Err(format!("unexpected structural timeout error: {other:?}").into());
        }
    }

    let typed_hole = expect_sdk_error(
        client
            .structural()
            .query()
            .native("match { function_item { { :[name.lambda] } } }")
            .pinned(pin())
            .top_k(2)
            .execute(),
        "unsupported typed hole must fail closed",
    )?;
    match typed_hole {
        SdkError::Remote { code, message, .. }
            if code.as_wire_str() == "STR_HOLE_KIND_UNSUPPORTED"
                && message.contains("typed hole kind `lambda`") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
            return Err(format!("unexpected typed-hole error: {other:?}").into());
        }
    }

    let mixed_boolean = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("main AND match { function_item :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &mixed_boolean,
        &pin(),
        "chunk-tree",
        "x",
        0,
        10,
        "sdk mixed lexical/structural boolean AND",
    )?;

    let mixed_or = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("main OR match { function_item :[x] }")
                .pinned(pin())
                .top_k(10)
                .execute()
        },
        // The OR survivor arrives through the lexical arm without
        // structural captures (TOPT-06: the old timeout-as-Ok wait
        // masked the bindings clause never becoming ready; the
        // assertions below never required them). AND-capture bindings
        // stay guarded by `mixed_boolean` above.
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if mixed_or.results.len() != 1 {
        return Err(format!(
            "sdk mixed lexical/structural OR expected one surviving candidate, got {:?}",
            mixed_or.results
        )
        .into());
    }
    let mixed_or_candidate = mixed_or
        .results
        .first()
        .ok_or_else(|| "sdk mixed lexical/structural boolean OR: missing candidate".to_string())?;
    if mixed_or.generation != pin() || mixed_or_candidate.candidate_id != "chunk-tree" {
        return Err(format!(
            "sdk mixed lexical/structural boolean OR: unexpected response {mixed_or:?}"
        )
        .into());
    }

    let mixed_and_not = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("main AND NOT match { trait_item }")
                .pinned(pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let mixed_and_not_candidate = mixed_and_not.results.first().ok_or_else(|| {
        "sdk mixed lexical/structural boolean AND NOT: missing candidate".to_string()
    })?;
    if mixed_and_not.generation != pin() || mixed_and_not_candidate.candidate_id != "chunk-tree" {
        return Err(format!(
            "sdk mixed lexical/structural boolean AND NOT: unexpected response {mixed_and_not:?}"
        )
        .into());
    }

    let pure_negative = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("NOT match { function_item }")
                .pinned(pin())
                .top_k(10)
                .execute()
        },
        |response| {
            response.generation == pin()
                && !response.results.is_empty()
                && response
                    .results
                    .iter()
                    .all(|candidate| candidate.candidate_id != "chunk-tree")
        },
    )?;
    for candidate in &pure_negative.results {
        if candidate.candidate_id == "chunk-tree" {
            return Err(format!(
                "pure-negative root must exclude function_item matches, got {candidate:?}"
            )
            .into());
        }
        if !candidate.bindings.is_empty() {
            return Err(format!(
                "pure-negative universe placeholder must not invent bindings, got {candidate:?}"
            )
            .into());
        }
    }

    let missing_patterntype = expect_sdk_error(
        client
            .structural()
            .query()
            .sourcegraph(r#""function_item""#)
            .pinned(pin())
            .top_k(2)
            .execute(),
        "Sourcegraph structural route requires patterntype:structural",
    )?;
    match missing_patterntype {
        SdkError::Remote { code, message, .. }
            if code.as_wire_str() == "BRIDGE_TRANSLATE_FAIL"
                && message.contains("patterntype:structural") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
            return Err(format!("unexpected patterntype error: {other:?}").into());
        }
    }

    let structural_follow_up = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { { :[name.expr] } } }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &structural_follow_up,
        &pin(),
        "chunk-tree",
        "name",
        3,
        7,
        "sdk structural follow-up after typed errors",
    )?;

    fixture.stop()
}

#[test]
fn sdk_contract_exact_query_request_frontdoors_roundtrip_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let corpus_batch = lexical_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let _history_receipt = client.history().publish(&history_batch())?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

    let lexical_request = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "todo".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(pinned_selector(pin())),
        top_k: 2,
        cursor: None,
    };
    let lexical = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.lexical().query_request(lexical_request.clone()),
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_candidate = lexical
        .results
        .first()
        .ok_or_else(|| "missing contract-exact lexical candidate".to_string())?;
    if lexical.generation != pin()
        || lexical_candidate.candidate_id != "chunk-dirty"
        || lexical_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        return Err(format!("unexpected contract-exact lexical response: {lexical:?}").into());
    }

    let symbol_request = SymbolQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "select:symbol MySdkSymbol".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(pinned_selector(pin())),
        top_k: 3,
        cursor: None,
    };
    let symbol = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client.symbol().query_request(symbol_request.clone())
    })?;
    assert_single_symbol_candidate(&symbol)?;

    let history_request = HistoryQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "type:commit rev:refs/heads/main author:alice fix".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 5,
            cursor: None,
        },
        order: HistoryOrderV1::Recency,
        cursor: None,
    };
    let history = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.history().query_request(history_request.clone()),
        |response| response.generation == pin() && response.commits.len() == 1,
    )?;
    let history_commit = history
        .commits
        .first()
        .ok_or_else(|| "missing contract-exact history candidate".to_string())?;
    if history.generation != pin()
        || history_commit.author != "alice"
        || history_commit.message != "fix: sample"
    {
        return Err(format!("unexpected contract-exact history response: {history:?}").into());
    }

    let runtime_request = RuntimeMetadataQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "dirty:yes todo".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 3,
            cursor: None,
        },
        cursor: None,
    };
    let runtime = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.runtime().query_request(runtime_request.clone()),
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let runtime_top = runtime
        .results
        .first()
        .ok_or_else(|| "missing contract-exact runtime candidate".to_string())?;
    if runtime.generation != pin() || runtime_top.candidate_id != "chunk-dirty" {
        return Err(format!("unexpected contract-exact runtime response: {runtime:?}").into());
    }

    let structural_request = StructuralQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text:
                r#"repo:repo-sdk path:src/lib.rs lang:rust patterntype:structural "function_item { { :[name.expr] } }""#
                    .to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 2,
            cursor: None,
        },
        cursor: None,
    };
    let structural = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query_request(structural_request.clone())
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_top = structural
        .results
        .first()
        .ok_or_else(|| "missing contract-exact structural candidate".to_string())?;
    let structural_binding = structural_top
        .bindings
        .first()
        .ok_or_else(|| "missing contract-exact structural binding".to_string())?;
    if structural.generation != pin()
        || structural_top.candidate_id != "chunk-tree"
        || structural_binding.metavariable != "name"
        || structural_binding.start_byte != 3
        || structural_binding.end_byte != 7
    {
        return Err(
            format!("unexpected contract-exact structural response: {structural:?}").into(),
        );
    }

    let semantic_request = SemanticQueryRequest {
        query_text: "quartz".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(pinned_selector(pin())),
        lexical_scope: Some(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "sphinx".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 2,
            cursor: None,
        }),
        top_k: 2,
    };
    let semantic = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.semantic().query_request(semantic_request.clone()),
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let semantic_top = semantic
        .results
        .first()
        .ok_or_else(|| "missing contract-exact semantic candidate".to_string())?;
    if semantic.generation != pin()
        || semantic_top.candidate_id != "alpha"
        || semantic.explanation.summary.is_empty()
    {
        return Err(format!("unexpected contract-exact semantic response: {semantic:?}").into());
    }

    let hybrid_request = HybridSeedQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "sphinx".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 2,
            cursor: None,
        },
        semantic_query_text: "quartz".to_string(),
        generation: None,
        generation_selector: Some(pinned_selector(pin())),
        dense_corpora: Vec::new(),
        top_k: 2,
    };
    let hybrid = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.search().hybrid_seed_request(hybrid_request.clone()),
        |response| response.generation == pin() && !response.seed_candidates.is_empty(),
    )?;
    let hybrid_top = hybrid
        .seed_candidates
        .first()
        .ok_or_else(|| "missing contract-exact hybrid seed candidate".to_string())?;
    if hybrid.generation != pin()
        || hybrid_top.entity_id != "alpha"
        || hybrid.explanation.summary.is_empty()
    {
        return Err(format!("unexpected contract-exact hybrid-seed response: {hybrid:?}").into());
    }

    fixture.stop()
}
