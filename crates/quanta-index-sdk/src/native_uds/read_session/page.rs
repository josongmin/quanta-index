//! SDK verification of the retained global lexical page contract.

use super::*;
use quanta_index_contract::native_protocol::{
    NativeTextCursor, NativeTextPage, native_text_query_digest,
};

impl<P: AsRef<Path>> NativeUdsClient<P> {
    pub fn search_text_page_in_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        after: Option<NativeTextCursor>,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextPage, NativeUdsError> {
        let digest = prepare_page(expected, instance, &query, after, None)?;
        let limit = query.top_k;
        let constraints = query.constraints.clone();
        let NativeIndexResponse::TextRankedPage(page) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
                session: expected.session,
                instance,
                query,
                after,
            }),
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected retained text page reply".into(),
            ));
        };
        validate_page(
            expected,
            instance,
            limit,
            digest,
            after,
            &constraints,
            &page,
        )?;
        Ok(page)
    }
}

pub(super) fn prepare_page(
    expected: &NativeReadSessionStatus,
    instance: IndexInstanceId,
    query: &TextQueryRequest,
    after: Option<NativeTextCursor>,
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
            "text page request is not an active Native query".into(),
        ));
    }
    let _page_limit =
        super::super::native_text_result_limit_checked_with_memory(query, memory, |parsed| {
            if parsed.options.count.is_some() {
                return Err(NativeUdsError::Protocol(
                    "text pages do not support count".into(),
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
    }) {
        return Err(NativeUdsError::Protocol(
            "text page cursor binding differs".into(),
        ));
    }
    Ok(digest)
}

fn validate_page(
    expected: &NativeReadSessionStatus,
    instance: IndexInstanceId,
    limit: u32,
    digest: quanta_index_contract::index::Digest,
    after: Option<NativeTextCursor>,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    page: &NativeTextPage,
) -> Result<(), NativeUdsError> {
    validate_page_with_memory(
        expected,
        instance,
        limit,
        digest,
        after,
        constraints,
        page,
        None,
    )
}

pub(super) fn validate_page_with_memory(
    expected: &NativeReadSessionStatus,
    instance: IndexInstanceId,
    limit: u32,
    digest: quanta_index_contract::index::Digest,
    after: Option<NativeTextCursor>,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    page: &NativeTextPage,
    memory: Option<&super::super::MemoryScope>,
) -> Result<(), NativeUdsError> {
    let count = u64::try_from(page.hits.len())
        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
    if page.session != expected.session
        || page.view != expected.view
        || page.instance != instance
        || page.remaining > page.matched
        || count != page.remaining.min(u64::from(limit))
        || (after.is_none() && page.remaining != page.matched)
        || after.is_some_and(|cursor| cursor.artifact != page.artifact)
        || page.after.is_some() != (page.remaining > count)
    {
        return Err(NativeUdsError::Protocol(
            "text page identity, cardinality or continuation differs".into(),
        ));
    }
    let _scratch = super::super::temporary_rows::<quanta_index_contract::index::OwnerKey>(
        memory,
        page.hits.len(),
    )?;
    let mut seen = Vec::new();
    seen.try_reserve_exact(page.hits.len())
        .map_err(|error| NativeUdsError::Transport(error.to_string()))?;
    let mut prior = after.map(|cursor| (cursor.score, cursor.owner));
    for hit in &page.hits {
        if !super::super::valid_native_source_path(&hit.path)
            || constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|expected| expected.as_str() != hit.path)
            || hit.score.get() < 0.0
            || prior.is_some_and(|(score, owner)| {
                score.get().total_cmp(&hit.score.get()).is_lt()
                    || (score == hit.score && owner >= hit.owner)
            })
        {
            return Err(NativeUdsError::Protocol(
                "text page path, owner or rank differs".into(),
            ));
        }
        seen.push(hit.owner);
        prior = Some((hit.score, hit.owner));
    }
    seen.sort_unstable();
    if seen.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(NativeUdsError::Protocol("duplicate text page owner".into()));
    }
    if let Some(cursor) = page.after {
        let last = page
            .hits
            .last()
            .ok_or_else(|| NativeUdsError::Protocol("empty text page has continuation".into()))?;
        if cursor
            != (NativeTextCursor {
                view: expected.view,
                instance,
                query: digest,
                artifact: page.artifact,
                score: last.score,
                owner: last.owner,
            })
        {
            return Err(NativeUdsError::Protocol(
                "text page continuation is not its last row".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use quanta_index_contract::{
        FiniteScore,
        index::{
            CollectionId, Digest, ObjectRef, OwnerKey, ProducerId, SlotId, StableOwnerId, TenantId,
            ViewRef, WorkspaceKey, WorktreeId,
        },
        native_protocol::{NativeReadSessionId, NativeTextHit},
    };

    pub(in crate::native_uds::read_session) fn fixture() -> Result<
        (
            NativeReadSessionStatus,
            NativeTextPage,
            IndexInstanceId,
            Digest,
        ),
        Box<dyn std::error::Error>,
    > {
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
        let instance = IndexInstanceId([7; 16]);
        let digest = Digest([8; 32]);
        let hit = NativeTextHit {
            owner: OwnerKey {
                producer: ProducerId([9; 16]),
                stable_id: StableOwnerId([10; 16]),
            },
            source: ObjectRef(Digest([11; 32])),
            path: "src/a.rs".into(),
            score: FiniteScore::new(0.5)?,
        };
        let artifact = ObjectRef(Digest([12; 32]));
        let page = NativeTextPage {
            session: status.session,
            view,
            instance,
            artifact,
            matched: 2,
            remaining: 2,
            after: Some(NativeTextCursor {
                view,
                instance,
                query: digest,
                artifact,
                score: hit.score,
                owner: hit.owner,
            }),
            hits: vec![hit],
        };
        Ok((status, page, instance, digest))
    }

    #[test]
    fn page_reply_refuses_wrong_view_and_fabricated_or_missing_anchor()
    -> Result<(), Box<dyn std::error::Error>> {
        let (status, mut page, instance, digest) = fixture()?;
        validate_page(
            &status,
            instance,
            1,
            digest,
            None,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            &page,
        )?;
        let original = page.clone();
        page.view.sequence = 2;
        assert!(
            validate_page(
                &status,
                instance,
                1,
                digest,
                None,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                &page
            )
            .is_err()
        );
        page = original.clone();
        page.after = None;
        assert!(
            validate_page(
                &status,
                instance,
                1,
                digest,
                None,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                &page
            )
            .is_err()
        );
        page = original;
        if let Some(cursor) = page.after.as_mut() {
            cursor.owner.stable_id = StableOwnerId([99; 16]);
        }
        assert!(
            validate_page(
                &status,
                instance,
                1,
                digest,
                None,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                &page
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn page_reply_refuses_repeated_rank_and_full_total_used_as_remaining()
    -> Result<(), Box<dyn std::error::Error>> {
        let (status, mut page, instance, digest) = fixture()?;
        let anchor = page.after;
        assert!(
            validate_page(
                &status,
                instance,
                1,
                digest,
                anchor,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                &page
            )
            .is_err()
        );
        page.hits[0].score = FiniteScore::new(0.4)?;
        page.remaining = 1;
        page.after = None;
        validate_page(
            &status,
            instance,
            1,
            digest,
            anchor,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            &page,
        )?;
        page.remaining = 3;
        assert!(
            validate_page(
                &status,
                instance,
                1,
                digest,
                anchor,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                &page
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn page_reply_refuses_a_path_outside_the_requested_exact_filter()
    -> Result<(), Box<dyn std::error::Error>> {
        let (status, mut page, instance, digest) = fixture()?;
        let constraints =
            quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(
                quanta_index_contract::ExactRepoRelativePathV1::new("src/a.rs")?,
            );
        validate_page(&status, instance, 1, digest, None, &constraints, &page)?;
        page.hits[0].path = "src/b.rs".into();
        assert!(matches!(
            validate_page(&status, instance, 1, digest, None, &constraints, &page),
            Err(NativeUdsError::Protocol(_))
        ));
        Ok(())
    }
}
