use super::*;

#[test]
fn sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;
    let ingest_socket = &fixture.ingest_socket;

    // The repo-metadata overlays belong to the generation's sealed
    // contract, so they are published before the seal (QI-BB-030); a
    // publish into the sealed generation would be refused typed.
    let _repo_commit_recency_receipt = client
        .history()
        .publish_repo_commit_recency(&repo_commit_recency_batch()?)?;
    let _repo_meta_receipt = client.history().publish_repo_meta(&repo_meta_batch())?;
    let _repo_description_receipt = client
        .history()
        .publish_repo_description(&repo_description_batch())?;
    let _repo_topic_receipt = client.history().publish_repo_topic(&repo_topic_batch())?;
    let _file_ownership_receipt = client
        .history()
        .publish_file_ownership(&file_ownership_batch())?;
    let _file_contributor_receipt = client
        .history()
        .publish_file_contributor(&file_contributor_batch())?;
    let corpus_batch = lexical_frontdoor_matrix_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let _history_receipt = client.history().publish(&history_batch())?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    publish_runtime_catalog_batch(ingest_socket)?;

    for &scenario in SDK_FRONTDOOR_SCENARIOS {
        match scenario.expected {
            SdkFrontdoorExpectation::CandidateIds(expected_ids) => match scenario.surface {
                SdkFrontdoorSurface::Lexical => {
                    let response = wait_for_sdk_observation(
                        SOCKET_TIMEOUT,
                        || match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .lexical()
                                .query()
                                .native(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::CodeSearch => code_search_uses_separate_fixture(),
                            TextQuerySyntax::Sourcegraph => client
                                .lexical()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                        },
                        |response| response.generation == pin(),
                    )?;
                    let observed = response
                        .results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect::<Vec<_>>();
                    let expected = expected_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect::<Vec<_>>();
                    if observed != expected {
                        return Err(format!(
                            "{} lexical candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::Symbol => {
                    let response =
                        wait_for_symbol_query(SOCKET_TIMEOUT, || match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .symbol()
                                .query()
                                .native(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::CodeSearch => code_search_uses_separate_fixture(),
                            TextQuerySyntax::Sourcegraph => client
                                .symbol()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                        })?;
                    let observed = response
                        .results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect::<Vec<_>>();
                    let expected = expected_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect::<Vec<_>>();
                    if observed != expected {
                        return Err(format!(
                            "{} symbol candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::Structural => {
                    let response = wait_for_sdk_observation(
                        SOCKET_TIMEOUT,
                        || match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .structural()
                                .query()
                                .native(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::CodeSearch => code_search_uses_separate_fixture(),
                            TextQuerySyntax::Sourcegraph => client
                                .structural()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                        },
                        |response| response.generation == pin(),
                    )?;
                    let observed = response
                        .results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect::<Vec<_>>();
                    let expected = expected_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect::<Vec<_>>();
                    if observed != expected {
                        return Err(format!(
                            "{} structural candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::RuntimeMetadata => {
                    let response = wait_for_sdk_observation(
                        SOCKET_TIMEOUT,
                        || match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .runtime()
                                .query()
                                .native(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::CodeSearch => code_search_uses_separate_fixture(),
                            TextQuerySyntax::Sourcegraph => client
                                .runtime()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                        },
                        |response| response.generation == pin(),
                    )?;
                    let observed = response
                        .results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect::<Vec<_>>();
                    let expected = expected_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect::<Vec<_>>();
                    if observed != expected {
                        return Err(format!(
                            "{} runtime candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::History => {
                    return Err(format!(
                        "{} used candidate-id expectation on history surface",
                        scenario.name
                    )
                    .into());
                }
            },
            SdkFrontdoorExpectation::CommitShas(expected_shas) => {
                if scenario.surface != SdkFrontdoorSurface::History {
                    return Err(format!(
                        "{} used commit expectation on non-history surface",
                        scenario.name
                    )
                    .into());
                }
                let response = wait_for_sdk_observation(
                    SOCKET_TIMEOUT,
                    || match scenario.syntax {
                        TextQuerySyntax::Native => client
                            .history()
                            .query()
                            .native(scenario.query_text)
                            .pinned(pin())
                            .top_k(10)
                            .order(HistoryOrderV1::Recency)
                            .execute(),
                        TextQuerySyntax::CodeSearch => code_search_uses_separate_fixture(),
                        TextQuerySyntax::Sourcegraph => client
                            .history()
                            .query()
                            .sourcegraph(scenario.query_text)
                            .pinned(pin())
                            .top_k(10)
                            .order(HistoryOrderV1::Recency)
                            .execute(),
                    },
                    |response| response.generation == pin(),
                )?;
                let observed = response
                    .commits
                    .iter()
                    .map(|commit| commit.sha.to_hex())
                    .collect::<Vec<_>>();
                let expected = expected_shas
                    .iter()
                    .map(|sha| (*sha).to_string())
                    .collect::<Vec<_>>();
                if observed != expected || !response.diffs.is_empty() {
                    return Err(format!(
                        "{} history commit drift: expected {:?}, got commits={:?} diffs={:?}",
                        scenario.name, expected, observed, response.diffs
                    )
                    .into());
                }
            }
            SdkFrontdoorExpectation::TypedError(expected_error) => {
                let err = match scenario.surface {
                    SdkFrontdoorSurface::Lexical => expect_sdk_error(
                        match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .lexical()
                                .query()
                                .native(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::CodeSearch => code_search_uses_separate_fixture(),
                            TextQuerySyntax::Sourcegraph => client
                                .lexical()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                        },
                        &format!("{} lexical typed error", scenario.name),
                    )?,
                    SdkFrontdoorSurface::History => {
                        wait_for_sdk_terminal_error(SOCKET_TIMEOUT, &["NOT_READY"], || {
                            match scenario.syntax {
                                TextQuerySyntax::Native => client
                                    .history()
                                    .query()
                                    .native(scenario.query_text)
                                    .pinned(pin())
                                    .top_k(10)
                                    .order(HistoryOrderV1::Recency)
                                    .execute(),
                                TextQuerySyntax::CodeSearch => code_search_uses_separate_fixture(),
                                TextQuerySyntax::Sourcegraph => client
                                    .history()
                                    .query()
                                    .sourcegraph(scenario.query_text)
                                    .pinned(pin())
                                    .top_k(10)
                                    .order(HistoryOrderV1::Recency)
                                    .execute(),
                            }
                        })?
                    }
                    SdkFrontdoorSurface::Structural => expect_sdk_error(
                        match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .structural()
                                .query()
                                .native(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::CodeSearch => code_search_uses_separate_fixture(),
                            TextQuerySyntax::Sourcegraph => client
                                .structural()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                        },
                        &format!("{} structural typed error", scenario.name),
                    )?,
                    other
                    @ (SdkFrontdoorSurface::Symbol | SdkFrontdoorSurface::RuntimeMetadata) => {
                        return Err(format!(
                            "{} typed error expectation on unsupported SDK surface {:?}",
                            scenario.name, other
                        )
                        .into());
                    }
                };
                match err {
                    SdkError::Remote { code, message, .. }
                        if code.as_wire_str() == expected_error.code
                            && message.contains(expected_error.message_contains) => {}
                    other @ (SdkError::Usage(_)
                    | SdkError::Protocol(_)
                    | SdkError::Serialization(_)
                    | SdkError::Transport(_)
                    | SdkError::Remote { .. }
                    | SdkError::Binding { .. }
                    | SdkError::AfterPublish { .. }
                    | SdkError::PlaneUnavailable { .. }) => {
                        return Err(format!(
                            "{} typed error drifted: expected code={} fragment={:?}, got {other:?}",
                            scenario.name, expected_error.code, expected_error.message_contains
                        )
                        .into());
                    }
                }
            }
        }
    }

    fixture.stop()
}

#[test]
fn sdk_text_frontdoor_rebinds_rev_at_time_generation_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let ancestor_batch = rev_at_time_lexical_batch(
        rev_at_time_ancestor_revision(),
        rev_at_time_ancestor_generation(),
        "fixture:rev-at-time:ancestor",
        None,
        "src/legacy.rs",
        "chunk-rev-at-time-ancestor",
        "needle_token legacy_choice",
    )?;
    let _ancestor_active = publish_and_activate_sdk_search_corpus(client, &ancestor_batch)?;
    let head_batch = rev_at_time_lexical_batch(
        rev_at_time_head_revision(),
        rev_at_time_head_generation(),
        "fixture:rev-at-time:head",
        Some("fixture:rev-at-time:ancestor"),
        "src/head.rs",
        "chunk-rev-at-time-head",
        "needle_token head_choice",
    )?;
    let _head_active = publish_and_activate_sdk_search_corpus(client, &head_batch)?;
    let _history_receipt = client.history().publish(&rev_at_time_history_batch()?)?;

    let head = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("rev:at.time(2100-01-01T00:00:00Z) needle_token")
                .pinned(rev_at_time_head_pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == rev_at_time_head_pin() && response.results.len() == 1,
    )?;
    let [head_candidate] = head.results.as_slice() else {
        return Err(format!("unexpected future rev:at.time response: {head:?}").into());
    };
    if head_candidate.candidate_id != "chunk-rev-at-time-head" {
        return Err(format!("unexpected future rev:at.time response: {head:?}").into());
    }

    let relative = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("rev:at.time(1 year ago) needle_token")
                .pinned(rev_at_time_head_pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == rev_at_time_ancestor_pin() && response.results.len() == 1,
    )?;
    let [relative_candidate] = relative.results.as_slice() else {
        return Err(format!("unexpected human relative rev:at.time response: {relative:?}").into());
    };
    if relative_candidate.candidate_id != "chunk-rev-at-time-ancestor" {
        return Err(format!("unexpected human relative rev:at.time response: {relative:?}").into());
    }

    let named = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("rev:at.time(yesterday) needle_token")
                .pinned(rev_at_time_head_pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == rev_at_time_ancestor_pin() && response.results.len() == 1,
    )?;
    let [named_candidate] = named.results.as_slice() else {
        return Err(format!("unexpected named relative rev:at.time response: {named:?}").into());
    };
    if named_candidate.candidate_id != "chunk-rev-at-time-ancestor" {
        return Err(format!("unexpected named relative rev:at.time response: {named:?}").into());
    }

    let calendar = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("rev:at.time(june 25 2017) needle_token")
                .pinned(rev_at_time_head_pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == rev_at_time_head_pin() && response.results.is_empty(),
    )?;
    if !calendar.results.is_empty() {
        return Err(format!("unexpected calendar rev:at.time response: {calendar:?}").into());
    }

    let invalid = expect_sdk_error(
        client
            .lexical()
            .query()
            .sourcegraph("rev:at.time(definitely-not-a-timeref) needle_token")
            .pinned(rev_at_time_head_pin())
            .top_k(10)
            .execute(),
        "rev:at.time invalid timeref",
    )?;
    expect_remote_code(invalid, "HISTORY_INVALID_TIMEREF")?;

    fixture.stop()
}

#[test]
fn sdk_history_query_frontdoor_surfaces_typed_absent_and_shard_errors() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let generation_not_ready = expect_sdk_error(
        client
            .history()
            .query()
            .sourcegraph("type:commit fix")
            .pinned(pin())
            .top_k(5)
            .order(HistoryOrderV1::Recency)
            .execute(),
        "history query without materialized authority should fail",
    )?;
    expect_remote_code(generation_not_ready, "HISTORY_GENERATION_NOT_READY")?;

    let _lexical_receipt = client.search_corpus().publish(&lexical_batch()?)?;
    let producer_unavailable = expect_sdk_error(
        client
            .history()
            .query()
            .sourcegraph("type:commit todo")
            .pinned(pin())
            .top_k(5)
            .order(HistoryOrderV1::Recency)
            .execute(),
        "history query should not fall back to lexical content",
    )?;
    expect_remote_code(producer_unavailable, "HISTORY_PRODUCER_UNAVAILABLE")?;

    let _history_receipt = client.history().publish(&history_commit_only_batch())?;
    let shard_unavailable = expect_sdk_error(
        client
            .history()
            .query()
            .sourcegraph("type:diff todo")
            .pinned(pin())
            .top_k(5)
            .order(HistoryOrderV1::Recency)
            .execute(),
        "history diff query should fail when diff shard is absent",
    )?;
    expect_remote_code(shard_unavailable, "HISTORY_SHARD_UNAVAILABLE")?;

    fixture.stop()
}

#[test]
fn sdk_structural_sourcegraph_frontdoor_supports_boolean_and_typed_hole_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    publish_sdk_search_corpus_ready(client)?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

    let typed_expr = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(r#"patterntype:structural "function_item { { :[name.expr] } }""#)
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &typed_expr,
        &pin(),
        "chunk-tree",
        "name",
        3,
        7,
        "sdk structural Sourcegraph typed expr",
    )?;

    let boolean_or = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(
                    r#"patterntype:structural "function_item { { identifier :[name] } }" OR "trait_item""#,
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &boolean_or,
        &pin(),
        "chunk-tree",
        "name",
        3,
        7,
        "sdk structural Sourcegraph boolean OR",
    )?;

    let boolean_not = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(
                    r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT "trait_item""#,
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &boolean_not,
        &pin(),
        "chunk-tree",
        "name",
        3,
        7,
        "sdk structural Sourcegraph boolean NOT",
    )?;

    fixture.stop()
}
