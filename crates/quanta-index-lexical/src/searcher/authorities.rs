//! The generation's decoded authorities, refused typed when absent.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::text_authority::ShardedTextAuthority;
use crate::text_docs::{authority_member, stored_text_authority_doc_id};
use crate::{
    FileContributorShard, FileOwnershipShard, RepoCommitRecencyShard, RepoDescriptionShard,
    RepoMetaShard, RepoTopicShard, TantivySearcher,
};
use quanta_index_core::{CoreError, RepoMetadataAuthorityV1};
use tantivy::schema::TantivyDocument;

impl TantivySearcher {
    pub(crate) fn text_authority(&self, feature: &str) -> Result<&ShardedTextAuthority, CoreError> {
        self.text_authority.as_ref().ok_or_else(|| CoreError::Typed {
            code: format!("{feature}_INDEX_MISSING"),
            message: format!(
                "lexical: {feature} execution requires a materialized text authority sidecar for this generation"
            ),
        })
    }

    pub(crate) fn repo_commit_recency_authority(
        &self,
    ) -> Result<&RepoCommitRecencyShard, CoreError> {
        self.repo_commit_recency.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::CommitRecency
                .unavailable_code()
                .to_string(),
            message: "lexical: repo.has.commit.after execution requires materialized source-repo commit recency authority for this generation".to_string(),
        })
    }

    pub(crate) fn repo_meta_authority(&self) -> Result<&RepoMetaShard, CoreError> {
        self.repo_meta.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::Meta.unavailable_code().to_string(),
            message: "lexical: repo.has.meta execution requires materialized source-repo repo metadata authority for this generation".to_string(),
        })
    }

    pub(crate) fn repo_topic_authority(&self) -> Result<&RepoTopicShard, CoreError> {
        self.repo_topic.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::Topic.unavailable_code().to_string(),
            message: "lexical: repo.has.topic execution requires materialized source-repo repo topic authority for this generation".to_string(),
        })
    }

    pub(crate) fn repo_description_authority(&self) -> Result<&RepoDescriptionShard, CoreError> {
        self.repo_description.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::Description
                .unavailable_code()
                .to_string(),
            message: "lexical: repo.has.description execution requires materialized source-repo repo description authority for this generation".to_string(),
        })
    }

    /// A stored text document's text-authority doc id as a restriction
    /// member; a text document without one is refused typed (the
    /// generation predates the id and must be rebuilt).
    pub(crate) fn stored_text_member(
        &self,
        doc: &TantivyDocument,
        candidate_id: &str,
        surface: &str,
    ) -> Result<u32, CoreError> {
        authority_member(
            stored_text_authority_doc_id(doc, &self.fields, candidate_id)?,
            surface,
        )
    }

    pub(crate) fn file_ownership_authority(&self) -> Result<&FileOwnershipShard, CoreError> {
        self.file_ownership.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::FileOwnership
                .unavailable_code()
                .to_string(),
            message: "lexical: file.has.owner execution requires materialized source-repo file ownership authority for this generation".to_string(),
        })
    }

    pub(crate) fn file_contributor_authority(&self) -> Result<&FileContributorShard, CoreError> {
        self.file_contributor.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::Contributor
                .unavailable_code()
                .to_string(),
            message: "lexical: file.has.contributor execution requires materialized source-repo file contributor authority for this generation".to_string(),
        })
    }
}
