//! Read-view domain and explanation contracts through the pinned old-Q daemon.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;

use quanta_index_contract::{
    DiagnosticAttemptFieldV1, ExplainCandidateV1, GenerationPin, HistoryOrderV1,
    HistoryQueryRequest, HybridQueryRequest, PlannerStage, QueryConstraintSetV1,
    RuntimeMetadataQueryRequest, SearchExplanation, SearchPlaneErrorCodeV2,
    SearchPlaneExplainQueryRequest, SearchPlaneQueryIpcRequest as Request,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse as Response,
    SearchPlaneQueryIpcResponseEnvelope, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{
    REPO_COMMIT_RECENCY_UNAVAILABLE_CODE, RUNTIME_NOT_READY_CODE, RepoMetadataAuthorityV1,
};
use quanta_index_sdk::{RepoId, RevisionId};
use quanta_index_searchd_harness::{
    legacy_reference::{LegacySearchdBinary, daemon_socket_paths, publish_legacy_source},
    private_tempdir,
};

#[path = "support/read_view_daemon_witness.rs"]
mod read_view_daemon_witness;

type TestResult = Result<(), Box<dyn Error>>;
const QUERY: &str = "needle";
const TOP_K: u32 = 10;
const HISTORY_UNAVAILABLE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::HistoryProducerUnavailable;

struct PinnedQueries {
    socket: PathBuf,
    pin: GenerationPin,
    next_id: u64,
}

impl PinnedQueries {
    fn text(&self, syntax: TextQuerySyntax, query: &str, top_k: u32) -> TextQueryRequest {
        TextQueryRequest {
            syntax,
            query_text: query.to_owned(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(self.pin.clone()),
            generation_selector: None,
            top_k,
            cursor: None,
        }
    }

    fn send(&mut self, payload: Request) -> Result<Response, Box<dyn Error>> {
        let request_id = self.next_id;
        self.next_id += 1;
        let response: SearchPlaneQueryIpcResponseEnvelope = quanta_index_ipc::send_request(
            &self.socket,
            &SearchPlaneQueryIpcRequestEnvelope {
                request_id,
                diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
                payload,
            },
            quanta_index_ipc::ClientIoPolicy::default(),
        )?;
        if response.request_id != request_id {
            return Err(format!("old-Q returned a foreign request id: {response:?}").into());
        }
        Ok(response.payload)
    }

    fn require_refusal(
        &mut self,
        request: Request,
        expected: SearchPlaneErrorCodeV2,
        what: &str,
    ) -> TestResult {
        let response = self.send(request)?;
        if !matches!(&response, Response::Error(error) if error.code == expected) {
            return Err(format!("{what}: expected {expected}, got {response:?}").into());
        }
        Ok(())
    }
}

fn require_trace(
    explain: &SearchExplanation,
    domains: &str,
    pin_detail: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let trace: Vec<_> = explain
        .planner_trace
        .iter()
        .filter(|entry| entry.stage == PlannerStage::Plan)
        .map(|entry| entry.detail.clone())
        .collect();
    let expected = [
        format!("read_view.domains={domains}"),
        "read_view.epochs=-".to_owned(),
        format!("read_view.pin={pin_detail}"),
    ];
    if !trace.starts_with(&expected) {
        return Err(format!("unexpected {domains} plan for {pin_detail}: {trace:?}").into());
    }
    Ok(trace)
}

fn verify_domain_refusals(
    queries: &mut PinnedQueries,
) -> Result<quanta_index_contract::LexicalCandidate, Box<dyn Error>> {
    let plain_request = queries.text(TextQuerySyntax::Native, QUERY, TOP_K);
    let Response::Text(plain) = queries.send(Request::Text(plain_request.clone()))? else {
        return Err("sealed lexical track did not serve plain text".into());
    };
    if plain.results.len() != 2 {
        return Err(format!("expected two fixture rows, got {:?}", plain.results).into());
    }

    let at_time = "rev:at.time(2024-01-01) needle";
    queries.require_refusal(
        Request::Text(queries.text(TextQuerySyntax::Sourcegraph, at_time, TOP_K)),
        HISTORY_UNAVAILABLE,
        at_time,
    )?;
    if REPO_COMMIT_RECENCY_UNAVAILABLE_CODE
        != RepoMetadataAuthorityV1::CommitRecency.unavailable_code()
    {
        return Err("commit-recency code differs from its authority".into());
    }
    for (query, authority) in [
        (
            "repo:has.commit.after(2020-01-01) needle",
            RepoMetadataAuthorityV1::CommitRecency,
        ),
        (
            "repo:has.meta(team:core) needle",
            RepoMetadataAuthorityV1::Meta,
        ),
        (
            "repo:has.topic(security) needle",
            RepoMetadataAuthorityV1::Topic,
        ),
        (
            "repo:has.description(\"search\") needle",
            RepoMetadataAuthorityV1::Description,
        ),
        (
            "file:has.owner(@alice) needle",
            RepoMetadataAuthorityV1::FileOwnership,
        ),
        (
            "select:file.owners needle",
            RepoMetadataAuthorityV1::FileOwnership,
        ),
        (
            "file:has.contributor(alice) needle",
            RepoMetadataAuthorityV1::Contributor,
        ),
    ] {
        queries.require_refusal(
            Request::Text(queries.text(TextQuerySyntax::Sourcegraph, query, TOP_K)),
            authority.unavailable_code(),
            query,
        )?;
    }
    queries.require_refusal(
        Request::History(HistoryQueryRequest {
            text_query: queries.text(TextQuerySyntax::Native, "type:commit needle", TOP_K),
            order: HistoryOrderV1::Recency,
            cursor: None,
        }),
        HISTORY_UNAVAILABLE,
        "history route",
    )?;
    queries.require_refusal(
        Request::RuntimeMetadata(RuntimeMetadataQueryRequest {
            text_query: queries.text(TextQuerySyntax::Native, "dirty:yes needle", TOP_K),
            cursor: None,
        }),
        RUNTIME_NOT_READY_CODE,
        "runtime metadata route",
    )?;
    let Response::Text(after) = queries.send(Request::Text(plain_request))? else {
        return Err("plain text did not serve after the refusal cases".into());
    };
    if !after
        .results
        .iter()
        .map(|row| &row.candidate_id)
        .eq(plain.results.iter().map(|row| &row.candidate_id))
    {
        return Err("domain refusal changed the plain lexical results".into());
    }

    after
        .results
        .into_iter()
        .next()
        .ok_or_else(|| "expected a lexical candidate".into())
}

fn verify_explanations(
    queries: &mut PinnedQueries,
    pin_detail: &str,
    candidate: quanta_index_contract::LexicalCandidate,
) -> TestResult {
    let Response::Hybrid(hybrid) = queries.send(Request::Hybrid(HybridQueryRequest {
        text_query: queries.text(TextQuerySyntax::Native, QUERY, 50),
        semantic_query_text: QUERY.to_owned(),
        generation: Some(queries.pin.clone()),
        generation_selector: None,
        top_k: TOP_K,
    }))?
    else {
        return Err("hybrid did not serve the sealed tracks".into());
    };
    let trace = require_trace(&hybrid.explanation, "lexical,semantic", pin_detail)?;
    for prefix in [
        "read_view.lexical_artifact=",
        "read_view.normalizer=",
        "read_view.semantic_artifact=",
        "read_view.profile=",
    ] {
        if !trace.iter().any(|detail| detail.starts_with(prefix)) {
            return Err(format!("hybrid trace omitted {prefix}: {trace:?}").into());
        }
    }
    for (domains, scope) in [
        ("semantic", None),
        (
            "lexical,semantic",
            Some(queries.text(TextQuerySyntax::Native, QUERY, 2)),
        ),
    ] {
        let Response::Semantic(semantic) =
            queries.send(Request::Semantic(SemanticQueryRequest {
                query_text: QUERY.to_owned(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(queries.pin.clone()),
                generation_selector: None,
                lexical_scope: scope,
                top_k: TOP_K,
            }))?
        else {
            return Err(format!("{domains} semantic query did not serve").into());
        };
        let trace = require_trace(&semantic.explanation, domains, pin_detail)?;
        if domains == "semantic"
            && trace
                .iter()
                .any(|detail| detail.starts_with("read_view.lexical_artifact="))
        {
            return Err(
                format!("unscoped semantic trace carried a lexical artifact: {trace:?}").into(),
            );
        }
    }
    let Response::Explain(explain) =
        queries.send(Request::Explain(SearchPlaneExplainQueryRequest {
            generation: queries.pin.clone(),
            candidate: ExplainCandidateV1::Lexical(candidate),
            text_query: None,
            semantic_query_text: None,
        }))?
    else {
        return Err("presence explain did not serve the lexical candidate".into());
    };
    let _trace = require_trace(&explain.explanation, "lexical", pin_detail)?;
    Ok(())
}

#[test]
fn sealed_track_read_view_scenarios_share_one_fixture() -> TestResult {
    let directory = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(directory.path(), 8)?;
    let client = process.connect()?;
    let repo = RepoId::new("read-view-domains")?;
    let revision = RevisionId::new("read-view-rev")?;
    let (head, _receipt) = publish_legacy_source(
        &client,
        &repo,
        &revision,
        1,
        None,
        &[
            ("src/needle.rs", "fn needle() { let needle = 1; }", 1),
            ("src/other.rs", "fn other() { let needle = 2; }", 1),
        ],
        None,
    )?;
    let pin = GenerationPin::new(repo, revision, head.generation.lexical.manifest_generation);
    let pin_detail = format!(
        "{}@{}#{}",
        pin.repo_id.as_str(),
        pin.revision_id.as_str(),
        pin.manifest_generation.get()
    );
    let [socket, _, _] = daemon_socket_paths(directory.path());
    let mut queries = PinnedQueries {
        socket,
        pin,
        next_id: 1,
    };

    let candidate = verify_domain_refusals(&mut queries)?;
    verify_explanations(&mut queries, &pin_detail, candidate)?;
    drop(client);
    let witness = read_view_daemon_witness::observe(process.process_id())?;
    process.stop()?;
    binary.verify()?;
    read_view_daemon_witness::write(
        [witness],
        "p04-read-view-domains.json",
        concat!(
            env!("CARGO_PKG_NAME"),
            "::process_readiness_owner_v1$",
            "legacy_read_view_domains::sealed_track_read_view_scenarios_share_one_fixture",
        ),
        None,
    )
}
