//! Retained `select:path` paging uses a repository/path group cursor.

use super::*;
use quanta_index_contract::native_protocol::{
    NativeTextPathCursorV1, NativeTextPathPageV1, compare_native_text_path_rank_v1,
    native_text_query_digest,
};
use quanta_index_contract::{LqFilter, LqSelect};

impl<P: AsRef<Path>> NativeUdsClient<P> {
    pub fn search_text_path_page_in_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        after: Option<NativeTextPathCursorV1>,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextPathPageV1, NativeUdsError> {
        let digest = prepare_path_page(expected, instance, &query, after.as_ref(), None)?;
        let limit = query.top_k;
        let constraints = query.constraints.clone();
        let NativeIndexResponse::TextPathPageV1(page) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPathPageV1 {
                session: expected.session,
                instance,
                query,
                after: after.clone(),
            }),
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected retained text path page reply".into(),
            ));
        };
        validate_path_page(
            expected,
            instance,
            limit,
            digest,
            after.as_ref(),
            &constraints,
            &page,
            None,
        )?;
        Ok(page)
    }
}

pub(super) fn prepare_path_page(
    expected: &NativeReadSessionStatus,
    instance: IndexInstanceId,
    query: &TextQueryRequest,
    after: Option<&NativeTextPathCursorV1>,
    memory: Option<&super::super::MemoryScope>,
) -> Result<quanta_index_contract::index::Digest, NativeUdsError> {
    if expected.state != NativeReadSessionState::Active
        || !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&query.top_k)
        || query.syntax == quanta_index_contract::TextQuerySyntax::CodeSearch
        || query.generation.is_some()
        || query.generation_selector.is_some()
        || query.cursor.is_some()
    {
        return Err(NativeUdsError::Protocol(
            "text path page request is not an active Native query".into(),
        ));
    }
    let _page_limit =
        super::super::native_text_result_limit_checked_with_memory(query, memory, |parsed| {
            if parsed.options.count.is_some() {
                return Err(NativeUdsError::Protocol(
                    "text path pages do not support count".into(),
                ));
            }
            if !parsed.filters.iter().any(|filter| {
                matches!(
                    filter,
                    LqFilter::Select {
                        dim: LqSelect::Path
                    }
                )
            }) {
                return Err(NativeUdsError::Protocol(
                    "text path page requires path projection".into(),
                ));
            }
            Ok(())
        })?;
    let digest = native_text_query_digest(instance, query).map_err(NativeUdsError::Protocol)?;
    if after.is_some_and(|cursor| {
        cursor.view != expected.view
            || cursor.instance != instance
            || cursor.query != digest
            || cursor.score.get() < 0.0
            || !super::super::valid_native_source_path(&cursor.path)
            || query
                .constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|path| path.as_str() != cursor.path)
    }) {
        return Err(NativeUdsError::Protocol(
            "text path page cursor binding differs".into(),
        ));
    }
    Ok(digest)
}

pub(super) fn validate_path_page(
    expected: &NativeReadSessionStatus,
    instance: IndexInstanceId,
    limit: u32,
    digest: quanta_index_contract::index::Digest,
    after: Option<&NativeTextPathCursorV1>,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    page: &NativeTextPathPageV1,
    memory: Option<&super::super::MemoryScope>,
) -> Result<(), NativeUdsError> {
    let result = &page.ranked.ranking;
    let count = u64::try_from(result.hits.len())
        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
    // A continuation excludes at least the group that supplied its input cursor.
    if page.session != expected.session
        || page.remaining > result.matched
        || (after.is_none() != (page.remaining == result.matched))
        || after.is_some_and(|cursor| cursor.artifact != result.artifact)
        || page.after.is_some() != (page.remaining > count)
    {
        return Err(NativeUdsError::Protocol(
            "text path page identity, cardinality or continuation differs".into(),
        ));
    }
    super::super::path_output_v1::validate_ranking(
        ViewSelection::Exact(expected.view),
        instance,
        limit,
        page.remaining.min(u64::from(limit)),
        constraints,
        &page.ranked,
        memory,
    )?;
    if let Some(cursor) = after {
        for (hit, repository) in result.hits.iter().zip(&page.ranked.repositories) {
            if repository == &cursor.repository && hit.path == cursor.path {
                return Err(NativeUdsError::Protocol(
                    "text path page repeats cursor group".into(),
                ));
            }
            if compare_native_text_path_rank_v1(
                (cursor.score, &cursor.repository, &cursor.path),
                (hit.score, repository, &hit.path),
            ) != std::cmp::Ordering::Less
            {
                return Err(NativeUdsError::Protocol(
                    "text path page precedes cursor".into(),
                ));
            }
        }
    }
    if let Some(cursor) = &page.after {
        let last = result.hits.last().ok_or_else(|| {
            NativeUdsError::Protocol("empty text path page has continuation".into())
        })?;
        let repository = page.ranked.repositories.last().ok_or_else(|| {
            NativeUdsError::Protocol("text path continuation has no repository".into())
        })?;
        if cursor.view != expected.view
            || cursor.instance != instance
            || cursor.query != digest
            || cursor.artifact != result.artifact
            || cursor.score != last.score
            || cursor.repository != *repository
            || cursor.path != last.path
        {
            return Err(NativeUdsError::Protocol(
                "text path continuation is not its last group".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::native_protocol::{
        NativeTextCoverage, NativeTextPathRankingV1, NativeTextResult,
    };
    use quanta_index_contract::{FiniteScore, QueryConstraintSetV1, RepoId, TextQuerySyntax};

    #[test]
    fn path_page_requires_projection_and_rejects_repeated_anchor_group()
    -> Result<(), Box<dyn std::error::Error>> {
        let (status, original, instance, digest) = super::super::page::tests::fixture()?;
        let query = TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "select:path needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        };
        let actual_digest = prepare_path_page(&status, instance, &query, None, None)?;
        assert_ne!(actual_digest, digest);
        let repository = RepoId::new("repo-sdk-path")?;
        let hit = original.hits[0].clone();
        let ranked = NativeTextPathRankingV1 {
            ranking: NativeTextResult {
                view: status.view,
                instance,
                coverage: NativeTextCoverage::CoreRequired,
                artifact: original.artifact,
                matched: 2,
                exhaustive: true,
                hits: vec![hit.clone()],
            },
            repositories: vec![repository.clone()],
        };
        let anchor = NativeTextPathCursorV1 {
            view: status.view,
            instance,
            query: actual_digest,
            artifact: original.artifact,
            score: hit.score,
            repository: repository.clone(),
            path: hit.path.clone(),
        };
        let page = NativeTextPathPageV1 {
            session: status.session,
            ranked,
            remaining: 2,
            after: Some(anchor.clone()),
        };
        validate_path_page(
            &status,
            instance,
            1,
            actual_digest,
            None,
            &query.constraints,
            &page,
            None,
        )?;
        let envelope = quanta_index_contract::native_protocol::NativeIndexResponseEnvelope {
            request_id: 1,
            payload: NativeIndexResponse::TextPathPageV1(page.clone()),
        };
        let mut wire = Vec::new();
        ciborium::ser::into_writer(&envelope, &mut wire)?;
        let decoded = super::super::super::admitted_reply_v1::prepare(
            &wire,
            super::super::super::admitted_reply_v1::ReplyMode::ReadSession,
        )?
        .decode()?;
        assert_eq!(decoded, envelope);
        let mut repeated = page.clone();
        repeated.remaining = 1;
        repeated.after = None;
        let mut earlier = anchor.clone();
        earlier.score = FiniteScore::new(hit.score.get() + 1.0)?;
        assert!(
            validate_path_page(
                &status,
                instance,
                1,
                actual_digest,
                Some(&earlier),
                &query.constraints,
                &repeated,
                None
            )
            .is_err()
        );
        earlier.path = "src/prior.rs".into();
        validate_path_page(
            &status,
            instance,
            1,
            actual_digest,
            Some(&earlier),
            &query.constraints,
            &repeated,
            None,
        )?;
        assert!(
            matches!(
                validate_path_page(
                    &status,
                    instance,
                    1,
                    actual_digest,
                    Some(&earlier),
                    &query.constraints,
                    &page,
                    None,
                ),
                Err(NativeUdsError::Protocol(message))
                    if message == "text path page identity, cardinality or continuation differs"
            ),
            "a continuation must not report its full count as remaining"
        );
        let mut wrong_repository = page.clone();
        wrong_repository.ranked.repositories.clear();
        assert!(
            validate_path_page(
                &status,
                instance,
                1,
                actual_digest,
                None,
                &query.constraints,
                &wrong_repository,
                None
            )
            .is_err()
        );
        let mut ordinary = query;
        ordinary.query_text = "needle".into();
        assert!(prepare_path_page(&status, instance, &ordinary, None, None).is_err());
        Ok(())
    }
}
