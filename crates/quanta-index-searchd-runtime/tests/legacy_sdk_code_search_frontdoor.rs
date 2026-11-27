//! Default CodeSearch file, cursor, and regex contract through a pinned old-Q SDK.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::time::Duration;

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SEMANTIC_WORK_OPERATIONAL_CAP_V1,
    SearchPlaneErrorCodeV2, SemanticQueryRequest, SemanticWorkBoundedQueryRequestV1,
};
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_searchd_harness::{legacy_reference::LegacySearchdBinary, private_tempdir};

use crate::fail_closed_wait::{RealTicker, wait_for};
use crate::sdk_frontdoor_lexical_fixture::{
    matrix_repo_meta_batch, matrix_source_batch, source_batch,
};

type TestResult = Result<(), Box<dyn Error>>;

fn pin() -> Result<GenerationPin, Box<dyn Error>> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    ))
}

#[test]
fn sdk_default_code_search_matches_terms_across_chunks_as_one_file() -> TestResult {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let pin = pin()?;
        let corpus_batch = source_batch(&pin)?;
        let (receipt, activation) = client
            .search_corpus()
            .publish_and_activate(&corpus_batch, None)?;
        if !receipt.applied
            || receipt.durable_sequence == 0
            || activation.active.generation.lexical.manifest_generation != pin.manifest_generation
            || client
                .generations()
                .active_head(pin.repo_id.clone(), pin.revision_id.clone())?
                != Some(activation.active)
        {
            return Err("CodeSearch fixture Source did not activate durably".into());
        }

        let response = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q CodeSearch file baseline ready",
            || {
                client
                    .lexical()
                    .query()
                    .text("todo main")
                    .active(pin.repo_id.clone(), pin.revision_id.clone())
                    .top_k(10)
                    .execute()
            },
            |response| response.generation == pin && response.results.len() == 1,
            |error| matches!(error, SdkError::Remote { code, .. } if code.as_wire_str() == "NOT_READY"),
        )?;
        let file = response.results.first().ok_or("missing file result")?;
        let scope = corpus_batch
            .replace_scopes()
            .iter()
            .find(|scope| scope.coverage.source.file.repo_relative_path.as_str() == "src/lib.rs")
            .ok_or("missing replacement scope")?;
        if file.repo_relative_path.as_str() != "src/lib.rs"
            || !file.candidate_id.starts_with("file:")
            || file.source.as_ref().map(|source| source.source_sha256)
                != Some(scope.coverage.source.source_sha256)
        {
            return Err(format!("default CodeSearch file identity drift: {file:?}").into());
        }

        let two_files = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q CodeSearch two-file baseline ready",
            || {
                client
                    .lexical()
                    .query()
                    .text("sphinx")
                    .active(pin.repo_id.clone(), pin.revision_id.clone())
                    .top_k(10)
                    .execute()
            },
            |response| response.generation == pin && response.results.len() == 2,
            |error| matches!(error, SdkError::Remote { code, .. } if code.as_wire_str() == "NOT_READY"),
        )?;
        let paths = two_files
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str())
            .collect::<BTreeSet<_>>();
        if paths != BTreeSet::from(["src/alpha.rs", "src/beta.rs"]) {
            return Err(format!("CodeSearch file ranking returned {paths:?}").into());
        }

        let first_page = client
            .lexical()
            .query()
            .text("sphinx")
            .active(pin.repo_id.clone(), pin.revision_id.clone())
            .top_k(1)
            .execute()?;
        if first_page.rank_unit != quanta_index_contract::TextRankUnit::File
            || first_page.results.len() != 1
        {
            return Err(format!("CodeSearch first file page drift: {first_page:?}").into());
        }
        let cursor = first_page
            .next_cursor
            .ok_or("CodeSearch first file page lacks continuation")?;
        let second_page = client
            .lexical()
            .query()
            .text("sphinx")
            .pinned(first_page.generation.clone())
            .top_k(1)
            .after(cursor)
            .execute()?;
        let first_file = first_page
            .results
            .first()
            .ok_or("missing first file page result")?;
        let second_file = second_page
            .results
            .first()
            .ok_or("missing second file page result")?;
        if second_page.rank_unit != quanta_index_contract::TextRankUnit::File
            || second_page.results.len() != 1
            || second_page.next_cursor.is_some()
            || first_file.repo_relative_path == second_file.repo_relative_path
        {
            return Err(format!("CodeSearch second file page drift: {second_page:?}").into());
        }

        for (query, expected_path) in [
            ("regex:/sphinx.*quartz/", "src/alpha.rs"),
            ("content:regex:/sphinx.*riddles/", "src/beta.rs"),
            ("path:regex:/beta[.]rs/", "src/beta.rs"),
            ("sphinx regex:/quartz/", "src/alpha.rs"),
        ] {
            let response = client
                .lexical()
                .query()
                .text(query)
                .active(pin.repo_id.clone(), pin.revision_id.clone())
                .top_k(10)
                .execute()?;
            let actual = response
                .results
                .iter()
                .map(|candidate| candidate.repo_relative_path.as_str())
                .collect::<Vec<_>>();
            if actual != [expected_path] {
                return Err(format!("CodeSearch regex {query:?} returned {actual:?}").into());
            }
            if query == "path:regex:/beta[.]rs/" {
                let hit = response.results.first().ok_or("missing path hit")?;
                if hit.preview.as_ref().map(|preview| preview.kind)
                    != Some(quanta_index_contract::PreviewKind::Path)
                    || hit
                        .excerpt
                        .as_ref()
                        .expect("fixture row carries a match excerpt")
                        .snippet
                        != "src/beta.rs"
                    || hit
                        .excerpt
                        .as_ref()
                        .expect("fixture row carries a match excerpt")
                        .snippet_hit_offset
                        != Some(4)
                    || hit
                        .excerpt
                        .as_ref()
                        .expect("fixture row carries a match excerpt")
                        .highlights
                        != [quanta_index_contract::HighlightSpan { start: 4, len: 7 }]
                {
                    return Err(format!("CodeSearch path match position drift: {hit:?}").into());
                }
            }
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

#[test]
fn sdk_binary_process_dsl_roundtrip() -> TestResult {
    let parent = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(parent.path(), 8)?;
    let result = (|| -> TestResult {
        let client = process.connect()?;
        let pin = GenerationPin::new(
            RepoId::new("repo-sdk")?,
            RevisionId::new("rev-sdk")?,
            ManifestGeneration::new(1),
        );
        let batch = matrix_source_batch(&pin)?;

        let request = batch.generation_reservation_request()?;
        let reservation = client.search_corpus().reserve_generation(&request)?;
        if reservation.repo_id != request.repo_id
            || reservation.revision_id != request.revision_id
            || reservation.source_event != request.source_event
            || reservation.generation != batch.generation()
        {
            return Err(format!("unexpected matrix Source reservation: {reservation:?}").into());
        }

        // The generation-scoped overlay is committed before the Source seal.
        let _repo_meta_receipt = client
            .history()
            .publish_repo_meta(&matrix_repo_meta_batch(&pin)?)?;
        let expected_active = client
            .generations()
            .active_head(pin.repo_id.clone(), pin.revision_id.clone())?;
        let (receipt, activation) = client
            .search_corpus()
            .publish_and_activate(&batch, expected_active)?;
        let active = activation.active.generation;
        if !receipt.applied
            || receipt.durable_sequence == 0
            || active.lexical.manifest_generation != pin.manifest_generation
            || active.semantic.manifest_generation != pin.manifest_generation
            || active.lexical.manifest_digest != batch.manifest_digest()
            || active.semantic.manifest_digest != batch.manifest_digest()
        {
            return Err(format!(
                "binary process promoted an unexpected composite generation: {active:?}"
            )
            .into());
        }

        let response = wait_for(
            &RealTicker::new(),
            Duration::from_secs(5),
            Duration::from_millis(10),
            "pinned old-Q select:path DSL ready",
            || {
                client
                    .lexical()
                    .query()
                    .native("select:path sphinx")
                    .active(pin.repo_id.clone(), pin.revision_id.clone())
                    .top_k(5)
                    .execute()
            },
            |response| response.generation == pin && response.results.len() == 2,
            |error| matches!(error, SdkError::Remote { code, .. } if code.as_wire_str() == "NOT_READY"),
        )?;
        let paths = response
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str().to_string())
            .collect::<BTreeSet<_>>();
        if response.generation != pin
            || paths != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
        {
            return Err(format!(
                "binary process DSL query diverged: response={response:?} paths={paths:?}"
            )
            .into());
        }

        let predicate = wait_for(
            &RealTicker::new(),
            Duration::from_secs(5),
            Duration::from_millis(10),
            "pinned old-Q repo metadata DSL ready",
            || {
                client
                    .lexical()
                    .query()
                    .sourcegraph("repo:has.meta(license:apache-2.0) shared_oracle_needle")
                    .active(pin.repo_id.clone(), pin.revision_id.clone())
                    .top_k(5)
                    .execute()
            },
            |response| response.generation == pin && response.results.len() == 2,
            |error| matches!(error, SdkError::Remote { code, .. } if code.as_wire_str() == "NOT_READY"),
        )?;
        let predicate_paths = predicate
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str().to_string())
            .collect::<Vec<_>>();
        if predicate.generation != pin
            || predicate_paths
                != [
                    "src/recency_a.rs".to_string(),
                    "src/recency_gate.rs".to_string(),
                ]
        {
            return Err(format!("binary process predicate query diverged: response={predicate:?} paths={predicate_paths:?}").into());
        }
        Ok(())
    })();
    let stopped = process.stop();
    result.and(stopped)?;
    binary.verify()
}

fn assert_binary_semantic_work_settlement(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    let mut request = SemanticWorkBoundedQueryRequestV1 {
        query: SemanticQueryRequest {
            query_text: "quartz".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: None,
            top_k: 2,
        },
        max_work_units: 1_000_000,
        canonical_ties: false,
    };
    let response = client.semantic().query_work_bounded_v1(request.clone())?;
    if response.query.generation != *pin
        || response.query.results.is_empty()
        || response.charged_work_units < 2
        || response.charged_work_units > request.max_work_units
    {
        return Err(format!("invalid bounded semantic settlement: {response:?}").into());
    }
    for allowance in [0, SEMANTIC_WORK_OPERATIONAL_CAP_V1 + 1] {
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
            .ok_or("successful bounded query did not charge work")?,
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
    let pin = GenerationPin::new(
        RepoId::new("repo-sdk")?,
        RevisionId::new("rev-sdk")?,
        ManifestGeneration::new(1),
    );
    let dir = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(dir.path(), 8)?;
    let result = (|| -> TestResult {
        let client = process.connect()?;
        let batch = source_batch(&pin)?;
        let expected_active = client
            .generations()
            .active_head(pin.repo_id.clone(), pin.revision_id.clone())?;
        let (receipt, activation) = client
            .search_corpus()
            .publish_and_activate(&batch, expected_active)?;
        if !receipt.applied
            || receipt.durable_sequence == 0
            || activation.active.generation.lexical.manifest_generation != pin.manifest_generation
            || activation.active.generation.semantic.manifest_generation != pin.manifest_generation
            || activation.active.generation.lexical.manifest_digest != batch.manifest_digest()
            || activation.active.generation.semantic.manifest_digest != batch.manifest_digest()
        {
            return Err(format!(
                "semantic fixture promoted an unexpected generation: {activation:?}"
            )
            .into());
        }
        assert_binary_semantic_work_settlement(&client, &pin)
    })();
    let stopped = process.stop();
    result.and(stopped)?;

    let restarted = binary.start(dir.path(), 8)?;
    let result = (|| -> TestResult {
        let client = restarted.connect()?;
        assert_binary_semantic_work_settlement(&client, &pin)
    })();
    let stopped = restarted.stop();
    result.and(stopped)?;
    binary.verify()
}
