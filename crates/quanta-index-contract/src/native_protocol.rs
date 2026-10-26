//! Opt-in native incremental index control wire. No generic object CRUD.

use crate::index::{
    CatalogView, CoverageBinding, IndexInstanceId, IndexProfile, IngressRef, MapRoot, ObjectRef,
    OperationKey, OperationReply, OperationStatus, OwnerKey, OwnerMutation, OwnerMutationSet,
    ProducerId, ProjectionBinding, ProjectionCertificate, ProjectionId,
    ProjectionOwnerContribution, SourceCut, SourceInput, SourceRecord, SourceTextAdmission,
    StableOwnerId, SubmitOutcome, UpdateRequest, ViewRef, ViewSelection, WorkClaim, WorkIntent,
    WorkStatus, WorkspaceKey,
};
use crate::{FiniteScore, SemanticQueryRequest, TextQueryRequest};

mod read_session;
pub use read_session::{
    NativeReadSessionId, NativeReadSessionReply, NativeReadSessionRequest,
    NativeReadSessionSettlement, NativeReadSessionState, NativeReadSessionStatus,
    NativeReadSettlementState,
};
mod graph;
mod projection_children;
pub use projection_children::NativeProjectionOutputChildren;
mod source_diff;
pub use source_diff::{NativeProjectionSourceDiff, NativeSourceDiffRows};
mod headless;
mod stub;
mod stub_schema;
mod stub_certificate;
pub use graph::{
    GraphEdge, GraphNode, GraphOwnerOutput, GraphProjectionRow, MAX_NATIVE_GRAPH_EDGES,
    MAX_NATIVE_GRAPH_ITEMS, MAX_REPOMAP_RANK_FOCUS_SUBJECTS, MAX_REPOMAP_RANK_HITS,
    MAX_REPOMAP_RANK_PATH_BYTES, MAX_REPOMAP_RANK_PROJECTION_LEAF_BYTES,
    MAX_REPOMAP_RANK_QUERY_BYTES, MAX_REPOMAP_RANK_SEARCH_TEXT_BYTES,
    MAX_REPOMAP_RANK_SUBJECT_BYTES, NATIVE_REPOMAP_RANK_POLICY_DESCRIPTOR, NativeGraphDirection,
    NativeGraphQuery, NativeGraphResult, NativeGraphRow, NativeRepoMapHit, NativeRepoMapQuery,
    NativeRepoMapResult, NativeRepoMapResultRef, native_repomap_rank_policy,
};
pub use headless::{
    NativeOverlayCorpusStamp, NativeOverlayDocument, NativeOverlayEffortContext, NativeOverlayMask,
    NativeOverlayReply, NativeOverlayRequest, NativeOverlayRevision, NativeOverlaySaveIntent,
    NativeOverlaySaveReceipt, NativeOverlaySemanticChunk, NativeOverlaySemanticHit,
    NativeOverlaySemanticHitOrigin, NativeOverlaySemanticResult, NativeOverlaySession,
    NativeOverlaySnapshot, NativeOverlayTextHit, NativeOverlayTextQuery, NativeOverlayTextResult,
    NativeOverlayTextSource,
};
pub use stub::{
    NativeStubBinding, NativeStubByteEdit, NativeStubCoordinateUnit, NativeStubEdit,
    NativeStubItem, NativeStubPosition, NativeStubRange, NativeStubReply, NativeStubRequest,
};
pub use stub_schema::native_stub_preflight_schema_v1;
pub use stub_certificate::{
    MAX_NATIVE_STUB_CERTIFICATE_BYTES, MAX_NATIVE_STUB_CERTIFICATE_ITEMS, NATIVE_STUB_AST_PRODUCER,
    NATIVE_STUB_AST_PROJECTION, NATIVE_STUB_AST_SCHEMA, NativeStubCertificate,
    NativeStubCertificateError, NativeStubCertificateHeader, NativeStubCertificateItem,
    NativeStubCertificateIter, encode_native_stub_certificate_into,
    native_stub_certificate_encoded_len, native_stub_certificate_policy,
};

mod semantic_chunk;
mod semantic_chunk_manifest;
pub use semantic_chunk::{
    NativeSemanticChunk, NativeSemanticChunkSpan, NativeSemanticChunkValidationError,
};
pub use semantic_chunk_manifest::{
    NativeSemanticChunkManifest, NativeSemanticManifestError, NativeSemanticManifestHeader,
    NativeSemanticManifestIter, NativeSemanticManifestLimits, NativeSemanticManifestMember,
    encode_native_semantic_manifest_into, native_semantic_manifest_encoded_len,
};

/// Native text reader cap; the canonical public request may permit a larger
/// `top_k`, which this native route refuses before executing a query.
pub const MAX_NATIVE_TEXT_TOP_K: u32 = 1_024;
/// Upper bound for one native semantic ranked page.
pub const MAX_NATIVE_SEMANTIC_TOP_K: u32 = 4_096;
/// Upper bound on UTF-8 query bytes accepted by the native semantic route.
pub const MAX_NATIVE_SEMANTIC_QUERY_BYTES: usize = 16 * 1_024;
/// Maximum number of owners the native semantic reader scans per query.
pub const MAX_NATIVE_SEMANTIC_OWNER_SCAN: u32 = 100_000;
/// Upper bound on a vector identity in one native semantic hit.
pub const MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES: usize = 1_024;
/// Maximum direct child references in one immutable projection output.
pub const MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN: usize = 4_096;
/// Maximum number of exact source-owner bindings in one lookup.
pub const MAX_NATIVE_SOURCE_OWNER_LOOKUP: usize = 128;

trait SourceOwnerItem {
    fn owner(&self) -> OwnerKey;
}

impl SourceOwnerItem for OwnerKey {
    fn owner(&self) -> OwnerKey {
        *self
    }
}

fn validate_source_owner_items<T: SourceOwnerItem, const ALLOW_EMPTY: bool>(
    items: &[T],
) -> Result<(), &'static str> {
    if (!ALLOW_EMPTY && items.is_empty()) || items.len() > MAX_NATIVE_SOURCE_OWNER_LOOKUP {
        return Err("source owner lookup requires 1..=128 owners");
    }
    let mut previous = None;
    for item in items {
        let owner = item.owner();
        if previous.is_some_and(|last| last >= owner) {
            return Err("source owner lookup owners must be strictly ascending and unique");
        }
        previous = Some(owner);
    }
    Ok(())
}

fn deserialize_source_owner_items<'de, D, T, const ALLOW_EMPTY: bool>(
    deserializer: D,
) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + SourceOwnerItem,
{
    use serde::de::{Error as _, Visitor};
    struct ItemsVisitor<T, const ALLOW_EMPTY: bool>(std::marker::PhantomData<T>);
    struct RejectExtra;

    impl<'de> serde::de::DeserializeSeed<'de> for RejectExtra {
        type Value = ();

        fn deserialize<D: serde::Deserializer<'de>>(
            self,
            _deserializer: D,
        ) -> Result<Self::Value, D::Error> {
            Err(D::Error::custom("source owner lookup count exceeds 128"))
        }
    }

    impl<'de, T, const ALLOW_EMPTY: bool> Visitor<'de> for ItemsVisitor<T, ALLOW_EMPTY>
    where
        T: serde::Deserialize<'de> + SourceOwnerItem,
    {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            if ALLOW_EMPTY {
                formatter.write_str("0..=128 strictly ascending unique source owners")
            } else {
                formatter.write_str("1..=128 strictly ascending unique source owners")
            }
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            if seq
                .size_hint()
                .is_some_and(|hint| hint > MAX_NATIVE_SOURCE_OWNER_LOOKUP)
            {
                return Err(A::Error::custom(
                    "source owner lookup declared count exceeds 128",
                ));
            }
            let mut items = Vec::new();
            if let Some(hint) = seq.size_hint() {
                items.try_reserve_exact(hint).map_err(A::Error::custom)?;
            }
            let mut previous = None;
            loop {
                if items.len() == MAX_NATIVE_SOURCE_OWNER_LOOKUP {
                    if seq.next_element_seed(RejectExtra)?.is_some() {
                        return Err(A::Error::custom("source owner lookup count exceeds 128"));
                    }
                    break;
                }
                let Some(item) = seq.next_element::<T>()? else {
                    break;
                };
                let owner = item.owner();
                if previous.is_some_and(|last| last >= owner) {
                    return Err(A::Error::custom(
                        "source owner lookup owners must be strictly ascending and unique",
                    ));
                }
                items.try_reserve_exact(1).map_err(A::Error::custom)?;
                items.push(item);
                previous = Some(owner);
            }
            if !ALLOW_EMPTY && items.is_empty() {
                return Err(A::Error::custom(
                    "source owner lookup requires at least one owner",
                ));
            }
            Ok(items)
        }
    }

    deserializer.deserialize_seq(ItemsVisitor::<T, ALLOW_EMPTY>(std::marker::PhantomData))
}

/// Ordered unique lookup keys. Construction and decoding enforce the same cap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSourceOwnerKeys(Vec<OwnerKey>);

impl NativeSourceOwnerKeys {
    pub fn new(owners: Vec<OwnerKey>) -> Result<Self, &'static str> {
        validate_source_owner_items::<_, false>(&owners)?;
        Ok(Self(owners))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[OwnerKey] {
        &self.0
    }
}

impl serde::Serialize for NativeSourceOwnerKeys {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for NativeSourceOwnerKeys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_source_owner_items::<_, _, false>(deserializer).map(Self)
    }
}

/// One source binding under the selected immutable source cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSourceOwnerBinding {
    pub owner: OwnerKey,
    pub source: Option<ObjectRef>,
}

impl SourceOwnerItem for NativeSourceOwnerBinding {
    fn owner(&self) -> OwnerKey {
        self.owner
    }
}

/// Ordered unique source bindings; the SDK additionally checks request equality.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSourceOwnerRows(Vec<NativeSourceOwnerBinding>);

impl NativeSourceOwnerRows {
    pub fn new(rows: Vec<NativeSourceOwnerBinding>) -> Result<Self, &'static str> {
        validate_source_owner_items::<_, false>(&rows)?;
        Ok(Self(rows))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[NativeSourceOwnerBinding] {
        &self.0
    }
}

impl serde::Serialize for NativeSourceOwnerRows {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for NativeSourceOwnerRows {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_source_owner_items::<_, _, false>(deserializer).map(Self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSourceOwnerLookup {
    pub view: ViewRef,
    pub source: SourceCut,
    pub rows: NativeSourceOwnerRows,
}

/// A projection contribution CAS stamp, distinct from a source record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeProjectionOwnerBinding {
    pub owner: OwnerKey,
    pub contribution: Option<ObjectRef>,
}

impl SourceOwnerItem for NativeProjectionOwnerBinding {
    fn owner(&self) -> OwnerKey {
        self.owner
    }
}

/// One bounded exact projection-owner lookup; missing owners retain None.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProjectionOwnerRows(Vec<NativeProjectionOwnerBinding>);

impl NativeProjectionOwnerRows {
    pub fn new(rows: Vec<NativeProjectionOwnerBinding>) -> Result<Self, &'static str> {
        validate_source_owner_items::<_, false>(&rows)?;
        Ok(Self(rows))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[NativeProjectionOwnerBinding] {
        &self.0
    }
}

impl serde::Serialize for NativeProjectionOwnerRows {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for NativeProjectionOwnerRows {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_source_owner_items::<_, _, false>(deserializer).map(Self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProjectionOwnerLookup {
    pub view: ViewRef,
    pub projection: ProjectionId,
    pub binding: Option<ProjectionBinding>,
    pub rows: NativeProjectionOwnerRows,
}

/// One existing source owner in a producer-scoped recovery page. Deleted or
/// absent owners are omitted rather than represented by a nullable source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeProducerSourceOwner {
    pub owner: OwnerKey,
    pub source: ObjectRef,
}

impl SourceOwnerItem for NativeProducerSourceOwner {
    fn owner(&self) -> OwnerKey {
        self.owner
    }
}

/// Zero to 128 strictly ordered owner bindings. An empty final page is valid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProducerSourceRows(Vec<NativeProducerSourceOwner>);

impl NativeProducerSourceRows {
    pub fn new(rows: Vec<NativeProducerSourceOwner>) -> Result<Self, &'static str> {
        validate_source_owner_items::<_, true>(&rows)?;
        Ok(Self(rows))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[NativeProducerSourceOwner] {
        &self.0
    }
}

impl serde::Serialize for NativeProducerSourceRows {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for NativeProducerSourceRows {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_source_owner_items::<_, _, true>(deserializer).map(Self)
    }
}

/// Exact current-view producer inventory for rebuilding a complete resync
/// baseline. `has_more` requires a nonempty page so its cursor can advance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProducerSourcePage {
    pub view: ViewRef,
    pub source: SourceCut,
    pub producer: ProducerId,
    pub source_epoch: Option<u64>,
    pub rows: NativeProducerSourceRows,
    pub has_more: bool,
}

impl NativeProducerSourcePage {
    fn validate(&self) -> Result<(), &'static str> {
        if self.has_more && self.rows.as_slice().is_empty() {
            return Err("producer source continuation requires a nonempty page");
        }
        if self.source_epoch.is_none() && !self.rows.as_slice().is_empty() {
            return Err("producer source rows require an established source epoch");
        }
        if self
            .rows
            .as_slice()
            .iter()
            .any(|row| row.owner.producer != self.producer)
        {
            return Err("producer source row belongs to another producer");
        }
        Ok(())
    }

    #[must_use]
    pub fn next_after(&self) -> Option<StableOwnerId> {
        if self.has_more {
            self.rows.as_slice().last().map(|row| row.owner.stable_id)
        } else {
            None
        }
    }
}

impl serde::Serialize for NativeProducerSourcePage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("NativeProducerSourcePage", 6)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "view", &self.view)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "source", &self.source)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "producer", &self.producer)?;
        serde::ser::SerializeStruct::serialize_field(
            &mut state,
            "source_epoch",
            &self.source_epoch,
        )?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "rows", &self.rows)?;
        serde::ser::SerializeStruct::serialize_field(&mut state, "has_more", &self.has_more)?;
        serde::ser::SerializeStruct::end(state)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeProducerSourcePageWire {
    view: ViewRef,
    source: SourceCut,
    producer: ProducerId,
    source_epoch: Option<u64>,
    rows: NativeProducerSourceRows,
    has_more: bool,
}

impl<'de> serde::Deserialize<'de> for NativeProducerSourcePage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = <NativeProducerSourcePageWire as serde::Deserialize>::deserialize(deserializer)?;
        let page = Self {
            view: wire.view,
            source: wire.source,
            producer: wire.producer,
            source_epoch: wire.source_epoch,
            rows: wire.rows,
            has_more: wire.has_more,
        };
        page.validate().map_err(serde::de::Error::custom)?;
        Ok(page)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeIndexRequestEnvelope {
    pub request_id: u64,
    pub payload: NativeIndexRequest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeIndexResponseEnvelope {
    pub request_id: u64,
    pub payload: NativeIndexResponse,
}

/// One server-issued foreground publisher session. A new session receives a
/// fresh opaque identity; an expired or ended identity cannot be renewed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NativeInteractionPublisherId(pub [u8; 32]);

/// Explicit effort request on the native stream route. Auto is reserved for
/// a server that has an installed policy and live signal source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeEffortPreference {
    Fast,
    Deep,
    Auto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeEffortReason {
    ExplicitFast,
    ExplicitDeep,
    RequiredProgress,
    StableRefinement,
    Pressure,
    UnknownTelemetry,
    Dwell,
    NoOpportunity,
    ResourceDenied,
    TransitionCapped,
    ProviderUnavailable,
    AwaitRequired,
    AwaitOptional,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeEffortDone {
    pub corpus: Option<NativeOverlayCorpusStamp>,
    pub view: ViewRef,
    pub reason: NativeEffortReason,
    pub deep_delivered: bool,
}

/// Authenticated projection CAS stamp from one ticket-pinned view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeProjectionRead {
    pub view: ViewRef,
    pub binding: Option<ProjectionBinding>,
}

#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "The admitted native request retains bounded control metadata inline; boxing would add a separate allocation to the current decoder and caller custody contract"
)]
pub enum NativeIndexRequest {
    ReadSession(NativeReadSessionRequest),
    CreateWorkspace {
        workspace: WorkspaceKey,
        epoch: u64,
    },
    /// Opens or fences one durable operation-scoped ingress before any input write.
    BeginInput(OperationKey),
    /// A staged record belongs to the ingress operation and must be retried
    /// with that operation key after an uncertain ACK.
    StageSource {
        ingress: IngressRef,
        path: String,
        bytes: Vec<u8>,
        text: SourceTextAdmission,
    },
    /// Bounded child-first source upload; reply record refs preserve input order.
    StageSources {
        ingress: IngressRef,
        inputs: Vec<SourceInput>,
    },
    StageProfile {
        ingress: IngressRef,
        profile: IndexProfile,
    },
    /// Opaque immutable projection output and its sorted, unique direct children.
    StageProjectionOutput {
        ingress: IngressRef,
        bytes: Vec<u8>,
        children: NativeProjectionOutputChildren,
    },
    StageProjectionOwner {
        ingress: IngressRef,
        contribution: ProjectionOwnerContribution,
    },
    StageProjectionCertificate {
        ingress: IngressRef,
        certificate: ProjectionCertificate,
    },
    /// At most 128 ordered owner changes per request; `base` is the previous
    /// sealed page root, not a substitute for the original source CAS.
    StageMutationPage {
        ingress: IngressRef,
        base: MapRoot,
        mutations: Vec<OwnerMutation>,
    },
    SealMutations {
        ingress: IngressRef,
        root: MapRoot,
        count: u64,
    },
    DiscardInput(IngressRef),
    /// Exact case-sensitive byte literal at one acquired view. `after` is an
    /// exclusive owner cursor from the previous page.
    SearchLiteral {
        selection: ViewSelection,
        instance: IndexInstanceId,
        literal: Vec<u8>,
        after: Option<Vec<u8>>,
        limit: u32,
    },
    /// Exact pinned file inventory prefix page.
    QueryPaths {
        selection: ViewSelection,
        instance: IndexInstanceId,
        prefix: String,
        after: Option<String>,
        limit: u32,
    },
    /// Ranked native text search under one selected, ticket-pinned view.
    SearchText {
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: TextQueryRequest,
    },
    /// Atomic unsaved corpus over one selected durable member. A supplied stamp
    /// refuses a different live overlay corpus before execution.
    SearchOverlayText(NativeOverlayTextQuery),
    /// Ranked native semantic search under one selected, ticket-pinned view.
    SearchSemantic {
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: SemanticQueryRequest,
    },
    /// One admitted intent and one selected view; the server returns a Fast
    /// stage, optional Deep stage, then one `EffortDone` or Error terminal.
    QueryEffortStream {
        overlay: Option<NativeOverlayEffortContext>,
        selection: ViewSelection,
        text_instance: IndexInstanceId,
        semantic_instance: IndexInstanceId,
        text_query: TextQueryRequest,
        semantic_query: SemanticQueryRequest,
        preference: NativeEffortPreference,
    },
    /// Acquire one finite foreground publisher lease. This is not an activity
    /// event and does not make idle time known until a real event is reported.
    InteractionBegin {
        window_millis: u64,
    },
    InteractionActivity {
        publisher: NativeInteractionPublisherId,
        sequence: u64,
    },
    InteractionRenew {
        publisher: NativeInteractionPublisherId,
        sequence: u64,
    },
    InteractionEnd {
        publisher: NativeInteractionPublisherId,
        sequence: u64,
    },
    Submit(UpdateRequest),
    /// Resume the original admitted operation from its immutable journal entry.
    Resume(OperationKey),
    Overlay(NativeOverlayRequest),
    Stub(NativeStubRequest),
    BindOverlaySave(NativeOverlaySaveIntent),
    CompleteOverlaySave(NativeOverlaySaveReceipt),
    QueryGraph {
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: NativeGraphQuery,
    },
    /// Ranked certified `RepoMap` projection under one ticket-pinned view.
    QueryRepoMap {
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: NativeRepoMapQuery,
    },
    ReadView(ViewSelection),
    /// Read the exact producer stamp; does not expose object CRUD.
    ReadProjection {
        selection: ViewSelection,
        projection: ProjectionId,
    },
    /// Resolve exact source-record references under one ticket-pinned view.
    LookupSourceOwners {
        selection: ViewSelection,
        owners: NativeSourceOwnerKeys,
    },
    LookupProjectionOwners {
        selection: ViewSelection,
        projection: ProjectionId,
        owners: NativeSourceOwnerKeys,
    },
    /// Bounded producer prefix inventory under one exact current view.
    ListProducerSourceOwners {
        selection: ViewSelection,
        producer: ProducerId,
        after: Option<StableOwnerId>,
        limit: u32,
    },
    /// Diff the selected projection's retained `SourceCut` against this view.
    /// No caller-supplied root or full-scan fallback is accepted.
    DiffProjectionSourceOwners {
        selection: ViewSelection,
        projection: ProjectionId,
        producer: ProducerId,
        after: Option<StableOwnerId>,
        limit: u32,
    },
    Status(OperationKey),
    ReadyWork {
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
    },
    ClaimWork {
        intent: WorkIntent,
        lease_millis: u64,
    },
    RenewWork {
        claim: WorkClaim,
        lease_millis: u64,
    },
    WorkStatus(WorkIntent),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeIndexResponse {
    ReadSession(NativeReadSessionReply),
    Created(ViewRef),
    InputOpened(IngressRef),
    Staged(ObjectRef),
    StagedSources(Vec<ObjectRef>),
    MutationPageStaged(MapRoot),
    MutationsStaged(OwnerMutationSet),
    InputDiscarded(IngressRef),
    LiteralPage(NativeLiteralPage),
    PathPage(NativePathPage),
    TextRanked(NativeTextResult),
    OverlayTextRanked(NativeOverlayTextResult),
    SemanticRanked(NativeSemanticResult),
    OverlaySemanticRanked(NativeOverlaySemanticResult),
    EffortDone(NativeEffortDone),
    InteractionBegun(NativeInteractionPublisherId),
    InteractionAccepted(NativeInteractionPublisherId),
    Submitted(OperationReply<SubmitOutcome>),
    View(CatalogView),
    Projection(NativeProjectionRead),
    Overlay(NativeOverlayReply),
    Stub(NativeStubReply),
    Graph(NativeGraphResult),
    RepoMapRanked(NativeRepoMapResult),
    SourceOwners(NativeSourceOwnerLookup),
    ProjectionOwners(NativeProjectionOwnerLookup),
    ProducerSourceOwners(NativeProducerSourcePage),
    ProjectionSourceDiff(NativeProjectionSourceDiff),
    Status(OperationReply<OperationStatus>),
    ReadyWork(Vec<WorkIntent>),
    Claimed(WorkClaim),
    Renewed(WorkClaim),
    WorkStatus(WorkStatus),
    Error(NativeIndexWireError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIndexErrorCode {
    Denied,
    InvalidInput,
    Conflict,
    DigestConflict,
    FenceLost,
    ResourceLimited,
    Cancelled,
    SnapshotUnavailable,
    CorruptObject,
    BackendViolation,
    Storage,
    RebuildRequired,
    MaintenanceRequired,
    Overloaded,
    ResponseTooLarge,
    CapabilityUnavailable,
    FocusSubjectNotFound,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeIndexWireError {
    pub code: NativeIndexErrorCode,
    pub message: String,
}

impl std::fmt::Display for NativeIndexWireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for NativeIndexWireError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLiteralHit {
    pub owner: OwnerKey,
    pub record: SourceRecord,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLiteralPage {
    pub view: ViewRef,
    pub hits: Vec<NativeLiteralHit>,
    pub after: Option<Vec<u8>>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeFilePathEntry {
    pub owner: OwnerKey,
    pub path: String,
    pub source: ObjectRef,
    pub bytes: ObjectRef,
    pub byte_len: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePathPage {
    pub view: ViewRef,
    pub entries: Vec<NativeFilePathEntry>,
    pub has_more: bool,
}

/// The current native text member is synchronously required. This is query
/// coverage, distinct from async publication `CoverageStatus`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTextCoverage {
    CoreRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextHit {
    pub owner: OwnerKey,
    pub source: ObjectRef,
    pub path: String,
    pub score: FiniteScore,
}

/// A fully scanned, ranked native text response.
///
/// Hits are distinct owners, ordered by descending score (`total_cmp`) and
/// ascending owner for equal scores. `matched` counts the full verified match
/// set. The current native first path requires `exhaustive == true` and returns
/// exactly `min(matched, query.top_k, count)` hits when a bounded Native `count:N`
/// is present, and `min(matched, query.top_k)` otherwise; a partial mode needs a new explicit
/// contract before it can return fewer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTextResult {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub coverage: NativeTextCoverage,
    pub artifact: ObjectRef,
    pub matched: u64,
    pub exhaustive: bool,
    pub hits: Vec<NativeTextHit>,
}

impl NativeIndexResponse {
    /// Borrow source references while their retained allocation owner lives.
    /// Serialize a refusal without allocating another canonical response DTO.
    pub fn serialize_error_ref<S: serde::Serializer>(
        error: &NativeIndexWireError,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 9, "Error", error)
    }

    pub fn serialize_staged_sources_ref<S: serde::Serializer>(
        references: &[ObjectRef],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 16, "StagedSources", references)
    }

    /// Borrow a canonical literal page while its result allocation owner lives.
    pub fn serialize_literal_page_ref<S: serde::Serializer>(
        result: &NativeLiteralPage,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 14, "LiteralPage", result)
    }

    /// Borrow a canonical path page while its result allocation owner lives.
    pub fn serialize_path_page_ref<S: serde::Serializer>(
        result: &NativePathPage,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 15, "PathPage", result)
    }

    /// Borrow the canonical semantic result while its allocation owner lives.
    pub fn serialize_semantic_ranked_ref<S: serde::Serializer>(
        result: &NativeSemanticResult,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 18, "SemanticRanked", result)
    }

    /// Serialize the canonical `TextRanked` variant from a protected borrowed
    /// result. Resource custody stays in the runtime owner, outside the DTO.
    pub fn serialize_text_ranked_ref<S: serde::Serializer>(
        result: &NativeTextResult,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 17, "TextRanked", result)
    }

    /// Serialize a canonical lookup while runtime-owned retained memory lives
    /// through the synchronous wire emission.
    pub fn serialize_projection_source_diff_ref<S: serde::Serializer>(
        result: &NativeProjectionSourceDiff,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant(
            "NativeIndexResponse",
            32,
            "ProjectionSourceDiff",
            result,
        )
    }

    pub fn serialize_source_owners_ref<S: serde::Serializer>(
        result: &NativeSourceOwnerLookup,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 22, "SourceOwners", result)
    }

    pub fn serialize_projection_owners_ref<S: serde::Serializer>(
        result: &NativeProjectionOwnerLookup,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 31, "ProjectionOwners", result)
    }

    /// Borrow a producer-scoped recovery page through the synchronous wire
    /// emission while the runtime retains the page's allocation custody.
    pub fn serialize_producer_source_owners_ref<S: serde::Serializer>(
        result: &NativeProducerSourcePage,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant(
            "NativeIndexResponse",
            23,
            "ProducerSourceOwners",
            result,
        )
    }
}

impl NativeIndexResponseEnvelope {
    /// The canonical envelope accepts a borrowed serialization owner without
    /// copying its payload or adding allocation fields to the wire.
    pub fn serialize_payload_ref<S: serde::Serializer, P: serde::Serialize>(
        request_id: u64,
        payload: &P,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let mut envelope = serializer.serialize_struct("NativeIndexResponseEnvelope", 2)?;
        envelope.serialize_field("request_id", &request_id)?;
        envelope.serialize_field("payload", payload)?;
        envelope.end()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSemanticHit {
    pub owner: OwnerKey,
    pub vector_id: Vec<u8>,
    pub score: FiniteScore,
    pub chunk: Option<NativeSemanticChunk>,
}

/// Search traversal is independent of source coverage. Approximate traversal
/// cannot certify exhaustive recall even when no explicit cap was reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSemanticTraversal {
    ExactExhausted,
    ExactCapped,
    Approximate { capped: bool },
}

impl NativeSemanticTraversal {
    #[must_use]
    pub fn certifies_exhaustive(self, coverage: &CoverageBinding, skipped_stale: u32) -> bool {
        matches!(self, Self::ExactExhausted)
            && matches!(coverage.status, crate::index::CoverageStatus::Complete)
            && coverage.covered == Some(coverage.desired)
            && skipped_stale == 0
    }
}

quanta_index_contract_base::native_serde_enum!(NativeSemanticTraversal { units { ExactExhausted = 0, ExactCapped = 1 } newtypes { } records { Approximate = 2 { capped: bool } } });

/// Ranked semantic result bound to the selected view and its full async
/// coverage identity. Complete coverage may still have a capped scan.
///
/// Hits are ordered by descending score (`total_cmp`), then ascending owner,
/// then ascending vector ID. Multiple vectors may belong to one owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSemanticResult {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub coverage: CoverageBinding,
    pub traversal: NativeSemanticTraversal,
    pub exhaustive: bool,
    pub scanned_owners: u32,
    pub skipped_stale: u32,
    pub hits: Vec<NativeSemanticHit>,
}

// Closed manual codecs preserve the named JSON/CBOR representation.
quanta_index_contract_base::native_serde_struct!(NativeIndexRequestEnvelope {
    request_id: u64,
    payload: NativeIndexRequest
});
quanta_index_contract_base::native_serde_struct!(NativeIndexResponseEnvelope {
    request_id: u64,
    payload: NativeIndexResponse
});
quanta_index_contract_base::native_serde_newtype!(NativeInteractionPublisherId, [u8; 32]);
quanta_index_contract_base::native_serde_enum!(NativeEffortPreference { units { Fast = 0, Deep = 1, Auto = 2 } newtypes { } records { } });
quanta_index_contract_base::native_serde_enum!(NativeEffortReason { units { ExplicitFast = 0, ExplicitDeep = 1, RequiredProgress = 2, StableRefinement = 3, Pressure = 4, UnknownTelemetry = 5, Dwell = 6, NoOpportunity = 7, ResourceDenied = 8, TransitionCapped = 9, ProviderUnavailable = 10, AwaitRequired = 11, AwaitOptional = 12 } newtypes { } records { } });
quanta_index_contract_base::native_serde_struct!(NativeEffortDone {
    corpus: Option<NativeOverlayCorpusStamp>,
    view: ViewRef,
    reason: NativeEffortReason,
    deep_delivered: bool
});
quanta_index_contract_base::native_serde_enum!(NativeIndexRequest { units {  } newtypes { Submit = 6 => UpdateRequest, Resume = 7 => OperationKey, ReadView = 8 => ViewSelection, Status = 9 => OperationKey, WorkStatus = 13 => WorkIntent, BeginInput = 14 => OperationKey, DiscardInput = 17 => IngressRef, Overlay = 31 => NativeOverlayRequest, Stub = 33 => NativeStubRequest, BindOverlaySave = 34 => NativeOverlaySaveIntent, CompleteOverlaySave = 35 => NativeOverlaySaveReceipt, SearchOverlayText = 36 => NativeOverlayTextQuery, ReadSession = 40 => NativeReadSessionRequest } records { CreateWorkspace = 0 { workspace: WorkspaceKey, epoch: u64 }, StageSource = 1 { ingress: IngressRef, path: String, bytes: Vec<u8>, text: SourceTextAdmission }, StageProfile = 2 { ingress: IngressRef, profile: IndexProfile }, StageProjectionOutput = 3 { ingress: IngressRef, bytes: Vec<u8>, children: NativeProjectionOutputChildren }, StageProjectionOwner = 4 { ingress: IngressRef, contribution: ProjectionOwnerContribution }, StageProjectionCertificate = 5 { ingress: IngressRef, certificate: ProjectionCertificate }, ReadyWork = 10 { workspace: WorkspaceKey, epoch: u64, limit: u32 }, ClaimWork = 11 { intent: WorkIntent, lease_millis: u64 }, RenewWork = 12 { claim: WorkClaim, lease_millis: u64 }, StageMutationPage = 15 { ingress: IngressRef, base: MapRoot, mutations: Vec<OwnerMutation> }, SealMutations = 16 { ingress: IngressRef, root: MapRoot, count: u64 }, SearchLiteral = 18 { selection: ViewSelection, instance: IndexInstanceId, literal: Vec<u8>, after: Option<Vec<u8>>, limit: u32 }, QueryPaths = 19 { selection: ViewSelection, instance: IndexInstanceId, prefix: String, after: Option<String>, limit: u32 }, StageSources = 20 { ingress: IngressRef, inputs: Vec<SourceInput> }, SearchText = 21 { selection: ViewSelection, instance: IndexInstanceId, query: TextQueryRequest }, SearchSemantic = 22 { selection: ViewSelection, instance: IndexInstanceId, query: SemanticQueryRequest }, QueryEffortStream = 23 { overlay: Option<NativeOverlayEffortContext>, selection: ViewSelection, text_instance: IndexInstanceId, semantic_instance: IndexInstanceId, text_query: TextQueryRequest, semantic_query: SemanticQueryRequest, preference: NativeEffortPreference }, InteractionBegin = 24 { window_millis: u64 }, InteractionActivity = 25 { publisher: NativeInteractionPublisherId, sequence: u64 }, InteractionRenew = 26 { publisher: NativeInteractionPublisherId, sequence: u64 }, InteractionEnd = 27 { publisher: NativeInteractionPublisherId, sequence: u64 }, LookupSourceOwners = 28 { selection: ViewSelection, owners: NativeSourceOwnerKeys }, ListProducerSourceOwners = 29 { selection: ViewSelection, producer: ProducerId, after: Option<StableOwnerId>, limit: u32 }, ReadProjection = 30 { selection: ViewSelection, projection: ProjectionId }, LookupProjectionOwners = 38 { selection: ViewSelection, projection: ProjectionId, owners: NativeSourceOwnerKeys }, DiffProjectionSourceOwners = 39 { selection: ViewSelection, projection: ProjectionId, producer: ProducerId, after: Option<StableOwnerId>, limit: u32 }, QueryGraph = 32 { selection: ViewSelection, instance: IndexInstanceId, query: NativeGraphQuery }, QueryRepoMap = 37 { selection: ViewSelection, instance: IndexInstanceId, query: NativeRepoMapQuery } } });
quanta_index_contract_base::native_serde_enum!(NativeIndexResponse { units { } newtypes { Created = 0 => ViewRef, Staged = 1 => ObjectRef, Submitted = 2 => OperationReply<SubmitOutcome>, View = 3 => CatalogView, Status = 4 => OperationReply<OperationStatus>, ReadyWork = 5 => Vec<WorkIntent>, Claimed = 6 => WorkClaim, Renewed = 7 => WorkClaim, WorkStatus = 8 => WorkStatus, Error = 9 => NativeIndexWireError, InputOpened = 10 => IngressRef, MutationPageStaged = 11 => MapRoot, MutationsStaged = 12 => OwnerMutationSet, InputDiscarded = 13 => IngressRef, LiteralPage = 14 => NativeLiteralPage, PathPage = 15 => NativePathPage, StagedSources = 16 => Vec<ObjectRef>, TextRanked = 17 => NativeTextResult, SemanticRanked = 18 => NativeSemanticResult, EffortDone = 19 => NativeEffortDone, InteractionBegun = 20 => NativeInteractionPublisherId, InteractionAccepted = 21 => NativeInteractionPublisherId, SourceOwners = 22 => NativeSourceOwnerLookup, ProducerSourceOwners = 23 => NativeProducerSourcePage, Projection = 24 => NativeProjectionRead, Overlay = 25 => NativeOverlayReply, Graph = 26 => NativeGraphResult, Stub = 27 => NativeStubReply, OverlayTextRanked = 28 => NativeOverlayTextResult, OverlaySemanticRanked = 29 => NativeOverlaySemanticResult, RepoMapRanked = 30 => NativeRepoMapResult, ProjectionOwners = 31 => NativeProjectionOwnerLookup, ProjectionSourceDiff = 32 => NativeProjectionSourceDiff, ReadSession = 33 => NativeReadSessionReply } records {  } });
quanta_index_contract_base::native_serde_struct!(NativeProjectionRead { view: ViewRef, binding: Option<ProjectionBinding> });
quanta_index_contract_base::native_serde_struct!(NativeProjectionOwnerBinding { owner: OwnerKey, contribution: Option<ObjectRef> });
quanta_index_contract_base::native_serde_struct!(NativeProjectionOwnerLookup { view: ViewRef, projection: ProjectionId, binding: Option<ProjectionBinding>, rows: NativeProjectionOwnerRows });
quanta_index_contract_base::native_serde_struct!(NativeSourceOwnerBinding { owner: OwnerKey, source: Option<ObjectRef> });
quanta_index_contract_base::native_serde_struct!(NativeProducerSourceOwner {
    owner: OwnerKey,
    source: ObjectRef
});
quanta_index_contract_base::native_serde_struct!(NativeProducerSourcePageWire {
    view: ViewRef,
    source: SourceCut,
    producer: ProducerId,
    source_epoch: Option<u64>,
    rows: NativeProducerSourceRows,
    has_more: bool
});
quanta_index_contract_base::native_serde_struct!(NativeSourceOwnerLookup {
    view: ViewRef,
    source: SourceCut,
    rows: NativeSourceOwnerRows
});
quanta_index_contract_base::native_serde_enum!(NativeIndexErrorCode { units { Denied = 0, InvalidInput = 1, Conflict = 2, DigestConflict = 3, FenceLost = 4, ResourceLimited = 5, Cancelled = 6, SnapshotUnavailable = 7, CorruptObject = 8, BackendViolation = 9, Storage = 10, RebuildRequired = 11, MaintenanceRequired = 12, Overloaded = 13, ResponseTooLarge = 14, CapabilityUnavailable = 15, FocusSubjectNotFound = 16 } newtypes {  } records {  } });
quanta_index_contract_base::native_serde_struct!(NativeIndexWireError {
    code: NativeIndexErrorCode,
    message: String
});
quanta_index_contract_base::native_serde_struct!(NativeLiteralHit {
    owner: OwnerKey,
    record: SourceRecord
});
quanta_index_contract_base::native_serde_struct!(NativeLiteralPage { view: ViewRef, hits: Vec<NativeLiteralHit>, after: Option<Vec<u8>>, has_more: bool });
quanta_index_contract_base::native_serde_struct!(NativeFilePathEntry {
    owner: OwnerKey,
    path: String,
    source: ObjectRef,
    bytes: ObjectRef,
    byte_len: u64
});
quanta_index_contract_base::native_serde_struct!(NativePathPage { view: ViewRef, entries: Vec<NativeFilePathEntry>, has_more: bool });
quanta_index_contract_base::native_serde_enum!(NativeTextCoverage { units { CoreRequired = 0 } newtypes { } records { } });
quanta_index_contract_base::native_serde_struct!(NativeTextHit {
    owner: OwnerKey,
    source: ObjectRef,
    path: String,
    score: FiniteScore
});
quanta_index_contract_base::native_serde_struct!(NativeTextResult { view: ViewRef, instance: IndexInstanceId, coverage: NativeTextCoverage, artifact: ObjectRef, matched: u64, exhaustive: bool, hits: Vec<NativeTextHit> });
quanta_index_contract_base::native_serde_struct!(NativeSemanticHit { owner: OwnerKey, vector_id: Vec<u8>, score: FiniteScore, chunk: Option<NativeSemanticChunk> });
quanta_index_contract_base::native_serde_struct!(NativeSemanticResult { view: ViewRef, instance: IndexInstanceId, coverage: CoverageBinding, traversal: NativeSemanticTraversal, exhaustive: bool, scanned_owners: u32, skipped_stale: u32, hits: Vec<NativeSemanticHit> });

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FiniteScore, QueryConstraintSetV1, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
        index::{
            CollectionId, CoverageBinding, CoverageStatus, Digest, InputBinding, ProducerId,
            SchemaId, SlotId, StableOwnerId, StreamId, TenantId, WorktreeId,
        },
    };

    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(format!("condition failed: {}", stringify!($condition)).into());
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(format!(
                    "values differ: {} and {}",
                    stringify!($left),
                    stringify!($right)
                )
                .into());
            }
        }};
    }

    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left == right {
                return Err(format!(
                    "values unexpectedly equal: {} and {}",
                    stringify!($left),
                    stringify!($right)
                )
                .into());
            }
        }};
    }

    struct BorrowedStagedSources<'a>(&'a [ObjectRef]);

    impl serde::Serialize for BorrowedStagedSources<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            NativeIndexResponse::serialize_staged_sources_ref(self.0, serializer)
        }
    }

    #[test]
    fn borrowed_staged_source_references_preserve_json_and_cbor_wire()
    -> Result<(), Box<dyn std::error::Error>> {
        let references = vec![ObjectRef(Digest([4; 32])), ObjectRef(Digest([5; 32]))];
        let borrowed = BorrowedStagedSources(&references);
        let canonical = NativeIndexResponse::StagedSources(references.clone());
        let json = serde_json::to_vec(&borrowed)?;
        check_eq!(json, serde_json::to_vec(&canonical)?);
        check_eq!(
            serde_json::from_slice::<NativeIndexResponse>(&json)?,
            canonical
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&canonical, &mut cbor)?;
        let mut borrowed_cbor = Vec::new();
        ciborium::ser::into_writer(&borrowed, &mut borrowed_cbor)?;
        check_eq!(borrowed_cbor, cbor);
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(borrowed_cbor.as_slice())?,
            canonical
        );
        Ok(())
    }

    fn source_lookup_owner(stable: u8) -> OwnerKey {
        OwnerKey {
            producer: ProducerId([3; 16]),
            stable_id: StableOwnerId([stable; 16]),
        }
    }

    struct BorrowedSourceOwners<'a>(&'a NativeSourceOwnerLookup);

    impl serde::Serialize for BorrowedSourceOwners<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            NativeIndexResponse::serialize_source_owners_ref(self.0, serializer)
        }
    }

    struct BorrowedProjectionOwners<'a>(&'a NativeProjectionOwnerLookup);

    impl serde::Serialize for BorrowedProjectionOwners<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            NativeIndexResponse::serialize_projection_owners_ref(self.0, serializer)
        }
    }

    #[test]
    fn projection_owner_lookup_wire_is_bounded_and_keeps_contribution_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 7,
            sequence: 9,
            digest: Digest([8; 32]),
        };
        let projection = ProjectionId([5; 16]);
        let owner = source_lookup_owner(1);
        let contribution = ObjectRef(Digest([6; 32]));
        let request = NativeIndexRequest::LookupProjectionOwners {
            selection: ViewSelection::Exact(view),
            projection,
            owners: NativeSourceOwnerKeys::new(vec![owner])?,
        };
        let request_wire = serde_json::to_vec(&request)?;
        check_eq!(
            serde_json::from_slice::<NativeIndexRequest>(&request_wire)?,
            request
        );
        let lookup = NativeProjectionOwnerLookup {
            view,
            projection,
            binding: Some(ProjectionBinding {
                projection,
                producer: owner.producer,
                owners: MapRoot(Some(ObjectRef(Digest([10; 32])))),
                source: SourceCut {
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                    frontiers: MapRoot(None),
                },
                schema: crate::index::SchemaId([11; 16]),
                producer_policy: Digest([12; 32]),
                dependency_context: Digest([13; 32]),
                certificate: ObjectRef(Digest([14; 32])),
            }),
            rows: NativeProjectionOwnerRows::new(vec![NativeProjectionOwnerBinding {
                owner,
                contribution: Some(contribution),
            }])?,
        };
        let canonical = NativeIndexResponse::ProjectionOwners(lookup.clone());
        let json = serde_json::to_vec(&BorrowedProjectionOwners(&lookup))?;
        check_eq!(json, serde_json::to_vec(&canonical)?);
        check_eq!(
            serde_json::from_slice::<NativeIndexResponse>(&json)?,
            canonical
        );
        let mut borrowed_cbor = Vec::new();
        ciborium::ser::into_writer(&BorrowedProjectionOwners(&lookup), &mut borrowed_cbor)?;
        let mut canonical_cbor = Vec::new();
        ciborium::ser::into_writer(&canonical, &mut canonical_cbor)?;
        check_eq!(borrowed_cbor, canonical_cbor);
        check!(NativeProjectionOwnerRows::new(vec![]).is_err());
        check!(
            NativeProjectionOwnerRows::new(vec![
                NativeProjectionOwnerBinding {
                    owner,
                    contribution: None
                },
                NativeProjectionOwnerBinding {
                    owner,
                    contribution: None
                },
            ])
            .is_err()
        );
        check!(
            NativeProjectionOwnerRows::new(
                (0..=128)
                    .map(|id| NativeProjectionOwnerBinding {
                        owner: source_lookup_owner(id),
                        contribution: None,
                    })
                    .collect()
            )
            .is_err()
        );
        let invalid = serde_json::json!({"ProjectionOwners": {
            "view": view, "projection": projection, "binding": null,
            "rows": [{"owner": owner, "source": contribution}]
        }});
        check!(serde_json::from_value::<NativeIndexResponse>(invalid).is_err());
        Ok(())
    }

    struct BorrowedProducerSourceOwners<'a>(&'a NativeProducerSourcePage);

    impl serde::Serialize for BorrowedProducerSourceOwners<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            NativeIndexResponse::serialize_producer_source_owners_ref(self.0, serializer)
        }
    }

    #[test]
    fn source_owner_lookup_wire_is_bounded_ordered_and_roundtrips()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let view = ViewRef {
            workspace,
            epoch: 7,
            sequence: 9,
            digest: Digest([8; 32]),
        };
        let owners =
            NativeSourceOwnerKeys::new(vec![source_lookup_owner(1), source_lookup_owner(2)])?;
        let request = NativeIndexRequest::LookupSourceOwners {
            selection: ViewSelection::Exact(view),
            owners,
        };
        let json = serde_json::to_value(&request)?;
        check_eq!(serde_json::from_value::<NativeIndexRequest>(json)?, request);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&request, &mut cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeIndexRequest, _>(cbor.as_slice())?,
            request
        );

        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let lookup = NativeSourceOwnerLookup {
            view,
            source,
            rows: NativeSourceOwnerRows::new(vec![
                NativeSourceOwnerBinding {
                    owner: source_lookup_owner(1),
                    source: Some(ObjectRef(Digest([5; 32]))),
                },
                NativeSourceOwnerBinding {
                    owner: source_lookup_owner(2),
                    source: None,
                },
            ])?,
        };
        let response = NativeIndexResponse::SourceOwners(lookup);
        let json = serde_json::to_value(&response)?;
        let NativeIndexResponse::SourceOwners(borrowed) = &response else {
            return Err("source-owner response variant changed".into());
        };
        check_eq!(serde_json::to_value(BorrowedSourceOwners(borrowed))?, json);
        check_eq!(
            serde_json::from_value::<NativeIndexResponse>(json)?,
            response
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&response, &mut cbor)?;
        let mut borrowed_cbor = Vec::new();
        ciborium::ser::into_writer(&BorrowedSourceOwners(borrowed), &mut borrowed_cbor)?;
        check_eq!(borrowed_cbor, cbor);
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(cbor.as_slice())?,
            response
        );

        check!(NativeSourceOwnerKeys::new(Vec::new()).is_err());
        check!(
            NativeSourceOwnerKeys::new(vec![source_lookup_owner(1), source_lookup_owner(1)])
                .is_err()
        );
        check!(
            NativeSourceOwnerKeys::new(vec![source_lookup_owner(2), source_lookup_owner(1)])
                .is_err()
        );
        check!(NativeSourceOwnerKeys::new((0..=128).map(source_lookup_owner).collect()).is_err());
        let boundary = NativeSourceOwnerKeys::new((0..128).map(source_lookup_owner).collect())?;
        let mut boundary_cbor = Vec::new();
        ciborium::ser::into_writer(&boundary, &mut boundary_cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeSourceOwnerKeys, _>(boundary_cbor.as_slice())?,
            boundary
        );
        let mut malformed_extra = serde_json::to_string(boundary.as_slice())?;
        if malformed_extra.pop() != Some(']') {
            return Err("source-owner JSON sequence shape changed".into());
        }
        malformed_extra.push_str(", {\"nested\": [");
        let refusal = serde_json::from_str::<NativeSourceOwnerKeys>(&malformed_extra)
            .err()
            .ok_or("malformed 129th source owner was accepted")?;
        check!(refusal.to_string().contains("count exceeds 128"));
        for invalid in [
            Vec::new(),
            vec![source_lookup_owner(1), source_lookup_owner(1)],
            vec![source_lookup_owner(2), source_lookup_owner(1)],
            (0..=128).map(source_lookup_owner).collect(),
        ] {
            let json = serde_json::to_value(&invalid)?;
            check!(serde_json::from_value::<NativeSourceOwnerKeys>(json.clone()).is_err());
            let mut invalid_request = serde_json::to_value(&request)?;
            let fields = invalid_request
                .get_mut("LookupSourceOwners")
                .and_then(serde_json::Value::as_object_mut)
                .ok_or("missing source-owner lookup request fields")?;
            let _previous = fields.insert("owners".into(), json);
            check!(serde_json::from_value::<NativeIndexRequest>(invalid_request).is_err());
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&invalid, &mut cbor)?;
            check!(ciborium::de::from_reader::<NativeSourceOwnerKeys, _>(cbor.as_slice()).is_err());
        }
        // The declared size is refused before allocating or decoding an item.
        check!(
            ciborium::de::from_reader::<NativeSourceOwnerKeys, _>(
                [0x9a, 0x00, 0x00, 0x10, 0x00].as_slice()
            )
            .is_err()
        );
        for invalid in [
            Vec::new(),
            vec![
                NativeSourceOwnerBinding {
                    owner: source_lookup_owner(1),
                    source: None,
                },
                NativeSourceOwnerBinding {
                    owner: source_lookup_owner(1),
                    source: None,
                },
            ],
            vec![
                NativeSourceOwnerBinding {
                    owner: source_lookup_owner(2),
                    source: None,
                },
                NativeSourceOwnerBinding {
                    owner: source_lookup_owner(1),
                    source: None,
                },
            ],
            (0..=128)
                .map(|stable| NativeSourceOwnerBinding {
                    owner: source_lookup_owner(stable),
                    source: None,
                })
                .collect(),
        ] {
            check!(NativeSourceOwnerRows::new(invalid.clone()).is_err());
            let json = serde_json::to_value(&invalid)?;
            check!(serde_json::from_value::<NativeSourceOwnerRows>(json).is_err());
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&invalid, &mut cbor)?;
            check!(ciborium::de::from_reader::<NativeSourceOwnerRows, _>(cbor.as_slice()).is_err());
        }
        Ok(())
    }

    #[test]
    fn producer_source_inventory_wire_is_closed_bounded_and_nonnullable()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 7,
            sequence: 9,
            digest: Digest([8; 32]),
        };
        let producer = ProducerId([3; 16]);
        let request = NativeIndexRequest::ListProducerSourceOwners {
            selection: ViewSelection::Exact(view),
            producer,
            after: Some(StableOwnerId([1; 16])),
            limit: 128,
        };
        let request_json = serde_json::to_value(&request)?;
        check_eq!(
            serde_json::from_value::<NativeIndexRequest>(request_json.clone())?,
            request
        );
        let mut request_cbor = Vec::new();
        ciborium::ser::into_writer(&request, &mut request_cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeIndexRequest, _>(request_cbor.as_slice())?,
            request
        );
        let mut unknown_request = request_json;
        let fields = unknown_request
            .get_mut("ListProducerSourceOwners")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("missing producer-source request fields")?;
        let _previous = fields.insert("undeclared".into(), serde_json::json!(true));
        check!(serde_json::from_value::<NativeIndexRequest>(unknown_request).is_err());

        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let row = |stable| NativeProducerSourceOwner {
            owner: source_lookup_owner(stable),
            source: ObjectRef(Digest([stable; 32])),
        };
        let page = NativeProducerSourcePage {
            view,
            source,
            producer,
            source_epoch: Some(13),
            rows: NativeProducerSourceRows::new(vec![row(2), row(3)])?,
            has_more: true,
        };
        check_eq!(page.next_after(), Some(StableOwnerId([3; 16])));
        let response = NativeIndexResponse::ProducerSourceOwners(page.clone());
        let response_json = serde_json::to_value(&response)?;
        let NativeIndexResponse::ProducerSourceOwners(borrowed) = &response else {
            return Err("producer-source response variant changed".into());
        };
        check_eq!(
            serde_json::to_value(BorrowedProducerSourceOwners(borrowed))?,
            response_json
        );
        check_eq!(
            serde_json::from_value::<NativeIndexResponse>(response_json.clone())?,
            response
        );
        let mut response_cbor = Vec::new();
        ciborium::ser::into_writer(&response, &mut response_cbor)?;
        let mut borrowed_cbor = Vec::new();
        ciborium::ser::into_writer(&BorrowedProducerSourceOwners(borrowed), &mut borrowed_cbor)?;
        check_eq!(borrowed_cbor, response_cbor);
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(response_cbor.as_slice())?,
            response
        );

        let final_page = NativeProducerSourcePage {
            rows: NativeProducerSourceRows::new(Vec::new())?,
            has_more: false,
            ..page
        };
        check_eq!(final_page.next_after(), None);
        let final_response = NativeIndexResponse::ProducerSourceOwners(final_page);
        let final_json = serde_json::to_value(&final_response)?;
        check_eq!(
            serde_json::from_value::<NativeIndexResponse>(final_json.clone())?,
            final_response
        );
        let mut final_cbor = Vec::new();
        ciborium::ser::into_writer(&final_response, &mut final_cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(final_cbor.as_slice())?,
            final_response
        );

        check!(NativeProducerSourceRows::new(vec![row(1), row(1)]).is_err());
        check!(NativeProducerSourceRows::new(vec![row(2), row(1)]).is_err());
        check!(NativeProducerSourceRows::new((0..=128).map(row).collect()).is_err());
        let boundary = NativeProducerSourceRows::new((0..128).map(row).collect())?;
        let mut boundary_cbor = Vec::new();
        ciborium::ser::into_writer(&boundary, &mut boundary_cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeProducerSourceRows, _>(boundary_cbor.as_slice())?,
            boundary
        );
        let mut malformed_extra = serde_json::to_string(boundary.as_slice())?;
        if malformed_extra.pop() != Some(']') {
            return Err("producer-source JSON sequence shape changed".into());
        }
        malformed_extra.push_str(", {\"nested\": [");
        let refusal = serde_json::from_str::<NativeProducerSourceRows>(&malformed_extra)
            .err()
            .ok_or("malformed 129th producer source owner was accepted")?;
        check!(refusal.to_string().contains("count exceeds 128"));
        check!(
            ciborium::de::from_reader::<NativeProducerSourceRows, _>(
                [0x9a, 0x00, 0x00, 0x10, 0x00].as_slice()
            )
            .is_err()
        );
        for invalid_rows in [
            vec![row(1), row(1)],
            vec![row(2), row(1)],
            (0..=128).map(row).collect(),
        ] {
            let json = serde_json::to_value(&invalid_rows)?;
            check!(serde_json::from_value::<NativeProducerSourceRows>(json).is_err());
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&invalid_rows, &mut cbor)?;
            check!(
                ciborium::de::from_reader::<NativeProducerSourceRows, _>(cbor.as_slice()).is_err()
            );
        }
        for mut invalid in [response_json, final_json] {
            let fields = invalid
                .get_mut("ProducerSourceOwners")
                .and_then(serde_json::Value::as_object_mut)
                .ok_or("missing producer-source response fields")?;
            let _previous = fields.insert("has_more".into(), serde_json::json!(true));
            if fields
                .get("rows")
                .and_then(serde_json::Value::as_array)
                .is_some_and(Vec::is_empty)
            {
                check!(serde_json::from_value::<NativeIndexResponse>(invalid.clone()).is_err());
                let mut cbor = Vec::new();
                ciborium::ser::into_writer(&invalid, &mut cbor)?;
                check!(
                    ciborium::de::from_reader::<NativeIndexResponse, _>(cbor.as_slice()).is_err()
                );
            } else {
                let _previous = fields.insert("undeclared".into(), serde_json::json!(true));
                check!(serde_json::from_value::<NativeIndexResponse>(invalid).is_err());
            }
        }
        let mut nullable = serde_json::to_value(&response)?;
        *nullable
            .pointer_mut("/ProducerSourceOwners/rows/0/source")
            .ok_or("missing source reference")? = serde_json::Value::Null;
        check!(serde_json::from_value::<NativeIndexResponse>(nullable.clone()).is_err());
        let mut nullable_cbor = Vec::new();
        ciborium::ser::into_writer(&nullable, &mut nullable_cbor)?;
        check!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(nullable_cbor.as_slice()).is_err()
        );
        let mut missing_epoch = serde_json::to_value(&response)?;
        *missing_epoch
            .pointer_mut("/ProducerSourceOwners/source_epoch")
            .ok_or("missing source epoch")? = serde_json::Value::Null;
        check!(serde_json::from_value::<NativeIndexResponse>(missing_epoch.clone()).is_err());
        let mut missing_epoch_cbor = Vec::new();
        ciborium::ser::into_writer(&missing_epoch, &mut missing_epoch_cbor)?;
        check!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(missing_epoch_cbor.as_slice())
                .is_err()
        );
        let invalid_page = NativeProducerSourcePage {
            source_epoch: None,
            ..(*borrowed).clone()
        };
        check!(serde_json::to_value(invalid_page).is_err());
        let mut wrong_producer = serde_json::to_value(&response)?;
        *wrong_producer
            .pointer_mut("/ProducerSourceOwners/rows/0/owner/producer")
            .ok_or("missing producer")? = serde_json::to_value(ProducerId([4; 16]))?;
        check!(serde_json::from_value::<NativeIndexResponse>(wrong_producer).is_err());
        Ok(())
    }

    fn inject_nonfinite_score(value: &mut ciborium::value::Value) -> bool {
        if let ciborium::value::Value::Map(entries) = value {
            for (key, child) in entries {
                if matches!(key, ciborium::value::Value::Text(name) if name == "score") {
                    *child = ciborium::value::Value::Float(f64::NAN);
                    return true;
                }
                if inject_nonfinite_score(child) {
                    return true;
                }
            }
            false
        } else if let ciborium::value::Value::Array(items) = value {
            items.iter_mut().any(inject_nonfinite_score)
        } else {
            false
        }
    }

    #[test]
    fn native_interaction_session_wire_is_closed_and_roundtrips()
    -> Result<(), Box<dyn std::error::Error>> {
        let publisher = NativeInteractionPublisherId([7; 32]);
        let requests = [
            NativeIndexRequest::InteractionBegin {
                window_millis: 1_000,
            },
            NativeIndexRequest::InteractionActivity {
                publisher,
                sequence: 1,
            },
            NativeIndexRequest::InteractionRenew {
                publisher,
                sequence: 2,
            },
            NativeIndexRequest::InteractionEnd {
                publisher,
                sequence: 3,
            },
        ];
        for request in requests {
            let json = serde_json::to_value(&request)?;
            check_eq!(
                serde_json::from_value::<NativeIndexRequest>(json.clone())?,
                request
            );
            let mut missing = json.clone();
            let fields = missing
                .as_object_mut()
                .and_then(|variant| variant.values_mut().next())
                .and_then(serde_json::Value::as_object_mut)
                .ok_or("interaction request record missing")?;
            let field = fields
                .keys()
                .next()
                .cloned()
                .ok_or("interaction field missing")?;
            let _removed = fields
                .remove(&field)
                .ok_or("interaction field disappeared")?;
            check!(serde_json::from_value::<NativeIndexRequest>(missing).is_err());
            let mut unknown = json;
            let fields = unknown
                .as_object_mut()
                .and_then(|variant| variant.values_mut().next())
                .and_then(serde_json::Value::as_object_mut)
                .ok_or("interaction request record missing")?;
            let _previous = fields.insert("undeclared".into(), serde_json::json!(true));
            check!(serde_json::from_value::<NativeIndexRequest>(unknown).is_err());
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&request, &mut cbor)?;
            check_eq!(
                ciborium::de::from_reader::<NativeIndexRequest, _>(cbor.as_slice())?,
                request
            );
        }
        check_eq!(
            serde_json::to_value(NativeIndexRequest::InteractionBegin {
                window_millis: 1_000
            })?,
            serde_json::json!({"InteractionBegin": {"window_millis": 1000}})
        );
        let mut missing = serde_json::to_value(NativeIndexRequest::InteractionActivity {
            publisher,
            sequence: 1,
        })?;
        check!(
            missing
                .get_mut("InteractionActivity")
                .and_then(serde_json::Value::as_object_mut)
                .ok_or("missing activity record")?
                .remove("sequence")
                .is_some()
        );
        check!(serde_json::from_value::<NativeIndexRequest>(missing).is_err());
        let mut unknown = serde_json::to_value(NativeIndexRequest::InteractionRenew {
            publisher,
            sequence: 2,
        })?;
        let _previous = unknown
            .get_mut("InteractionRenew")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("missing renewal record")?
            .insert("undeclared".into(), serde_json::json!(true));
        check!(serde_json::from_value::<NativeIndexRequest>(unknown).is_err());
        check!(serde_json::from_str::<NativeIndexRequest>(r#"{"InteractionUnknown":{}}"#).is_err());
        let responses = [
            NativeIndexResponse::InteractionBegun(publisher),
            NativeIndexResponse::InteractionAccepted(publisher),
        ];
        for response in responses {
            let json = serde_json::to_value(&response)?;
            check_eq!(
                serde_json::from_value::<NativeIndexResponse>(json)?,
                response
            );
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&response, &mut cbor)?;
            check_eq!(
                ciborium::de::from_reader::<NativeIndexResponse, _>(cbor.as_slice())?,
                response
            );
        }
        check!(
            serde_json::from_str::<NativeIndexResponse>(r#"{"InteractionUnknown":{}}"#).is_err()
        );
        check!(serde_json::from_str::<NativeIndexResponse>(r#"{"InteractionBegun":[]}"#).is_err());
        check!(
            serde_json::from_str::<NativeIndexResponse>(r#"{"InteractionAccepted":[]}"#).is_err()
        );
        check!(serde_json::from_str::<NativeInteractionPublisherId>("[]").is_err());
        Ok(())
    }

    fn ingress() -> IngressRef {
        IngressRef {
            operation: OperationKey {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                producer: ProducerId([5; 16]),
                stream: StreamId([6; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            fence: 1,
        }
    }

    #[test]
    fn source_text_admission_is_required_and_roundtrips_on_both_wires()
    -> Result<(), Box<dyn std::error::Error>> {
        for text in [
            SourceTextAdmission::Excluded,
            SourceTextAdmission::Utf8 {
                policy: Digest([7; 32]),
            },
        ] {
            let request = NativeIndexRequest::StageSource {
                ingress: ingress(),
                path: "a.rs".into(),
                bytes: b"source".to_vec(),
                text,
            };
            let json = serde_json::to_value(&request)?;
            check_eq!(
                serde_json::from_value::<NativeIndexRequest>(json.clone())?,
                request
            );
            let mut cbor = Vec::new();
            ciborium::ser::into_writer(&request, &mut cbor)?;
            check_eq!(
                ciborium::de::from_reader::<NativeIndexRequest, _>(cbor.as_slice())?,
                request
            );
            let mut absent = json;
            check!(
                absent
                    .get_mut("StageSource")
                    .ok_or("missing StageSource variant")?
                    .as_object_mut()
                    .ok_or("missing request record")?
                    .remove("text")
                    .is_some()
            );
            check!(serde_json::from_value::<NativeIndexRequest>(absent).is_err());
            let input = SourceInput {
                path: "a.rs".into(),
                bytes: b"source".to_vec(),
                text,
            };
            let record = SourceRecord {
                path: "a.rs".into(),
                bytes: ObjectRef(Digest([8; 32])),
                byte_len: 6,
                text,
            };
            let mut input_json = serde_json::to_value(&input)?;
            check!(
                input_json
                    .as_object_mut()
                    .ok_or("missing input record")?
                    .remove("text")
                    .is_some()
            );
            check!(serde_json::from_value::<SourceInput>(input_json).is_err());
            let mut record_json = serde_json::to_value(&record)?;
            check!(
                record_json
                    .as_object_mut()
                    .ok_or("missing source record")?
                    .remove("text")
                    .is_some()
            );
            check!(serde_json::from_value::<SourceRecord>(record_json).is_err());
        }
        Ok(())
    }

    #[test]
    fn source_text_policy_rejects_unknown_and_incomplete_admission() {
        for invalid in [
            r#"{"Utf8":{}}"#,
            r#"{"Utf8":{"policy":[1]}}"#,
            r#""Auto""#,
            r#"{"Utf8":{"policy":null}}"#,
        ] {
            assert!(serde_json::from_str::<SourceTextAdmission>(invalid).is_err());
        }
    }

    #[test]
    fn borrowed_text_stage_preserves_canonical_json_and_cbor_envelope()
    -> Result<(), Box<dyn std::error::Error>> {
        struct BorrowedText<'a>(&'a NativeTextResult);
        impl serde::Serialize for BorrowedText<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                NativeIndexResponse::serialize_text_ranked_ref(self.0, serializer)
            }
        }
        struct BorrowedEnvelope<'a> {
            request_id: u64,
            text: &'a NativeTextResult,
        }
        impl serde::Serialize for BorrowedEnvelope<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                NativeIndexResponseEnvelope::serialize_payload_ref(
                    self.request_id,
                    &BorrowedText(self.text),
                    serializer,
                )
            }
        }
        let text = text_result();
        let canonical = NativeIndexResponseEnvelope {
            request_id: 91,
            payload: NativeIndexResponse::TextRanked(text.clone()),
        };
        let borrowed = BorrowedEnvelope {
            request_id: 91,
            text: &text,
        };
        let json = serde_json::to_vec(&borrowed)?;
        check_eq!(json, serde_json::to_vec(&canonical)?);
        let from_json: NativeIndexResponseEnvelope = serde_json::from_slice(&json)?;
        check_eq!(from_json, canonical);
        let mut borrowed_cbor = Vec::new();
        ciborium::ser::into_writer(&borrowed, &mut borrowed_cbor)?;
        let mut canonical_cbor = Vec::new();
        ciborium::ser::into_writer(&canonical, &mut canonical_cbor)?;
        check_eq!(borrowed_cbor, canonical_cbor);
        let from_cbor: NativeIndexResponseEnvelope =
            ciborium::de::from_reader(borrowed_cbor.as_slice())?;
        check_eq!(from_cbor, canonical);
        Ok(())
    }

    fn text_query() -> TextQueryRequest {
        TextQueryRequest {
            syntax: TextQuerySyntax::CodeSearch,
            query_text: "needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        }
    }

    fn text_result() -> NativeTextResult {
        NativeTextResult {
            view: ViewRef {
                workspace: ingress().operation.workspace,
                epoch: 1,
                sequence: 3,
                digest: Digest([8; 32]),
            },
            instance: IndexInstanceId([9; 16]),
            coverage: NativeTextCoverage::CoreRequired,
            artifact: ObjectRef(Digest([10; 32])),
            matched: 1,
            exhaustive: true,
            hits: vec![NativeTextHit {
                owner: OwnerKey {
                    producer: ProducerId([5; 16]),
                    stable_id: StableOwnerId([11; 16]),
                },
                source: ObjectRef(Digest([12; 32])),
                path: "src/lib.rs".into(),
                score: FiniteScore::new(1.25).expect("finite score"),
            }],
        }
    }

    #[test]
    fn native_text_request_reuses_canonical_query_and_has_closed_wire_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let selection = ViewSelection::Exact(text_result().view);
        let request = NativeIndexRequest::SearchText {
            selection,
            instance: IndexInstanceId([9; 16]),
            query: text_query(),
        };
        let json = serde_json::to_value(&request)?;
        check_eq!(
            json,
            serde_json::json!({
                "SearchText": {
                    "selection": serde_json::to_value(selection)?,
                    "instance": serde_json::to_value(IndexInstanceId([9; 16]))?,
                    "query": {
                        "syntax": "code_search",
                        "query_text": "needle",
                        "constraints": {"language_any_of": []},
                        "top_k": 1
                    }
                }
            })
        );
        check_eq!(
            serde_json::from_value::<NativeIndexRequest>(json.clone())?,
            request
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&request, &mut cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeIndexRequest, _>(cbor.as_slice())?,
            request
        );
        let mut missing_query = json.clone();
        check!(
            missing_query
                .get_mut("SearchText")
                .ok_or("missing SearchText variant")?
                .as_object_mut()
                .ok_or("request record")?
                .remove("query")
                .is_some()
        );
        check!(serde_json::from_value::<NativeIndexRequest>(missing_query).is_err());
        let mut unknown = json;
        let _previous = unknown
            .get_mut("SearchText")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("missing SearchText record")?
            .insert("undeclared".into(), serde_json::json!(true));
        check!(serde_json::from_value::<NativeIndexRequest>(unknown).is_err());
        Ok(())
    }

    #[test]
    fn native_text_result_roundtrips_and_refuses_unknown_coverage_or_score()
    -> Result<(), Box<dyn std::error::Error>> {
        let result = text_result();
        let response = NativeIndexResponse::TextRanked(result.clone());
        let json = serde_json::to_value(&response)?;
        check_eq!(
            json,
            serde_json::json!({
                "TextRanked": {
                    "view": serde_json::to_value(result.view)?,
                    "instance": serde_json::to_value(result.instance)?,
                    "coverage": "CoreRequired",
                    "artifact": serde_json::to_value(result.artifact)?,
                    "matched": 1,
                    "exhaustive": true,
                    "hits": [{
                        "owner": serde_json::to_value(result.hits.first().ok_or("missing text hit")?.owner)?,
                        "source": serde_json::to_value(result.hits.first().ok_or("missing text hit")?.source)?,
                        "path": "src/lib.rs",
                        "score": 1.25
                    }]
                }
            })
        );
        check_eq!(
            serde_json::from_value::<NativeIndexResponse>(json.clone())?,
            response
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&response, &mut cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(cbor.as_slice())?,
            response
        );
        let mut poisoned: ciborium::value::Value = ciborium::de::from_reader(cbor.as_slice())?;
        check!(inject_nonfinite_score(&mut poisoned));
        let mut poisoned_cbor = Vec::new();
        ciborium::ser::into_writer(&poisoned, &mut poisoned_cbor)?;
        check!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(poisoned_cbor.as_slice()).is_err()
        );
        let mut unknown_coverage = json.clone();
        *unknown_coverage
            .pointer_mut("/TextRanked/coverage")
            .ok_or("missing text coverage")? = serde_json::json!("Complete");
        check!(serde_json::from_value::<NativeIndexResponse>(unknown_coverage).is_err());
        let mut malformed_score = json.clone();
        *malformed_score
            .pointer_mut("/TextRanked/hits/0/score")
            .ok_or("missing text score")? = serde_json::json!("NaN");
        check!(serde_json::from_value::<NativeIndexResponse>(malformed_score).is_err());
        let mut missing_instance = json;
        check!(
            missing_instance
                .get_mut("TextRanked")
                .ok_or("missing TextRanked variant")?
                .as_object_mut()
                .ok_or("result record")?
                .remove("instance")
                .is_some()
        );
        check!(serde_json::from_value::<NativeIndexResponse>(missing_instance).is_err());
        check!(serde_json::from_str::<NativeIndexResponse>(r#"{"TextRankedUnknown":{}}"#).is_err());
        Ok(())
    }

    fn semantic_query() -> SemanticQueryRequest {
        SemanticQueryRequest {
            query_text: "dense needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            lexical_scope: None,
            top_k: 2,
        }
    }

    fn semantic_result() -> NativeSemanticResult {
        let instance = IndexInstanceId([9; 16]);
        let owner_root = MapRoot(Some(ObjectRef(Digest([16; 32]))));
        let desired = InputBinding {
            root: owner_root,
            schema: SchemaId([13; 16]),
            policy: Digest([14; 32]),
            context: Digest([15; 32]),
        };
        NativeSemanticResult {
            view: text_result().view,
            instance,
            coverage: CoverageBinding {
                instance,
                desired,
                covered: Some(desired),
                searchable: owner_root.0,
                status: CoverageStatus::Complete,
                owners: owner_root,
            },
            traversal: NativeSemanticTraversal::ExactExhausted,
            exhaustive: true,
            scanned_owners: 2,
            skipped_stale: 0,
            hits: vec![NativeSemanticHit {
                owner: OwnerKey {
                    producer: ProducerId([5; 16]),
                    stable_id: StableOwnerId([11; 16]),
                },
                vector_id: b"vector-1".to_vec(),
                score: FiniteScore::new(0.75).expect("finite cosine"),
                chunk: None,
            }],
        }
    }

    #[test]
    fn borrowed_literal_and_path_pages_preserve_canonical_json_and_cbor()
    -> Result<(), Box<dyn std::error::Error>> {
        struct BorrowedLiteral<'a>(&'a NativeLiteralPage);
        impl serde::Serialize for BorrowedLiteral<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                NativeIndexResponse::serialize_literal_page_ref(self.0, serializer)
            }
        }
        struct BorrowedPath<'a>(&'a NativePathPage);
        impl serde::Serialize for BorrowedPath<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                NativeIndexResponse::serialize_path_page_ref(self.0, serializer)
            }
        }
        let view = semantic_result().view;
        let owner = source_lookup_owner(1);
        let bytes = ObjectRef(Digest([2; 32]));
        let literal = NativeLiteralPage {
            view,
            hits: vec![NativeLiteralHit {
                owner,
                record: SourceRecord {
                    path: "a.rs".into(),
                    bytes,
                    byte_len: 9,
                    text: SourceTextAdmission::Excluded,
                },
            }],
            after: Some(vec![3; 32]),
            has_more: true,
        };
        let path = NativePathPage {
            view,
            entries: vec![NativeFilePathEntry {
                owner,
                path: "a.rs".into(),
                source: ObjectRef(Digest([4; 32])),
                bytes,
                byte_len: 9,
            }],
            has_more: false,
        };
        let canonical_literal = NativeIndexResponse::LiteralPage(literal.clone());
        let canonical_path = NativeIndexResponse::PathPage(path.clone());
        check_eq!(
            serde_json::to_vec(&BorrowedLiteral(&literal))?,
            serde_json::to_vec(&canonical_literal)?
        );
        check_eq!(
            serde_json::to_vec(&BorrowedPath(&path))?,
            serde_json::to_vec(&canonical_path)?
        );
        let mut encoded_literal = Vec::new();
        let mut encoded_path = Vec::new();
        let mut expected_literal = Vec::new();
        let mut expected_path = Vec::new();
        ciborium::ser::into_writer(&BorrowedLiteral(&literal), &mut encoded_literal)?;
        ciborium::ser::into_writer(&BorrowedPath(&path), &mut encoded_path)?;
        ciborium::ser::into_writer(&canonical_literal, &mut expected_literal)?;
        ciborium::ser::into_writer(&canonical_path, &mut expected_path)?;
        check_eq!(encoded_literal, expected_literal);
        check_eq!(encoded_path, expected_path);
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(encoded_literal.as_slice())?,
            canonical_literal
        );
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(encoded_path.as_slice())?,
            canonical_path
        );
        Ok(())
    }

    #[test]
    fn borrowed_semantic_stage_preserves_canonical_json_and_cbor()
    -> Result<(), Box<dyn std::error::Error>> {
        struct BorrowedSemantic<'a>(&'a NativeSemanticResult);
        impl serde::Serialize for BorrowedSemantic<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                NativeIndexResponse::serialize_semantic_ranked_ref(self.0, serializer)
            }
        }
        let result = semantic_result();
        let canonical = NativeIndexResponse::SemanticRanked(result.clone());
        let borrowed = BorrowedSemantic(&result);
        let json = serde_json::to_vec(&borrowed)?;
        check_eq!(json, serde_json::to_vec(&canonical)?);
        let value: serde_json::Value = serde_json::from_slice(&json)?;
        check!(value.get("SemanticRanked").is_some());
        check!(value.get("TextRanked").is_none());
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&borrowed, &mut cbor)?;
        let mut canonical_cbor = Vec::new();
        ciborium::ser::into_writer(&canonical, &mut canonical_cbor)?;
        check_eq!(cbor, canonical_cbor);
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(cbor.as_slice())?,
            canonical
        );
        Ok(())
    }

    #[test]
    fn native_semantic_request_reuses_canonical_ir_and_closed_distinct_wire_tag()
    -> Result<(), Box<dyn std::error::Error>> {
        let result = semantic_result();
        let selection = ViewSelection::Exact(result.view);
        let request = NativeIndexRequest::SearchSemantic {
            selection,
            instance: result.instance,
            query: semantic_query(),
        };
        let json = serde_json::to_value(&request)?;
        check_eq!(
            json,
            serde_json::json!({
                "SearchSemantic": {
                    "selection": serde_json::to_value(selection)?,
                    "instance": serde_json::to_value(result.instance)?,
                    "query": {
                        "query_text": "dense needle",
                        "constraints": {"language_any_of": []},
                        "top_k": 2
                    }
                }
            })
        );
        check_eq!(
            serde_json::from_value::<NativeIndexRequest>(json.clone())?,
            request
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&request, &mut cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeIndexRequest, _>(cbor.as_slice())?,
            request
        );
        let text = NativeIndexRequest::SearchText {
            selection,
            instance: result.instance,
            query: text_query(),
        };
        let mut text_cbor = Vec::new();
        ciborium::ser::into_writer(&text, &mut text_cbor)?;
        check_ne!(cbor, text_cbor);
        let mut absent = json.clone();
        check!(
            absent
                .get_mut("SearchSemantic")
                .ok_or("missing SearchSemantic variant")?
                .as_object_mut()
                .ok_or("semantic request")?
                .remove("query")
                .is_some()
        );
        check!(serde_json::from_value::<NativeIndexRequest>(absent).is_err());
        let mut unknown = json;
        let _previous = unknown
            .get_mut("SearchSemantic")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("missing SearchSemantic record")?
            .insert("undeclared".into(), serde_json::json!(true));
        check!(serde_json::from_value::<NativeIndexRequest>(unknown).is_err());
        check!(
            serde_json::from_str::<NativeIndexRequest>(r#"{"SearchSemanticUnknown":{}}"#).is_err()
        );
        Ok(())
    }

    #[test]
    fn semantic_traversal_has_fixed_wire_and_independent_coverage_truth_table()
    -> Result<(), Box<dyn std::error::Error>> {
        use NativeSemanticTraversal::{Approximate, ExactCapped, ExactExhausted};
        let traversals = [
            ExactExhausted,
            ExactCapped,
            Approximate { capped: false },
            Approximate { capped: true },
        ];
        let fixed_json = [
            serde_json::json!("ExactExhausted"),
            serde_json::json!("ExactCapped"),
            serde_json::json!({"Approximate": {"capped": false}}),
            serde_json::json!({"Approximate": {"capped": true}}),
        ];
        for (traversal, gold) in traversals.into_iter().zip(fixed_json) {
            check_eq!(serde_json::to_value(traversal)?, gold);
            check_eq!(
                serde_json::from_value::<NativeSemanticTraversal>(gold)?,
                traversal
            );
            let mut bytes = Vec::new();
            ciborium::ser::into_writer(&traversal, &mut bytes)?;
            check_eq!(
                ciborium::de::from_reader::<NativeSemanticTraversal, _>(bytes.as_slice())?,
                traversal
            );
        }
        let complete = semantic_result().coverage;
        let mut partial = complete;
        partial.status = CoverageStatus::Partial {
            missing: MapRoot(None),
        };
        partial.covered = None;
        let mut pending = partial;
        pending.status = CoverageStatus::Pending {
            previous_missing: MapRoot(None),
        };
        let mut inconsistent = complete;
        inconsistent.covered = None;
        for (coverage, stale, gold) in [
            (complete, 0, [true, false, false, false]),
            (complete, 1, [false, false, false, false]),
            (partial, 0, [false, false, false, false]),
            (pending, 0, [false, false, false, false]),
            (inconsistent, 0, [false, false, false, false]),
        ] {
            for (traversal, expected) in traversals.into_iter().zip(gold) {
                check_eq!(traversal.certifies_exhaustive(&coverage, stale), expected);
            }
        }
        let mut missing = serde_json::to_value(semantic_result())?;
        check!(
            missing
                .as_object_mut()
                .ok_or("semantic record")?
                .remove("traversal")
                .is_some()
        );
        check!(serde_json::from_value::<NativeSemanticResult>(missing).is_err());
        check!(serde_json::from_str::<NativeSemanticTraversal>(r#"{"Approximate":{}}"#).is_err());
        check!(serde_json::from_str::<NativeSemanticTraversal>(r#""Unknown""#).is_err());
        Ok(())
    }

    #[test]
    fn native_semantic_result_roundtrips_full_coverage_and_rejects_bad_wire()
    -> Result<(), Box<dyn std::error::Error>> {
        let result = semantic_result();
        let response = NativeIndexResponse::SemanticRanked(result.clone());
        let json = serde_json::to_value(&response)?;
        check_eq!(
            json,
            serde_json::json!({
                "SemanticRanked": {
                    "view": serde_json::to_value(result.view)?,
                    "instance": serde_json::to_value(result.instance)?,
                    "coverage": {
                        "instance": serde_json::to_value(result.instance)?,
                        "desired": serde_json::to_value(result.coverage.desired)?,
                        "covered": serde_json::to_value(result.coverage.covered)?,
                        "searchable": serde_json::to_value(result.coverage.searchable)?,
                        "status": "Complete",
                        "owners": serde_json::to_value(result.coverage.owners)?
                    },
                    "exhaustive": true,
                    "traversal": "ExactExhausted",
                    "scanned_owners": 2,
                    "skipped_stale": 0,
                    "hits": [{
                        "owner": serde_json::to_value(result.hits.first().ok_or("missing semantic hit")?.owner)?,
                        "vector_id": b"vector-1".to_vec(),
                        "score": 0.75,
                        "chunk": null
                    }]
                }
            })
        );
        check_eq!(
            serde_json::from_value::<NativeIndexResponse>(json.clone())?,
            response
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&response, &mut cbor)?;
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(cbor.as_slice())?,
            response
        );
        let mut unknown = json.clone();
        let _previous = unknown
            .get_mut("SemanticRanked")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("missing SemanticRanked record")?
            .insert("undeclared".into(), serde_json::json!(true));
        check!(serde_json::from_value::<NativeIndexResponse>(unknown).is_err());
        let mut missing_coverage = json.clone();
        check!(
            missing_coverage
                .get_mut("SemanticRanked")
                .ok_or("missing SemanticRanked variant")?
                .as_object_mut()
                .ok_or("semantic result")?
                .remove("coverage")
                .is_some()
        );
        check!(serde_json::from_value::<NativeIndexResponse>(missing_coverage).is_err());
        let mut malformed_score = json.clone();
        *malformed_score
            .pointer_mut("/SemanticRanked/hits/0/score")
            .ok_or("missing semantic score")? = serde_json::json!("NaN");
        check!(serde_json::from_value::<NativeIndexResponse>(malformed_score).is_err());
        let mut unknown_status = json;
        *unknown_status
            .pointer_mut("/SemanticRanked/coverage/status")
            .ok_or("missing semantic coverage status")? = serde_json::json!("Invented");
        check!(serde_json::from_value::<NativeIndexResponse>(unknown_status).is_err());
        check!(
            serde_json::from_str::<NativeIndexResponse>(r#"{"SemanticRankedUnknown":{}}"#).is_err()
        );
        Ok(())
    }

    #[test]
    fn effort_stream_tags_roundtrip_and_refuse_unknown_or_incomplete_records()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = text_result().view;
        let request = NativeIndexRequest::QueryEffortStream {
            overlay: None,
            selection: ViewSelection::Exact(view),
            text_instance: text_result().instance,
            semantic_instance: semantic_result().instance,
            text_query: text_query(),
            semantic_query: semantic_query(),
            preference: NativeEffortPreference::Deep,
        };
        let request_json = serde_json::to_value(&request)?;
        check_eq!(
            request_json
                .pointer("/QueryEffortStream/preference")
                .ok_or("missing effort preference")?,
            &serde_json::json!("Deep")
        );
        check_eq!(
            request_json.as_object().ok_or("request enum object")?.len(),
            1
        );
        check_eq!(
            serde_json::from_value::<NativeIndexRequest>(request_json.clone())?,
            request
        );
        let mut request_cbor = Vec::new();
        ciborium::ser::into_writer(&request, &mut request_cbor)?;
        let request_value: ciborium::value::Value =
            ciborium::de::from_reader(request_cbor.as_slice())?;
        check!(
            matches!(request_value, ciborium::value::Value::Map(ref entries)
            if entries.len() == 1
                && matches!(entries.first(), Some((ciborium::value::Value::Text(name), _)) if name == "QueryEffortStream"))
        );
        check_eq!(
            ciborium::de::from_reader::<NativeIndexRequest, _>(request_cbor.as_slice())?,
            request
        );
        let mut missing = request_json.clone();
        check!(
            missing
                .get_mut("QueryEffortStream")
                .ok_or("missing effort request variant")?
                .as_object_mut()
                .ok_or("request record")?
                .remove("semantic_query")
                .is_some()
        );
        check!(serde_json::from_value::<NativeIndexRequest>(missing).is_err());
        let mut unknown = request_json;
        *unknown
            .pointer_mut("/QueryEffortStream/preference")
            .ok_or("missing effort preference")? = serde_json::json!("Speculative");
        check!(serde_json::from_value::<NativeIndexRequest>(unknown).is_err());

        let response = NativeIndexResponse::EffortDone(NativeEffortDone {
            view,
            reason: NativeEffortReason::ExplicitDeep,
            deep_delivered: true,
            corpus: None,
        });
        let response_json = serde_json::to_value(&response)?;
        check_eq!(
            response_json
                .pointer("/EffortDone/reason")
                .ok_or("missing effort reason")?,
            &serde_json::json!("ExplicitDeep")
        );
        check_eq!(
            response_json
                .pointer("/EffortDone/deep_delivered")
                .ok_or("missing effort delivery flag")?,
            &serde_json::json!(true)
        );
        check_eq!(
            serde_json::from_value::<NativeIndexResponse>(response_json.clone())?,
            response
        );
        let mut response_cbor = Vec::new();
        ciborium::ser::into_writer(&response, &mut response_cbor)?;
        let response_value: ciborium::value::Value =
            ciborium::de::from_reader(response_cbor.as_slice())?;
        check!(
            matches!(response_value, ciborium::value::Value::Map(ref entries)
            if entries.len() == 1
                && matches!(entries.first(), Some((ciborium::value::Value::Text(name), _)) if name == "EffortDone"))
        );
        check_eq!(
            ciborium::de::from_reader::<NativeIndexResponse, _>(response_cbor.as_slice())?,
            response
        );
        let mut incomplete = response_json.clone();
        check!(
            incomplete
                .get_mut("EffortDone")
                .ok_or("missing effort result variant")?
                .as_object_mut()
                .ok_or("response record")?
                .remove("deep_delivered")
                .is_some()
        );
        check!(serde_json::from_value::<NativeIndexResponse>(incomplete).is_err());
        let mut bad_reason = response_json;
        *bad_reason
            .pointer_mut("/EffortDone/reason")
            .ok_or("missing effort reason")? = serde_json::json!("Invented");
        check!(serde_json::from_value::<NativeIndexResponse>(bad_reason).is_err());
        check_eq!(
            serde_json::to_value(NativeIndexErrorCode::CapabilityUnavailable)?,
            "CapabilityUnavailable"
        );
        check!(serde_json::from_str::<NativeIndexErrorCode>("\"Invented\"").is_err());
        Ok(())
    }
}
