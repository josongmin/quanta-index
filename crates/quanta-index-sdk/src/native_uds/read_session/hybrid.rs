//! Verifies both complete bounded file pools before accepting the fused page.

use super::*;
use quanta_index_contract::{
    index::CoverageStatus,
    native_protocol::{
        MAX_NATIVE_SEMANTIC_OWNER_SCAN, MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES,
        NativeHybridFileResult, rank_native_hybrid_files, validate_native_hybrid_file_query,
    },
};

impl<P: AsRef<Path>> NativeUdsClient<P> {
    pub fn search_hybrid_file_in_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        text_instance: IndexInstanceId,
        semantic_instance: IndexInstanceId,
        text_query: TextQueryRequest,
        semantic_query: quanta_index_contract::SemanticQueryRequest,
        file_top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<NativeHybridFileResult, NativeUdsError> {
        let lexical_limit =
            prepare_hybrid(expected, &text_query, &semantic_query, file_top_k, None)?;
        let dense_limit = semantic_query.top_k;
        let constraints = text_query.constraints.clone();
        let NativeIndexResponse::HybridFileRanked(result) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
                session: expected.session,
                text_instance,
                semantic_instance,
                text_query,
                semantic_query,
                file_top_k,
            }),
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected retained Hybrid file reply".into(),
            ));
        };
        validate_hybrid_reply(
            expected,
            text_instance,
            semantic_instance,
            lexical_limit,
            dense_limit,
            file_top_k,
            &constraints,
            &result,
        )?;
        Ok(result)
    }
}

pub(super) fn prepare_hybrid(
    expected: &NativeReadSessionStatus,
    text_query: &TextQueryRequest,
    semantic_query: &quanta_index_contract::SemanticQueryRequest,
    file_top_k: u32,
    memory: Option<&super::super::MemoryScope>,
) -> Result<u32, NativeUdsError> {
    validate_native_hybrid_file_query(text_query, semantic_query, file_top_k)
        .map_err(NativeUdsError::Protocol)?;
    if expected.state != NativeReadSessionState::Active {
        return Err(NativeUdsError::Protocol(
            "Hybrid session is not active".into(),
        ));
    }
    let lexical_limit =
        super::super::native_text_result_limit_checked_with_memory(text_query, memory, |parsed| {
            if !parsed.filters.is_empty() {
                return Err(NativeUdsError::Protocol(
                    "Native Hybrid requires shared constraints instead of inline lexical filters"
                        .into(),
                ));
            }
            Ok(())
        })?;
    Ok(lexical_limit)
}

fn validate_hybrid_reply(
    expected: &NativeReadSessionStatus,
    text_instance: IndexInstanceId,
    semantic_instance: IndexInstanceId,
    lexical_limit: u32,
    dense_limit: u32,
    file_top_k: u32,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    result: &NativeHybridFileResult,
) -> Result<(), NativeUdsError> {
    validate_hybrid_reply_with_memory(
        expected,
        text_instance,
        semantic_instance,
        lexical_limit,
        dense_limit,
        file_top_k,
        constraints,
        result,
        None,
    )
}

pub(super) fn validate_hybrid_reply_with_memory(
    expected: &NativeReadSessionStatus,
    text_instance: IndexInstanceId,
    semantic_instance: IndexInstanceId,
    lexical_limit: u32,
    dense_limit: u32,
    file_top_k: u32,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    result: &NativeHybridFileResult,
    memory: Option<&super::super::MemoryScope>,
) -> Result<(), NativeUdsError> {
    let coverage = &result.semantic_coverage;
    let complete = matches!(coverage.status, CoverageStatus::Complete);
    if result.session != expected.session
        || result.view != expected.view
        || result.text_instance != text_instance
        || result.semantic_instance != semantic_instance
        || result.file_top_k != file_top_k
        || coverage.instance != semantic_instance
        || result.lexical_pool.len() as u64 != result.lexical_matched.min(u64::from(lexical_limit))
        || result.dense_pool.len() as u64 > u64::from(dense_limit)
        || result.skipped_stale > result.scanned_owners
        || result.dense_pool.len() as u64
            > u64::from(result.scanned_owners.saturating_sub(result.skipped_stale))
        || result.scanned_owners > MAX_NATIVE_SEMANTIC_OWNER_SCAN
        || (coverage.owners.0.is_some() && coverage.searchable.is_none())
        || (complete && coverage.covered != Some(coverage.desired))
        || result.semantic_exhaustive
            != result
                .semantic_traversal
                .certifies_exhaustive(coverage, result.skipped_stale)
        || (coverage.owners.0.is_none()
            && (result.scanned_owners != 0 || !result.dense_pool.is_empty()))
    {
        return Err(NativeUdsError::Protocol(
            "Hybrid authority, file pool or coverage differs".into(),
        ));
    }
    for (path, id) in result
        .lexical_pool
        .iter()
        .map(|row| (row.path.as_str(), None))
        .chain(
            result
                .dense_pool
                .iter()
                .map(|row| (row.path.as_str(), Some(row.vector_id.as_slice()))),
        )
    {
        if !super::super::valid_native_source_path(path)
            || constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|expected| expected.as_str() != path)
            || id.is_some_and(|id| id.is_empty() || id.len() > MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES)
        {
            return Err(NativeUdsError::Protocol(
                "Hybrid candidate path/filter/vector witness differs".into(),
            ));
        }
    }
    for row in &result.dense_pool {
        if let Some(chunk) = row.chunk {
            chunk
                .validate_file_binding(&row.vector_id, coverage.desired.policy, row.source)
                .map_err(|error| {
                    NativeUdsError::Protocol(format!("Hybrid vector witness differs: {error}"))
                })?;
        }
    }
    let capacity = result
        .lexical_pool
        .len()
        .checked_add(result.dense_pool.len())
        .ok_or_else(|| NativeUdsError::Protocol("Hybrid union overflow".into()))?;
    let _scratch = super::super::temporary_rows::<
        quanta_index_contract::native_protocol::NativeHybridFileHit,
    >(memory, capacity)?;
    let mut gold = Vec::new();
    gold.try_reserve_exact(capacity)
        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
    rank_native_hybrid_files(
        &result.lexical_pool,
        &result.dense_pool,
        file_top_k,
        &mut gold,
    )
    .map_err(NativeUdsError::Protocol)?;
    if result.hits != gold {
        return Err(NativeUdsError::Protocol(
            "Hybrid result differs from its complete bounded file pools".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use quanta_index_contract::{
        FiniteScore, QueryConstraintSetV1,
        index::{
            CollectionId, CoverageBinding, Digest, InputBinding, MapRoot, ObjectRef, OwnerKey,
            ProducerId, SchemaId, SlotId, StableOwnerId, TenantId, ViewRef, WorkspaceKey,
            WorktreeId,
        },
        native_protocol::{
            NativeHybridFileHit, NativeReadSessionId, NativeSemanticFileHit,
            NativeSemanticTraversal,
        },
    };

    pub(in crate::native_uds::read_session) fn fixture()
    -> Result<(NativeReadSessionStatus, NativeHybridFileResult), Box<dyn std::error::Error>> {
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 1,
            digest: Digest([5; 32]),
        };
        let status = NativeReadSessionStatus {
            session: NativeReadSessionId([6; 32]),
            view,
            expires_at_ms: 100,
            revision: 1,
            state: NativeReadSessionState::Active,
        };
        let text_instance = IndexInstanceId([7; 16]);
        let semantic_instance = IndexInstanceId([8; 16]);
        let owner = OwnerKey {
            producer: ProducerId([9; 16]),
            stable_id: StableOwnerId([10; 16]),
        };
        let root = MapRoot(Some(ObjectRef(Digest([11; 32]))));
        let desired = InputBinding {
            root,
            schema: SchemaId([12; 16]),
            policy: Digest([13; 32]),
            context: Digest([14; 32]),
        };
        let result = NativeHybridFileResult {
            session: status.session,
            view,
            text_instance,
            semantic_instance,
            lexical_artifact: ObjectRef(Digest([15; 32])),
            lexical_matched: 0,
            semantic_coverage: CoverageBinding {
                instance: semantic_instance,
                desired,
                covered: Some(desired),
                searchable: root.0,
                status: CoverageStatus::Complete,
                owners: root,
            },
            semantic_traversal: NativeSemanticTraversal::ExactCapped,
            semantic_exhaustive: false,
            scanned_owners: 2,
            skipped_stale: 1,
            file_top_k: 1,
            lexical_pool: Vec::new(),
            dense_pool: vec![NativeSemanticFileHit {
                owner,
                source: ObjectRef(Digest([16; 32])),
                path: "src/a.rs".into(),
                projection_owner: owner,
                vector_id: b"vector-1".to_vec(),
                chunk: None,
                score: FiniteScore::new(0.75)?,
            }],
            hits: vec![NativeHybridFileHit {
                owner,
                lexical_rank: None,
                dense_rank: Some(1),
                score: FiniteScore::new(1.0 / 61.0)?,
            }],
        };
        Ok((status, result))
    }

    #[test]
    fn hybrid_reply_refuses_files_without_fresh_scanned_owners()
    -> Result<(), Box<dyn std::error::Error>> {
        let (status, mut result) = fixture()?;
        let text_instance = result.text_instance;
        let semantic_instance = result.semantic_instance;
        let constraints = QueryConstraintSetV1::unconstrained();
        let validate = |result: &NativeHybridFileResult| {
            validate_hybrid_reply(
                &status,
                text_instance,
                semantic_instance,
                1,
                2,
                1,
                &constraints,
                result,
            )
        };
        validate(&result)?;
        for (scanned, stale) in [(0, 0), (1, 1)] {
            result.scanned_owners = scanned;
            result.skipped_stale = stale;
            assert!(
                matches!(validate(&result), Err(NativeUdsError::Protocol(_))),
                "accepted dense file without a fresh scan: scanned={scanned}, stale={stale}"
            );
        }
        result.scanned_owners = 2;
        result.skipped_stale = 1;
        let mut second = result.dense_pool[0].clone();
        second.owner.stable_id = StableOwnerId([11; 16]);
        second.projection_owner = second.owner;
        second.source = ObjectRef(Digest([17; 32]));
        second.path = "src/b.rs".into();
        second.vector_id = b"vector-2".to_vec();
        second.score = FiniteScore::new(0.5)?;
        result.dense_pool.push(second);
        assert!(matches!(
            validate(&result),
            Err(NativeUdsError::Protocol(_))
        ));
        result.scanned_owners = 3;
        validate(&result)?;
        result.scanned_owners = 1;
        result.dense_pool.clear();
        result.hits.clear();
        validate(&result)?;
        Ok(())
    }

    #[test]
    fn hybrid_request_refuses_inline_file_filters_before_transport()
    -> Result<(), Box<dyn std::error::Error>> {
        let (status, identity) = fixture()?;
        let directory = tempfile::tempdir()?;
        let client = NativeUdsClient::new(directory.path().join("no-listener.sock"));
        let call = |source: &str| {
            client.search_hybrid_file_in_read_session(
                &status,
                identity.text_instance,
                identity.semantic_instance,
                TextQueryRequest {
                    syntax: quanta_index_contract::TextQuerySyntax::Native,
                    query_text: source.into(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: None,
                    generation_selector: None,
                    top_k: 2,
                    cursor: None,
                },
                quanta_index_contract::SemanticQueryRequest {
                    query_text: "query".into(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: None,
                    generation_selector: None,
                    lexical_scope: None,
                    top_k: 2,
                },
                1,
                &RequestBudgetV1::for_duration(std::time::Duration::from_secs(2)),
            )
        };
        for source in ["alpha file:alpha", "alpha path:alpha"] {
            let parsed = quanta_index_contract::parse_native_content_query(source)?;
            assert!(matches!(
                parsed.filters.as_slice(),
                [quanta_index_contract::LqFilter::File { .. }]
            ));
            assert!(
                matches!(call(source), Err(NativeUdsError::Protocol(_))),
                "inline eligibility reached transport: {source}"
            );
        }
        let quoted = r#""file:alpha""#;
        assert!(
            quanta_index_contract::parse_native_content_query(quoted)?
                .filters
                .is_empty()
        );
        assert!(matches!(call(quoted), Err(NativeUdsError::Transport(_))));
        Ok(())
    }
}
