use quanta_index_contract::lex::DirtyRecord;
use quanta_index_contract::{
    ChunkId, ContinuationTokenV2, DirtyDelete, DirtyIngestBatch, DirtyMutation, GenerationPin,
    GenerationSelector, ManifestGeneration, RepoId, RevisionId, RuntimeMetadataQueryRequest,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneRuntimeMetadataQueryResponse, TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchReceipt, QuantaIndex, SdkError, stamp_batch_digest_v1};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirtyBatchMutation {
    Upsert(DirtyRecord),
    Delete { doc_id: ChunkId },
}

/// A dirty-overlay publish.
///
/// Its `batch_digest` is the canonical digest of the wire body, computed
/// when it is sent (QI-BB-032); see [`Self::batch_digest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub overlay_epoch_ms: u64,
    pub entries: Vec<DirtyBatchMutation>,
}

impl DirtyBatch {
    #[must_use]
    pub fn new(
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
        overlay_epoch_ms: u64,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            overlay_epoch_ms,
            entries: Vec::new(),
        }
    }

    /// The canonical digest this batch publishes under.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
    }

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<DirtyIngestBatch, SdkError> {
        let mut wire = DirtyIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            overlay_epoch_ms: self.overlay_epoch_ms,
            batch_digest: String::new(),
            entries: self
                .entries
                .iter()
                .map(|entry| match entry {
                    DirtyBatchMutation::Upsert(record) => DirtyMutation::Upsert(record.clone()),
                    DirtyBatchMutation::Delete { doc_id } => DirtyMutation::Delete(DirtyDelete {
                        doc_id: doc_id.clone(),
                    }),
                })
                .collect(),
        };
        stamp_batch_digest_v1(&mut wire)
            .map_err(|err| SdkError::Serialization(format!("dirty batch digest: {err}")))?;
        Ok(wire)
    }

    #[must_use]
    pub fn upsert(mut self, record: DirtyRecord) -> Self {
        self.entries.push(DirtyBatchMutation::Upsert(record));
        self
    }

    #[must_use]
    pub fn delete(mut self, doc_id: ChunkId) -> Self {
        self.entries.push(DirtyBatchMutation::Delete { doc_id });
        self
    }
}

pub struct RuntimeNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> RuntimeNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> RuntimeQueryBuilder<'a> {
        <RuntimeNs as crate::NamespaceQuery>::query(self.client)
    }

    pub fn publish_dirty(&self, batch: &DirtyBatch) -> Result<BatchReceipt, SdkError> {
        <RuntimeNs as crate::NamespaceIngest>::publish(self.client, batch)
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(
        &self,
        request: RuntimeMetadataQueryRequest,
    ) -> Result<SearchPlaneRuntimeMetadataQueryResponse, SdkError> {
        dispatch_runtime_query_request_v1(self.client, request)
    }
}

struct RuntimeNs;

impl crate::NamespaceIngest for RuntimeNs {
    type Batch = DirtyBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &DirtyBatch) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        let response =
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishDirtyBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::DirtyReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "dirty receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}

impl crate::NamespaceQuery for RuntimeNs {
    type QueryBuilder<'a> = RuntimeQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> RuntimeQueryBuilder<'_> {
        RuntimeQueryBuilder::new(client)
    }
}

pub struct RuntimeQueryBuilder<
    'a,
    const HAS_TEXT: bool = false,
    const HAS_SELECTION: bool = false,
    const HAS_TOP_K: bool = false,
> {
    client: &'a QuantaIndex,
    state: TextQueryBuilderState,
    cursor: Option<ContinuationTokenV2>,
}

impl<'a> RuntimeQueryBuilder<'a> {
    fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: TextQueryBuilderState::new(),
            cursor: None,
        }
    }
}

impl<'a, const HAS_TEXT: bool, const HAS_SELECTION: bool, const HAS_TOP_K: bool>
    RuntimeQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K>
{
    fn transition<const NEXT_TEXT: bool, const NEXT_SELECTION: bool, const NEXT_TOP_K: bool>(
        mut self,
        update: impl FnOnce(&mut TextQueryBuilderState),
    ) -> RuntimeQueryBuilder<'a, NEXT_TEXT, NEXT_SELECTION, NEXT_TOP_K> {
        update(&mut self.state);
        RuntimeQueryBuilder {
            client: self.client,
            state: self.state,
            cursor: self.cursor,
        }
    }

    /// Continue from the cursor a previous page returned (QI-BB-025 W4):
    /// the page holds the next `top_k` rows in candidate-id order after
    /// it.
    ///
    /// The page is cut from the runtime and structural epochs the cursor
    /// names (QI-BB-020 W2). The cursor is passed through untouched; a
    /// walk whose epoch the plane no longer retains is refused
    /// `AUX_EPOCH_EXPIRED` and must start over.
    #[must_use]
    pub fn after(mut self, cursor: ContinuationTokenV2) -> Self {
        self.cursor = Some(cursor);
        self
    }

    #[must_use]
    pub fn native(
        self,
        query_text: impl Into<String>,
    ) -> RuntimeQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Native;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> RuntimeQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Sourcegraph;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn pinned(self, pin: GenerationPin) -> RuntimeQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Pinned(pin));
        })
    }

    #[must_use]
    pub fn active(
        self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> RuntimeQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Active {
                repo_id,
                revision_id,
            });
        })
    }

    #[must_use]
    pub fn top_k(self, top_k: u32) -> RuntimeQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, true> {
        self.transition(|state| {
            state.top_k = Some(top_k);
        })
    }
}

impl RuntimeQueryBuilder<'_, true, true, true> {
    pub fn execute(self) -> Result<SearchPlaneRuntimeMetadataQueryResponse, SdkError> {
        let text_query = self.state.build_request("runtime")?;
        dispatch_runtime_query_request_v1(
            self.client,
            RuntimeMetadataQueryRequest {
                text_query,
                cursor: self.cursor,
            },
        )
    }
}

fn dispatch_runtime_query_request_v1(
    client: &QuantaIndex,
    request: RuntimeMetadataQueryRequest,
) -> Result<SearchPlaneRuntimeMetadataQueryResponse, SdkError> {
    let response = client.dispatch_query(SearchPlaneQueryIpcRequest::RuntimeMetadata(request))?;
    match response {
        SearchPlaneQueryIpcResponse::RuntimeMetadata(results) => Ok(results),
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
            "runtime response",
            QuantaIndex::query_response_kind(&other),
        )),
    }
}
