//! Sealed semantic content roots are activation authority on pinned old-Q.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SearchCorpusGenerationIdentityV1,
    SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneTrackKind, SemanticContentRootsV1,
};
use quanta_index_sdk::PublishedBatchEvidence;
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths, legacy_source_batch,
        request_when_ready,
    },
    private_tempdir,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// A foreign row root must fail before activation mutates the active pair.
#[test]
fn activation_refuses_semantic_content_roots_the_generation_did_not_seal() -> TestResult {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let command = binary.benchmark_command(&state_root, 8)?;
    let process = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    let result = (|| -> TestResult {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let batch = legacy_source_batch(
            &repo,
            &revision,
            1,
            None,
            &[("src/first.rs", "fn first() { needle_first }", 1)],
        )?;
        let published = client.search_corpus().publish_outcome(&batch)?;
        if !published.receipt.applied || !published.receipt.sealed {
            return Err(format!("fixture Source did not seal: {published:?}").into());
        }
        let semantic_content = published
            .receipt
            .semantic_content
            .clone()
            .ok_or("sealed receipt attests no semantic content roots")?;
        let attested = SearchCorpusGenerationIdentityV1 {
            lexical: published.publication.target.clone(),
            semantic: quanta_index_contract::GenerationSnapshot {
                track: SearchPlaneTrackKind::Semantic,
                ..published.publication.target.clone()
            },
            semantic_content,
        };
        attested.validate_v1()?;

        let mut foreign = attested.clone();
        foreign.semantic_content = SemanticContentRootsV1 {
            row_root_digest: format!("sha256:{:0>64x}", 0xdead_beef_u64),
            membership_root_digest: attested.semantic_content.membership_root_digest.clone(),
        };
        let request = SearchPlaneControlIpcRequestEnvelope {
            request_id: 1,
            diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
            payload: SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate: foreign,
                    expected_active: None,
                },
            ),
        };
        let refused: SearchPlaneControlIpcResponseEnvelope = quanta_index_ipc::send_request(
            &daemon_socket_paths(&state_root)[1],
            &request,
            quanta_index_ipc::ClientIoPolicy::default(),
        )?;
        if refused.request_id != request.request_id {
            return Err(format!("foreign root response changed request ID: {refused:?}").into());
        }
        let SearchPlaneControlIpcResponse::Error(error) = &refused.payload else {
            return Err(format!(
                "foreign roots must be refused typed as SEMANTIC_ROW_ROOT_MISMATCH, got {refused:?}"
            )
            .into());
        };
        if error.code.as_wire_str() != "SEMANTIC_ROW_ROOT_MISMATCH" {
            return Err(format!(
                "foreign roots must be refused typed as SEMANTIC_ROW_ROOT_MISMATCH, got {}: {}",
                error.code, error.message
            )
            .into());
        }
        if !error
            .message
            .contains(&attested.semantic_content.row_root_digest)
        {
            return Err(format!("the refusal names the sealed root: {}", error.message).into());
        }

        let status = client
            .generations()
            .status(repo.clone(), revision.clone())?;
        if !status.tracks.is_empty() || status.semantic_content.is_some() {
            return Err(format!("a refused activation must not activate: {status:?}").into());
        }

        let activated = client
            .search_corpus()
            .activate_published(&PublishedBatchEvidence::from(&published), None)?;
        if activated.active.generation != attested {
            return Err(format!("activation changed sealed identity: {activated:?}").into());
        }
        let pin = GenerationPin::new(repo.clone(), revision.clone(), ManifestGeneration::new(1));
        let served = request_when_ready(|| {
            client
                .lexical()
                .query()
                .native("needle_first")
                .pinned(pin.clone())
                .top_k(5)
                .execute()
        })?;
        if served.generation != pin
            || served.results.len() != 1
            || served
                .results
                .first()
                .is_none_or(|row| row.repo_relative_path.as_str() != "src/first.rs")
        {
            return Err(
                format!("the attested candidate did not serve its source: {served:?}").into(),
            );
        }
        let status = client.generations().status(repo, revision)?;
        if status.semantic_content.as_ref() != Some(&attested.semantic_content) {
            return Err(format!(
                "the status report names the roots the pair was activated under: {status:?}"
            )
            .into());
        }
        Ok(())
    })();
    let stopped = process.stop();
    result.and(stopped)?;
    binary.verify()
}
