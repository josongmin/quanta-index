use super::*;

#[test]
fn sdk_binary_process_dsl_roundtrip() -> TestResult {
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let runtime = SearchdBinaryProcess::start(dir.path())?;
    let result = (|| -> TestResult {
        let client = runtime.connect()?;
        // The repo-meta overlay is part of the sealed contract: published
        // before the seal, never into the sealed generation (QI-BB-030).
        let _repo_meta_receipt = client.history().publish_repo_meta(&repo_meta_batch())?;
        let batch = lexical_frontdoor_matrix_batch()?;
        let active = publish_and_activate_sdk_search_corpus(&client, &batch)?;
        if active.lexical.manifest_generation != generation()
            || active.semantic.manifest_generation != generation()
            || active.lexical.manifest_digest != batch.manifest_digest()
            || active.semantic.manifest_digest != batch.manifest_digest()
        {
            return Err(format!(
                "binary process promoted an unexpected composite generation: {active:?}"
            )
            .into());
        }

        let response = wait_for_sdk_observation(
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
        let paths = response
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str().to_string())
            .collect::<BTreeSet<_>>();
        if response.generation != pin()
            || paths != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
        {
            return Err(format!(
                "binary process DSL query diverged: response={response:?} paths={paths:?}"
            )
            .into());
        }

        let predicate = wait_for_sdk_observation(
            SOCKET_TIMEOUT,
            || {
                client
                    .lexical()
                    .query()
                    .sourcegraph("repo:has.meta(license:apache-2.0) shared_oracle_needle")
                    .active(repo(), revision())
                    .top_k(5)
                    .execute()
            },
            // The predicate query correlates two source rows (TOPT-06:
            // the old timeout-as-Ok wait masked this `len == 1` never
            // becoming ready; the assertion below always wanted both).
            |response| response.generation == pin() && response.results.len() == 2,
        )?;
        let predicate_paths = predicate
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str().to_string())
            .collect::<Vec<_>>();
        if predicate.generation != pin()
            || predicate_paths
                != [
                    "src/recency_a.rs".to_string(),
                    "src/recency_gate.rs".to_string(),
                ]
        {
            return Err(format!(
                "binary process predicate query lost source-repo correlation: response={predicate:?}"
            )
            .into());
        }
        Ok(())
    })();
    let stop = runtime.stop();
    result.and(stop)
}

fn assert_binary_semantic_work_settlement(client: &QuantaIndex) -> TestResult {
    let mut request = quanta_index_contract::SemanticWorkBoundedQueryRequestV1 {
        query: SemanticQueryRequest {
            query_text: "quartz".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin()),
            generation_selector: None,
            lexical_scope: None,
            top_k: 2,
        },
        max_work_units: 1_000_000,
    };
    let response = client.semantic().query_work_bounded_v1(request.clone())?;
    if response.query.generation != pin()
        || response.query.results.is_empty()
        || response.charged_work_units < 2
        || response.charged_work_units > request.max_work_units
    {
        return Err(format!("invalid bounded semantic settlement: {response:?}").into());
    }
    for allowance in [
        0,
        quanta_index_contract::SEMANTIC_WORK_OPERATIONAL_CAP_V1 + 1,
    ] {
        request.max_work_units = allowance;
        let refused = client.semantic().query_work_bounded_v1(request.clone());
        if !matches!(
            refused,
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::InvalidRequest,
                ..
            })
        ) {
            return Err(
                format!("invalid allowance {allowance} failed to refuse: {refused:?}").into(),
            );
        }
    }
    request.max_work_units = response.charged_work_units;
    let exact = client.semantic().query_work_bounded_v1(request.clone())?;
    if exact.charged_work_units != response.charged_work_units
        || exact.query.results != response.query.results
    {
        return Err("exact work allowance changed semantic results or settlement".into());
    }
    for allowance in [
        1,
        response
            .charged_work_units
            .checked_sub(1)
            .expect("successful bounded query charged work"),
    ] {
        request.max_work_units = allowance;
        let refused = client.semantic().query_work_bounded_v1(request.clone());
        if !matches!(
            refused,
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::SemanticWorkBudgetExceeded,
                ..
            })
        ) {
            return Err(format!("allowance {allowance} failed to refuse: {refused:?}").into());
        }
    }
    request.max_work_units = response.charged_work_units;
    let retried = client.semantic().query_work_bounded_v1(request)?;
    if retried.query.results != response.query.results {
        return Err("a refused request changed the next semantic result".into());
    }
    Ok(())
}

#[test]
fn sdk_binary_semantic_work_settlement_survives_refusal_and_restart() -> TestResult {
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let runtime = SearchdBinaryProcess::start(dir.path())?;
    let client = runtime.connect()?;
    let _active = publish_and_activate_sdk_search_corpus(&client, &lexical_batch()?)?;
    assert_binary_semantic_work_settlement(&client)?;
    runtime.stop()?;
    let restarted = SearchdBinaryProcess::start(dir.path())?;
    assert_binary_semantic_work_settlement(&restarted.connect()?)?;
    restarted.stop()
}
