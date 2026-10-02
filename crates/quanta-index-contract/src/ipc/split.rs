use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    ActiveGenerationResolutionV1, ClusterMembershipBatchReadRequestV1,
    ClusterMembershipBatchReadResponseV1, CurrentGenerationRequest, GenerationPin,
    GenerationSnapshot, GenerationStatusReport, GenerationStatusRequest, HistoryQueryRequest,
    HybridQueryRequest, HybridQueryResponse, HybridSeedQueryRequest, HybridSeedQueryResponse,
    MetricsSnapshotRequest, MetricsSnapshotV1, ProcessReadinessRequest, ProcessReadinessV1,
    ProcessRequestEventsRequestV1, ProcessRequestEventsV1, QuarantineDiscardAck,
    QuarantineDiscardRequest, QuarantineInventoryRequest, QuarantineInventoryV1,
    RepoMapActivateGenerationRequestV2, RepoMapActiveHeadRequestV2, RepoMapActiveHeadResponseV2,
    RepoMapQueryRequest, RepoMapQueryResponse, RepoMapTerminalReceiptV2,
    RuntimeMetadataQueryRequest, SearchCorpusActiveHeadObservationV1,
    SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse, SearchPlaneIpcError,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneRuntimeMetadataQueryResponse,
    SearchPlaneSearchCorpusActivationCasAck, SearchPlaneSearchCorpusRollbackCasAck,
    SearchPlaneStructuralQueryResponse, SemanticQueryRequest, SemanticQueryResponse,
    StructuralQueryRequest, SymbolQueryRequest, SymbolQueryResponse, TextQueryRequest,
    TextQueryResponse,
};

const SEARCH_PLANE_ENVELOPE_FIELDS: &[&str] = &["request_id", "payload"];
const SEARCH_PLANE_ADJACENT_TAG_FIELDS: &[&str] = &["kind", "payload"];
const SEARCH_PLANE_QUERY_IPC_REQUEST_VARIANTS: &[&str] = &[
    "ResolveActiveGeneration",
    "ResolveLexicalGeneration",
    "Text",
    "Symbol",
    "Semantic",
    "SemanticWorkBoundedV1",
    "Hybrid",
    "HybridSeed",
    "History",
    "RuntimeMetadata",
    "Structural",
    "RepoMapQuery",
    "Explain",
    "ClusterMembershipRead",
];
const SEARCH_PLANE_QUERY_IPC_RESPONSE_VARIANTS: &[&str] = &[
    "ActiveGenerationSnapshot",
    "ResolvedLexicalGeneration",
    "Text",
    "Symbol",
    "Semantic",
    "SemanticWorkBoundedV1",
    "Hybrid",
    "HybridSeed",
    "History",
    "RuntimeMetadata",
    "Structural",
    "RepoMapQuery",
    "Explain",
    "ClusterMembershipRead",
    "Error",
];
const SEARCH_PLANE_CONTROL_IPC_REQUEST_VARIANTS: &[&str] = &[
    "ActivateSearchCorpusGenerationCas",
    "RollbackSearchCorpusGenerationCas",
    "RepoMapActivateV2",
    "RepoMapActiveHeadV2",
    "CurrentGeneration",
    "GenerationStatus",
    "SearchCorpusActiveHead",
    "MetricsSnapshot",
    "QuarantineInventory",
    "QuarantineDiscard",
    "ProcessReadiness",
    "ProcessRequestEventsV1",
];
const SEARCH_PLANE_CONTROL_IPC_RESPONSE_VARIANTS: &[&str] = &[
    "SearchCorpusActivationCasAck",
    "SearchCorpusRollbackCasAck",
    "RepoMapTerminalReceiptV2",
    "RepoMapActiveHeadV2",
    "Error",
    "CurrentGenerationSnapshot",
    "GenerationStatusReport",
    "SearchCorpusActiveHeadObservation",
    "MetricsSnapshot",
    "QuarantineInventory",
    "QuarantineDiscardAck",
    "ProcessReadinessReport",
    "ProcessRequestEventsV1",
];

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneQueryIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneQueryIpcRequest,
}

/// Dedicated work-bounded semantic request.
///
/// A V1 semantic request remains
/// unchanged; this variant requires an exact generation and a finite work
/// allowance, and never falls back to the ordinary semantic route. Units cover
/// query input bytes, the sealed exact-scan vector-component upper bound, and
/// result rows. They do not represent all embedding-provider or Arrow CPU.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticWorkBoundedQueryRequestV1 {
    pub query: SemanticQueryRequest,
    pub max_work_units: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticWorkBoundedQueryResponseV1 {
    pub query: SemanticQueryResponse,
    pub charged_work_units: u64,
}

const SEMANTIC_WORK_REQUEST_FIELDS: &[&str] = &["query", "max_work_units"];
const SEMANTIC_WORK_RESPONSE_FIELDS: &[&str] = &["query", "charged_work_units"];

fn deserialize_semantic_work<'de, A, Query>(
    mut map: A,
    work_field: &'static str,
    fields: &'static [&'static str],
) -> Result<(Query, u64), A::Error>
where
    A: MapAccess<'de>,
    Query: Deserialize<'de>,
{
    let mut query = None;
    let mut work_units = None;
    while let Some(key) = map.next_key::<String>()? {
        if key == "query" {
            if query.is_some() {
                return Err(de::Error::duplicate_field("query"));
            }
            query = Some(map.next_value()?);
        } else if key == work_field {
            if work_units.is_some() {
                return Err(de::Error::duplicate_field(work_field));
            }
            work_units = Some(map.next_value()?);
        } else {
            return Err(de::Error::unknown_field(&key, fields));
        }
    }
    Ok((
        query.ok_or_else(|| de::Error::missing_field("query"))?,
        work_units.ok_or_else(|| de::Error::missing_field(work_field))?,
    ))
}

impl Serialize for SemanticWorkBoundedQueryRequestV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("SemanticWorkBoundedQueryRequestV1", 2)?;
        state.serialize_field("query", &self.query)?;
        state.serialize_field("max_work_units", &self.max_work_units)?;
        state.end()
    }
}

struct SemanticWorkBoundedQueryRequestVisitorV1;

impl<'de> Visitor<'de> for SemanticWorkBoundedQueryRequestVisitorV1 {
    type Value = SemanticWorkBoundedQueryRequestV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticWorkBoundedQueryRequestV1 map")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let (query, max_work_units) =
            deserialize_semantic_work(map, "max_work_units", SEMANTIC_WORK_REQUEST_FIELDS)?;
        Ok(SemanticWorkBoundedQueryRequestV1 {
            query,
            max_work_units,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticWorkBoundedQueryRequestV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "SemanticWorkBoundedQueryRequestV1",
            SEMANTIC_WORK_REQUEST_FIELDS,
            SemanticWorkBoundedQueryRequestVisitorV1,
        )
    }
}

impl Serialize for SemanticWorkBoundedQueryResponseV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("SemanticWorkBoundedQueryResponseV1", 2)?;
        state.serialize_field("query", &self.query)?;
        state.serialize_field("charged_work_units", &self.charged_work_units)?;
        state.end()
    }
}

struct SemanticWorkBoundedQueryResponseVisitorV1;

impl<'de> Visitor<'de> for SemanticWorkBoundedQueryResponseVisitorV1 {
    type Value = SemanticWorkBoundedQueryResponseV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticWorkBoundedQueryResponseV1 map")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let (query, charged_work_units) =
            deserialize_semantic_work(map, "charged_work_units", SEMANTIC_WORK_RESPONSE_FIELDS)?;
        Ok(SemanticWorkBoundedQueryResponseV1 {
            query,
            charged_work_units,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticWorkBoundedQueryResponseV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "SemanticWorkBoundedQueryResponseV1",
            SEMANTIC_WORK_RESPONSE_FIELDS,
            SemanticWorkBoundedQueryResponseVisitorV1,
        )
    }
}

/// Service admission cap for deterministic semantic query work units.
pub const SEMANTIC_WORK_OPERATIONAL_CAP_V1: u64 = 10_000_000;

/// Maximum uncompressed IPC body, including semantic query responses.
/// Clients reserve their response materialization before opening this frame.
pub const MAX_IPC_FRAME_BODY_BYTES_V1: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "wire payloads retain the existing by-value API; boxing adds allocation and changes construction across unrelated query routes"
)]
pub enum SearchPlaneQueryIpcRequest {
    ResolveActiveGeneration(CurrentGenerationRequest),
    ResolveLexicalGeneration(TextQueryRequest),
    Text(TextQueryRequest),
    Symbol(SymbolQueryRequest),
    Semantic(SemanticQueryRequest),
    SemanticWorkBoundedV1(SemanticWorkBoundedQueryRequestV1),
    Hybrid(HybridQueryRequest),
    HybridSeed(HybridSeedQueryRequest),
    History(HistoryQueryRequest),
    RuntimeMetadata(RuntimeMetadataQueryRequest),
    Structural(StructuralQueryRequest),
    RepoMapQuery(RepoMapQueryRequest),
    Explain(SearchPlaneExplainQueryRequest),
    ClusterMembershipRead(ClusterMembershipBatchReadRequestV1),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneQueryIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneQueryIpcResponse,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneQueryIpcResponse {
    ActiveGenerationSnapshot(ActiveGenerationResolutionV1),
    ResolvedLexicalGeneration(GenerationPin),
    Text(TextQueryResponse),
    Symbol(SymbolQueryResponse),
    Semantic(SemanticQueryResponse),
    SemanticWorkBoundedV1(SemanticWorkBoundedQueryResponseV1),
    Hybrid(HybridQueryResponse),
    HybridSeed(HybridSeedQueryResponse),
    History(SearchPlaneHistoryQueryResponse),
    RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse),
    Structural(SearchPlaneStructuralQueryResponse),
    RepoMapQuery(RepoMapQueryResponse),
    Explain(SearchPlaneExplainQueryResponse),
    ClusterMembershipRead(ClusterMembershipBatchReadResponseV1),
    Error(SearchPlaneIpcError),
}

impl SearchPlaneQueryIpcResponse {
    /// Stamp the transport request id onto the carried explanation (S21-10),
    /// so a typed payload correlates without its envelope. Only the five
    /// explanation-carrying variants (`Semantic`, `SemanticWorkBoundedV1`,
    /// `Hybrid`, `HybridSeed`, `Explain`) change; every other variant has no explanation to stamp
    /// and is left untouched.
    pub fn stamp_request_id(&mut self, request_id: u64) {
        let explanation = match self {
            SearchPlaneQueryIpcResponse::Semantic(response) => Some(&mut response.explanation),
            SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(response) => {
                Some(&mut response.query.explanation)
            }
            SearchPlaneQueryIpcResponse::Hybrid(response) => Some(&mut response.explanation),
            SearchPlaneQueryIpcResponse::HybridSeed(response) => Some(&mut response.explanation),
            SearchPlaneQueryIpcResponse::Explain(response) => Some(&mut response.explanation),
            SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
            | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
            | SearchPlaneQueryIpcResponse::Error(_) => None,
        };
        if let Some(explanation) = explanation {
            explanation.request_id = request_id;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneControlIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneControlIpcRequest,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneControlIpcRequest {
    /// Atomic activation of the complete lexical + semantic corpus identity.
    ActivateSearchCorpusGenerationCas(SearchPlaneActivateSearchCorpusGenerationCasRequest),
    /// Explicit composite rollback CAS; normal activation remains monotonic.
    RollbackSearchCorpusGenerationCas(SearchPlaneRollbackSearchCorpusGenerationCasRequest),
    RepoMapActivateV2(RepoMapActivateGenerationRequestV2),
    RepoMapActiveHeadV2(RepoMapActiveHeadRequestV2),
    /// QI-ACT-01: read-only generation admin query for one
    /// `(repo, revision, track)` triple.
    CurrentGeneration(CurrentGenerationRequest),
    /// QI-ACT-01: read-only status query returning all activated tracks for
    /// one `(repo, revision)` pair.
    GenerationStatus(GenerationStatusRequest),
    /// Read the catalog-owned optional composite head for producer CAS.
    SearchCorpusActiveHead(GenerationStatusRequest),
    /// QI-BB-015: read-only scrape of every metric the daemon aggregates.
    MetricsSnapshot(MetricsSnapshotRequest),
    /// QI-BB-026: what the adapters quarantine right now.
    QuarantineInventory(QuarantineInventoryRequest),
    /// QI-BB-026: remove one quarantined entry exactly as it was listed.
    QuarantineDiscard(QuarantineDiscardRequest),
    /// S21-10: process-wide readiness synthesis (supervisor, required
    /// planes, maintenance, backend, provider, candidate integrity).
    /// Deliberately distinct from repository generation status.
    ProcessReadiness(ProcessReadinessRequest),
    /// Operator-only, bounded projection of the existing transport event ring.
    ProcessRequestEventsV1(ProcessRequestEventsRequestV1),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneControlIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneControlIpcResponse,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneControlIpcResponse {
    SearchCorpusActivationCasAck(SearchPlaneSearchCorpusActivationCasAck),
    SearchCorpusRollbackCasAck(SearchPlaneSearchCorpusRollbackCasAck),
    RepoMapTerminalReceiptV2(RepoMapTerminalReceiptV2),
    RepoMapActiveHeadV2(RepoMapActiveHeadResponseV2),
    Error(SearchPlaneIpcError),
    /// QI-ACT-01: response to [`SearchPlaneControlIpcRequest::CurrentGeneration`].
    CurrentGenerationSnapshot(GenerationSnapshot),
    /// QI-ACT-01: response to [`SearchPlaneControlIpcRequest::GenerationStatus`].
    GenerationStatusReport(GenerationStatusReport),
    SearchCorpusActiveHeadObservation(SearchCorpusActiveHeadObservationV1),
    /// QI-BB-015: response to [`SearchPlaneControlIpcRequest::MetricsSnapshot`].
    MetricsSnapshot(MetricsSnapshotV1),
    /// QI-BB-026: response to [`SearchPlaneControlIpcRequest::QuarantineInventory`].
    QuarantineInventory(QuarantineInventoryV1),
    /// QI-BB-026: response to [`SearchPlaneControlIpcRequest::QuarantineDiscard`].
    QuarantineDiscardAck(QuarantineDiscardAck),
    /// S21-10: response to [`SearchPlaneControlIpcRequest::ProcessReadiness`].
    ProcessReadinessReport(ProcessReadinessV1),
    ProcessRequestEventsV1(ProcessRequestEventsV1),
}

fn serialize_envelope<S, Payload>(
    name: &'static str,
    request_id: u64,
    payload: &Payload,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    Payload: Serialize,
{
    let mut state = serializer.serialize_struct(name, 2)?;
    state.serialize_field("request_id", &request_id)?;
    state.serialize_field("payload", payload)?;
    state.end()
}

fn deserialize_envelope<'de, A, Payload>(mut map: A) -> Result<(u64, Payload), A::Error>
where
    A: MapAccess<'de>,
    Payload: Deserialize<'de>,
{
    let mut request_id: Option<u64> = None;
    let mut payload: Option<Payload> = None;
    while let Some(key) = map.next_key::<String>()? {
        match key.as_str() {
            "request_id" => {
                if request_id.is_some() {
                    return Err(de::Error::duplicate_field("request_id"));
                }
                request_id = Some(map.next_value()?);
            }
            "payload" => {
                if payload.is_some() {
                    return Err(de::Error::duplicate_field("payload"));
                }
                payload = Some(map.next_value()?);
            }
            _other => {
                let _: de::IgnoredAny = map.next_value()?;
            }
        }
    }
    Ok((
        request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
        payload.ok_or_else(|| de::Error::missing_field("payload"))?,
    ))
}

fn serialize_adjacent_tagged<S, Payload>(
    name: &'static str,
    kind: &'static str,
    payload: &Payload,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    Payload: Serialize,
{
    let mut state = serializer.serialize_struct(name, 2)?;
    state.serialize_field("kind", kind)?;
    state.serialize_field("payload", payload)?;
    state.end()
}

fn payload_before_kind_error<E>(name: &'static str) -> E
where
    E: de::Error,
{
    de::Error::custom(format!(
        "{name} payload arrived before kind; canonical adjacent-tag order is required"
    ))
}

impl Serialize for SearchPlaneQueryIpcRequestEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_envelope(
            "SearchPlaneQueryIpcRequestEnvelope",
            self.request_id,
            &self.payload,
            serializer,
        )
    }
}

struct SearchPlaneQueryIpcRequestEnvelopeVisitor;

impl<'de> Visitor<'de> for SearchPlaneQueryIpcRequestEnvelopeVisitor {
    type Value = SearchPlaneQueryIpcRequestEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneQueryIpcRequestEnvelope map")
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let (request_id, payload) = deserialize_envelope(map)?;
        Ok(SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneQueryIpcRequestEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneQueryIpcRequestEnvelope",
            SEARCH_PLANE_ENVELOPE_FIELDS,
            SearchPlaneQueryIpcRequestEnvelopeVisitor,
        )
    }
}

impl Serialize for SearchPlaneQueryIpcRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::ResolveActiveGeneration(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "ResolveActiveGeneration",
                payload,
                serializer,
            ),
            Self::ResolveLexicalGeneration(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "ResolveLexicalGeneration",
                payload,
                serializer,
            ),
            Self::Text(payload) => {
                serialize_adjacent_tagged("SearchPlaneQueryIpcRequest", "Text", payload, serializer)
            }
            Self::Symbol(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "Symbol",
                payload,
                serializer,
            ),
            Self::Semantic(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "Semantic",
                payload,
                serializer,
            ),
            Self::SemanticWorkBoundedV1(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "SemanticWorkBoundedV1",
                payload,
                serializer,
            ),
            Self::Hybrid(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "Hybrid",
                payload,
                serializer,
            ),
            Self::HybridSeed(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "HybridSeed",
                payload,
                serializer,
            ),
            Self::History(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "History",
                payload,
                serializer,
            ),
            Self::RuntimeMetadata(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "RuntimeMetadata",
                payload,
                serializer,
            ),
            Self::Structural(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "Structural",
                payload,
                serializer,
            ),
            Self::RepoMapQuery(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "RepoMapQuery",
                payload,
                serializer,
            ),
            Self::Explain(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "Explain",
                payload,
                serializer,
            ),
            Self::ClusterMembershipRead(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcRequest",
                "ClusterMembershipRead",
                payload,
                serializer,
            ),
        }
    }
}

struct SearchPlaneQueryIpcRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneQueryIpcRequestVisitor {
    type Value = SearchPlaneQueryIpcRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneQueryIpcRequest adjacent-tagged map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut payload: Option<SearchPlaneQueryIpcRequest> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let kind_value = kind
                        .as_deref()
                        .ok_or_else(|| payload_before_kind_error("SearchPlaneQueryIpcRequest"))?;
                    let decoded = match kind_value {
                        "ResolveActiveGeneration" => {
                            SearchPlaneQueryIpcRequest::ResolveActiveGeneration(map.next_value()?)
                        }
                        "ResolveLexicalGeneration" => {
                            SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(map.next_value()?)
                        }
                        "Text" => SearchPlaneQueryIpcRequest::Text(map.next_value()?),
                        "Symbol" => SearchPlaneQueryIpcRequest::Symbol(map.next_value()?),
                        "Semantic" => SearchPlaneQueryIpcRequest::Semantic(map.next_value()?),
                        "SemanticWorkBoundedV1" => {
                            SearchPlaneQueryIpcRequest::SemanticWorkBoundedV1(map.next_value()?)
                        }
                        "Hybrid" => SearchPlaneQueryIpcRequest::Hybrid(map.next_value()?),
                        "HybridSeed" => SearchPlaneQueryIpcRequest::HybridSeed(map.next_value()?),
                        "History" => SearchPlaneQueryIpcRequest::History(map.next_value()?),
                        "RuntimeMetadata" => {
                            SearchPlaneQueryIpcRequest::RuntimeMetadata(map.next_value()?)
                        }
                        "Structural" => SearchPlaneQueryIpcRequest::Structural(map.next_value()?),
                        "RepoMapQuery" => {
                            SearchPlaneQueryIpcRequest::RepoMapQuery(map.next_value()?)
                        }
                        "Explain" => SearchPlaneQueryIpcRequest::Explain(map.next_value()?),
                        "ClusterMembershipRead" => {
                            SearchPlaneQueryIpcRequest::ClusterMembershipRead(map.next_value()?)
                        }
                        other => {
                            return Err(de::Error::unknown_variant(
                                other,
                                SEARCH_PLANE_QUERY_IPC_REQUEST_VARIANTS,
                            ));
                        }
                    };
                    payload = Some(decoded);
                }
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        if payload.is_none() {
            if SEARCH_PLANE_QUERY_IPC_REQUEST_VARIANTS.contains(&kind.as_str()) {
                return Err(de::Error::missing_field("payload"));
            }
            return Err(de::Error::unknown_variant(
                kind.as_str(),
                SEARCH_PLANE_QUERY_IPC_REQUEST_VARIANTS,
            ));
        }
        payload.map_or_else(|| Err(de::Error::missing_field("payload")), Ok)
    }
}

impl<'de> Deserialize<'de> for SearchPlaneQueryIpcRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneQueryIpcRequest",
            SEARCH_PLANE_ADJACENT_TAG_FIELDS,
            SearchPlaneQueryIpcRequestVisitor,
        )
    }
}

impl Serialize for SearchPlaneQueryIpcResponseEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_envelope(
            "SearchPlaneQueryIpcResponseEnvelope",
            self.request_id,
            &self.payload,
            serializer,
        )
    }
}

struct SearchPlaneQueryIpcResponseEnvelopeVisitor;

impl<'de> Visitor<'de> for SearchPlaneQueryIpcResponseEnvelopeVisitor {
    type Value = SearchPlaneQueryIpcResponseEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneQueryIpcResponseEnvelope map")
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let (request_id, payload) = deserialize_envelope(map)?;
        Ok(SearchPlaneQueryIpcResponseEnvelope {
            request_id,
            payload,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneQueryIpcResponseEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneQueryIpcResponseEnvelope",
            SEARCH_PLANE_ENVELOPE_FIELDS,
            SearchPlaneQueryIpcResponseEnvelopeVisitor,
        )
    }
}

impl Serialize for SearchPlaneQueryIpcResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::ActiveGenerationSnapshot(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "ActiveGenerationSnapshot",
                payload,
                serializer,
            ),
            Self::ResolvedLexicalGeneration(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "ResolvedLexicalGeneration",
                payload,
                serializer,
            ),
            Self::Text(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "Text",
                payload,
                serializer,
            ),
            Self::Symbol(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "Symbol",
                payload,
                serializer,
            ),
            Self::Semantic(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "Semantic",
                payload,
                serializer,
            ),
            Self::SemanticWorkBoundedV1(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "SemanticWorkBoundedV1",
                payload,
                serializer,
            ),
            Self::Hybrid(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "Hybrid",
                payload,
                serializer,
            ),
            Self::HybridSeed(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "HybridSeed",
                payload,
                serializer,
            ),
            Self::History(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "History",
                payload,
                serializer,
            ),
            Self::RuntimeMetadata(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "RuntimeMetadata",
                payload,
                serializer,
            ),
            Self::Structural(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "Structural",
                payload,
                serializer,
            ),
            Self::RepoMapQuery(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "RepoMapQuery",
                payload,
                serializer,
            ),
            Self::Explain(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "Explain",
                payload,
                serializer,
            ),
            Self::ClusterMembershipRead(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "ClusterMembershipRead",
                payload,
                serializer,
            ),
            Self::Error(payload) => serialize_adjacent_tagged(
                "SearchPlaneQueryIpcResponse",
                "Error",
                payload,
                serializer,
            ),
        }
    }
}

struct SearchPlaneQueryIpcResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneQueryIpcResponseVisitor {
    type Value = SearchPlaneQueryIpcResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneQueryIpcResponse adjacent-tagged map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut payload: Option<SearchPlaneQueryIpcResponse> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let kind_value = kind
                        .as_deref()
                        .ok_or_else(|| payload_before_kind_error("SearchPlaneQueryIpcResponse"))?;
                    let decoded = match kind_value {
                        "ActiveGenerationSnapshot" => {
                            SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(map.next_value()?)
                        }
                        "ResolvedLexicalGeneration" => {
                            SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(
                                map.next_value()?,
                            )
                        }
                        "Text" => SearchPlaneQueryIpcResponse::Text(map.next_value()?),
                        "Symbol" => SearchPlaneQueryIpcResponse::Symbol(map.next_value()?),
                        "Semantic" => SearchPlaneQueryIpcResponse::Semantic(map.next_value()?),
                        "SemanticWorkBoundedV1" => {
                            SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(map.next_value()?)
                        }
                        "Hybrid" => SearchPlaneQueryIpcResponse::Hybrid(map.next_value()?),
                        "HybridSeed" => SearchPlaneQueryIpcResponse::HybridSeed(map.next_value()?),
                        "History" => SearchPlaneQueryIpcResponse::History(map.next_value()?),
                        "RuntimeMetadata" => {
                            SearchPlaneQueryIpcResponse::RuntimeMetadata(map.next_value()?)
                        }
                        "Structural" => SearchPlaneQueryIpcResponse::Structural(map.next_value()?),
                        "RepoMapQuery" => {
                            SearchPlaneQueryIpcResponse::RepoMapQuery(map.next_value()?)
                        }
                        "Explain" => SearchPlaneQueryIpcResponse::Explain(map.next_value()?),
                        "ClusterMembershipRead" => {
                            SearchPlaneQueryIpcResponse::ClusterMembershipRead(map.next_value()?)
                        }
                        "Error" => SearchPlaneQueryIpcResponse::Error(map.next_value()?),
                        other => {
                            return Err(de::Error::unknown_variant(
                                other,
                                SEARCH_PLANE_QUERY_IPC_RESPONSE_VARIANTS,
                            ));
                        }
                    };
                    payload = Some(decoded);
                }
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        if payload.is_none() {
            if SEARCH_PLANE_QUERY_IPC_RESPONSE_VARIANTS.contains(&kind.as_str()) {
                return Err(de::Error::missing_field("payload"));
            }
            return Err(de::Error::unknown_variant(
                kind.as_str(),
                SEARCH_PLANE_QUERY_IPC_RESPONSE_VARIANTS,
            ));
        }
        payload.map_or_else(|| Err(de::Error::missing_field("payload")), Ok)
    }
}

impl<'de> Deserialize<'de> for SearchPlaneQueryIpcResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneQueryIpcResponse",
            SEARCH_PLANE_ADJACENT_TAG_FIELDS,
            SearchPlaneQueryIpcResponseVisitor,
        )
    }
}

impl Serialize for SearchPlaneControlIpcRequestEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_envelope(
            "SearchPlaneControlIpcRequestEnvelope",
            self.request_id,
            &self.payload,
            serializer,
        )
    }
}

struct SearchPlaneControlIpcRequestEnvelopeVisitor;

impl<'de> Visitor<'de> for SearchPlaneControlIpcRequestEnvelopeVisitor {
    type Value = SearchPlaneControlIpcRequestEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneControlIpcRequestEnvelope map")
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let (request_id, payload) = deserialize_envelope(map)?;
        Ok(SearchPlaneControlIpcRequestEnvelope {
            request_id,
            payload,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneControlIpcRequestEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneControlIpcRequestEnvelope",
            SEARCH_PLANE_ENVELOPE_FIELDS,
            SearchPlaneControlIpcRequestEnvelopeVisitor,
        )
    }
}

impl Serialize for SearchPlaneControlIpcRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::ActivateSearchCorpusGenerationCas(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "ActivateSearchCorpusGenerationCas",
                payload,
                serializer,
            ),
            Self::RollbackSearchCorpusGenerationCas(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "RollbackSearchCorpusGenerationCas",
                payload,
                serializer,
            ),
            Self::RepoMapActivateV2(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "RepoMapActivateV2",
                payload,
                serializer,
            ),
            Self::RepoMapActiveHeadV2(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "RepoMapActiveHeadV2",
                payload,
                serializer,
            ),
            Self::CurrentGeneration(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "CurrentGeneration",
                payload,
                serializer,
            ),
            Self::GenerationStatus(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "GenerationStatus",
                payload,
                serializer,
            ),
            Self::SearchCorpusActiveHead(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "SearchCorpusActiveHead",
                payload,
                serializer,
            ),
            Self::MetricsSnapshot(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "MetricsSnapshot",
                payload,
                serializer,
            ),
            Self::QuarantineInventory(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "QuarantineInventory",
                payload,
                serializer,
            ),
            Self::QuarantineDiscard(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "QuarantineDiscard",
                payload,
                serializer,
            ),
            Self::ProcessReadiness(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "ProcessReadiness",
                payload,
                serializer,
            ),
            Self::ProcessRequestEventsV1(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "ProcessRequestEventsV1",
                payload,
                serializer,
            ),
        }
    }
}

struct SearchPlaneControlIpcRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneControlIpcRequestVisitor {
    type Value = SearchPlaneControlIpcRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneControlIpcRequest adjacent-tagged map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut payload: Option<SearchPlaneControlIpcRequest> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let kind_value = kind
                        .as_deref()
                        .ok_or_else(|| payload_before_kind_error("SearchPlaneControlIpcRequest"))?;
                    let decoded = match kind_value {
                        "ActivateSearchCorpusGenerationCas" => {
                            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                                map.next_value()?,
                            )
                        }
                        "RollbackSearchCorpusGenerationCas" => {
                            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
                                map.next_value()?,
                            )
                        }
                        "RepoMapActivateV2" => {
                            SearchPlaneControlIpcRequest::RepoMapActivateV2(map.next_value()?)
                        }
                        "RepoMapActiveHeadV2" => {
                            SearchPlaneControlIpcRequest::RepoMapActiveHeadV2(map.next_value()?)
                        }
                        "CurrentGeneration" => {
                            SearchPlaneControlIpcRequest::CurrentGeneration(map.next_value()?)
                        }
                        "GenerationStatus" => {
                            SearchPlaneControlIpcRequest::GenerationStatus(map.next_value()?)
                        }
                        "SearchCorpusActiveHead" => {
                            SearchPlaneControlIpcRequest::SearchCorpusActiveHead(map.next_value()?)
                        }
                        "MetricsSnapshot" => {
                            SearchPlaneControlIpcRequest::MetricsSnapshot(map.next_value()?)
                        }
                        "QuarantineInventory" => {
                            SearchPlaneControlIpcRequest::QuarantineInventory(map.next_value()?)
                        }
                        "QuarantineDiscard" => {
                            SearchPlaneControlIpcRequest::QuarantineDiscard(map.next_value()?)
                        }
                        "ProcessReadiness" => {
                            SearchPlaneControlIpcRequest::ProcessReadiness(map.next_value()?)
                        }
                        "ProcessRequestEventsV1" => {
                            SearchPlaneControlIpcRequest::ProcessRequestEventsV1(map.next_value()?)
                        }
                        other => {
                            return Err(de::Error::unknown_variant(
                                other,
                                SEARCH_PLANE_CONTROL_IPC_REQUEST_VARIANTS,
                            ));
                        }
                    };
                    payload = Some(decoded);
                }
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        if payload.is_none() {
            if SEARCH_PLANE_CONTROL_IPC_REQUEST_VARIANTS.contains(&kind.as_str()) {
                return Err(de::Error::missing_field("payload"));
            }
            return Err(de::Error::unknown_variant(
                kind.as_str(),
                SEARCH_PLANE_CONTROL_IPC_REQUEST_VARIANTS,
            ));
        }
        payload.map_or_else(|| Err(de::Error::missing_field("payload")), Ok)
    }
}

impl<'de> Deserialize<'de> for SearchPlaneControlIpcRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneControlIpcRequest",
            SEARCH_PLANE_ADJACENT_TAG_FIELDS,
            SearchPlaneControlIpcRequestVisitor,
        )
    }
}

impl Serialize for SearchPlaneControlIpcResponseEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_envelope(
            "SearchPlaneControlIpcResponseEnvelope",
            self.request_id,
            &self.payload,
            serializer,
        )
    }
}

struct SearchPlaneControlIpcResponseEnvelopeVisitor;

impl<'de> Visitor<'de> for SearchPlaneControlIpcResponseEnvelopeVisitor {
    type Value = SearchPlaneControlIpcResponseEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneControlIpcResponseEnvelope map")
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let (request_id, payload) = deserialize_envelope(map)?;
        Ok(SearchPlaneControlIpcResponseEnvelope {
            request_id,
            payload,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneControlIpcResponseEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneControlIpcResponseEnvelope",
            SEARCH_PLANE_ENVELOPE_FIELDS,
            SearchPlaneControlIpcResponseEnvelopeVisitor,
        )
    }
}

impl Serialize for SearchPlaneControlIpcResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::SearchCorpusActivationCasAck(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "SearchCorpusActivationCasAck",
                payload,
                serializer,
            ),
            Self::SearchCorpusRollbackCasAck(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "SearchCorpusRollbackCasAck",
                payload,
                serializer,
            ),
            Self::RepoMapTerminalReceiptV2(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "RepoMapTerminalReceiptV2",
                payload,
                serializer,
            ),
            Self::RepoMapActiveHeadV2(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "RepoMapActiveHeadV2",
                payload,
                serializer,
            ),
            Self::Error(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "Error",
                payload,
                serializer,
            ),
            Self::CurrentGenerationSnapshot(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "CurrentGenerationSnapshot",
                payload,
                serializer,
            ),
            Self::GenerationStatusReport(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "GenerationStatusReport",
                payload,
                serializer,
            ),
            Self::SearchCorpusActiveHeadObservation(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "SearchCorpusActiveHeadObservation",
                payload,
                serializer,
            ),
            Self::MetricsSnapshot(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "MetricsSnapshot",
                payload,
                serializer,
            ),
            Self::QuarantineInventory(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "QuarantineInventory",
                payload,
                serializer,
            ),
            Self::QuarantineDiscardAck(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "QuarantineDiscardAck",
                payload,
                serializer,
            ),
            Self::ProcessReadinessReport(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "ProcessReadinessReport",
                payload,
                serializer,
            ),
            Self::ProcessRequestEventsV1(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "ProcessRequestEventsV1",
                payload,
                serializer,
            ),
        }
    }
}

struct SearchPlaneControlIpcResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneControlIpcResponseVisitor {
    type Value = SearchPlaneControlIpcResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneControlIpcResponse adjacent-tagged map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut payload: Option<SearchPlaneControlIpcResponse> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let kind_value = kind.as_deref().ok_or_else(|| {
                        payload_before_kind_error("SearchPlaneControlIpcResponse")
                    })?;
                    let decoded = match kind_value {
                        "SearchCorpusActivationCasAck" => {
                            SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
                                map.next_value()?,
                            )
                        }
                        "SearchCorpusRollbackCasAck" => {
                            SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
                                map.next_value()?,
                            )
                        }
                        "RepoMapTerminalReceiptV2" => {
                            SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(
                                map.next_value()?,
                            )
                        }
                        "RepoMapActiveHeadV2" => {
                            SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(map.next_value()?)
                        }
                        "Error" => SearchPlaneControlIpcResponse::Error(map.next_value()?),
                        "CurrentGenerationSnapshot" => {
                            SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(
                                map.next_value()?,
                            )
                        }
                        "GenerationStatusReport" => {
                            SearchPlaneControlIpcResponse::GenerationStatusReport(map.next_value()?)
                        }
                        "SearchCorpusActiveHeadObservation" => {
                            SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(
                                map.next_value()?,
                            )
                        }
                        "MetricsSnapshot" => {
                            SearchPlaneControlIpcResponse::MetricsSnapshot(map.next_value()?)
                        }
                        "QuarantineInventory" => {
                            SearchPlaneControlIpcResponse::QuarantineInventory(map.next_value()?)
                        }
                        "QuarantineDiscardAck" => {
                            SearchPlaneControlIpcResponse::QuarantineDiscardAck(map.next_value()?)
                        }
                        "ProcessReadinessReport" => {
                            SearchPlaneControlIpcResponse::ProcessReadinessReport(map.next_value()?)
                        }
                        "ProcessRequestEventsV1" => {
                            SearchPlaneControlIpcResponse::ProcessRequestEventsV1(map.next_value()?)
                        }
                        other => {
                            return Err(de::Error::unknown_variant(
                                other,
                                SEARCH_PLANE_CONTROL_IPC_RESPONSE_VARIANTS,
                            ));
                        }
                    };
                    payload = Some(decoded);
                }
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        if payload.is_none() {
            if SEARCH_PLANE_CONTROL_IPC_RESPONSE_VARIANTS.contains(&kind.as_str()) {
                return Err(de::Error::missing_field("payload"));
            }
            return Err(de::Error::unknown_variant(
                kind.as_str(),
                SEARCH_PLANE_CONTROL_IPC_RESPONSE_VARIANTS,
            ));
        }
        payload.map_or_else(|| Err(de::Error::missing_field("payload")), Ok)
    }
}

impl<'de> Deserialize<'de> for SearchPlaneControlIpcResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneControlIpcResponse",
            SEARCH_PLANE_ADJACENT_TAG_FIELDS,
            SearchPlaneControlIpcResponseVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ManifestGeneration, RepoId, RevisionId, SearchCorpusActivationTokenV1,
        SearchCorpusActiveHeadV1, SearchCorpusGenerationIdentityV1, SearchPlaneTrackKind,
        SemanticContentRootsV1, TextQuerySyntax,
    };
    use serde_json::json;
    use std::num::NonZeroU64;

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::into_writer(value, &mut buf)?;
        Ok(buf)
    }

    fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
    where
        T: for<'de> Deserialize<'de>,
    {
        Ok(ciborium::from_reader(bytes)?)
    }

    fn fixture_repo() -> RepoId {
        RepoId::new("repo").expect("static fixture ID satisfies canonical policy")
    }

    fn fixture_revision() -> RevisionId {
        RevisionId::new("rev").expect("static fixture ID satisfies canonical policy")
    }

    #[test]
    fn work_bounded_semantic_has_distinct_wire_kind_and_settlement() {
        let pin = crate::GenerationPin::new(
            fixture_repo(),
            fixture_revision(),
            ManifestGeneration::new(1),
        );
        let request =
            SearchPlaneQueryIpcRequest::SemanticWorkBoundedV1(SemanticWorkBoundedQueryRequestV1 {
                query: SemanticQueryRequest {
                    query_text: "auth".into(),
                    constraints: crate::QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    lexical_scope: None,
                    top_k: 2,
                },
                max_work_units: 4_096,
            });
        assert_eq!(
            serde_json::to_value(&request)
                .expect("JSON")
                .get("kind")
                .cloned(),
            Some(serde_json::json!("SemanticWorkBoundedV1"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneQueryIpcRequest>(
                serde_json::to_value(&request).expect("request JSON")
            )
            .expect("request JSON decode"),
            request
        );
        assert_eq!(
            decode::<SearchPlaneQueryIpcRequest>(&encode(&request).expect("CBOR")).expect("decode"),
            request
        );
        let mut response = SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(
            SemanticWorkBoundedQueryResponseV1 {
                query: SemanticQueryResponse {
                    generation: pin,
                    results: Vec::new(),
                    window: crate::QueryResultWindowV2::exact_probe(0),
                    explanation: crate::SearchExplanation::empty(),
                },
                charged_work_units: 5,
            },
        );
        response.stamp_request_id(19);
        assert_eq!(
            serde_json::to_value(&response)
                .expect("JSON")
                .get("kind")
                .cloned(),
            Some(serde_json::json!("SemanticWorkBoundedV1"))
        );
        assert_eq!(
            decode::<SearchPlaneQueryIpcResponse>(&encode(&response).expect("CBOR"))
                .expect("decode"),
            response
        );
        let SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(settled) = response else {
            panic!("dedicated response variant");
        };
        assert_eq!(settled.query.explanation.request_id, 19);
        assert_eq!(settled.charged_work_units, 5);
    }

    #[test]
    fn work_bounded_semantic_wire_refuses_ambiguous_or_incomplete_fields() {
        let pin = crate::GenerationPin::new(
            fixture_repo(),
            fixture_revision(),
            ManifestGeneration::new(1),
        );
        let request_query = SemanticQueryRequest {
            query_text: "auth".into(),
            constraints: crate::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: None,
            top_k: 2,
        };
        let query = serde_json::to_string(&request_query).expect("request query JSON");
        for malformed in [
            format!("{{\"query\":{query},\"max_work_units\":1,\"max_work_units\":2}}"),
            format!("{{\"query\":{query},\"query\":{query},\"max_work_units\":1}}"),
            format!("{{\"query\":{query}}}"),
            "{\"max_work_units\":1}".into(),
            format!("{{\"query\":{query},\"max_work_units\":1,\"extra\":0}}"),
            format!("{{\"query\":{query},\"max_work_units\":-1}}"),
            format!("{{\"query\":{query},\"max_work_units\":1.5}}"),
        ] {
            assert!(
                serde_json::from_str::<SemanticWorkBoundedQueryRequestV1>(&malformed).is_err(),
                "accepted ambiguous request: {malformed}"
            );
        }
        let response_query = SemanticQueryResponse {
            generation: pin,
            results: Vec::new(),
            window: crate::QueryResultWindowV2::exact_probe(0),
            explanation: crate::SearchExplanation::empty(),
        };
        let query = serde_json::to_string(&response_query).expect("response query JSON");
        for malformed in [
            format!("{{\"query\":{query},\"charged_work_units\":1,\"charged_work_units\":2}}"),
            format!("{{\"query\":{query},\"query\":{query},\"charged_work_units\":1}}"),
            format!("{{\"query\":{query}}}"),
            "{\"charged_work_units\":1}".into(),
            format!("{{\"query\":{query},\"charged_work_units\":1,\"extra\":0}}"),
            format!("{{\"query\":{query},\"charged_work_units\":-1}}"),
            format!("{{\"query\":{query},\"charged_work_units\":1.5}}"),
        ] {
            assert!(
                serde_json::from_str::<SemanticWorkBoundedQueryResponseV1>(&malformed).is_err(),
                "accepted ambiguous response: {malformed}"
            );
        }
    }

    #[test]
    fn active_resolution_query_variants_round_trip_over_json_and_cbor() {
        let request = SearchPlaneQueryIpcRequestEnvelope {
            request_id: 29,
            payload: SearchPlaneQueryIpcRequest::ResolveActiveGeneration(
                CurrentGenerationRequest {
                    repo_id: fixture_repo(),
                    revision_id: fixture_revision(),
                    track: SearchPlaneTrackKind::Lexical,
                },
            ),
        };
        let request_json = serde_json::to_value(&request).expect("resolution request JSON");
        assert_eq!(
            request_json.pointer("/payload/kind"),
            Some(&json!("ResolveActiveGeneration"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneQueryIpcRequestEnvelope>(request_json)
                .expect("resolution request JSON decode"),
            request
        );
        assert_eq!(
            decode::<SearchPlaneQueryIpcRequestEnvelope>(
                &encode(&request).expect("resolution request CBOR")
            )
            .expect("resolution request CBOR decode"),
            request
        );

        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 29,
            payload: SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(
                ActiveGenerationResolutionV1 {
                    track: SearchPlaneTrackKind::Lexical,
                    head: SearchCorpusActiveHeadV1 {
                        generation: SearchCorpusGenerationIdentityV1 {
                            lexical: GenerationSnapshot {
                                repo_id: fixture_repo(),
                                revision_id: fixture_revision(),
                                track: SearchPlaneTrackKind::Lexical,
                                manifest_generation: ManifestGeneration::new(7),
                                manifest_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                            },
                            semantic: GenerationSnapshot {
                                repo_id: fixture_repo(),
                                revision_id: fixture_revision(),
                                track: SearchPlaneTrackKind::Semantic,
                                manifest_generation: ManifestGeneration::new(7),
                                manifest_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                            },
                            semantic_content: SemanticContentRootsV1 {
                                row_root_digest: format!("sha256:{}", "a".repeat(64)),
                                membership_root_digest: format!("sha256:{}", "b".repeat(64)),
                            },
                        },
                        activation_token: SearchCorpusActivationTokenV1::new(
                            [7; 16],
                            NonZeroU64::new(1).expect("fixture sequence is positive"),
                        )
                        .expect("fixture incarnation is nonzero"),
                    },
                },
            ),
        };
        let response_json = serde_json::to_value(&response).expect("resolution response JSON");
        assert_eq!(
            response_json.pointer("/payload/kind"),
            Some(&json!("ActiveGenerationSnapshot"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneQueryIpcResponseEnvelope>(response_json.clone())
                .expect("resolution response JSON decode"),
            response
        );
        for (path, replacement) in [
            ("/payload/payload/track", json!("Structural")),
            (
                "/payload/payload/head/generation/semantic/manifest_generation",
                json!(8),
            ),
            (
                "/payload/payload/head/activation_token/activation_sequence",
                json!(0),
            ),
        ] {
            let mut invalid = response_json.clone();
            *invalid.pointer_mut(path).expect("fixture path exists") = replacement;
            assert!(
                serde_json::from_value::<SearchPlaneQueryIpcResponseEnvelope>(invalid).is_err(),
                "invalid active resolution at {path} must be refused"
            );
        }
        assert_eq!(
            decode::<SearchPlaneQueryIpcResponseEnvelope>(
                &encode(&response).expect("resolution response CBOR")
            )
            .expect("resolution response CBOR decode"),
            response
        );
    }

    #[test]
    fn lexical_plan_resolution_variants_round_trip_over_json_and_cbor() {
        let request = SearchPlaneQueryIpcRequestEnvelope {
            request_id: 31,
            payload: SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "needle rev:at.time(2024-06-01T12:34:56Z)".to_string(),
                constraints: crate::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(
                    fixture_repo(),
                    fixture_revision(),
                    ManifestGeneration::new(7),
                )),
                generation_selector: None,
                top_k: 5,
                cursor: None,
            }),
        };
        let request_json = serde_json::to_value(&request).expect("lexical resolution JSON");
        assert_eq!(
            request_json.pointer("/payload/kind"),
            Some(&json!("ResolveLexicalGeneration"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneQueryIpcRequestEnvelope>(request_json)
                .expect("lexical resolution JSON decode"),
            request
        );
        assert_eq!(
            decode::<SearchPlaneQueryIpcRequestEnvelope>(
                &encode(&request).expect("lexical resolution CBOR")
            )
            .expect("lexical resolution CBOR decode"),
            request
        );

        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 31,
            payload: SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(GenerationPin::new(
                fixture_repo(),
                RevisionId::new("ancestor").expect("fixture revision"),
                ManifestGeneration::new(3),
            )),
        };
        let response_json = serde_json::to_value(&response).expect("lexical resolution JSON");
        assert_eq!(
            response_json.pointer("/payload/kind"),
            Some(&json!("ResolvedLexicalGeneration"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneQueryIpcResponseEnvelope>(response_json)
                .expect("lexical resolution JSON decode"),
            response
        );
        assert_eq!(
            decode::<SearchPlaneQueryIpcResponseEnvelope>(
                &encode(&response).expect("lexical resolution CBOR")
            )
            .expect("lexical resolution CBOR decode"),
            response
        );
    }

    #[test]
    fn query_request_envelope_json_shape_and_cbor_round_trip() {
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id: 7,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "needle".to_string(),
                constraints: crate::QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 5,
                cursor: None,
            }),
        };
        let value = match serde_json::to_value(&envelope) {
            Ok(value) => value,
            Err(err) => {
                assert!(
                    false,
                    "failed to encode query request envelope to json: {err}"
                );
                return;
            }
        };
        assert_eq!(
            value,
            json!({
                "request_id": 7,
                "payload": {
                    "kind": "Text",
                    "payload": {
                        "syntax": "native",
                        "query_text": "needle",
                        "constraints": {
                            "language_any_of": []
                        },
                        "top_k": 5
                    }
                }
            })
        );
        let decoded_json: SearchPlaneQueryIpcRequestEnvelope = match serde_json::from_value(value) {
            Ok(decoded) => decoded,
            Err(err) => {
                assert!(
                    false,
                    "failed to decode query request envelope from json: {err}"
                );
                return;
            }
        };
        assert_eq!(decoded_json, envelope);
        let bytes = match encode(&envelope) {
            Ok(bytes) => bytes,
            Err(err) => {
                assert!(
                    false,
                    "failed to encode query request envelope to cbor: {err}"
                );
                return;
            }
        };
        let decoded_cbor: SearchPlaneQueryIpcRequestEnvelope = match decode(&bytes) {
            Ok(decoded) => decoded,
            Err(err) => {
                assert!(
                    false,
                    "failed to decode query request envelope from cbor: {err}"
                );
                return;
            }
        };
        assert_eq!(decoded_cbor, envelope);
    }

    #[test]
    fn query_response_envelope_json_shape_and_cbor_round_trip() {
        let envelope = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 8,
            payload: SearchPlaneQueryIpcResponse::Error(SearchPlaneIpcError {
                code: crate::SearchPlaneErrorCodeV2::InvalidRequest,
                message: "bad request".to_string(),
                repair: None,
            }),
        };
        let value = match serde_json::to_value(&envelope) {
            Ok(value) => value,
            Err(err) => {
                assert!(
                    false,
                    "failed to encode query response envelope to json: {err}"
                );
                return;
            }
        };
        assert_eq!(
            value,
            json!({
                "request_id": 8,
                "payload": {
                    "kind": "Error",
                    "payload": {
                        "code": "INVALID_REQUEST",
                        "message": "bad request",
                        "repair": null
                    }
                }
            })
        );
        let decoded_json: SearchPlaneQueryIpcResponseEnvelope = match serde_json::from_value(value)
        {
            Ok(decoded) => decoded,
            Err(err) => {
                assert!(
                    false,
                    "failed to decode query response envelope from json: {err}"
                );
                return;
            }
        };
        assert_eq!(decoded_json, envelope);
        let bytes = match encode(&envelope) {
            Ok(bytes) => bytes,
            Err(err) => {
                assert!(
                    false,
                    "failed to encode query response envelope to cbor: {err}"
                );
                return;
            }
        };
        let decoded_cbor: SearchPlaneQueryIpcResponseEnvelope = match decode(&bytes) {
            Ok(decoded) => decoded,
            Err(err) => {
                assert!(
                    false,
                    "failed to decode query response envelope from cbor: {err}"
                );
                return;
            }
        };
        assert_eq!(decoded_cbor, envelope);
    }

    #[test]
    fn control_request_envelope_json_shape_and_cbor_round_trip() {
        let envelope = SearchPlaneControlIpcRequestEnvelope {
            request_id: 9,
            payload: SearchPlaneControlIpcRequest::CurrentGeneration(CurrentGenerationRequest {
                repo_id: fixture_repo(),
                revision_id: fixture_revision(),
                track: SearchPlaneTrackKind::Lexical,
            }),
        };
        let value = match serde_json::to_value(&envelope) {
            Ok(value) => value,
            Err(err) => {
                assert!(
                    false,
                    "failed to encode control request envelope to json: {err}"
                );
                return;
            }
        };
        assert_eq!(
            value,
            json!({
                "request_id": 9,
                "payload": {
                    "kind": "CurrentGeneration",
                    "payload": {
                        "repo_id": "repo",
                        "revision_id": "rev",
                        "track": "Lexical"
                    }
                }
            })
        );
        let decoded_json: SearchPlaneControlIpcRequestEnvelope = match serde_json::from_value(value)
        {
            Ok(decoded) => decoded,
            Err(err) => {
                assert!(
                    false,
                    "failed to decode control request envelope from json: {err}"
                );
                return;
            }
        };
        assert_eq!(decoded_json, envelope);
        let bytes = match encode(&envelope) {
            Ok(bytes) => bytes,
            Err(err) => {
                assert!(
                    false,
                    "failed to encode control request envelope to cbor: {err}"
                );
                return;
            }
        };
        let decoded_cbor: SearchPlaneControlIpcRequestEnvelope = match decode(&bytes) {
            Ok(decoded) => decoded,
            Err(err) => {
                assert!(
                    false,
                    "failed to decode control request envelope from cbor: {err}"
                );
                return;
            }
        };
        assert_eq!(decoded_cbor, envelope);
    }

    #[test]
    fn legacy_activation_and_scalar_rollback_wire_tags_are_unknown() {
        for legacy_kind in [
            "ActivateGeneration",
            "ActivateGenerationCas",
            "RollbackGeneration",
            "RepoMapActivate",
        ] {
            let wire = json!({
                "request_id": 12,
                "payload": {"kind": legacy_kind, "payload": {}}
            });
            let mut old_cbor = Vec::new();
            ciborium::ser::into_writer(&wire, &mut old_cbor).expect("legacy fixture encodes");
            let error = serde_json::from_value::<SearchPlaneControlIpcRequestEnvelope>(wire)
                .expect_err("removed single-track activation wire tag must fail closed");
            assert!(
                error.to_string().contains("unknown variant"),
                "legacy tag {legacy_kind} must fail as an unknown control variant: {error}"
            );
            let error = ciborium::de::from_reader::<SearchPlaneControlIpcRequestEnvelope, _>(
                old_cbor.as_slice(),
            )
            .expect_err("removed control opcode must fail closed on CBOR");
            assert!(error.to_string().contains("unknown variant"), "{error}");
        }

        let response = json!({
            "request_id": 12,
            "payload": {"kind": "RollbackAck", "payload": {}}
        });
        let error = serde_json::from_value::<SearchPlaneControlIpcResponseEnvelope>(response)
            .expect_err("removed scalar rollback response tag must fail closed");
        assert!(error.to_string().contains("unknown variant"));
    }

    fn corpus_identity_v1(generation: u64, digest: &str) -> SearchCorpusGenerationIdentityV1 {
        SearchCorpusGenerationIdentityV1 {
            lexical: GenerationSnapshot {
                repo_id: fixture_repo(),
                revision_id: fixture_revision(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: digest.to_string(),
            },
            semantic: GenerationSnapshot {
                repo_id: fixture_repo(),
                revision_id: fixture_revision(),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: digest.to_string(),
            },
            semantic_content: SemanticContentRootsV1 {
                row_root_digest: format!("sha256:{generation:0>64x}"),
                membership_root_digest: format!("sha256:{:0>64x}", generation.saturating_add(1000)),
            },
        }
    }

    fn corpus_head_v1(generation: u64, digest: &str, sequence: u64) -> SearchCorpusActiveHeadV1 {
        SearchCorpusActiveHeadV1 {
            generation: corpus_identity_v1(generation, digest),
            activation_token: crate::SearchCorpusActivationTokenV1::new(
                [7; crate::ACTIVATION_ROOT_INCARNATION_BYTES_V1],
                std::num::NonZeroU64::new(sequence).expect("fixture sequence is positive"),
            )
            .expect("fixture incarnation is nonzero"),
        }
    }

    #[test]
    fn composite_search_corpus_activation_control_variants_round_trip_v1() {
        let previous = corpus_head_v1(10, "digest-10", 1);
        let candidate = corpus_identity_v1(11, "digest-11");
        let request = SearchPlaneControlIpcRequestEnvelope {
            request_id: 13,
            payload: SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate,
                    expected_active: Some(previous.clone()),
                },
            ),
        };
        let request_value =
            serde_json::to_value(&request).expect("encode composite activation request");
        assert_eq!(
            request_value.pointer("/payload/kind"),
            Some(&json!("ActivateSearchCorpusGenerationCas"))
        );
        assert_eq!(
            decode::<SearchPlaneControlIpcRequestEnvelope>(
                &encode(&request).expect("encode composite activation request cbor"),
            )
            .expect("decode composite activation request cbor"),
            request
        );

        let response = SearchPlaneControlIpcResponseEnvelope {
            request_id: 13,
            payload: SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
                SearchPlaneSearchCorpusActivationCasAck {
                    active: corpus_head_v1(11, "digest-11", 2),
                    previous_sealed_active: Some(previous),
                },
            ),
        };
        let response_value =
            serde_json::to_value(&response).expect("encode composite activation ack");
        assert_eq!(
            response_value.pointer("/payload/kind"),
            Some(&json!("SearchCorpusActivationCasAck"))
        );
        assert_eq!(
            decode::<SearchPlaneControlIpcResponseEnvelope>(
                &encode(&response).expect("encode composite activation ack cbor"),
            )
            .expect("decode composite activation ack cbor"),
            response
        );
    }

    #[test]
    fn search_corpus_active_head_control_variants_round_trip_v1() {
        let request = SearchPlaneControlIpcRequestEnvelope {
            request_id: 14,
            payload: SearchPlaneControlIpcRequest::SearchCorpusActiveHead(
                GenerationStatusRequest {
                    repo_id: fixture_repo(),
                    revision_id: fixture_revision(),
                },
            ),
        };
        assert_eq!(
            serde_json::from_value::<SearchPlaneControlIpcRequestEnvelope>(
                serde_json::to_value(&request).expect("encode request")
            )
            .expect("decode request"),
            request
        );
        assert_eq!(
            decode::<SearchPlaneControlIpcRequestEnvelope>(
                &encode(&request).expect("encode request cbor")
            )
            .expect("decode request cbor"),
            request
        );
        let response = SearchPlaneControlIpcResponseEnvelope {
            request_id: 14,
            payload: SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(
                SearchCorpusActiveHeadObservationV1::new(
                    fixture_repo(),
                    fixture_revision(),
                    Some(corpus_head_v1(11, "digest-11", 2)),
                )
                .expect("matching fixture head"),
            ),
        };
        assert_eq!(
            serde_json::from_value::<SearchPlaneControlIpcResponseEnvelope>(
                serde_json::to_value(&response).expect("encode response")
            )
            .expect("decode response"),
            response
        );
        assert_eq!(
            decode::<SearchPlaneControlIpcResponseEnvelope>(
                &encode(&response).expect("encode response cbor")
            )
            .expect("decode response cbor"),
            response
        );
    }

    #[test]
    fn control_response_envelope_json_shape_and_cbor_round_trip() {
        let envelope = SearchPlaneControlIpcResponseEnvelope {
            request_id: 10,
            payload: SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(GenerationSnapshot {
                repo_id: fixture_repo(),
                revision_id: fixture_revision(),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(11),
                manifest_digest: "digest-11".to_string(),
            }),
        };
        let value = match serde_json::to_value(&envelope) {
            Ok(value) => value,
            Err(err) => {
                assert!(
                    false,
                    "failed to encode control response envelope to json: {err}"
                );
                return;
            }
        };
        assert_eq!(
            value,
            json!({
                "request_id": 10,
                "payload": {
                    "kind": "CurrentGenerationSnapshot",
                    "payload": {
                        "repo_id": "repo",
                        "revision_id": "rev",
                        "track": "Semantic",
                        "manifest_generation": 11,
                        "manifest_digest": "digest-11"
                    }
                }
            })
        );
        let decoded_json: SearchPlaneControlIpcResponseEnvelope =
            match serde_json::from_value(value) {
                Ok(decoded) => decoded,
                Err(err) => {
                    assert!(
                        false,
                        "failed to decode control response envelope from json: {err}"
                    );
                    return;
                }
            };
        assert_eq!(decoded_json, envelope);
        let bytes = match encode(&envelope) {
            Ok(bytes) => bytes,
            Err(err) => {
                assert!(
                    false,
                    "failed to encode control response envelope to cbor: {err}"
                );
                return;
            }
        };
        let decoded_cbor: SearchPlaneControlIpcResponseEnvelope = match decode(&bytes) {
            Ok(decoded) => decoded,
            Err(err) => {
                assert!(
                    false,
                    "failed to decode control response envelope from cbor: {err}"
                );
                return;
            }
        };
        assert_eq!(decoded_cbor, envelope);
    }

    #[test]
    fn composite_rollback_control_variants_round_trip_over_json_and_cbor() {
        let expected_active = corpus_head_v1(11, "digest-11", 2);
        let target = corpus_identity_v1(10, "digest-10");
        let request = SearchPlaneControlIpcRequestEnvelope {
            request_id: 11,
            payload: SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
                SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: expected_active.clone(),
                    target,
                },
            ),
        };
        let request_value = serde_json::to_value(&request).expect("encode rollback request");
        assert_eq!(
            request_value.pointer("/payload/kind"),
            Some(&json!("RollbackSearchCorpusGenerationCas"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneControlIpcRequestEnvelope>(request_value)
                .expect("decode rollback request"),
            request
        );
        assert_eq!(
            decode::<SearchPlaneControlIpcRequestEnvelope>(
                &encode(&request).expect("encode rollback request cbor")
            )
            .expect("decode rollback request cbor"),
            request
        );

        let response = SearchPlaneControlIpcResponseEnvelope {
            request_id: 11,
            payload: SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
                SearchPlaneSearchCorpusRollbackCasAck {
                    active: corpus_head_v1(10, "digest-10", 3),
                    previous_sealed_active: expected_active,
                },
            ),
        };
        let response_value = serde_json::to_value(&response).expect("encode rollback response");
        assert_eq!(
            response_value.pointer("/payload/kind"),
            Some(&json!("SearchCorpusRollbackCasAck"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneControlIpcResponseEnvelope>(response_value)
                .expect("decode rollback response"),
            response
        );
        assert_eq!(
            decode::<SearchPlaneControlIpcResponseEnvelope>(
                &encode(&response).expect("encode rollback response cbor")
            )
            .expect("decode rollback response cbor"),
            response
        );
    }

    #[test]
    fn cluster_membership_query_variants_round_trip_over_json_and_cbor_v1() {
        let generation = crate::GenerationPin::new(
            fixture_repo(),
            fixture_revision(),
            ManifestGeneration::new(23),
        );
        let request = SearchPlaneQueryIpcRequestEnvelope {
            request_id: 23,
            payload: SearchPlaneQueryIpcRequest::ClusterMembershipRead(
                ClusterMembershipBatchReadRequestV1::single_v1(
                    crate::ClusterMembershipReadRequestV1 {
                        cluster_record_id: "cluster-card:auth-service".to_string(),
                        generation: generation.clone(),
                        expected_authority_digest: "cluster-authority-digest".to_string(),
                        limit: 2,
                    },
                ),
            ),
        };
        let request_value = serde_json::to_value(&request).expect("membership request JSON");
        assert_eq!(
            request_value.pointer("/payload/kind"),
            Some(&json!("ClusterMembershipRead"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneQueryIpcRequestEnvelope>(request_value)
                .expect("membership request JSON decode"),
            request
        );
        assert_eq!(
            decode::<SearchPlaneQueryIpcRequestEnvelope>(
                &encode(&request).expect("membership request CBOR")
            )
            .expect("membership request CBOR decode"),
            request
        );

        let response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 23,
            payload: SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                ClusterMembershipBatchReadResponseV1 {
                    outcomes: vec![crate::ClusterMembershipReadOutcomeV1::Available(
                        crate::ClusterMembershipSnapshotV1 {
                            cluster_record_id: "cluster-card:auth-service".to_string(),
                            generation,
                            authority_digest: "cluster-authority-digest".to_string(),
                            members: vec![
                                crate::SymbolId::new("symbol:auth::authenticate"),
                                crate::SymbolId::new("symbol:auth::authorize"),
                            ],
                            completeness: crate::ClusterMembershipCompletenessV1::Complete,
                        },
                    )],
                },
            ),
        };
        let response_value = serde_json::to_value(&response).expect("membership response JSON");
        assert_eq!(
            response_value.pointer("/payload/kind"),
            Some(&json!("ClusterMembershipRead"))
        );
        assert_eq!(
            serde_json::from_value::<SearchPlaneQueryIpcResponseEnvelope>(response_value)
                .expect("membership response JSON decode"),
            response
        );
        assert_eq!(
            decode::<SearchPlaneQueryIpcResponseEnvelope>(
                &encode(&response).expect("membership response CBOR")
            )
            .expect("membership response CBOR decode"),
            response
        );
    }

    #[test]
    fn stamp_request_id_marks_exactly_the_explanation_carrying_variants() {
        let pin = || {
            crate::GenerationPin::new(
                fixture_repo(),
                fixture_revision(),
                ManifestGeneration::new(1),
            )
        };
        let window = || crate::QueryResultWindowV2::exact_probe(0);
        // (name, response, carries an explanation)
        let mut cases: Vec<(&str, SearchPlaneQueryIpcResponse, bool)> = vec![
            (
                "Semantic",
                SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
                    generation: pin(),
                    results: Vec::new(),
                    window: window(),
                    explanation: crate::SearchExplanation::empty(),
                }),
                true,
            ),
            (
                "Hybrid",
                SearchPlaneQueryIpcResponse::Hybrid(HybridQueryResponse {
                    generation: pin(),
                    results: Vec::new(),
                    window: window(),
                    explanation: crate::SearchExplanation::empty(),
                }),
                true,
            ),
            (
                "HybridSeed",
                SearchPlaneQueryIpcResponse::HybridSeed(HybridSeedQueryResponse {
                    generation: pin(),
                    manifest_digest: String::new(),
                    seed_candidates: Vec::new(),
                    window: window(),
                    explanation: crate::SearchExplanation::empty(),
                }),
                true,
            ),
            (
                "Explain",
                SearchPlaneQueryIpcResponse::Explain(SearchPlaneExplainQueryResponse {
                    generation: pin(),
                    presence: crate::CandidatePresenceV1::NotIndexed,
                    explanation: crate::SearchExplanation::empty(),
                }),
                true,
            ),
            (
                "Error",
                SearchPlaneQueryIpcResponse::Error(SearchPlaneIpcError {
                    code: crate::SearchPlaneErrorCodeV2::InvalidRequest,
                    message: "bad request".to_string(),
                    repair: None,
                }),
                false,
            ),
            // W10-R2: the stamp is a no-op on explanation-less variants —
            // byte-identical before and after, not merely "no explanation
            // read back".
            (
                "Text",
                SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
                    rank_unit: crate::TextRankUnit::Chunk,
                    explanation: crate::SearchExplanation::empty(),
                    generation: pin(),
                    results: Vec::new(),
                    window: window(),
                    file_owner_rows: None,
                    next_cursor: None,
                }),
                false,
            ),
            (
                "Symbol",
                SearchPlaneQueryIpcResponse::Symbol(SymbolQueryResponse {
                    generation: pin(),
                    results: Vec::new(),
                    window: window(),
                    next_cursor: None,
                }),
                false,
            ),
        ];
        for (name, response, carries) in &mut cases {
            let before = response.clone();
            response.stamp_request_id(99);
            if !*carries {
                assert_eq!(
                    *response, before,
                    "{name}: stamping an explanation-less variant must change nothing"
                );
            }
            let stamped = match response {
                SearchPlaneQueryIpcResponse::Semantic(r) => Some(r.explanation.request_id),
                SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(r) => {
                    Some(r.query.explanation.request_id)
                }
                SearchPlaneQueryIpcResponse::Hybrid(r) => Some(r.explanation.request_id),
                SearchPlaneQueryIpcResponse::HybridSeed(r) => Some(r.explanation.request_id),
                SearchPlaneQueryIpcResponse::Explain(r) => Some(r.explanation.request_id),
                SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
                | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
                | SearchPlaneQueryIpcResponse::Text(_)
                | SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::Structural(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
                | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
                | SearchPlaneQueryIpcResponse::Error(_) => None,
            };
            assert_eq!(
                stamped,
                (*carries).then_some(99),
                "{name}: stamp must reach exactly the explanation-carrying variants"
            );
        }
    }

    // W10-R2: the wire shape is frozen `u64` — serde still round-trips 0
    // in both formats. The nonzero rule is enforced above serde, by the
    // transport's `validated` gate, never by narrowing the shape.
    #[test]
    fn zero_request_id_survives_serde_shape_frozen() {
        let result = (|| -> Result<(), String> {
            let envelope = SearchPlaneQueryIpcRequestEnvelope {
                request_id: 0,
                payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "needle".to_string(),
                    constraints: crate::QueryConstraintSetV1::unconstrained(),
                    generation: None,
                    generation_selector: None,
                    top_k: 5,
                    cursor: None,
                }),
            };
            let json =
                serde_json::to_value(&envelope).map_err(|err| format!("json encode: {err}"))?;
            if json.get("request_id") != Some(&serde_json::json!(0)) {
                return Err(format!("json must carry request_id 0, got {json}"));
            }
            let decoded_json: SearchPlaneQueryIpcRequestEnvelope =
                serde_json::from_value(json).map_err(|err| format!("json decode: {err}"))?;
            if decoded_json != envelope {
                return Err("json round-trip must preserve the zero id".to_string());
            }
            let cbor = encode(&envelope).map_err(|err| format!("cbor encode: {err}"))?;
            let decoded_cbor: SearchPlaneQueryIpcRequestEnvelope =
                decode(&cbor).map_err(|err| format!("cbor decode: {err}"))?;
            if decoded_cbor != envelope {
                return Err("cbor round-trip must preserve the zero id".to_string());
            }
            Ok(())
        })();
        if let Err(err) = result {
            assert!(false, "{err}");
        }
    }
}
