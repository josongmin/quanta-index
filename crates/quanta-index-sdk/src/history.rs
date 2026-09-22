use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
use quanta_index_contract::{
    FileContributorEntry, FileContributorIdentityEntry, FileContributorIngestBatch,
    ContinuationTokenV2, FileOwnershipEntry, FileOwnershipIngestBatch, GenerationPin,
    GenerationSelector, HistoryDiffHunkUpsert, HistoryIngestBatch, HistoryOrderV1,
    HistoryQueryRequest,
    HistoryRefDelete, HistoryRefMutation, HistoryRefUpsert, HistoryTagMutation, ManifestGeneration,
    RepoCommitRecencyEntry, RepoCommitRecencyIngestBatch, RepoDescriptionEntry,
    RepoDescriptionIngestBatch, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath,
    RepoTopicEntry, RepoTopicIngestBatch, RevisionId, SearchPlaneHistoryQueryResponse,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchReceipt, QuantaIndex, SdkError, stamp_batch_digest_v1};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefMutation {
    Upsert { name: String, sha: CommitSha },
    Delete { name: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunkMutation {
    pub commit_sha: CommitSha,
    pub file_path: String,
    pub record: DiffHunkRecord,
}

/// A history publish.
///
/// Like every SDK ingest batch, its `batch_digest` is not chosen by the
/// caller: it is the canonical digest of the wire body, computed when the
/// batch is sent (QI-BB-032), so a resend of the same content replays the
/// same idempotency key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub manifest_digest: Option<String>,
    pub commits: Vec<CommitRecord>,
    pub refs: Vec<RefMutation>,
    pub tags: Vec<RefMutation>,
    pub diff_hunks: Vec<DiffHunkMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoCommitRecencyBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub entries: Vec<RepoCommitRecencyMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoCommitRecencyMutation {
    pub source_repo_id: RepoId,
    pub latest_committer_time_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMetaBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub entries: Vec<RepoMetaMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMetaMutation {
    pub source_repo_id: RepoId,
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoTopicBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub entries: Vec<RepoTopicMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoTopicMutation {
    pub source_repo_id: RepoId,
    pub topic: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoDescriptionBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub entries: Vec<RepoDescriptionMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoDescriptionMutation {
    pub source_repo_id: RepoId,
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileOwnershipBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub entries: Vec<FileOwnershipMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileOwnershipMutation {
    pub source_repo_id: RepoId,
    pub repo_relative_path: RepoRelativePath,
    pub owners: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContributorBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub entries: Vec<FileContributorMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContributorMutation {
    pub source_repo_id: RepoId,
    pub repo_relative_path: RepoRelativePath,
    pub contributors: Vec<FileContributorIdentityEntry>,
}

impl HistoryBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            manifest_digest: None,
            commits: Vec::new(),
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        }
    }

    #[must_use]
    pub fn manifest_digest(mut self, manifest_digest: impl Into<String>) -> Self {
        self.manifest_digest = Some(manifest_digest.into());
        self
    }

    #[must_use]
    pub fn commit(mut self, record: CommitRecord) -> Self {
        self.commits.push(record);
        self
    }

    #[must_use]
    pub fn ref_upsert(mut self, name: impl Into<String>, sha: CommitSha) -> Self {
        self.refs.push(RefMutation::Upsert {
            name: name.into(),
            sha,
        });
        self
    }

    #[must_use]
    pub fn ref_delete(mut self, name: impl Into<String>) -> Self {
        self.refs.push(RefMutation::Delete { name: name.into() });
        self
    }

    #[must_use]
    pub fn tag_upsert(mut self, name: impl Into<String>, sha: CommitSha) -> Self {
        self.tags.push(RefMutation::Upsert {
            name: name.into(),
            sha,
        });
        self
    }

    #[must_use]
    pub fn tag_delete(mut self, name: impl Into<String>) -> Self {
        self.tags.push(RefMutation::Delete { name: name.into() });
        self
    }

    #[must_use]
    pub fn diff_hunk(
        mut self,
        commit_sha: CommitSha,
        file_path: impl Into<String>,
        record: DiffHunkRecord,
    ) -> Self {
        self.diff_hunks.push(DiffHunkMutation {
            commit_sha,
            file_path: file_path.into(),
            record,
        });
        self
    }

    /// The canonical digest this batch publishes under.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
    }

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<HistoryIngestBatch, SdkError> {
        let mut wire = HistoryIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            manifest_digest: self.manifest_digest.clone(),
            batch_digest: String::new(),
            commits: self.commits.clone(),
            refs: self.refs.iter().map(map_ref_mutation).collect(),
            tags: self.tags.iter().map(map_tag_mutation).collect(),
            diff_hunks: self
                .diff_hunks
                .iter()
                .map(|mutation| HistoryDiffHunkUpsert {
                    commit_sha: mutation.commit_sha,
                    file_path: mutation.file_path.clone().into_boxed_str(),
                    record: mutation.record.clone(),
                })
                .collect(),
        };
        stamp_batch_digest_v1(&mut wire)
            .map_err(|err| SdkError::Serialization(format!("History batch digest: {err}")))?;
        Ok(wire)
    }
}

impl RepoCommitRecencyBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn entry(mut self, source_repo_id: RepoId, latest_committer_time_ms: u64) -> Self {
        self.entries.push(RepoCommitRecencyMutation {
            source_repo_id,
            latest_committer_time_ms,
        });
        self
    }

    /// The canonical digest this batch publishes under.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
    }

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<RepoCommitRecencyIngestBatch, SdkError> {
        let mut wire = RepoCommitRecencyIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            batch_digest: String::new(),
            entries: self
                .entries
                .iter()
                .map(|entry| RepoCommitRecencyEntry {
                    source_repo_id: entry.source_repo_id.clone(),
                    latest_committer_time_ms: entry.latest_committer_time_ms,
                })
                .collect(),
        };
        stamp_batch_digest_v1(&mut wire).map_err(|err| {
            SdkError::Serialization(format!("RepoCommitRecency batch digest: {err}"))
        })?;
        Ok(wire)
    }
}

impl RepoMetaBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn entry(
        mut self,
        source_repo_id: RepoId,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.entries.push(RepoMetaMutation {
            source_repo_id,
            key: key.into(),
            value: value.into(),
        });
        self
    }

    /// The canonical digest this batch publishes under.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
    }

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<RepoMetaIngestBatch, SdkError> {
        let mut wire = RepoMetaIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            batch_digest: String::new(),
            entries: self
                .entries
                .iter()
                .map(|entry| RepoMetaEntry {
                    source_repo_id: entry.source_repo_id.clone(),
                    key: entry.key.clone(),
                    value: entry.value.clone(),
                })
                .collect(),
        };
        stamp_batch_digest_v1(&mut wire)
            .map_err(|err| SdkError::Serialization(format!("RepoMeta batch digest: {err}")))?;
        Ok(wire)
    }
}

impl RepoTopicBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn entry(mut self, source_repo_id: RepoId, topic: impl Into<String>) -> Self {
        self.entries.push(RepoTopicMutation {
            source_repo_id,
            topic: topic.into(),
        });
        self
    }

    /// The canonical digest this batch publishes under.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
    }

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<RepoTopicIngestBatch, SdkError> {
        let mut wire = RepoTopicIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            batch_digest: String::new(),
            entries: self
                .entries
                .iter()
                .map(|entry| RepoTopicEntry {
                    source_repo_id: entry.source_repo_id.clone(),
                    topic: entry.topic.clone(),
                })
                .collect(),
        };
        stamp_batch_digest_v1(&mut wire)
            .map_err(|err| SdkError::Serialization(format!("RepoTopic batch digest: {err}")))?;
        Ok(wire)
    }
}

impl RepoDescriptionBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn entry(mut self, source_repo_id: RepoId, description: impl Into<String>) -> Self {
        self.entries.push(RepoDescriptionMutation {
            source_repo_id,
            description: description.into(),
        });
        self
    }

    /// The canonical digest this batch publishes under.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
    }

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<RepoDescriptionIngestBatch, SdkError> {
        let mut wire = RepoDescriptionIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            batch_digest: String::new(),
            entries: self
                .entries
                .iter()
                .map(|entry| RepoDescriptionEntry {
                    source_repo_id: entry.source_repo_id.clone(),
                    description: entry.description.clone(),
                })
                .collect(),
        };
        stamp_batch_digest_v1(&mut wire).map_err(|err| {
            SdkError::Serialization(format!("RepoDescription batch digest: {err}"))
        })?;
        Ok(wire)
    }
}

impl FileOwnershipBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn entry(
        mut self,
        source_repo_id: RepoId,
        repo_relative_path: RepoRelativePath,
        owners: Vec<String>,
    ) -> Self {
        self.entries.push(FileOwnershipMutation {
            source_repo_id,
            repo_relative_path,
            owners,
        });
        self
    }

    /// The canonical digest this batch publishes under.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
    }

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<FileOwnershipIngestBatch, SdkError> {
        let mut wire = FileOwnershipIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            batch_digest: String::new(),
            entries: self
                .entries
                .iter()
                .map(|entry| FileOwnershipEntry {
                    source_repo_id: entry.source_repo_id.clone(),
                    repo_relative_path: entry.repo_relative_path.clone(),
                    owners: entry.owners.clone(),
                })
                .collect(),
        };
        stamp_batch_digest_v1(&mut wire)
            .map_err(|err| SdkError::Serialization(format!("FileOwnership batch digest: {err}")))?;
        Ok(wire)
    }
}

impl FileContributorBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn entry(
        mut self,
        source_repo_id: RepoId,
        repo_relative_path: RepoRelativePath,
        contributors: Vec<String>,
    ) -> Self {
        self = self.entry_identities(
            source_repo_id,
            repo_relative_path,
            contributors
                .into_iter()
                .map(|canonical| FileContributorIdentityEntry {
                    canonical,
                    name: None,
                    email: None,
                })
                .collect(),
        );
        self
    }

    #[must_use]
    pub fn entry_identities(
        mut self,
        source_repo_id: RepoId,
        repo_relative_path: RepoRelativePath,
        contributors: Vec<FileContributorIdentityEntry>,
    ) -> Self {
        self.entries.push(FileContributorMutation {
            source_repo_id,
            repo_relative_path,
            contributors,
        });
        self
    }

    /// The canonical digest this batch publishes under.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
    }

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<FileContributorIngestBatch, SdkError> {
        let mut wire = FileContributorIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            batch_digest: String::new(),
            entries: self
                .entries
                .iter()
                .map(|entry| FileContributorEntry {
                    source_repo_id: entry.source_repo_id.clone(),
                    repo_relative_path: entry.repo_relative_path.clone(),
                    contributors: entry.contributors.clone(),
                })
                .collect(),
        };
        stamp_batch_digest_v1(&mut wire).map_err(|err| {
            SdkError::Serialization(format!("FileContributor batch digest: {err}"))
        })?;
        Ok(wire)
    }
}

pub struct HistoryNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> HistoryNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> HistoryQueryBuilder<'a> {
        <HistoryNs as crate::NamespaceQuery>::query(self.client)
    }

    pub fn publish(&self, batch: &HistoryBatch) -> Result<BatchReceipt, SdkError> {
        <HistoryNs as crate::NamespaceIngest>::publish(self.client, batch)
    }

    pub fn publish_repo_commit_recency(
        &self,
        batch: &RepoCommitRecencyBatch,
    ) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        let response = self.client.dispatch_ingest(
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(wire),
        )?;
        match response {
            SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repo commit recency receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }

    pub fn publish_repo_meta(&self, batch: &RepoMetaBatch) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        let response = self
            .client
            .dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repo meta receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }

    pub fn publish_repo_topic(&self, batch: &RepoTopicBatch) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        let response = self
            .client
            .dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repo topic receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }

    pub fn publish_repo_description(
        &self,
        batch: &RepoDescriptionBatch,
    ) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        let response = self.client.dispatch_ingest(
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(wire),
        )?;
        match response {
            SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repo description receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }

    pub fn publish_file_ownership(
        &self,
        batch: &FileOwnershipBatch,
    ) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        let response = self
            .client
            .dispatch_ingest(SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "file ownership receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }

    pub fn publish_file_contributor(
        &self,
        batch: &FileContributorBatch,
    ) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        let response = self.client.dispatch_ingest(
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(wire),
        )?;
        match response {
            SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "file contributor receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(
        &self,
        request: HistoryQueryRequest,
    ) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
        dispatch_history_query_request_v1(self.client, request)
    }
}

struct HistoryNs;

impl crate::NamespaceIngest for HistoryNs {
    type Batch = HistoryBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &HistoryBatch) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        let response =
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishHistoryBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::HistoryReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "history receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}

impl crate::NamespaceQuery for HistoryNs {
    type QueryBuilder<'a> = HistoryQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> HistoryQueryBuilder<'_> {
        HistoryQueryBuilder::new(client)
    }
}

fn map_ref_mutation(mutation: &RefMutation) -> HistoryRefMutation {
    match mutation {
        RefMutation::Upsert { name, sha } => HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: name.clone().into_boxed_str(),
            sha: *sha,
        }),
        RefMutation::Delete { name } => HistoryRefMutation::Delete(HistoryRefDelete {
            name: name.clone().into_boxed_str(),
        }),
    }
}

fn map_tag_mutation(mutation: &RefMutation) -> HistoryTagMutation {
    map_ref_mutation(mutation)
}

/// A history query builder.
///
/// The text, the generation selection, the `top_k` and the order are
/// each required and each tracked in the type: `execute` exists only once
/// all four are set, so a request cannot leave the SDK without saying
/// which order its pages are in (QI-BB-023 follow-up #1).
pub struct HistoryQueryBuilder<
    'a,
    const HAS_TEXT: bool = false,
    const HAS_SELECTION: bool = false,
    const HAS_TOP_K: bool = false,
    const HAS_ORDER: bool = false,
> {
    client: &'a QuantaIndex,
    state: TextQueryBuilderState,
    order: Option<HistoryOrderV1>,
    cursor: Option<ContinuationTokenV2>,
}

impl<'a> HistoryQueryBuilder<'a> {
    fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: TextQueryBuilderState::new(),
            order: None,
            cursor: None,
        }
    }
}

impl<
    'a,
    const HAS_TEXT: bool,
    const HAS_SELECTION: bool,
    const HAS_TOP_K: bool,
    const HAS_ORDER: bool,
> HistoryQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K, HAS_ORDER>
{
    fn transition<
        const NEXT_TEXT: bool,
        const NEXT_SELECTION: bool,
        const NEXT_TOP_K: bool,
        const NEXT_ORDER: bool,
    >(
        mut self,
        update: impl FnOnce(&mut TextQueryBuilderState),
    ) -> HistoryQueryBuilder<'a, NEXT_TEXT, NEXT_SELECTION, NEXT_TOP_K, NEXT_ORDER> {
        update(&mut self.state);
        HistoryQueryBuilder {
            client: self.client,
            state: self.state,
            order: self.order,
            cursor: self.cursor,
        }
    }

    /// Continue from the cursor a previous page returned (QI-BB-023).
    ///
    /// The page holds the next `top_k` results after it, under the order
    /// the cursor was issued under — which must be the order this walk
    /// asks for, or the plane refuses it `HISTORY_CURSOR_ORDER_MISMATCH`.
    ///
    /// The page is cut from the epoch the cursor names (QI-BB-020 W2). The
    /// cursor is passed through untouched; a walk whose epoch the plane no
    /// longer retains is refused `AUX_EPOCH_EXPIRED` and must start over.
    #[must_use]
    pub fn after(mut self, cursor: ContinuationTokenV2) -> Self {
        self.cursor = Some(cursor);
        self
    }

    /// The order the pages are in: `recency` (newest commit first, the
    /// text is a filter) or `relevance` (BM25 over the indexed text, every
    /// row scored). Required; there is no default.
    #[must_use]
    pub fn order(
        mut self,
        order: HistoryOrderV1,
    ) -> HistoryQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K, true> {
        self.order = Some(order);
        self.transition(|_state| {})
    }

    #[must_use]
    pub fn native(
        self,
        query_text: impl Into<String>,
    ) -> HistoryQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K, HAS_ORDER> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Native;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> HistoryQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K, HAS_ORDER> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Sourcegraph;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn pinned(
        self,
        pin: GenerationPin,
    ) -> HistoryQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K, HAS_ORDER> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Pinned(pin));
        })
    }

    #[must_use]
    pub fn active(
        self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> HistoryQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K, HAS_ORDER> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Active {
                repo_id,
                revision_id,
            });
        })
    }

    #[must_use]
    pub fn top_k(
        self,
        top_k: u32,
    ) -> HistoryQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, true, HAS_ORDER> {
        self.transition(|state| {
            state.top_k = Some(top_k);
        })
    }
}

impl HistoryQueryBuilder<'_, true, true, true, true> {
    pub fn execute(self) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
        let text_query = self.state.build_request("history")?;
        let order = self
            .order
            .ok_or_else(|| SdkError::Usage("history order is required".to_string()))?;
        dispatch_history_query_request_v1(
            self.client,
            HistoryQueryRequest {
                text_query,
                order,
                cursor: self.cursor,
            },
        )
    }
}

fn dispatch_history_query_request_v1(
    client: &QuantaIndex,
    request: HistoryQueryRequest,
) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
    let response = client.dispatch_query(SearchPlaneQueryIpcRequest::History(request))?;
    match response {
        SearchPlaneQueryIpcResponse::History(results) => Ok(results),
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
            "history response",
            QuantaIndex::query_response_kind(&other),
        )),
    }
}
