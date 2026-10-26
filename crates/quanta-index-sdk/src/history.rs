use ciborium::de::FiniteParserAllocationAdmissionV3;
use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
use quanta_index_contract::{
    BatchPublishReceipt, QueryErrorRepair, SearchPlaneErrorCodeV2,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneIpcError, admitted_auxiliary_response_seed_v1,
};
use quanta_index_contract::{
    BorrowedAuxiliaryIngestIpcRequestV1, ContinuationTokenV2, FileContributorEntry,
    FileContributorIdentityEntry, FileContributorIngestBatch, FileOwnershipEntry,
    FileOwnershipIngestBatch, GenerationPin, GenerationSelector, HistoryDiffHunkUpsert,
    HistoryIngestBatch, HistoryOrderV1, HistoryQueryRequest, HistoryRefDelete, HistoryRefMutation,
    HistoryRefUpsert, HistoryTagMutation, ManifestGeneration, RepoCommitRecencyEntry,
    RepoCommitRecencyIngestBatch, RepoDescriptionEntry, RepoDescriptionIngestBatch, RepoId,
    RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath, RepoTopicEntry, RepoTopicIngestBatch,
    RevisionId, SearchPlaneHistoryQueryResponse, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    TextQuerySyntax,
};
use quanta_index_ipc::{
    AdmittedCborDecodeErrorV1, AdmittedClientResponseBody, AdmittedDecodedCborV1,
    CanonicalBatchDigestProgressError, IpcError, MemoryScope, RequestBudgetV1,
    canonical_batch_digest_with_progress_v1, decode_cbor_seed_admitted_v1,
    matches_batch_digest_token_v1,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchReceipt, QuantaIndexClientPayloadV1, SdkError, stamp_batch_digest_v1};

/// Admission capabilities required by the auxiliary response decoder. This
/// boundary lets callers retain their Original owner without depending on the
/// SDK's CBOR implementation crate.
pub trait AdmittedAuxiliaryResponseOwnerV1:
    Clone + quanta_index_contract::SourceIdentityDecodeAdmissionV1 + FiniteParserAllocationAdmissionV3
{
}

impl<T> AdmittedAuxiliaryResponseOwnerV1 for T where
    T: Clone
        + quanta_index_contract::SourceIdentityDecodeAdmissionV1
        + FiniteParserAllocationAdmissionV3
{
}

fn verify_history_wire_digest_v1<B>(batch: &B) -> Result<(), SdkError>
where
    B: quanta_index_ipc::IngestBatchBodyV1 + serde::Serialize,
{
    match quanta_index_ipc::verify_batch_digest_v1(batch)
        .map_err(|error| SdkError::Serialization(error.to_string()))?
    {
        quanta_index_ipc::BatchDigestVerdictV1::Verified(_) => Ok(()),
        quanta_index_ipc::BatchDigestVerdictV1::Mismatch { carried, expected } => {
            Err(SdkError::Protocol(format!(
                "Auxiliary batch_digest {carried} is not the canonical digest {expected}"
            )))
        }
    }
}

/// A decoded receipt whose text and parser allocations remain under the
/// caller's Original owner until the wrapper is dropped. The canonical value
/// drops before the owner and raw response body.
pub struct AdmittedAuxiliaryBatchReceiptV1<O> {
    decoded: AdmittedDecodedCborV1<SearchPlaneIngestIpcResponseEnvelope, O>,
    _body: AdmittedClientResponseBody,
}

impl<O> AdmittedAuxiliaryBatchReceiptV1<O> {
    #[must_use]
    #[expect(
        clippy::unreachable,
        reason = "the private constructor admits only the five receipt variants matched here"
    )]
    pub fn receipt(&self) -> &BatchPublishReceipt {
        match &self.decoded.value().payload {
            SearchPlaneIngestIpcResponse::HistoryReceipt(receipt)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(receipt)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(receipt) => receipt,
            SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_)
            | SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_) => {
                unreachable!("validated auxiliary receipt wrapper has a receipt")
            }
        }
    }
}

/// The admitted remote error retains its decoded String grant while the
/// caller examines the code and message. No bare remote message escapes.
pub struct AdmittedAuxiliaryRemoteErrorV1<O> {
    decoded: AdmittedDecodedCborV1<SearchPlaneIngestIpcResponseEnvelope, O>,
    _body: AdmittedClientResponseBody,
}

impl<O> AdmittedAuxiliaryRemoteErrorV1<O> {
    #[expect(
        clippy::unreachable,
        reason = "the private constructor admits only an Error response after request and variant checks"
    )]
    fn error(&self) -> &SearchPlaneIpcError {
        match &self.decoded.value().payload {
            SearchPlaneIngestIpcResponse::Error(error) => error,
            SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_)
            | SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => {
                unreachable!("validated auxiliary remote-error wrapper has an error")
            }
        }
    }

    #[must_use]
    pub fn code(&self) -> SearchPlaneErrorCodeV2 {
        self.error().code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.error().message
    }

    #[must_use]
    pub fn repair(&self) -> Option<&QueryErrorRepair> {
        self.error().repair.as_ref()
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "Remote retains the admitted decoded response, owner, and raw body inline; boxing would allocate outside their Original custody"
)]
pub enum AuxiliaryAdmittedPublishErrorV1<E, O> {
    Progress(E),
    DigestEncode(IpcError),
    DigestMismatch,
    Sdk(SdkError),
    Decode(AdmittedCborDecodeErrorV1<E>),
    ResponseRequestIdMismatch,
    ResponseVariantMismatch,
    ResponseCommitmentMismatch,
    Remote(AdmittedAuxiliaryRemoteErrorV1<O>),
}

impl<E: std::fmt::Debug, O> std::fmt::Debug for AuxiliaryAdmittedPublishErrorV1<E, O> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Progress(cause) => formatter.debug_tuple("Progress").field(cause).finish(),
            Self::DigestEncode(cause) => {
                formatter.debug_tuple("DigestEncode").field(cause).finish()
            }
            Self::DigestMismatch => formatter.write_str("DigestMismatch"),
            Self::Sdk(cause) => formatter.debug_tuple("Sdk").field(cause).finish(),
            Self::Decode(cause) => formatter.debug_tuple("Decode").field(cause).finish(),
            Self::ResponseRequestIdMismatch => formatter.write_str("ResponseRequestIdMismatch"),
            Self::ResponseVariantMismatch => formatter.write_str("ResponseVariantMismatch"),
            Self::ResponseCommitmentMismatch => formatter.write_str("ResponseCommitmentMismatch"),
            Self::Remote(error) => formatter
                .debug_tuple("Remote")
                .field(&error.code())
                .finish(),
        }
    }
}

impl<E: std::fmt::Display, O> std::fmt::Display for AuxiliaryAdmittedPublishErrorV1<E, O> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Progress(cause) => write!(formatter, "Auxiliary publish interrupted: {cause}"),
            Self::DigestEncode(cause) => write!(formatter, "Auxiliary digest encode: {cause}"),
            Self::DigestMismatch => formatter.write_str("Auxiliary batch digest mismatch"),
            Self::Sdk(cause) => write!(formatter, "Auxiliary SDK: {cause}"),
            Self::Decode(_cause) => formatter.write_str("Auxiliary response decode refused"),
            Self::ResponseRequestIdMismatch => {
                formatter.write_str("Auxiliary response request id mismatch")
            }
            Self::ResponseVariantMismatch => {
                formatter.write_str("Auxiliary response variant mismatch")
            }
            Self::ResponseCommitmentMismatch => {
                formatter.write_str("Auxiliary response commitment mismatch")
            }
            Self::Remote(error) => write!(
                formatter,
                "Auxiliary remote {}: {}",
                error.code(),
                error.message()
            ),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display, O> std::error::Error
    for AuxiliaryAdmittedPublishErrorV1<E, O>
{
}

#[expect(
    clippy::result_large_err,
    reason = "the typed remote refusal retains its admitted decoded body and owner inline without an unfunded box"
)]
fn verify_auxiliary_wire_digest_with_progress_v1<B, E, F, O>(
    batch: &B,
    work: &mut F,
) -> Result<(), AuxiliaryAdmittedPublishErrorV1<E, O>>
where
    B: quanta_index_ipc::IngestBatchBodyV1 + serde::Serialize,
    F: FnMut(usize) -> Result<(), E>,
{
    let digest =
        canonical_batch_digest_with_progress_v1(batch, work).map_err(|error| match error {
            CanonicalBatchDigestProgressError::Encode(cause) => {
                AuxiliaryAdmittedPublishErrorV1::DigestEncode(cause)
            }
            CanonicalBatchDigestProgressError::Progress(cause) => {
                AuxiliaryAdmittedPublishErrorV1::Progress(cause)
            }
        })?;
    if matches_batch_digest_token_v1(batch.batch_digest(), &digest) {
        Ok(())
    } else {
        Err(AuxiliaryAdmittedPublishErrorV1::DigestMismatch)
    }
}

#[expect(
    clippy::result_large_err,
    reason = "the typed remote refusal retains its admitted decoded body and owner inline without an unfunded box"
)]
pub(crate) fn publish_borrowed_auxiliary_admitted_v1<E, F, O>(
    client: &QuantaIndexClientPayloadV1,
    payload: BorrowedAuxiliaryIngestIpcRequestV1<'_>,
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
    owner: O,
    work: &mut F,
) -> Result<AdmittedAuxiliaryBatchReceiptV1<O>, AuxiliaryAdmittedPublishErrorV1<E, O>>
where
    F: FnMut(usize) -> Result<(), E>,
    E: std::fmt::Debug,
    O: AdmittedAuxiliaryResponseOwnerV1,
{
    match &payload {
        BorrowedAuxiliaryIngestIpcRequestV1::History(batch) => {
            verify_auxiliary_wire_digest_with_progress_v1(*batch, work)?;
        }
        BorrowedAuxiliaryIngestIpcRequestV1::RepoCommitRecency(batch) => {
            verify_auxiliary_wire_digest_with_progress_v1(*batch, work)?;
        }
        BorrowedAuxiliaryIngestIpcRequestV1::FileContributor(batch) => {
            verify_auxiliary_wire_digest_with_progress_v1(*batch, work)?;
        }
        BorrowedAuxiliaryIngestIpcRequestV1::Dirty(batch) => {
            verify_auxiliary_wire_digest_with_progress_v1(*batch, work)?;
        }
        BorrowedAuxiliaryIngestIpcRequestV1::Structural(batch) => {
            verify_auxiliary_wire_digest_with_progress_v1(*batch, work)?;
        }
        BorrowedAuxiliaryIngestIpcRequestV1::RepoMap(_) => {
            return Err(AuxiliaryAdmittedPublishErrorV1::Sdk(SdkError::Protocol(
                "RepoMap requires its exact source-bundle receipt binding route".to_string(),
            )));
        }
    }

    let mut transport_refusal = None;
    let sent = client.dispatch_borrowed_auxiliary_ingest_admitted_v1(
        payload,
        budget,
        memory,
        &mut |bytes| match work(bytes) {
            Ok(()) => Ok(()),
            Err(cause) => {
                transport_refusal = Some(cause);
                Err(IpcError::Io(std::io::ErrorKind::Interrupted.into()))
            }
        },
    );
    if let Some(cause) = transport_refusal {
        return Err(AuxiliaryAdmittedPublishErrorV1::Progress(cause));
    }
    let (request_id, body) = sent.map_err(AuxiliaryAdmittedPublishErrorV1::Sdk)?;
    let visitor_owner = owner.clone();
    let mut scratch = [0_u8; 128];
    let decoded = decode_cbor_seed_admitted_v1(
        body.as_slice(),
        &mut scratch,
        admitted_auxiliary_response_seed_v1(&visitor_owner, body.as_slice().len()),
        work,
        quanta_index_contract::MAX_IPC_FRAME_BODY_BYTES_V1,
        owner,
    )
    .map_err(|error| match error {
        AdmittedCborDecodeErrorV1::Work(cause) => AuxiliaryAdmittedPublishErrorV1::Progress(cause),
        other @ (AdmittedCborDecodeErrorV1::InputTooLarge { .. }
        | AdmittedCborDecodeErrorV1::ScratchTooSmall { .. }
        | AdmittedCborDecodeErrorV1::Frame(_)
        | AdmittedCborDecodeErrorV1::Parser(_)) => AuxiliaryAdmittedPublishErrorV1::Decode(other),
    })?;
    if decoded.value().request_id != request_id {
        return Err(AuxiliaryAdmittedPublishErrorV1::ResponseRequestIdMismatch);
    }
    let receipt = match (&payload, &decoded.value().payload) {
        (
            BorrowedAuxiliaryIngestIpcRequestV1::History(_),
            SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
        )
        | (
            BorrowedAuxiliaryIngestIpcRequestV1::RepoCommitRecency(_),
            SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt),
        )
        | (
            BorrowedAuxiliaryIngestIpcRequestV1::FileContributor(_),
            SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt),
        )
        | (
            BorrowedAuxiliaryIngestIpcRequestV1::Dirty(_),
            SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
        )
        | (
            BorrowedAuxiliaryIngestIpcRequestV1::Structural(_),
            SearchPlaneIngestIpcResponse::StructuralReceipt(receipt),
        ) => receipt,
        (_, SearchPlaneIngestIpcResponse::Error(_)) => {
            return Err(AuxiliaryAdmittedPublishErrorV1::Remote(
                AdmittedAuxiliaryRemoteErrorV1 {
                    decoded,
                    _body: body,
                },
            ));
        }
        _ => return Err(AuxiliaryAdmittedPublishErrorV1::ResponseVariantMismatch),
    };
    let (generation, digest) = match payload {
        BorrowedAuxiliaryIngestIpcRequestV1::History(batch) => {
            (batch.generation, batch.batch_digest.as_str())
        }
        BorrowedAuxiliaryIngestIpcRequestV1::RepoCommitRecency(batch) => {
            (batch.generation, batch.batch_digest.as_str())
        }
        BorrowedAuxiliaryIngestIpcRequestV1::FileContributor(batch) => {
            (batch.generation, batch.batch_digest.as_str())
        }
        BorrowedAuxiliaryIngestIpcRequestV1::Dirty(batch) => {
            (batch.generation, batch.batch_digest.as_str())
        }
        BorrowedAuxiliaryIngestIpcRequestV1::Structural(batch) => {
            (batch.generation, batch.batch_digest.as_str())
        }
        BorrowedAuxiliaryIngestIpcRequestV1::RepoMap(_) => {
            return Err(AuxiliaryAdmittedPublishErrorV1::Sdk(SdkError::Protocol(
                "RepoMap requires its exact source-bundle receipt binding route".to_string(),
            )));
        }
    };
    if !crate::binding::auxiliary_receipt_matches_v1(generation, digest, receipt) {
        return Err(AuxiliaryAdmittedPublishErrorV1::ResponseCommitmentMismatch);
    }
    Ok(AdmittedAuxiliaryBatchReceiptV1 {
        decoded,
        _body: body,
    })
}

fn history_receipt_v1(response: SearchPlaneIngestIpcResponse) -> Result<BatchReceipt, SdkError> {
    if let SearchPlaneIngestIpcResponse::HistoryReceipt(receipt) = response {
        Ok(receipt)
    } else {
        Err(SdkError::unexpected_response(
            "history receipt",
            QuantaIndexClientPayloadV1::ingest_response_kind(&response),
        ))
    }
}

fn repo_commit_recency_receipt_v1(
    response: SearchPlaneIngestIpcResponse,
) -> Result<BatchReceipt, SdkError> {
    if let SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt) = response {
        Ok(receipt)
    } else {
        Err(SdkError::unexpected_response(
            "repo commit recency receipt",
            QuantaIndexClientPayloadV1::ingest_response_kind(&response),
        ))
    }
}

fn file_contributor_receipt_v1(
    response: SearchPlaneIngestIpcResponse,
) -> Result<BatchReceipt, SdkError> {
    if let SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt) = response {
        Ok(receipt)
    } else {
        Err(SdkError::unexpected_response(
            "file contributor receipt",
            QuantaIndexClientPayloadV1::ingest_response_kind(&response),
        ))
    }
}

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
    client: &'a QuantaIndexClientPayloadV1,
}

impl<'a> HistoryNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndexClientPayloadV1) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> HistoryQueryBuilder<'a> {
        <HistoryNs as crate::NamespaceQuery>::query(self.client)
    }

    pub fn publish(&self, batch: &HistoryBatch) -> Result<BatchReceipt, SdkError> {
        <HistoryNs as crate::NamespaceIngest>::publish(self.client, batch)
    }

    /// Publish a caller-retained, canonical History wire batch without copying
    /// its commit and diff-hunk bodies. The caller must retain its admission
    /// and memory custody until this synchronous call returns.
    pub fn publish_history_borrowed_v1(
        &self,
        batch: &HistoryIngestBatch,
    ) -> Result<BatchReceipt, SdkError> {
        verify_history_wire_digest_v1(batch)?;
        history_receipt_v1(self.client.dispatch_borrowed_auxiliary_ingest_v1(
            BorrowedAuxiliaryIngestIpcRequestV1::History(batch),
        )?)
    }

    /// Publish a caller-retained History DTO under the same execution budget,
    /// memory scope and parser/visitor owner as its Original source.
    #[expect(
        clippy::result_large_err,
        reason = "the typed remote refusal retains decoded response, owner, and raw body inline under Original custody"
    )]
    pub fn publish_history_borrowed_admitted_v1<E, F, O>(
        &self,
        batch: &HistoryIngestBatch,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
        owner: O,
        work: &mut F,
    ) -> Result<AdmittedAuxiliaryBatchReceiptV1<O>, AuxiliaryAdmittedPublishErrorV1<E, O>>
    where
        F: FnMut(usize) -> Result<(), E>,
        E: std::fmt::Debug,
        O: AdmittedAuxiliaryResponseOwnerV1,
    {
        publish_borrowed_auxiliary_admitted_v1(
            self.client,
            BorrowedAuxiliaryIngestIpcRequestV1::History(batch),
            budget,
            memory,
            owner,
            work,
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the typed remote refusal retains decoded response, owner, and raw body inline under Original custody"
    )]
    pub fn publish_repo_commit_recency_borrowed_admitted_v1<E, F, O>(
        &self,
        batch: &RepoCommitRecencyIngestBatch,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
        owner: O,
        work: &mut F,
    ) -> Result<AdmittedAuxiliaryBatchReceiptV1<O>, AuxiliaryAdmittedPublishErrorV1<E, O>>
    where
        F: FnMut(usize) -> Result<(), E>,
        E: std::fmt::Debug,
        O: AdmittedAuxiliaryResponseOwnerV1,
    {
        publish_borrowed_auxiliary_admitted_v1(
            self.client,
            BorrowedAuxiliaryIngestIpcRequestV1::RepoCommitRecency(batch),
            budget,
            memory,
            owner,
            work,
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the typed remote refusal retains decoded response, owner, and raw body inline under Original custody"
    )]
    pub fn publish_file_contributor_borrowed_admitted_v1<E, F, O>(
        &self,
        batch: &FileContributorIngestBatch,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
        owner: O,
        work: &mut F,
    ) -> Result<AdmittedAuxiliaryBatchReceiptV1<O>, AuxiliaryAdmittedPublishErrorV1<E, O>>
    where
        F: FnMut(usize) -> Result<(), E>,
        E: std::fmt::Debug,
        O: AdmittedAuxiliaryResponseOwnerV1,
    {
        publish_borrowed_auxiliary_admitted_v1(
            self.client,
            BorrowedAuxiliaryIngestIpcRequestV1::FileContributor(batch),
            budget,
            memory,
            owner,
            work,
        )
    }

    pub fn publish_repo_commit_recency_borrowed_v1(
        &self,
        batch: &RepoCommitRecencyIngestBatch,
    ) -> Result<BatchReceipt, SdkError> {
        verify_history_wire_digest_v1(batch)?;
        repo_commit_recency_receipt_v1(self.client.dispatch_borrowed_auxiliary_ingest_v1(
            BorrowedAuxiliaryIngestIpcRequestV1::RepoCommitRecency(batch),
        )?)
    }

    pub fn publish_file_contributor_borrowed_v1(
        &self,
        batch: &FileContributorIngestBatch,
    ) -> Result<BatchReceipt, SdkError> {
        verify_history_wire_digest_v1(batch)?;
        file_contributor_receipt_v1(self.client.dispatch_borrowed_auxiliary_ingest_v1(
            BorrowedAuxiliaryIngestIpcRequestV1::FileContributor(batch),
        )?)
    }

    pub fn publish_repo_commit_recency(
        &self,
        batch: &RepoCommitRecencyBatch,
    ) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        verify_history_wire_digest_v1(&wire)?;
        let response = self.client.dispatch_ingest(
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(wire),
        )?;
        repo_commit_recency_receipt_v1(response)
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
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repo meta receipt",
                QuantaIndexClientPayloadV1::ingest_response_kind(&other),
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
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repo topic receipt",
                QuantaIndexClientPayloadV1::ingest_response_kind(&other),
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
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repo description receipt",
                QuantaIndexClientPayloadV1::ingest_response_kind(&other),
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
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "file ownership receipt",
                QuantaIndexClientPayloadV1::ingest_response_kind(&other),
            )),
        }
    }

    pub fn publish_file_contributor(
        &self,
        batch: &FileContributorBatch,
    ) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        verify_history_wire_digest_v1(&wire)?;
        let response = self.client.dispatch_ingest(
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(wire),
        )?;
        file_contributor_receipt_v1(response)
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

    fn publish(
        client: &QuantaIndexClientPayloadV1,
        batch: &HistoryBatch,
    ) -> Result<BatchReceipt, SdkError> {
        let wire = batch.to_wire_batch()?;
        verify_history_wire_digest_v1(&wire)?;
        let response =
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishHistoryBatch(wire))?;
        history_receipt_v1(response)
    }
}

impl crate::NamespaceQuery for HistoryNs {
    type QueryBuilder<'a> = HistoryQueryBuilder<'a>;

    fn query(client: &QuantaIndexClientPayloadV1) -> HistoryQueryBuilder<'_> {
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
    client: &'a QuantaIndexClientPayloadV1,
    state: TextQueryBuilderState,
    order: Option<HistoryOrderV1>,
    cursor: Option<ContinuationTokenV2>,
}

impl<'a> HistoryQueryBuilder<'a> {
    fn new(client: &'a QuantaIndexClientPayloadV1) -> Self {
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
    client: &QuantaIndexClientPayloadV1,
    request: HistoryQueryRequest,
) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
    let response = client.dispatch_query(SearchPlaneQueryIpcRequest::History(request))?;
    match response {
        SearchPlaneQueryIpcResponse::History(results) => Ok(results),
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
            "history response",
            QuantaIndexClientPayloadV1::query_response_kind(&other),
        )),
    }
}
