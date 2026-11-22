//! Pinned old-Q public SDK regression for filters on both Hybrid lanes.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::time::Duration;

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, HybridQueryRequest, HybridSeedQueryRequest,
    ManifestGeneration, PlannerStage, QueryConstraintSetV1, QueryResultWindowV2, RepoId,
    RepoRelativePath, RevisionId, SearchExplanation, SearchPlaneErrorCodeV2, TextQueryRequest,
    TextQuerySyntax, lex::LanguageCode,
};
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, legacy_source_replacement_from_chunks,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;
const DENSE_QUERY: &str = "needle focus";

struct Fixture {
    alpha: String,
    beta: String,
    gamma: String,
}

impl Fixture {
    fn all(&self) -> BTreeSet<&str> {
        [self.alpha.as_str(), self.beta.as_str(), self.gamma.as_str()]
            .into_iter()
            .collect()
    }
}

fn pin() -> Result<GenerationPin, Box<dyn Error>> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    ))
}

fn ingest_fixture(client: &QuantaIndex) -> Result<Fixture, Box<dyn Error>> {
    let pin = pin()?;
    let alpha = "e2e-1-src/lib.rs";
    let beta = "e2e-2-src/main.rs";
    let gamma = "e2e-3-src/lib.py";
    let chunk = |id: &str,
                 path: &str,
                 content: &str,
                 language: &str|
     -> Result<ChunkRecord, Box<dyn Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new(id),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new(language)?,
            start_byte: 0,
            end_byte: u32::try_from(content.len())?,
            start_line: 1,
            end_line: 2,
            text: content.to_owned().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        })
    };
    let chunks = vec![
        chunk(alpha, "src/lib.rs", "needle focus lib alpha", "rust")?,
        chunk(beta, "src/main.rs", "needle focus main beta", "rust")?,
        chunk(gamma, "src/lib.py", "needle focus lib gamma", "python")?,
    ];
    let batch = legacy_source_replacement_from_chunks(&pin.repo_id, &pin.revision_id, 1, chunks)?;
    let (receipt, activation) = client.search_corpus().publish_and_activate(&batch, None)?;
    if !receipt.applied
        || receipt.durable_sequence == 0
        || activation.active.generation.lexical.manifest_generation != pin.manifest_generation
        || client
            .generations()
            .active_head(pin.repo_id.clone(), pin.revision_id.clone())?
            .as_ref()
            != Some(&activation.active)
    {
        return Err("Hybrid fixture Source was not durably activated".into());
    }
    Ok(Fixture {
        alpha: alpha.to_owned(),
        beta: beta.to_owned(),
        gamma: gamma.to_owned(),
    })
}

struct HybridPage {
    ids: Vec<String>,
    window: QueryResultWindowV2,
    explanation: SearchExplanation,
}

fn hybrid_request(
    syntax: TextQuerySyntax,
    text_query: &str,
) -> Result<HybridQueryRequest, Box<dyn Error>> {
    let pin = pin()?;
    Ok(HybridQueryRequest {
        text_query: TextQueryRequest {
            syntax,
            query_text: text_query.to_owned(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: 50,
            cursor: None,
        },
        semantic_query_text: DENSE_QUERY.to_owned(),
        generation: Some(pin),
        generation_selector: None,
        top_k: 10,
    })
}

fn hybrid_page(client: &QuantaIndex, text_query: &str) -> Result<HybridPage, Box<dyn Error>> {
    hybrid_page_in(client, TextQuerySyntax::Sourcegraph, text_query)
}

fn hybrid_page_in(
    client: &QuantaIndex,
    syntax: TextQuerySyntax,
    text_query: &str,
) -> Result<HybridPage, Box<dyn Error>> {
    let result = client
        .search()
        .hybrid_request(hybrid_request(syntax, text_query)?)?;
    Ok(HybridPage {
        ids: result
            .results
            .iter()
            .map(|row| row.candidate.candidate_id.clone())
            .collect(),
        window: result.window,
        explanation: result.explanation,
    })
}

fn hybrid_refusal(
    client: &QuantaIndex,
    syntax: TextQuerySyntax,
    text_query: &str,
) -> Result<SearchPlaneErrorCodeV2, Box<dyn Error>> {
    match client
        .search()
        .hybrid_request(hybrid_request(syntax, text_query)?)
    {
        Err(SdkError::Remote { code, .. }) => Ok(code),
        other => Err(format!("hybrid `{text_query}` did not refuse remotely: {other:?}").into()),
    }
}

fn hybrid_seed_page(
    client: &QuantaIndex,
    text_query: &str,
) -> Result<(Vec<String>, QueryResultWindowV2, SearchExplanation), Box<dyn Error>> {
    let pin = pin()?;
    let response = client
        .search()
        .hybrid_seed_request(HybridSeedQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: text_query.to_owned(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            },
            semantic_query_text: DENSE_QUERY.to_owned(),
            generation: Some(pin),
            generation_selector: None,
            dense_corpora: Vec::new(),
            top_k: 10,
        })?;
    Ok((
        response
            .seed_candidates
            .iter()
            .map(|candidate| candidate.entity_id.clone())
            .collect(),
        response.window,
        response.explanation,
    ))
}

fn trace_detail(explanation: &SearchExplanation, prefix: &str) -> Option<String> {
    explanation
        .planner_trace
        .iter()
        .filter(|entry| matches!(entry.stage, PlannerStage::Plan | PlannerStage::ExecFanout))
        .find(|entry| entry.detail.starts_with(prefix))
        .map(|entry| entry.detail.clone())
}

fn as_set(ids: &[String]) -> BTreeSet<&str> {
    ids.iter().map(String::as_str).collect()
}

/// The fixture is what the tests assume: without a filter, the dense lane
/// ranks all three chunks, so any later exclusion is the filter's doing.
///
/// The caller first waits for the activated sealed generation to serve.
fn assert_dense_lane_ranks_all_three(client: &QuantaIndex, fixture: &Fixture) -> TestResult {
    let request = hybrid_request(TextQuerySyntax::Sourcegraph, "needle")?;
    let result = wait_for(
        &RealTicker::new(),
        Duration::from_secs(20),
        Duration::from_millis(50),
        "pinned old-Q Hybrid baseline ready",
        || client.search().hybrid_request(request.clone()),
        |_| true,
        |error| matches!(error, SdkError::Remote { code, .. } if code.as_wire_str() == "NOT_READY"),
    )?;
    let page = HybridPage {
        ids: result
            .results
            .iter()
            .map(|row| row.candidate.candidate_id.clone())
            .collect(),
        window: result.window,
        explanation: result.explanation,
    };
    if as_set(&page.ids) != fixture.all() {
        return Err(format!(
            "the unfiltered hybrid page must carry all three: {:?}",
            page.ids
        )
        .into());
    }
    let explanation = page.explanation;
    let lanes =
        trace_detail(&explanation, "hybrid.lanes=").ok_or("the hybrid trace names its lanes")?;
    if !lanes.contains("semantic_hits=3") {
        return Err(format!("the dense lane must rank all three chunks: {lanes}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — `file:` binds the dense lane: only the
// chunk in that file is served, though the dense lane ranks all three.
fn verify_file_filter(client: &QuantaIndex, fixture: &Fixture) -> TestResult {
    let page = hybrid_page(client, "file:src/lib.rs needle")?;
    if page.ids != [fixture.alpha.clone()] {
        return Err(format!(
            "file:src/lib.rs must serve alpha alone, got {:?} (beta={}, gamma={})",
            page.ids, fixture.beta, fixture.gamma
        )
        .into());
    }
    if page.window.returned() != 1
        || page.window.candidate_count().lower_bound() != 1
        || page.window.has_more() == Some(true)
    {
        return Err(format!(
            "expected one admitted row without observed continuation: {:?}",
            page.window
        )
        .into());
    }
    let filters = trace_detail(&page.explanation, "hybrid.filters=")
        .ok_or("the hybrid trace states the push-down class per filter")?;
    if filters != "hybrid.filters=exact:file" {
        return Err(format!("file must be classed exact: {filters}").into());
    }
    let admission = trace_detail(&page.explanation, "hybrid.dense_admission=")
        .ok_or("the hybrid trace states the dense admission outcome")?;
    if admission != "hybrid.dense_admission=exhausted; examined=3; admitted=1" {
        return Err(
            format!("the dense lane must examine all three and admit one: {admission}").into(),
        );
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — `repo:` excluding the generation answers
// an honest empty window, not the dense rows and not an error.
//
// The lexical repo predicate is a whole-term regex over the document's
// repo id, so `repo:other` names exactly the repo `other`; the fixture's
// generation belongs to another repo and no lane may answer from it.
fn verify_repo_filter(client: &QuantaIndex, _fixture: &Fixture) -> TestResult {
    let page = hybrid_page(client, "repo:other needle")?;
    if !page.ids.is_empty() {
        return Err(format!(
            "a repo the query excluded must not be served: {:?}",
            page.ids
        )
        .into());
    }
    if page.window.returned() != 0
        || page.window.candidate_count().lower_bound() != 0
        || page.window.has_more() == Some(true)
    {
        return Err(format!("expected an honest empty window: {:?}", page.window).into());
    }
    if page.explanation.strategy != "empty" || !page.explanation.engines_touched.is_empty() {
        return Err(format!(
            "an empty page claims no lane: {} {:?}",
            page.explanation.strategy, page.explanation.engines_touched
        )
        .into());
    }
    let filters = trace_detail(&page.explanation, "hybrid.filters=")
        .ok_or("the hybrid trace states the push-down class per filter")?;
    if filters != "hybrid.filters=exact:repo" {
        return Err(format!("repo must be classed exact: {filters}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — `lang:` is pushed down typed and `type:`
// evaluated exactly; each class is named in the trace.
fn verify_lang_and_type_filters(client: &QuantaIndex, fixture: &Fixture) -> TestResult {
    let page = hybrid_page(client, "lang:python needle")?;
    if page.ids != [fixture.gamma.clone()] {
        return Err(format!("lang:python must serve gamma alone: {:?}", page.ids).into());
    }
    let filters = trace_detail(&page.explanation, "hybrid.filters=")
        .ok_or("the hybrid trace states the push-down class per filter")?;
    if filters != "hybrid.filters=pushdown:lang" {
        return Err(format!("lang must be classed pushdown: {filters}").into());
    }

    // `type:file` admits text chunks: all three. This fixture did not
    // request symbol extraction, so `type:symbol` cannot claim that the
    // symbol universe is empty; strict coverage must refuse it.
    let page = hybrid_page(client, "type:file needle")?;
    if as_set(&page.ids) != fixture.all() {
        return Err(format!("type:file must serve every chunk: {:?}", page.ids).into());
    }
    let filters = trace_detail(&page.explanation, "hybrid.filters=")
        .ok_or("the hybrid trace states the push-down class per filter")?;
    if filters != "hybrid.filters=exact:type:file" {
        return Err(format!("type must be classed exact: {filters}").into());
    }
    let code = hybrid_refusal(client, TextQuerySyntax::Sourcegraph, "type:symbol needle")?;
    if code != SearchPlaneErrorCodeV2::SymbolCoverageIncomplete {
        return Err(format!("type:symbol needs complete coverage, got {code}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 / item 3 — a filter or option no lane can
// apply to dense rows is refused typed, never served lexical-only; and the
// native grammar has no negated filter, so `-file:` never reaches a lane.
fn verify_refusals(client: &QuantaIndex, _fixture: &Fixture) -> TestResult {
    for query in ["select:file needle", "count:2 needle", "type:path needle"] {
        let code = hybrid_refusal(client, TextQuerySyntax::Sourcegraph, query)?;
        if code.as_wire_str() != "HYBRID_FILTER_UNSUPPORTED" {
            return Err(
                format!("`{query}` must refuse HYBRID_FILTER_UNSUPPORTED, got {code}").into(),
            );
        }
    }
    // The native grammar refuses a dash before a filter at parse time
    // (`NOT/- without expression`): a negated filter is not a filter either
    // lane could apply, and the refusal is the parser's.
    let code = hybrid_refusal(client, TextQuerySyntax::Native, "-file:src/lib.rs needle")?;
    if code.as_wire_str() != "PARSE_FAIL" {
        return Err(format!("a negated filter must be refused by the grammar, got {code}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — the hybrid-seed route binds its dense
// lane under the same contract, on the same fixture.
fn verify_hybrid_seed_filters(client: &QuantaIndex, fixture: &Fixture) -> TestResult {
    let (seeds, _window, explanation) = hybrid_seed_page(client, "needle")?;
    if as_set(&seeds) != fixture.all() {
        return Err(format!("the unfiltered seed set must carry all three: {seeds:?}").into());
    }
    let admission = trace_detail(&explanation, "hybrid_seed.dense_admission[global]=")
        .ok_or("the seed trace states the dense admission outcome")?;
    if admission != "hybrid_seed.dense_admission[global]=not_needed" {
        return Err(format!("no filter, no admission: {admission}").into());
    }

    let (seeds, window, explanation) = hybrid_seed_page(client, "file:src/lib.rs needle")?;
    if seeds != [fixture.alpha.clone()] {
        return Err(format!("file:src/lib.rs must seed alpha alone: {seeds:?}").into());
    }
    if window.returned() != 1
        || window.candidate_count().lower_bound() != 1
        || window.has_more() == Some(true)
    {
        return Err(format!(
            "expected one admitted seed without observed continuation: {window:?}"
        )
        .into());
    }
    let admission = trace_detail(&explanation, "hybrid_seed.dense_admission[global]=")
        .ok_or("the seed trace states the dense admission outcome")?;
    if admission != "hybrid_seed.dense_admission[global]=exhausted; examined=3; admitted=1" {
        return Err(format!("the seed dense lane must admit alpha alone: {admission}").into());
    }
    let filters = trace_detail(&explanation, "hybrid_seed.filters=")
        .ok_or("the seed trace states the push-down class per filter")?;
    if filters != "hybrid_seed.filters=exact:file" {
        return Err(format!("file must be classed exact: {filters}").into());
    }

    let (seeds, window, _explanation) = hybrid_seed_page(client, "repo:other needle")?;
    if !seeds.is_empty() || window.returned() != 0 || window.candidate_count().lower_bound() != 0 {
        return Err(format!("an excluded repo must seed nothing: {seeds:?} {window:?}").into());
    }

    let (seeds, _window, _explanation) = hybrid_seed_page(client, "lang:python needle")?;
    if seeds != [fixture.gamma.clone()] {
        return Err(format!("lang:python must seed gamma alone: {seeds:?}").into());
    }

    match hybrid_seed_page(client, "select:file needle") {
        Ok((seeds, _, _)) => {
            return Err(
                format!("select:file must be refused on hybrid-seed, seeded {seeds:?}").into(),
            );
        }
        Err(err)
            if matches!(
                err.downcast_ref::<SdkError>(),
                Some(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::HybridFilterUnsupported,
                    ..
                })
            ) => {}
        Err(err) => return Err(format!("unexpected hybrid-seed refusal: {err}").into()),
    }
    Ok(())
}

#[test]
fn hybrid_filters_share_one_indexed_fixture() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let fixture = ingest_fixture(&client)?;
        let _ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q Hybrid backend ready",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        assert_dense_lane_ranks_all_three(&client, &fixture)?;
        let verify_file_filter_fn: fn(&QuantaIndex, &Fixture) -> TestResult = verify_file_filter;
        for (name, verify) in [
            ("file_filter", verify_file_filter_fn),
            ("repo_filter", verify_repo_filter),
            ("lang_and_type_filters", verify_lang_and_type_filters),
            ("typed_refusals", verify_refusals),
            ("hybrid_seed_filters", verify_hybrid_seed_filters),
        ] {
            verify(&client, &fixture)
                .map_err(|error| -> Box<dyn Error> { format!("{name}: {error}").into() })?;
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}
