//! Native History keeps the established commit/diff DTOs under one held view.

use super::NativeReadSessionId;
use crate::{
    AuxEpochV1, CandidateCountV1, CommitCandidate, DiffCandidate, HistoryCursor, HistoryOrderV1,
    HistoryQueryRequest, QueryResultWindowV2,
    index::{Digest, IndexInstanceId, ObjectRef, ViewRef},
};
use sha2::{Digest as _, Sha256};

/// The boundary is the existing History rank/epoch, bound to its Native member.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeHistoryCursor {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub member: ObjectRef,
    pub query: Digest,
    pub boundary: HistoryCursor,
}

/// A page of commit or diff rows, never a file-search result or generation alias.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeHistoryPage {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub member: ObjectRef,
    pub order: HistoryOrderV1,
    pub commits: Vec<CommitCandidate>,
    pub diffs: Vec<DiffCandidate>,
    /// All matching rows before the continuation cut.
    pub matched: u64,
    /// Matching rows strictly after the supplied boundary.
    pub remaining: u64,
    pub window: QueryResultWindowV2,
    pub read_epoch: AuxEpochV1,
    pub examined: u64,
    pub after: Option<NativeHistoryCursor>,
}

quanta_index_contract_base::native_serde_struct!(NativeHistoryCursor {
    view: ViewRef,
    instance: IndexInstanceId,
    member: ObjectRef,
    query: Digest,
    boundary: HistoryCursor
});
quanta_index_contract_base::native_serde_struct!(NativeHistoryPage {
    session: NativeReadSessionId,
    view: ViewRef,
    instance: IndexInstanceId,
    member: ObjectRef,
    order: HistoryOrderV1,
    commits: Vec<CommitCandidate>,
    diffs: Vec<DiffCandidate>,
    matched: u64,
    remaining: u64,
    window: QueryResultWindowV2,
    read_epoch: AuxEpochV1,
    examined: u64,
    after: Option<NativeHistoryCursor>
});

/// Validate only the Native envelope; the History owner validates its grammar.
pub fn validate_native_history_query(query: &HistoryQueryRequest) -> Result<(), String> {
    if query.text_query.generation.is_some()
        || query.text_query.generation_selector.is_some()
        || query.text_query.cursor.is_some()
        || query.cursor.is_some()
        || !(1..=super::super::MAX_NATIVE_TEXT_TOP_K).contains(&query.text_query.top_k)
    {
        return Err("Native History requires a bounded same-session query without Legacy selectors or tokens".into());
    }
    Ok(())
}

/// Hash canonical request bytes without allocating a second encoded request.
pub fn native_history_query_digest(
    instance: IndexInstanceId,
    query: &HistoryQueryRequest,
) -> Result<Digest, String> {
    validate_native_history_query(query)?;
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    writer.0.update(b"quanta-index/native-history/query/v1\0");
    writer.0.update(instance.0);
    serde_json::to_writer(&mut writer, query).map_err(|error| error.to_string())?;
    Ok(Digest(writer.0.finalize().into()))
}

impl NativeHistoryCursor {
    pub fn validate_request(
        &self,
        view: ViewRef,
        instance: IndexInstanceId,
        query: &HistoryQueryRequest,
    ) -> Result<(), String> {
        if self.view != view
            || self.instance != instance
            || self.query != native_history_query_digest(instance, query)?
            || self.boundary.order.order() != query.order
        {
            return Err("Native History cursor view, instance, query or order differs".into());
        }
        Ok(())
    }
}

impl NativeHistoryPage {
    /// Structural reply binding only; the producer proves rows, order and counts.
    pub fn validate_reply(
        &self,
        session: NativeReadSessionId,
        view: ViewRef,
        instance: IndexInstanceId,
        query: &HistoryQueryRequest,
        after: Option<&NativeHistoryCursor>,
    ) -> Result<(), String> {
        validate_native_history_query(query)?;
        let row_count = self
            .commits
            .len()
            .checked_add(self.diffs.len())
            .ok_or("Native History reply row count overflow")?;
        let count = u32::try_from(row_count).map_err(|error| error.to_string())?;
        let CandidateCountV1::Exact(matched) = self.window.candidate_count() else {
            return Err("Native History requires an exact matched count".into());
        };
        if self.session != session
            || self.view != view
            || self.instance != instance
            || self.order != query.order
            || self.remaining != matched
            || self.remaining > self.matched
            || (after.is_none() && self.remaining != self.matched)
            || count != self.window.returned()
            || count
                != u32::try_from(matched.min(u64::from(query.text_query.top_k)))
                    .map_err(|error| error.to_string())?
            || (!self.commits.is_empty() && !self.diffs.is_empty())
            || self.window.has_more() != Some(self.after.is_some())
            || self.examined < self.matched
            || self
                .commits
                .iter()
                .any(|row| row.score.is_some() != (query.order == HistoryOrderV1::Relevance))
            || self
                .diffs
                .iter()
                .any(|row| row.score.is_some() != (query.order == HistoryOrderV1::Relevance))
        {
            return Err("Native History reply binding, row kind, window or score differs".into());
        }
        if let Some(prior) = after {
            prior.validate_request(view, instance, query)?;
            if prior.member != self.member || prior.boundary.aux_epoch != self.read_epoch {
                return Err("Native History continued a different member or epoch".into());
            }
        }
        if let Some(next) = &self.after {
            next.validate_request(view, instance, query)?;
            if count == 0
                || next.member != self.member
                || next.boundary.aux_epoch != self.read_epoch
            {
                return Err("Native History next cursor member or epoch differs".into());
            }
        }
        Ok(())
    }
}

/// Allocation-free view of the canonical page used while its producer owns DATA.
/// Every field has the same wire representation as NativeHistoryPage.
pub struct NativeHistoryPageRef<'a> {
    pub session: NativeReadSessionId,
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub member: ObjectRef,
    pub query: Digest,
    pub order: HistoryOrderV1,
    pub commits: &'a [CommitCandidate],
    pub diffs: &'a [DiffCandidate],
    pub matched: u64,
    pub remaining: u64,
    pub window: &'a QueryResultWindowV2,
    pub read_epoch: AuxEpochV1,
    pub examined: u64,
    pub after: Option<&'a HistoryCursor>,
}

impl serde::Serialize for NativeHistoryPageRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        struct CursorRef<'a> {
            page: &'a NativeHistoryPageRef<'a>,
            boundary: &'a HistoryCursor,
        }
        impl serde::Serialize for CursorRef<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut state = serializer.serialize_struct("NativeHistoryCursor", 5)?;
                state.serialize_field("view", &self.page.view)?;
                state.serialize_field("instance", &self.page.instance)?;
                state.serialize_field("member", &self.page.member)?;
                state.serialize_field("query", &self.page.query)?;
                state.serialize_field("boundary", self.boundary)?;
                state.end()
            }
        }
        let mut state = serializer.serialize_struct("NativeHistoryPage", 13)?;
        state.serialize_field("session", &self.session)?;
        state.serialize_field("view", &self.view)?;
        state.serialize_field("instance", &self.instance)?;
        state.serialize_field("member", &self.member)?;
        state.serialize_field("order", &self.order)?;
        state.serialize_field("commits", self.commits)?;
        state.serialize_field("diffs", self.diffs)?;
        state.serialize_field("matched", &self.matched)?;
        state.serialize_field("remaining", &self.remaining)?;
        state.serialize_field("window", self.window)?;
        state.serialize_field("read_epoch", &self.read_epoch)?;
        state.serialize_field("examined", &self.examined)?;
        let after = self.after.map(|boundary| CursorRef {
            page: self,
            boundary,
        });
        state.serialize_field("after", &after)?;
        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax,
        index::{CollectionId, SlotId, TenantId, WorkspaceKey, WorktreeId},
    };

    fn query() -> HistoryQueryRequest {
        HistoryQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "type:commit fix author:alice".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 2,
                cursor: None,
            },
            order: HistoryOrderV1::Recency,
            cursor: None,
        }
    }
    fn view() -> ViewRef {
        ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 9,
            digest: Digest([5; 32]),
        }
    }
    #[test]
    fn history_digest_binds_order_filter_cap_and_exact_instance() -> Result<(), String> {
        let instance = IndexInstanceId([8; 16]);
        let query = query();
        let gold = native_history_query_digest(instance, &query)?;
        for changed in [
            HistoryQueryRequest {
                order: HistoryOrderV1::Relevance,
                ..query.clone()
            },
            HistoryQueryRequest {
                text_query: TextQueryRequest {
                    top_k: 1,
                    ..query.text_query.clone()
                },
                ..query.clone()
            },
            HistoryQueryRequest {
                text_query: TextQueryRequest {
                    query_text: "type:commit fix author:bob".into(),
                    ..query.text_query.clone()
                },
                ..query.clone()
            },
        ] {
            assert_ne!(native_history_query_digest(instance, &changed)?, gold);
        }
        assert_ne!(
            native_history_query_digest(IndexInstanceId([9; 16]), &query)?,
            gold
        );
        let mut bad = query;
        bad.text_query.top_k = 0;
        assert!(validate_native_history_query(&bad).is_err());
        Ok(())
    }
    #[test]
    fn history_reply_refuses_member_epoch_view_and_score_drift() -> Result<(), String> {
        let query = query();
        let instance = IndexInstanceId([8; 16]);
        let view = view();
        let cursor = NativeHistoryCursor {
            view,
            instance,
            member: ObjectRef(Digest([7; 32])),
            query: native_history_query_digest(instance, &query)?,
            boundary: HistoryCursor {
                order: crate::HistoryCursorOrderV1::Recency,
                committer_time_ms: 1000,
                sha: crate::lex::CommitSha::from_bytes([1; 20]),
                file_path: None,
                aux_epoch: AuxEpochV1::new(3),
            },
        };
        let page = NativeHistoryPage {
            session: NativeReadSessionId([6; 32]),
            view,
            instance,
            member: cursor.member,
            order: query.order,
            commits: Vec::new(),
            diffs: Vec::new(),
            matched: 4,
            remaining: 0,
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: AuxEpochV1::new(3),
            examined: 4,
            after: None,
        };
        page.validate_reply(page.session, view, instance, &query, Some(&cursor))?;
        let mut wrong = page.clone();
        wrong.member = ObjectRef(Digest([9; 32]));
        assert!(
            wrong
                .validate_reply(page.session, view, instance, &query, Some(&cursor))
                .is_err()
        );
        let mut wrong = page.clone();
        wrong.read_epoch = AuxEpochV1::new(4);
        assert!(
            wrong
                .validate_reply(page.session, view, instance, &query, Some(&cursor))
                .is_err()
        );
        let mut wrong = page.clone();
        wrong.view.sequence = 10;
        assert!(
            wrong
                .validate_reply(page.session, view, instance, &query, Some(&cursor))
                .is_err()
        );
        let mut wrong = page;
        wrong.commits.push(CommitCandidate {
            sha: crate::lex::CommitSha::from_bytes([2; 20]),
            parent_ids: Vec::new(),
            committed_at_unix_s: 2,
            author: "alice".into(),
            committer: "alice".into(),
            message: "fix".into(),
            is_merge: false,
            tags: Vec::new(),
            score: Some(crate::HistoryScoreV1::try_new(1.0).map_err(|error| error.to_string())?),
        });
        wrong.window = QueryResultWindowV2::exact_probe(1);
        wrong.remaining = 1;
        assert!(
            wrong
                .validate_reply(wrong.session, view, instance, &query, Some(&cursor))
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn history_second_page_keeps_full_matched_and_short_remaining_gold() -> Result<(), String> {
        let query = query();
        let instance = IndexInstanceId([8; 16]);
        let view = view();
        let prior = NativeHistoryCursor {
            view,
            instance,
            member: ObjectRef(Digest([7; 32])),
            query: native_history_query_digest(instance, &query)?,
            boundary: HistoryCursor {
                order: crate::HistoryCursorOrderV1::Recency,
                committer_time_ms: 200_000,
                sha: crate::lex::CommitSha::from_bytes([5; 20]),
                file_path: None,
                aux_epoch: AuxEpochV1::new(3),
            },
        };
        let page = NativeHistoryPage {
            session: NativeReadSessionId([6; 32]),
            view,
            instance,
            member: prior.member,
            order: query.order,
            commits: vec![CommitCandidate {
                sha: crate::lex::CommitSha::from_bytes([1; 20]),
                parent_ids: Vec::new(),
                committed_at_unix_s: 123,
                author: "alice".into(),
                committer: "maintainer".into(),
                message: "fix last".into(),
                is_merge: false,
                tags: Vec::new(),
                score: None,
            }],
            diffs: Vec::new(),
            matched: 4,
            remaining: 1,
            window: QueryResultWindowV2::exact_probe(1),
            read_epoch: AuxEpochV1::new(3),
            examined: 7,
            after: None,
        };
        page.validate_reply(page.session, view, instance, &query, Some(&prior))?;
        assert_eq!(page.matched, 4);
        assert_eq!(page.remaining, 1);
        assert!(page.commits.len() < 2);
        let borrowed = NativeHistoryPageRef {
            session: page.session,
            view,
            instance,
            member: page.member,
            query: prior.query,
            order: page.order,
            commits: &page.commits,
            diffs: &page.diffs,
            matched: 4,
            remaining: 1,
            window: &page.window,
            read_epoch: page.read_epoch,
            examined: 7,
            after: None,
        };
        let actual = serde_json::to_value(&borrowed).map_err(|error| error.to_string())?;
        assert_eq!(actual["matched"], serde_json::json!(4));
        assert_eq!(actual["remaining"], serde_json::json!(1));
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&borrowed, &mut bytes).map_err(|error| error.to_string())?;
        assert_eq!(
            ciborium::de::from_reader::<NativeHistoryPage, _>(bytes.as_slice())
                .map_err(|error| error.to_string())?,
            page
        );
        let mut wrong = page.clone();
        wrong.remaining = 2;
        assert!(
            wrong
                .validate_reply(page.session, view, instance, &query, Some(&prior))
                .is_err()
        );
        let mut wrong = page;
        wrong.matched = 0;
        assert!(
            wrong
                .validate_reply(wrong.session, view, instance, &query, Some(&prior))
                .is_err()
        );
        Ok(())
    }
}
