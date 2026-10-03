use super::*;

#[test]
fn sdk_search_frontdoor_routes_lexical_semantic_hybrid_explain_and_repomap_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let corpus_batch = lexical_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let repo_map_receipt = client
        .repomap()
        .publish(&RepoMapPublishBundleRequestV2::new(repo_map_bundle()?)?)?;
    if repo_map_receipt.mutation.manifest_generation != generation() {
        return Err(format!("unexpected repo-map publish ack: {repo_map_receipt:?}").into());
    }

    if client.repomap().active_head(repo(), revision())?.is_some() {
        return Err("RepoMap publish activated a head without an activation request".into());
    }

    let repo_map_activation = client.repomap().activate(repo_map_activate_request()?)?;
    if repo_map_activation.mutation.manifest_generation != generation() {
        return Err(format!("unexpected repo-map activate ack: {repo_map_activation:?}").into());
    }
    let active = client
        .repomap()
        .active_head(repo(), revision())?
        .ok_or("RepoMap activation did not expose a catalog head")?;
    if active.epoch().get() != repo_map_activation.mutation.activation_epoch
        || active.candidate_commitment().to_wire_string()
            != repo_map_activation.mutation.new_candidate_commitment
    {
        return Err(format!(
            "RepoMap active head does not match the activation receipt: {active:?}"
        )
        .into());
    }

    let lexical = wait_for_sdk_observation(
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
    let lexical_candidate = lexical
        .results
        .first()
        .cloned()
        .ok_or_else(|| "missing lexical candidate".to_string())?;
    if lexical.generation != pin()
        || lexical_candidate.candidate_id != "chunk-dirty"
        || lexical_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        return Err(format!("unexpected lexical response: {lexical:?}").into());
    }

    let lexical_select_path = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("select:path sphinx")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let lexical_select_path_paths = lexical_select_path
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if lexical_select_path.generation != pin()
        || lexical_select_path_paths
            != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
    {
        return Err(
            format!("unexpected select:path lexical response: {lexical_select_path:?}").into(),
        );
    }

    let lexical_select_content_match = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("select:content.match sphinx")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let lexical_select_content_match_paths = lexical_select_content_match
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if lexical_select_content_match.generation != pin()
        || lexical_select_content_match_paths
            != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
        || lexical_select_content_match
            .results
            .iter()
            .any(|candidate| !candidate.snippet.contains("sphinx"))
    {
        return Err(format!(
            "unexpected select:content.match lexical response: {lexical_select_content_match:?}"
        )
        .into());
    }

    let lexical_native_select_path = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .native("select:path sphinx")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let lexical_native_select_path_paths = lexical_native_select_path
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if lexical_native_select_path.generation != pin()
        || lexical_native_select_path_paths
            != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
    {
        return Err(format!(
            "unexpected native select:path lexical response: {lexical_native_select_path:?}"
        )
        .into());
    }

    let lexical_native_select_content_match = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .native("select:content.match sphinx")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let lexical_native_select_content_match_paths = lexical_native_select_content_match
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if lexical_native_select_content_match.generation != pin()
        || lexical_native_select_content_match_paths
            != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
        || lexical_native_select_content_match
            .results
            .iter()
            .any(|candidate| !candidate.snippet.contains("sphinx"))
    {
        return Err(format!(
            "unexpected native select:content.match lexical response: \
             {lexical_native_select_content_match:?}"
        )
        .into());
    }

    let explain = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client.search().explain(pin(), lexical_candidate.clone())
    })?;
    if explain.generation != pin()
        || !explain.explanation.summary.contains("present")
        || !explain.explanation.summary.contains("chunk-dirty")
    {
        return Err(format!("unexpected explain response: {explain:?}").into());
    }

    let semantic = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .semantic()
                .query()
                .text("quartz")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let semantic_top = semantic
        .results
        .first()
        .ok_or_else(|| "missing semantic candidate".to_string())?;
    if semantic.generation != pin()
        || semantic_top.candidate_id != "alpha"
        || semantic.explanation.summary.is_empty()
    {
        return Err(format!("unexpected semantic response: {semantic:?}").into());
    }

    let hybrid = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .search()
                .hybrid_seed()
                .sourcegraph("sphinx")
                .semantic_text("quartz")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.seed_candidates.is_empty(),
    )?;
    let hybrid_top = hybrid
        .seed_candidates
        .first()
        .ok_or_else(|| "missing hybrid seed candidate".to_string())?;
    if hybrid.generation != pin()
        || hybrid_top.entity_id != "alpha"
        || hybrid.explanation.summary.is_empty()
    {
        return Err(format!("unexpected hybrid-seed response: {hybrid:?}").into());
    }

    // QI-BB-018: the true-hybrid route from the SDK builder — two
    // independent lanes fused by RRF, every row carrying its lane
    // provenance — and QI-BB-022: its top row explained through the SDK
    // under both queries, re-derived against the index on every axis.
    let true_hybrid = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .search()
                .hybrid()
                .sourcegraph("sphinx")
                .semantic_text("quartz")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let true_hybrid_top = true_hybrid
        .results
        .first()
        .ok_or_else(|| "missing hybrid candidate".to_string())?;
    let true_hybrid_rrf: f64 = true_hybrid_top
        .contributions
        .iter()
        .map(|contribution| 1.0 / (60.0 + f64::from(contribution.rank)))
        .sum();
    if true_hybrid.generation != pin()
        || true_hybrid_top.candidate.candidate_id != "alpha"
        || true_hybrid_top.contributions.is_empty()
        || true_hybrid_top.fused_score.to_bits() != true_hybrid_rrf.to_bits()
        || true_hybrid.explanation.summary.is_empty()
    {
        return Err(format!("unexpected hybrid response: {true_hybrid:?}").into());
    }
    let hybrid_explain = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client.search().explain_hybrid_under_queries(
            pin(),
            true_hybrid_top.clone(),
            TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "sphinx".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 2,
                cursor: None,
            },
            "quartz",
        )
    })?;
    let reconciled = |axis: &str| {
        hybrid_explain
            .explanation
            .planner_trace
            .iter()
            .any(|entry| entry.detail == format!("explain.{axis}_reconciled=true"))
    };
    if hybrid_explain.generation != pin()
        || hybrid_explain.explanation.strategy != "hybrid_score_trace"
        || !reconciled("score")
        || !reconciled("dense")
        || !reconciled("fused")
    {
        return Err(format!("unexpected hybrid explain response: {hybrid_explain:?}").into());
    }

    let repo_map = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.repomap().query(repo_map_query_request()),
        |response| response.manifest_generation == generation() && !response.entries.is_empty(),
    )?;
    assert_repo_map_happy_path(&repo_map)?;

    fixture.stop()
}

#[test]
fn sdk_query_frontdoor_routes_history_runtime_and_structural_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let corpus_batch = lexical_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let _history_receipt = client.history().publish(&history_batch())?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

    let history_commit = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .history()
                .query()
                .sourcegraph("type:commit rev:refs/heads/main author:alice fix")
                .pinned(pin())
                .top_k(5)
                .order(HistoryOrderV1::Recency)
                .execute()
        },
        |response| {
            response.generation == pin() && response.commits.len() == 1 && response.diffs.is_empty()
        },
    )?;
    if history_commit.generation != pin()
        || history_commit.commits.len() != 1
        || !history_commit.diffs.is_empty()
    {
        return Err(format!("unexpected history commit response: {history_commit:?}").into());
    }
    let commit = history_commit
        .commits
        .first()
        .ok_or_else(|| "missing history commit candidate".to_string())?;
    if commit.author != "alice" || commit.message != "fix: sample" {
        return Err(format!("unexpected history commit candidate: {commit:?}").into());
    }

    let history_diff = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .history()
                .query()
                .native("type:diff todo")
                .pinned(pin())
                .top_k(5)
                .order(HistoryOrderV1::Recency)
                .execute()
        },
        |response| {
            response.generation == pin() && response.commits.is_empty() && response.diffs.len() == 1
        },
    )?;
    if history_diff.generation != pin()
        || !history_diff.commits.is_empty()
        || history_diff.diffs.len() != 1
    {
        return Err(format!("unexpected history diff response: {history_diff:?}").into());
    }

    let symbol_select_native = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .native("select:symbol MySdkSymbol")
            .pinned(pin())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_select_native)?;

    let symbol_type_native = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .native("type:symbol MySdkSymbol")
            .pinned(pin())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_type_native)?;

    let symbol_select_sourcegraph = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .sourcegraph("select:symbol MySdkSymbol")
            .pinned(pin())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_select_sourcegraph)?;

    let symbol_type_sourcegraph = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .sourcegraph("type:symbol MySdkSymbol")
            .pinned(pin())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_type_sourcegraph)?;

    let runtime_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .runtime()
                .query()
                .sourcegraph("dirty:yes todo")
                .pinned(pin())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if runtime_query.generation != pin() || runtime_query.results.len() != 1 {
        return Err(format!("unexpected runtime response: {runtime_query:?}").into());
    }
    let runtime_candidate = runtime_query
        .results
        .first()
        .ok_or_else(|| "missing runtime candidate".to_string())?;
    if runtime_candidate.candidate_id != "chunk-dirty"
        || runtime_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        return Err(format!("unexpected runtime candidate: {runtime_candidate:?}").into());
    }

    let structural_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_query.generation != pin() || structural_query.results.len() != 1 {
        return Err(format!("unexpected structural response: {structural_query:?}").into());
    }
    let structural_candidate = structural_query
        .results
        .first()
        .ok_or_else(|| "missing structural candidate".to_string())?;
    if structural_candidate.candidate_id != "chunk-tree" || structural_candidate.bindings.len() != 1
    {
        return Err(format!("unexpected structural candidate: {structural_candidate:?}").into());
    }
    let structural_binding = structural_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural binding".to_string())?;
    if structural_binding.metavariable != "x"
        || structural_binding.start_byte != 0
        || structural_binding.end_byte != 10
    {
        return Err(format!("unexpected structural binding: {structural_binding:?}").into());
    }

    let structural_pinned_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_pinned_query.generation != pin() || structural_pinned_query.results.len() != 1 {
        return Err(
            format!("unexpected pinned structural response: {structural_pinned_query:?}").into(),
        );
    }
    let structural_pinned_candidate = structural_pinned_query
        .results
        .first()
        .ok_or_else(|| "missing pinned structural candidate".to_string())?;
    if structural_pinned_candidate.candidate_id != "chunk-tree"
        || structural_pinned_candidate.bindings.len() != 1
    {
        return Err(format!(
            "unexpected pinned structural candidate: {structural_pinned_candidate:?}"
        )
        .into());
    }

    let structural_root_kind = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_root_kind.generation != pin() || structural_root_kind.results.len() != 1 {
        return Err(
            format!("unexpected structural root-kind response: {structural_root_kind:?}").into(),
        );
    }
    let structural_root_kind_candidate = structural_root_kind
        .results
        .first()
        .ok_or_else(|| "missing structural root-kind candidate".to_string())?;
    if structural_root_kind_candidate.candidate_id != "chunk-tree"
        || !structural_root_kind_candidate.bindings.is_empty()
    {
        return Err(format!(
            "unexpected structural root-kind candidate: {structural_root_kind_candidate:?}"
        )
        .into());
    }

    let structural_root_kind_capture = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_root_kind_capture.generation != pin()
        || structural_root_kind_capture.results.len() != 1
    {
        return Err(format!(
            "unexpected structural root-kind+capture response: {structural_root_kind_capture:?}"
        )
        .into());
    }
    let structural_root_kind_capture_candidate = structural_root_kind_capture
        .results
        .first()
        .ok_or_else(|| "missing structural root-kind+capture candidate".to_string())?;
    if structural_root_kind_capture_candidate.candidate_id != "chunk-tree"
        || structural_root_kind_capture_candidate.bindings.len() != 1
    {
        return Err(format!(
            "unexpected structural root-kind+capture candidate: \
             {structural_root_kind_capture_candidate:?}"
        )
        .into());
    }
    let structural_root_kind_capture_binding = structural_root_kind_capture_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural root-kind+capture binding".to_string())?;
    if structural_root_kind_capture_binding.metavariable != "x"
        || structural_root_kind_capture_binding.start_byte != 0
        || structural_root_kind_capture_binding.end_byte != 10
    {
        return Err(format!(
            "unexpected structural root-kind+capture binding: \
             {structural_root_kind_capture_binding:?}"
        )
        .into());
    }

    let structural_boolean_and = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item } AND match { function_item :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_boolean_and_candidate = structural_boolean_and
        .results
        .first()
        .ok_or_else(|| "missing structural boolean AND candidate".to_string())?;
    let structural_boolean_and_binding = structural_boolean_and_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural boolean AND binding".to_string())?;
    if structural_boolean_and_candidate.candidate_id != "chunk-tree"
        || structural_boolean_and_binding.metavariable != "x"
        || structural_boolean_and_binding.start_byte != 0
        || structural_boolean_and_binding.end_byte != 10
    {
        return Err(format!(
            "unexpected structural boolean AND candidate/binding: \
             {structural_boolean_and_candidate:?}"
        )
        .into());
    }

    let structural_child_capture = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { { identifier :[name] } } }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_child_capture.generation != pin() || structural_child_capture.results.len() != 1 {
        return Err(format!(
            "unexpected structural child-capture response: {structural_child_capture:?}"
        )
        .into());
    }
    let structural_child_capture_candidate = structural_child_capture
        .results
        .first()
        .ok_or_else(|| "missing structural child-capture candidate".to_string())?;
    let structural_child_capture_binding = structural_child_capture_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural child-capture binding".to_string())?;
    if structural_child_capture_candidate.candidate_id != "chunk-tree"
        || structural_child_capture_binding.metavariable != "name"
        || structural_child_capture_binding.start_byte != 3
        || structural_child_capture_binding.end_byte != 7
    {
        return Err(format!(
            "unexpected structural child-capture candidate/binding: \
             {structural_child_capture_candidate:?}"
        )
        .into());
    }

    let structural_typed_expr = wait_for_sdk_observation(
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
    let structural_typed_expr_candidate = structural_typed_expr
        .results
        .first()
        .ok_or_else(|| "missing structural typed-expr candidate".to_string())?;
    let structural_typed_expr_binding = structural_typed_expr_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural typed-expr binding".to_string())?;
    if structural_typed_expr_candidate.candidate_id != "chunk-tree"
        || structural_typed_expr_binding.metavariable != "name"
        || structural_typed_expr_binding.start_byte != 3
        || structural_typed_expr_binding.end_byte != 7
    {
        return Err(format!(
            "unexpected structural typed-expr candidate/binding: \
             {structural_typed_expr_candidate:?}"
        )
        .into());
    }

    let structural_typed_item = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { :[root.item] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_typed_item_candidate = structural_typed_item
        .results
        .first()
        .ok_or_else(|| "missing structural typed-item candidate".to_string())?;
    let structural_typed_item_binding = structural_typed_item_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural typed-item binding".to_string())?;
    if structural_typed_item_candidate.candidate_id != "chunk-tree"
        || structural_typed_item_binding.metavariable != "root"
        || structural_typed_item_binding.start_byte != 0
        || structural_typed_item_binding.end_byte != 10
    {
        return Err(format!(
            "unexpected structural typed-item candidate/binding: \
             {structural_typed_item_candidate:?}"
        )
        .into());
    }

    let structural_typed_stmt = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { { :[body.stmt] } } }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_typed_stmt_candidate = structural_typed_stmt
        .results
        .first()
        .ok_or_else(|| "missing structural typed-stmt candidate".to_string())?;
    let structural_typed_stmt_binding = structural_typed_stmt_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural typed-stmt binding".to_string())?;
    if structural_typed_stmt_candidate.candidate_id != "chunk-tree"
        || structural_typed_stmt_binding.metavariable != "body"
        || structural_typed_stmt_binding.start_byte != 8
        || structural_typed_stmt_binding.end_byte != 10
    {
        return Err(format!(
            "unexpected structural typed-stmt candidate/binding: \
             {structural_typed_stmt_candidate:?}"
        )
        .into());
    }

    let structural_where_inside_outside = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native(
                    "match { identifier :[name] where :[name] == \"main\" inside { function_item } outside { trait_item } }",
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_where_inside_outside_candidate = structural_where_inside_outside
        .results
        .first()
        .ok_or_else(|| "missing structural where/inside/outside candidate".to_string())?;
    let structural_where_inside_outside_binding = structural_where_inside_outside_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural where/inside/outside binding".to_string())?;
    if structural_where_inside_outside_candidate.candidate_id != "chunk-tree"
        || structural_where_inside_outside_binding.metavariable != "name"
        || structural_where_inside_outside_binding.start_byte != 3
        || structural_where_inside_outside_binding.end_byte != 7
    {
        return Err(format!(
            "unexpected structural where/inside/outside candidate/binding: \
             {structural_where_inside_outside_candidate:?}"
        )
        .into());
    }

    let structural_where_regex = client
        .structural()
        .query()
        .native("match { identifier :[name] where :[name] == /^main$/ inside { function_item } outside { trait_item } }")
        .pinned(pin())
        .top_k(2)
        .execute()?;
    if structural_where_regex.results != structural_where_inside_outside.results {
        return Err(format!(
            "prepared structural regex changed the authoritative match: {structural_where_regex:?}"
        )
        .into());
    }

    let structural_variadic = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { :[...prefix] block } }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_variadic_candidate = structural_variadic
        .results
        .first()
        .ok_or_else(|| "missing structural variadic candidate".to_string())?;
    let structural_variadic_binding = structural_variadic_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural variadic binding".to_string())?;
    if structural_variadic_candidate.candidate_id != "chunk-tree"
        || structural_variadic_binding.metavariable != "prefix"
        || structural_variadic_binding.start_byte != 3
        || structural_variadic_binding.end_byte != 7
    {
        return Err(format!(
            "unexpected structural variadic candidate/binding: {structural_variadic_candidate:?}"
        )
        .into());
    }

    let structural_filtered_native = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native(
                    "repo:repo-sdk file:src/lib.rs lang:rust match { function_item { { identifier :[name] } } }",
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_filtered_native.generation != pin()
        || structural_filtered_native.results.len() != 1
    {
        return Err(format!(
            "unexpected filtered native structural response: {structural_filtered_native:?}"
        )
        .into());
    }
    let structural_filtered_native_candidate = structural_filtered_native
        .results
        .first()
        .ok_or_else(|| "missing filtered native structural candidate".to_string())?;
    let structural_filtered_native_binding = structural_filtered_native_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing filtered native structural binding".to_string())?;
    if structural_filtered_native_candidate.candidate_id != "chunk-tree"
        || structural_filtered_native_binding.metavariable != "name"
        || structural_filtered_native_binding.start_byte != 3
        || structural_filtered_native_binding.end_byte != 7
    {
        return Err(format!(
            "unexpected filtered native structural candidate/binding: \
             {structural_filtered_native_candidate:?}"
        )
        .into());
    }

    let Err(structural_err) = client
        .structural()
        .query()
        .native("lang:java match { :[x] }")
        .pinned(pin())
        .top_k(2)
        .execute()
    else {
        return Err("unsupported-lang structural query unexpectedly succeeded".into());
    };
    expect_remote_code(structural_err, "STR_LANG_NOT_SUPPORTED")?;

    let structural_file_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("file:src/lib.rs match { :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_file_query.generation != pin() || structural_file_query.results.len() != 1 {
        return Err(
            format!("unexpected structural file response: {structural_file_query:?}").into(),
        );
    }

    let structural_repo_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("repo:repo-sdk match { :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_repo_query.generation != pin() || structural_repo_query.results.len() != 1 {
        return Err(
            format!("unexpected structural repo response: {structural_repo_query:?}").into(),
        );
    }

    let structural_repo_file_lang_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("repo:repo-sdk file:src/lib.rs lang:rust match { function_item :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_repo_file_lang_query.generation != pin()
        || structural_repo_file_lang_query.results.len() != 1
    {
        return Err(format!(
            "unexpected structural repo+file+lang response: \
                 {structural_repo_file_lang_query:?}"
        )
        .into());
    }
    let structural_repo_file_lang_candidate = structural_repo_file_lang_query
        .results
        .first()
        .ok_or_else(|| "missing structural repo+file+lang candidate".to_string())?;
    if structural_repo_file_lang_candidate.candidate_id != "chunk-tree"
        || structural_repo_file_lang_candidate.bindings.len() != 1
    {
        return Err(format!(
            "unexpected structural repo+file+lang candidate: \
             {structural_repo_file_lang_candidate:?}"
        )
        .into());
    }

    let structural_sourcegraph = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(
                    r#"repo:repo-sdk path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[name] } }""#,
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_sourcegraph.generation != pin() || structural_sourcegraph.results.len() != 1 {
        return Err(format!(
            "unexpected Sourcegraph structural response: {structural_sourcegraph:?}"
        )
        .into());
    }
    let structural_sourcegraph_candidate = structural_sourcegraph
        .results
        .first()
        .ok_or_else(|| "missing Sourcegraph structural candidate".to_string())?;
    let structural_sourcegraph_binding = structural_sourcegraph_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing Sourcegraph structural binding".to_string())?;
    if structural_sourcegraph_candidate.candidate_id != "chunk-tree"
        || structural_sourcegraph_binding.metavariable != "name"
        || structural_sourcegraph_binding.start_byte != 3
        || structural_sourcegraph_binding.end_byte != 7
    {
        return Err(format!(
            "unexpected Sourcegraph structural candidate/binding: \
             {structural_sourcegraph_candidate:?}"
        )
        .into());
    }

    let structural_sourcegraph_regex = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(
                    r"repo:repo-sdk path:src/lib.rs lang:rust patterntype:structural /^main$/",
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_sourcegraph_regex.generation != pin()
        || structural_sourcegraph_regex.results.len() != 1
    {
        return Err(format!(
            "unexpected Sourcegraph structural regex response: {structural_sourcegraph_regex:?}"
        )
        .into());
    }
    let structural_sourcegraph_regex_candidate = structural_sourcegraph_regex
        .results
        .first()
        .ok_or_else(|| "missing Sourcegraph structural regex candidate".to_string())?;
    let structural_sourcegraph_regex_binding = structural_sourcegraph_regex_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing Sourcegraph structural regex binding".to_string())?;
    if structural_sourcegraph_regex_candidate.candidate_id != "chunk-tree"
        || !structural_sourcegraph_regex_binding
            .metavariable
            .starts_with("__sg_regex_")
        || structural_sourcegraph_regex_binding.start_byte != 3
        || structural_sourcegraph_regex_binding.end_byte != 7
    {
        return Err(format!(
            "unexpected Sourcegraph structural regex candidate/binding: \
             {structural_sourcegraph_regex_candidate:?}"
        )
        .into());
    }

    let structural_repo_miss = client
        .structural()
        .query()
        .native("repo:other-repo match { :[x] }")
        .pinned(pin())
        .top_k(2)
        .execute()?;
    if structural_repo_miss.generation != pin() || !structural_repo_miss.results.is_empty() {
        return Err(
            format!("unexpected structural repo-miss response: {structural_repo_miss:?}").into(),
        );
    }

    let Err(structural_invalid_request_err) = client
        .structural()
        .query()
        .native("select:repo match { :[x] }")
        .pinned(pin())
        .top_k(2)
        .execute()
    else {
        return Err("invalid-filter structural query unexpectedly succeeded".into());
    };
    expect_remote_code(structural_invalid_request_err, "STR_INVALID_REQUEST")?;

    let Err(structural_sourcegraph_invalid_request_err) = client
        .structural()
        .query()
        .sourcegraph(r#"select:repo patterntype:structural "function_item""#)
        .pinned(pin())
        .top_k(2)
        .execute()
    else {
        return Err("invalid Sourcegraph structural filter unexpectedly succeeded".into());
    };
    expect_remote_code(
        structural_sourcegraph_invalid_request_err,
        "STR_INVALID_REQUEST",
    )?;

    let structural_native_pinned = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_native_pinned.generation != pin() || structural_native_pinned.results.len() != 1 {
        return Err(format!(
            "unexpected pinned structural native response: {structural_native_pinned:?}"
        )
        .into());
    }
    let structural_native_pinned_candidate = structural_native_pinned
        .results
        .first()
        .ok_or_else(|| "missing pinned structural native candidate".to_string())?;
    if structural_native_pinned_candidate.candidate_id != "chunk-tree" {
        return Err(format!(
            "unexpected pinned structural native candidate: \
             {structural_native_pinned_candidate:?}"
        )
        .into());
    }

    fixture.stop()
}
