use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode, ParseRoleTag,
    ParseTreeRecord, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship,
    SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ActiveGenerationResolutionV1, BatchPublishReceipt, CapabilityStatusV1, ChunkId, ChunkRecord,
    ContinuationTokenV2, DiffHunkSide, ExactRepoRelativePathV1, GenerationSelector,
    GenerationSnapshot, HistoryQueryRequest, HybridSeedQueryResponse, ManifestGeneration,
    OwnerDocKind, PlannerStage, PlannerTraceEntry, QueryResultWindowV2, RepoId,
    RepoMapChunkExactness, RepoMapExactnessSummary, RepoMapGraphCoverageClass,
    RepoMapItemIndexAvailability, RepoMapMutationAck, RepoMapRedactionState, RepoRelativePath,
    RevisionId, RuntimeMetadataQueryRequest, SearchCorpusActivationTokenV1,
    SearchCorpusActiveHeadV1, SearchCorpusGenerationIdentityV1, SearchExplanation,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneErrorCodeV2, SearchPlaneHistoryQueryResponse, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneIpcError, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneRuntimeMetadataQueryResponse,
    SearchPlaneSearchCorpusActivationCasAck, SearchPlaneSearchCorpusRollbackCasAck,
    SearchPlaneStructuralQueryResponse, SearchScopeKey, SearchScopeSurface, SemanticCorpusKindV1,
    SemanticQueryResponse, SemanticSourceRecordV1, SemanticSourceScopeKeyV1, SourceRoleV1,
    StructuralQueryRequest, SymbolId, TextQueryResponse,
};

use crate::config::{EnvLookup, EnvLookupError};
use crate::{
    ConnectOptions, ControlTransport, DirtyBatch, HistoryBatch, IngestTransport, QuantaIndex,
    QueryTransport, SearchCorpusBatch, StructuralBatch, Track,
};

/// QI-SDK-01: small helper to unwrap a `Result` inside a `#[test]` with
/// a clear panic message. Replaces an earlier `assert!(false, ...)` +
/// `return;` macro that tripped `clippy::assertions_on_constants`.
macro_rules! ok_or_fail {
    ($expr:expr $(,)?) => {
        match $expr {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err}"),
        }
    };
}

struct StubQueryTransport {
    requests: Mutex<Vec<SearchPlaneQueryIpcRequestEnvelope>>,
    responses: Mutex<VecDeque<SearchPlaneQueryIpcResponse>>,
    active_resolution: Option<GenerationSnapshot>,
}

impl StubQueryTransport {
    fn new(response: SearchPlaneQueryIpcResponse) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(VecDeque::from([response])),
            active_resolution: Some(GenerationSnapshot {
                repo_id: repo_id(),
                revision_id: revision_id(),
                track: quanta_index_contract::SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_string(),
            }),
        }
    }

    fn sequence(responses: impl IntoIterator<Item = SearchPlaneQueryIpcResponse>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into_iter().collect()),
            active_resolution: None,
        }
    }

    fn active(mut response: SearchPlaneQueryIpcResponse) -> Self {
        let head = search_corpus_head(
            7,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1,
        );
        match &mut response {
            SearchPlaneQueryIpcResponse::Text(page) => page.selected_active_head = Some(head),
            SearchPlaneQueryIpcResponse::Symbol(page) => page.selected_active_head = Some(head),
            SearchPlaneQueryIpcResponse::Semantic(page) => page.selected_active_head = Some(head),
            SearchPlaneQueryIpcResponse::Hybrid(page) => page.selected_active_head = Some(head),
            SearchPlaneQueryIpcResponse::HybridSeed(page) => page.selected_active_head = Some(head),
            SearchPlaneQueryIpcResponse::History(page) => page.selected_active_head = Some(head),
            SearchPlaneQueryIpcResponse::RuntimeMetadata(page) => {
                page.selected_active_head = Some(head);
            }
            SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
            | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
            | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::Error(_) => {}
        }
        Self::new(response)
    }
}

impl QueryTransport for StubQueryTransport {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, crate::SdkError> {
        self.requests
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("query transport poisoned: {err}")))?
            .push(request.clone());
        if let quanta_index_contract::SearchPlaneQueryIpcRequest::ResolveActiveGeneration(resolve) =
            &request.payload
            && let Some(mut snapshot) = self.active_resolution.clone()
        {
            snapshot.repo_id = resolve.repo_id.clone();
            snapshot.revision_id = resolve.revision_id.clone();
            snapshot.track = resolve.track;
            return Ok(SearchPlaneQueryIpcResponseEnvelope {
                request_id: request.request_id,
                payload: SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(active_resolution(
                    snapshot,
                )),
            });
        }
        let payload = self
            .responses
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("query response poisoned: {err}")))?
            .pop_front()
            .ok_or_else(|| crate::SdkError::Protocol("missing stub query response".to_string()))?;
        Ok(SearchPlaneQueryIpcResponseEnvelope {
            request_id: request.request_id,
            payload,
        })
    }

    fn send_observed(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<
        (
            SearchPlaneQueryIpcResponseEnvelope,
            quanta_index_ipc::ClientIpcTimingV1,
        ),
        crate::SdkError,
    > {
        let response = self.send(request)?;
        Ok((
            response,
            quanta_index_ipc::ClientIpcTimingV1 {
                total: 20,
                encode: 1,
                connect: 2,
                write: 3,
                decode_call: 5,
                read_io: 4,
            },
        ))
    }
}

struct StubControlTransport {
    requests: Mutex<Vec<SearchPlaneControlIpcRequestEnvelope>>,
    response: Mutex<Option<quanta_index_contract::SearchPlaneControlIpcResponse>>,
    request_id_offset: u64,
}

impl StubControlTransport {
    fn new(response: quanta_index_contract::SearchPlaneControlIpcResponse) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
            request_id_offset: 0,
        }
    }

    fn with_request_id_offset(
        response: quanta_index_contract::SearchPlaneControlIpcResponse,
        request_id_offset: u64,
    ) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
            request_id_offset,
        }
    }
}

impl ControlTransport for StubControlTransport {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, crate::SdkError> {
        self.requests
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("control transport poisoned: {err}")))?
            .push(request.clone());
        let payload = self
            .response
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("control response poisoned: {err}")))?
            .take()
            .ok_or_else(|| {
                crate::SdkError::Protocol("missing stub control response".to_string())
            })?;
        Ok(SearchPlaneControlIpcResponseEnvelope {
            request_id: request.request_id.wrapping_add(self.request_id_offset),
            payload,
        })
    }
}

/// QI-SDK-01: stub ingest transport.
///
/// Replaces the old channel-publisher fixtures the SDK used to spin up.
/// Records incoming requests so tests can assert on the typed batch the SDK
/// assembled, answers one canned response and, like the search plane,
/// admits only a batch whose carried `batch_digest` is the canonical digest
/// of its body (QI-BB-032).
struct StubIngestTransport {
    requests: Mutex<Vec<SearchPlaneIngestIpcRequestEnvelope>>,
    response: Mutex<Option<SearchPlaneIngestIpcResponse>>,
    /// Like the search plane, the receipt names the verified digest of the
    /// batch it answers; `false` answers the canned receipt verbatim, for
    /// tests that inject a receipt the SDK must refuse.
    names_request_digest: bool,
    corpus_receipt: Mutex<Option<BatchPublishReceipt>>,
}

impl StubIngestTransport {
    fn new(response: SearchPlaneIngestIpcResponse) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
            names_request_digest: true,
            corpus_receipt: Mutex::new(None),
        }
    }

    // Bind an explicitly supplied receipt to the incoming corpus request.
    // Receipt mutations remain intact except the existing canonical-digest echo.
    fn for_corpus_receipt(receipt: BatchPublishReceipt) -> Self {
        Self {
            corpus_receipt: Mutex::new(Some(receipt)),
            ..Self::new(unused_ingest_response())
        }
    }

    fn answering_verbatim(response: SearchPlaneIngestIpcResponse) -> Self {
        Self {
            names_request_digest: false,
            ..Self::new(response)
        }
    }
}

/// Verify the carried digest of a receipt-bearing request as the search
/// plane does, returning the verified token; a repo-map bundle carries
/// none.
fn verified_request_digest(
    request: &SearchPlaneIngestIpcRequest,
) -> Result<Option<String>, crate::SdkError> {
    fn verify<B: quanta_index_ipc::IngestBatchBodyV1 + serde::Serialize + Clone>(
        body: &B,
    ) -> Result<Option<String>, crate::SdkError> {
        let mut body = body.clone();
        match quanta_index_ipc::verify_batch_digest_v1(&mut body)
            .map_err(|err| crate::SdkError::Serialization(err.to_string()))?
        {
            quanta_index_ipc::BatchDigestVerdictV1::Verified(_) => {
                Ok(Some(body.batch_digest().to_string()))
            }
            quanta_index_ipc::BatchDigestVerdictV1::Mismatch { carried, expected } => {
                Err(crate::SdkError::Protocol(format!(
                    "stub ingest: batch_digest {carried} is not the body's digest {expected}"
                )))
            }
        }
    }
    match request {
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => verify(batch),
        SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(_) => Ok(None),
    }
}

/// Name `digest` in whichever receipt `payload` carries.
fn name_receipt_digest(payload: &mut SearchPlaneIngestIpcResponse, digest: &str) {
    match payload {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome) => {
            outcome.receipt.batch_digest = digest.to_string();
        }
        SearchPlaneIngestIpcResponse::HistoryReceipt(receipt)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(receipt)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(receipt)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(receipt)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(receipt) => {
            receipt.batch_digest = digest.to_string();
        }
        SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
        | SearchPlaneIngestIpcResponse::Error(_) => {}
    }
}

impl IngestTransport for StubIngestTransport {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, crate::SdkError> {
        let verified = verified_request_digest(&request.payload)?;
        self.requests
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("ingest transport poisoned: {err}")))?
            .push(request.clone());
        let original_payload = self
            .response
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("ingest response poisoned: {err}")))?
            .take()
            .ok_or_else(|| crate::SdkError::Protocol("missing stub ingest response".to_string()))?;
        let corpus_receipt = self
            .corpus_receipt
            .lock()
            .map_err(|error| crate::SdkError::Protocol(error.to_string()))?
            .take();
        let mut payload = if let Some(receipt) = corpus_receipt {
            let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) = &request.payload
            else {
                return Err(crate::SdkError::Protocol(
                    "corpus fixture received another route".into(),
                ));
            };
            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                quanta_index_contract::SearchCorpusPublishOutcome {
                    publication: quanta_index_contract::SourcePublicationBinding::for_batch(batch),
                    receipt,
                    observation: None,
                },
            )
        } else {
            original_payload
        };
        if let (true, Some(digest)) = (self.names_request_digest, verified) {
            name_receipt_digest(&mut payload, &digest);
        }
        Ok(SearchPlaneIngestIpcResponseEnvelope {
            request_id: request.request_id,
            payload,
        })
    }
}

fn sample_generation_pin() -> quanta_index_contract::GenerationPin {
    quanta_index_contract::GenerationPin::new(repo_id(), revision_id(), ManifestGeneration::new(7))
}

fn repo_id() -> RepoId {
    RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-1").expect("static fixture ID satisfies canonical policy")
}

fn sample_cluster_membership_request() -> quanta_index_contract::ClusterMembershipReadRequestV1 {
    quanta_index_contract::ClusterMembershipReadRequestV1 {
        cluster_record_id: "cluster-card:auth-service".to_string(),
        generation: sample_generation_pin(),
        expected_authority_digest: "cluster-authority-digest".to_string(),
        limit: 2,
    }
}

fn sample_cluster_membership_batch(
    count: usize,
) -> quanta_index_contract::ClusterMembershipBatchReadRequestV1 {
    quanta_index_contract::ClusterMembershipBatchReadRequestV1 {
        generation: sample_generation_pin(),
        items: (0..count)
            .map(
                |index| quanta_index_contract::ClusterMembershipBatchReadItemV1 {
                    cluster_record_id: format!("cluster-card:{index:02}"),
                    expected_authority_digest: format!("authority:{index:02}"),
                    limit: 1,
                },
            )
            .collect(),
    }
}

fn sample_cluster_membership_batch_response(
    request: &quanta_index_contract::ClusterMembershipBatchReadRequestV1,
) -> quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
    quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
        outcomes: request
            .items
            .iter()
            .map(|item| {
                quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(
                    quanta_index_contract::ClusterMembershipSnapshotV1 {
                        cluster_record_id: item.cluster_record_id.clone(),
                        generation: request.generation.clone(),
                        authority_digest: item.expected_authority_digest.clone(),
                        members: vec![SymbolId::new(format!("symbol:{}", item.cluster_record_id))],
                        completeness:
                            quanta_index_contract::ClusterMembershipCompletenessV1::Complete,
                    },
                )
            })
            .collect(),
    }
}

/// The semantic content roots a test generation "sealed" (QI-BB-028):
/// what a sealed receipt attests and an activation names.
fn semantic_roots(generation: u64) -> quanta_index_contract::SemanticContentRootsV1 {
    quanta_index_contract::SemanticContentRootsV1 {
        row_root_digest: format!("sha256:{generation:0>64x}"),
        membership_root_digest: format!("sha256:{:0>64x}", generation.saturating_add(0x1000)),
    }
}

fn search_corpus_identity(generation: u64, digest: &str) -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: GenerationSnapshot {
            repo_id: repo_id(),
            revision_id: revision_id(),
            track: Track::Lexical,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        },
        semantic: GenerationSnapshot {
            repo_id: repo_id(),
            revision_id: revision_id(),
            track: Track::Semantic,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        },
        semantic_content: semantic_roots(generation),
    }
}

fn head_with_generation(
    generation: SearchCorpusGenerationIdentityV1,
    sequence: u64,
) -> SearchCorpusActiveHeadV1 {
    SearchCorpusActiveHeadV1 {
        generation,
        activation_token: SearchCorpusActivationTokenV1::new(
            [7; quanta_index_contract::ACTIVATION_ROOT_INCARNATION_BYTES_V1],
            std::num::NonZeroU64::new(sequence).expect("fixture sequence is positive"),
        )
        .expect("fixture incarnation is nonzero"),
    }
}

fn search_corpus_head(generation: u64, digest: &str, sequence: u64) -> SearchCorpusActiveHeadV1 {
    head_with_generation(search_corpus_identity(generation, digest), sequence)
}

fn active_resolution(snapshot: GenerationSnapshot) -> ActiveGenerationResolutionV1 {
    let track = snapshot.track;
    let generation = snapshot.manifest_generation.get();
    let lexical = GenerationSnapshot {
        track: Track::Lexical,
        ..snapshot.clone()
    };
    let semantic = GenerationSnapshot {
        track: Track::Semantic,
        ..snapshot
    };
    ActiveGenerationResolutionV1 {
        track,
        head: SearchCorpusActiveHeadV1 {
            generation: SearchCorpusGenerationIdentityV1 {
                lexical,
                semantic,
                semantic_content: semantic_roots(generation),
            },
            activation_token: SearchCorpusActivationTokenV1::new(
                [7; 16],
                NonZeroU64::new(1).expect("fixture activation sequence is positive"),
            )
            .expect("fixture incarnation is nonzero"),
        },
    }
}

fn assert_active_selector(selector: Option<&GenerationSelector>) {
    assert!(matches!(
        selector,
        Some(GenerationSelector::Active {
            repo_id: selected_repo,
            revision_id: selected_revision,
        }) if selected_repo == &repo_id()
            && selected_revision == &revision_id()
    ));
}

fn sample_hit() -> quanta_index_contract::LexicalCandidate {
    quanta_index_contract::LexicalCandidate {
        source_repo_id: repo_id(),
        source: None,
        preview: None,
        candidate_id: "chunk-1".to_string(),
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 10,
        end_line: 20,
        score: 1.0,
        snippet: "fn sample() {}".to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

fn sample_hybrid_seed_candidate() -> quanta_index_contract::SeedCandidate {
    quanta_index_contract::SeedCandidate {
        record_id: "lex-1".to_string(),
        entity_id: "lex-1".to_string(),
        owner_kind: OwnerDocKind::Chunk,
        corpus_kind: None,
        authority_digest: None,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        snippet: "fn sample() {}".to_string(),
        seed_rank: 1,
        contributions: vec![
            quanta_index_contract::SeedContribution {
                lane: quanta_index_contract::SeedLane::Bm25,
                rank: 1,
                raw_score: Some(1.0),
                corpus_kind: None,
            },
            quanta_index_contract::SeedContribution {
                lane: quanta_index_contract::SeedLane::Dense,
                rank: 2,
                raw_score: Some(0.5),
                corpus_kind: None,
            },
        ],
        degraded_reasons: Vec::new(),
    }
}

fn sample_symbol_hit() -> quanta_index_contract::SymbolCandidate {
    quanta_index_contract::SymbolCandidate {
        source_repo_id: repo_id(),
        source: None,
        preview: None,
        candidate_id: "sym-1".to_string(),
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 1,
        end_line: 1,
        score: 1.0,
        snippet: "sample crate".to_string(),
        symbol_kind: ok_or_fail!(SymbolKindCode::new("function")),
        symbol_kind_family: Some(SymbolKindFamily::Callable),
    }
}

fn sample_explanation() -> SearchExplanation {
    SearchExplanation {
        planner_trace: vec![PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: "planned".to_string(),
        }],
        engines_touched: vec![quanta_index_contract::EngineTouched::Semantic],
        engines_executed: vec![quanta_index_contract::EngineTouched::Semantic],
        request_id: 0,
        stage_timings: None,
        early_stop_reason: None,
        contributions: Vec::new(),
        ranker_weights_hash: [0; 32],
        strategy: "test".to_string(),
        summary: "ok".to_string(),
    }
}

fn sample_symbol() -> SymbolRecord {
    SymbolRecord {
        symbol_id: SymbolId::new("sym-1"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: ok_or_fail!(LanguageCode::new("rust")),
        symbol_kind: ok_or_fail!(SymbolKindCode::new("function")),
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: "sample".into(),
        qualified_name: "crate::sample".into(),
        signature: Some("fn sample()".into()),
        visibility: None,
        definition_span: SymbolSpan {
            path: "src/lib.rs".into(),
            byte_start: 0,
            byte_end: 10,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: None,
        relationship: SymbolRelationship::Def,
    }
}

fn sample_chunk() -> ChunkRecord {
    ChunkRecord {
        chunk_id: ChunkId::new("chunk-1"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: ok_or_fail!(LanguageCode::new("rust")),
        start_byte: 0,
        end_byte: 14,
        start_line: 1,
        end_line: 1,
        text: "fn sample() {}".into(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    }
}

fn sample_search_scope() -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::File,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
    }
}

fn sample_source_coverage() -> quanta_index_contract::SourceFileCoverage {
    quanta_index_contract::SourceFileCoverage {
        source: quanta_index_contract::SourceFileRevision {
            file: quanta_index_contract::SourceFileKey {
                source_repo_id: repo_id(),
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            },
            revision_id: revision_id(),
            // SHA-256 of `fn sample() {}`.
            source_sha256: [
                141, 215, 14, 38, 175, 38, 187, 96, 1, 20, 112, 58, 208, 57, 50, 151, 166, 138,
                186, 20, 225, 218, 254, 223, 192, 214, 250, 146, 215, 249, 111, 164,
            ],
        },
        language: ok_or_fail!(LanguageCode::new("rust")),
        producer_policy_sha256: [2; 32],
        symbol_name_source_policy: quanta_index_contract::SymbolNameSourcePolicyV1::Unspecified,
        unit_set_sha256: ok_or_fail!(quanta_index_contract::source_file_unit_set_sha256(
            &[sample_chunk()],
            &[sample_symbol()],
        )),
        text_admitted: true,
        symbols: quanta_index_contract::SymbolCoverage::Complete { symbol_count: 1 },
    }
}

fn sample_source_event() -> quanta_index_contract::SourcePublicationEvent {
    quanta_index_contract::SourcePublicationEvent {
        stream_id: "sdk-fixture-stream".into(),
        event_id: "sdk-fixture-event".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32], // SDK recomputes the payload commitment.
    }
}

fn sample_semantic_scope(owner_id: &str) -> SemanticSourceScopeKeyV1 {
    SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: owner_id.to_string(),
    }
}

fn sample_semantic_source(owner_id: &str) -> SemanticSourceRecordV1 {
    SemanticSourceRecordV1 {
        record_id: format!("record-{owner_id}"),
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: owner_id.to_string(),
        source_doc_id: format!("doc-{owner_id}"),
        parent_owner_id: None,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: Some("rust".to_string()),
        package: Some("crate".to_string()),
        symbol_kind: Some("function".to_string()),
        visibility: Some("pub".to_string()),
        source_role: SourceRoleV1::CardText,
        generated: false,
        capability_status: CapabilityStatusV1::Full,
        raw_fallback_reason: None,
        authority_digest: format!("authority:{owner_id}"),
        render_policy_digest: "render:v1".to_string(),
        card_schema_version: 1,
        text: format!("semantic source for {owner_id}"),
    }
}

fn sample_cluster_semantic_scope(owner_id: &str) -> SemanticSourceScopeKeyV1 {
    SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::ClusterCard,
        owner_kind: OwnerDocKind::Module,
        owner_id: owner_id.to_string(),
    }
}

fn sample_cluster_semantic_source(owner_id: &str, record_suffix: &str) -> SemanticSourceRecordV1 {
    let mut source = sample_semantic_source(owner_id);
    source.record_id = format!("cluster-record-{record_suffix}");
    source.corpus_kind = SemanticCorpusKindV1::ClusterCard;
    source.owner_kind = OwnerDocKind::Module;
    source.authority_digest = format!("cluster-authority-{record_suffix}");
    source.text = "rendered text mentions symbol:fake and is not membership authority".to_string();
    source
}

fn sample_cluster_membership(
    source: &SemanticSourceRecordV1,
    member_suffix: &str,
) -> quanta_index_contract::ClusterMembershipReplaceV1 {
    quanta_index_contract::ClusterMembershipReplaceV1 {
        cluster_record_id: source.record_id.clone(),
        authority_digest: source.authority_digest.clone(),
        members: vec![SymbolId::new(format!("symbol:member:{member_suffix}"))],
    }
}

fn sample_repomap_focus_subject() -> quanta_index_contract::RepoMapFocusSubjectDto {
    quanta_index_contract::RepoMapFocusSubjectDto {
        subject_identity: "subject://repomap".to_string(),
        subject_doc_type: quanta_index_contract::RepoMapDocType::Symbol,
    }
}

fn sample_repomap_snapshot_meta() -> quanta_index_contract::RepoMapSnapshotMeta {
    quanta_index_contract::RepoMapSnapshotMeta {
        snapshot_id: "snap-1".to_string(),
        projection_version: 7,
        authority_digest: "blake3:deadbeef".to_string(),
        item_index_availability: RepoMapItemIndexAvailability::Full,
        graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        exactness_summary: RepoMapExactnessSummary::Exact,
    }
}

fn sample_repomap_entry() -> quanta_index_contract::RepoMapEntryDto {
    quanta_index_contract::RepoMapEntryDto {
        subject_identity: "entry::ident".to_string(),
        subject_doc_type: quanta_index_contract::RepoMapDocType::Symbol,
        subject_kind: "function".to_string(),
        owner_path: "src/lib.rs".to_string(),
        score: 0.875,
        final_score_millis: 875,
        rank: 1,
        importance_score_millis: 500,
        utility_score_millis: 400,
        freshness_score_millis: 300,
        evidence_priority_millis: 200,
        token_budget_hint: 1024,
        contributing_signals: std::collections::BTreeMap::from([
            ("centrality".to_string(), 100_i64),
            ("recency".to_string(), -3_i64),
        ]),
        projection_evidence_kind: "authoritative".to_string(),
        projection_authority_artifact_id: "art-1".to_string(),
        projection_authority_digest: "blake3:cafebabe".to_string(),
        projection_status: "ok".to_string(),
        redaction_state: RepoMapRedactionState::Unredacted,
    }
}

fn sample_repomap_query_request() -> quanta_index_contract::RepoMapQueryRequest {
    quanta_index_contract::RepoMapQueryRequest {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        query_text: "repo map focus".to_string(),
        top_k: 5,
        token_budget: 2048,
        focus_subjects: vec![sample_repomap_focus_subject()],
    }
}

fn sample_repomap_query_response() -> quanta_index_contract::RepoMapQueryResponse {
    quanta_index_contract::RepoMapQueryResponse {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        snapshot_meta: sample_repomap_snapshot_meta(),
        entries: vec![sample_repomap_entry()],
        dropped_entries_count: 1,
        drop_reason_codes: vec!["token_budget".to_string()],
        degraded_reason_codes: vec!["partial_authority".to_string()],
    }
}

fn sample_commit_sha() -> CommitSha {
    CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ])
}

fn sample_commit_record() -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: sample_commit_sha(),
        parents: vec![],
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        author: "alice".into(),
        author_name: None,
        author_email: None,
        committer: "alice".into(),
        committer_name: None,
        committer_email: None,
        message: "fix: sample".into(),
        is_merge: false,
        tags: vec!["v1.0.0".into()],
    }
}

fn sample_diff_record() -> DiffHunkRecord {
    DiffHunkRecord {
        wire_version: 1,
        hunk_header: "@@ -1,1 +1,2 @@".into(),
        side: DiffHunkSide::After,
        added_text: "todo!".into(),
        removed_text: "".into(),
        touched_text: "todo!".into(),
        byte_start: 0,
        byte_end: 5,
    }
}

fn sample_dirty_record() -> DirtyRecord {
    DirtyRecord {
        wire_version: 1,
        doc_id: ChunkId::new("chunk-dirty"),
        applied_at_ms: 55,
        payload_hash: [7; 32],
    }
}

fn sample_parse_tree_record() -> ParseTreeRecord {
    ParseTreeRecord {
        wire_version: 1,
        lang: ok_or_fail!(LanguageCode::new("rust")),
        root: ParseNode {
            kind: "function_item".into(),
            byte_start: 0,
            byte_end: 10,
            children: vec![],
        },
        source_hash: compute_parse_tree_source_hash("fn sample() {}"),
        role_tag_schema_version: 1,
        role_tags: vec![ParseRoleTag {
            role: "expr".into(),
            byte_start: 0,
            byte_end: 4,
        }],
    }
}

fn unused_query() -> Arc<StubQueryTransport> {
    Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            selected_active_head: None,
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            file_owner_rows: None,
            next_cursor: None,
        },
    )))
}

fn unused_control() -> Arc<StubControlTransport> {
    Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
            code: SearchPlaneErrorCodeV2::Internal,
            message: "test must install an explicit control response".to_string(),
            repair: None,
        }),
    ))
}

fn unused_ingest_response() -> SearchPlaneIngestIpcResponse {
    SearchPlaneIngestIpcResponse::Error(SearchPlaneIpcError {
        code: SearchPlaneErrorCodeV2::Internal,
        message: "test must install an explicit ingest response".into(),
        repair: None,
    })
}

fn unused_ingest() -> Arc<StubIngestTransport> {
    Arc::new(StubIngestTransport::new(unused_ingest_response()))
}

fn only_query_request(
    transport: &StubQueryTransport,
) -> Result<SearchPlaneQueryIpcRequestEnvelope, crate::SdkError> {
    let requests = transport
        .requests
        .lock()
        .map_err(|err| crate::SdkError::Protocol(format!("query request list poisoned: {err}")))?;
    let len = requests.len();
    let Some((last, preceding)) = requests.split_last() else {
        return Err(crate::SdkError::Protocol(format!(
            "expected one query preceded only by active resolutions, got {len} requests"
        )));
    };
    if !preceding.iter().all(|request| {
        matches!(
            &request.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_)
        )
    }) {
        return Err(crate::SdkError::Protocol(format!(
            "expected one query preceded only by active resolutions, got {len} requests"
        )));
    }
    let result = last.clone();
    drop(requests);
    Ok(result)
}

fn single_active_query(
    transport: &StubQueryTransport,
) -> Result<SearchPlaneQueryIpcRequestEnvelope, crate::SdkError> {
    let requests = transport
        .requests
        .lock()
        .map_err(|err| crate::SdkError::Protocol(format!("query request list poisoned: {err}")))?;
    let Some((last, preceding)) = requests.split_last() else {
        return Err(crate::SdkError::Protocol(
            "expected one active query".to_string(),
        ));
    };
    if !preceding.is_empty() {
        return Err(crate::SdkError::Protocol(
            "expected one active query".to_string(),
        ));
    }
    let result = last.clone();
    drop(requests);
    Ok(result)
}

fn only_control_request(
    transport: &StubControlTransport,
) -> Result<SearchPlaneControlIpcRequestEnvelope, crate::SdkError> {
    let requests = transport.requests.lock().map_err(|err| {
        crate::SdkError::Protocol(format!("control request list poisoned: {err}"))
    })?;
    let len = requests.len();
    if len != 1 {
        return Err(crate::SdkError::Protocol(format!(
            "expected exactly one control request, got {len}"
        )));
    }
    requests
        .first()
        .cloned()
        .ok_or_else(|| crate::SdkError::Protocol("missing captured control request".to_string()))
}

fn only_ingest_request(
    transport: &StubIngestTransport,
) -> Result<SearchPlaneIngestIpcRequestEnvelope, crate::SdkError> {
    let requests = transport
        .requests
        .lock()
        .map_err(|err| crate::SdkError::Protocol(format!("ingest request list poisoned: {err}")))?;
    let len = requests.len();
    if len != 1 {
        return Err(crate::SdkError::Protocol(format!(
            "expected exactly one ingest request, got {len}"
        )));
    }
    requests
        .first()
        .cloned()
        .ok_or_else(|| crate::SdkError::Protocol("missing captured ingest request".to_string()))
}

mod builder_tests;
mod connect_options_tests;
mod control_tests;
mod corpus_ingest_tests;
mod other_ingest_tests;
mod query_routes_tests;
mod query_tests;
mod replay_tests;
mod resolution;
