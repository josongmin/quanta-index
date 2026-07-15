use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport, GenerationStatusRequest,
    HistoryQueryRequest, HybridQueryRequest, HybridQueryResponse, HybridSeedQueryRequest,
    HybridSeedQueryResponse, RepoMapActivateGenerationRequest, RepoMapMutationAck,
    RepoMapQueryRequest, RepoMapQueryResponse, RuntimeMetadataQueryRequest,
    SearchPlaneActivateGenerationRequest, SearchPlaneActivationAck, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse, SearchPlaneIpcError,
    SearchPlaneRollbackGenerationAck, SearchPlaneRollbackGenerationRequest,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    SemanticQueryRequest, SemanticQueryResponse, StructuralQueryRequest, SymbolQueryRequest,
    SymbolQueryResponse, TextQueryRequest, TextQueryResponse,
};

const SEARCH_PLANE_ENVELOPE_FIELDS: &[&str] = &["request_id", "payload"];
const SEARCH_PLANE_ADJACENT_TAG_FIELDS: &[&str] = &["kind", "payload"];
const SEARCH_PLANE_QUERY_IPC_REQUEST_VARIANTS: &[&str] = &[
    "Text",
    "Symbol",
    "Semantic",
    "Hybrid",
    "HybridSeed",
    "History",
    "RuntimeMetadata",
    "Structural",
    "RepoMapQuery",
    "Explain",
];
const SEARCH_PLANE_QUERY_IPC_RESPONSE_VARIANTS: &[&str] = &[
    "Text",
    "Symbol",
    "Semantic",
    "Hybrid",
    "HybridSeed",
    "History",
    "RuntimeMetadata",
    "Structural",
    "RepoMapQuery",
    "Explain",
    "Error",
];
const SEARCH_PLANE_CONTROL_IPC_REQUEST_VARIANTS: &[&str] = &[
    "ActivateGeneration",
    "RollbackGeneration",
    "RepoMapActivate",
    "CurrentGeneration",
    "GenerationStatus",
];
const SEARCH_PLANE_CONTROL_IPC_RESPONSE_VARIANTS: &[&str] = &[
    "ActivationAck",
    "RollbackAck",
    "RepoMapMutationAck",
    "Error",
    "CurrentGenerationSnapshot",
    "GenerationStatusReport",
];

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneQueryIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneQueryIpcRequest,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneQueryIpcRequest {
    Text(TextQueryRequest),
    Symbol(SymbolQueryRequest),
    Semantic(SemanticQueryRequest),
    Hybrid(HybridQueryRequest),
    HybridSeed(HybridSeedQueryRequest),
    History(HistoryQueryRequest),
    RuntimeMetadata(RuntimeMetadataQueryRequest),
    Structural(StructuralQueryRequest),
    RepoMapQuery(RepoMapQueryRequest),
    Explain(SearchPlaneExplainQueryRequest),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneQueryIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneQueryIpcResponse,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneQueryIpcResponse {
    Text(TextQueryResponse),
    Symbol(SymbolQueryResponse),
    Semantic(SemanticQueryResponse),
    Hybrid(HybridQueryResponse),
    HybridSeed(HybridSeedQueryResponse),
    History(SearchPlaneHistoryQueryResponse),
    RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse),
    Structural(SearchPlaneStructuralQueryResponse),
    RepoMapQuery(RepoMapQueryResponse),
    Explain(SearchPlaneExplainQueryResponse),
    Error(SearchPlaneIpcError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneControlIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneControlIpcRequest,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneControlIpcRequest {
    ActivateGeneration(SearchPlaneActivateGenerationRequest),
    /// Explicit rollback CAS; normal activation remains monotonic.
    RollbackGeneration(SearchPlaneRollbackGenerationRequest),
    // QI-INT-01: `RepoMapIngest(RepoMapSourceBundle)` was removed from the
    // control surface. All RepoMap bundle publishes now go through the
    // typed ingest IPC (`SearchPlaneIngestIpcRequest::PublishRepoMapBundle`)
    // — the SDK switched in QI-SDK-01 and external consumers are expected to
    // follow. Breaking-first per CLAUDE.md "compatibility preservation is
    // not the default".
    RepoMapActivate(RepoMapActivateGenerationRequest),
    /// QI-ACT-01: read-only generation admin query for one
    /// `(repo, revision, track)` triple.
    CurrentGeneration(CurrentGenerationRequest),
    /// QI-ACT-01: read-only status query returning all activated tracks for
    /// one `(repo, revision)` pair.
    GenerationStatus(GenerationStatusRequest),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneControlIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneControlIpcResponse,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneControlIpcResponse {
    ActivationAck(SearchPlaneActivationAck),
    RollbackAck(SearchPlaneRollbackGenerationAck),
    RepoMapMutationAck(RepoMapMutationAck),
    Error(SearchPlaneIpcError),
    /// QI-ACT-01: response to [`SearchPlaneControlIpcRequest::CurrentGeneration`].
    CurrentGenerationSnapshot(GenerationSnapshot),
    /// QI-ACT-01: response to [`SearchPlaneControlIpcRequest::GenerationStatus`].
    GenerationStatusReport(GenerationStatusReport),
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
                        "Text" => SearchPlaneQueryIpcRequest::Text(map.next_value()?),
                        "Symbol" => SearchPlaneQueryIpcRequest::Symbol(map.next_value()?),
                        "Semantic" => SearchPlaneQueryIpcRequest::Semantic(map.next_value()?),
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
                        "Text" => SearchPlaneQueryIpcResponse::Text(map.next_value()?),
                        "Symbol" => SearchPlaneQueryIpcResponse::Symbol(map.next_value()?),
                        "Semantic" => SearchPlaneQueryIpcResponse::Semantic(map.next_value()?),
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
            Self::ActivateGeneration(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "ActivateGeneration",
                payload,
                serializer,
            ),
            Self::RollbackGeneration(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "RollbackGeneration",
                payload,
                serializer,
            ),
            Self::RepoMapActivate(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcRequest",
                "RepoMapActivate",
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
                        "ActivateGeneration" => {
                            SearchPlaneControlIpcRequest::ActivateGeneration(map.next_value()?)
                        }
                        "RollbackGeneration" => {
                            SearchPlaneControlIpcRequest::RollbackGeneration(map.next_value()?)
                        }
                        "RepoMapActivate" => {
                            SearchPlaneControlIpcRequest::RepoMapActivate(map.next_value()?)
                        }
                        "CurrentGeneration" => {
                            SearchPlaneControlIpcRequest::CurrentGeneration(map.next_value()?)
                        }
                        "GenerationStatus" => {
                            SearchPlaneControlIpcRequest::GenerationStatus(map.next_value()?)
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
            Self::ActivationAck(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "ActivationAck",
                payload,
                serializer,
            ),
            Self::RollbackAck(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "RollbackAck",
                payload,
                serializer,
            ),
            Self::RepoMapMutationAck(payload) => serialize_adjacent_tagged(
                "SearchPlaneControlIpcResponse",
                "RepoMapMutationAck",
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
                        "ActivationAck" => {
                            SearchPlaneControlIpcResponse::ActivationAck(map.next_value()?)
                        }
                        "RollbackAck" => {
                            SearchPlaneControlIpcResponse::RollbackAck(map.next_value()?)
                        }
                        "RepoMapMutationAck" => {
                            SearchPlaneControlIpcResponse::RepoMapMutationAck(map.next_value()?)
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
    use crate::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind, TextQuerySyntax};
    use serde_json::json;

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
        RepoId::new("repo")
    }

    fn fixture_revision() -> RevisionId {
        RevisionId::new("rev")
    }

    #[test]
    fn query_request_envelope_json_shape_and_cbor_round_trip() {
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id: 7,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "needle".to_string(),
                generation: None,
                generation_selector: None,
                top_k: 5,
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
                code: "INVALID_REQUEST".to_string(),
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
                        "message": "bad request"
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
    fn rollback_control_variants_round_trip_over_json_and_cbor() {
        let request = SearchPlaneControlIpcRequestEnvelope {
            request_id: 11,
            payload: SearchPlaneControlIpcRequest::RollbackGeneration(
                SearchPlaneRollbackGenerationRequest {
                    repo_id: fixture_repo(),
                    revision_id: fixture_revision(),
                    track: SearchPlaneTrackKind::Semantic,
                    expected_active_generation: ManifestGeneration::new(11),
                    expected_active_manifest_digest: "digest-11".to_string(),
                    target_generation: ManifestGeneration::new(10),
                    target_manifest_digest: "digest-10".to_string(),
                },
            ),
        };
        let request_value = serde_json::to_value(&request).expect("encode rollback request");
        assert_eq!(
            request_value["payload"]["kind"],
            json!("RollbackGeneration")
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
            payload: SearchPlaneControlIpcResponse::RollbackAck(SearchPlaneRollbackGenerationAck {
                repo_id: fixture_repo(),
                revision_id: fixture_revision(),
                track: SearchPlaneTrackKind::Semantic,
                previous_generation: ManifestGeneration::new(11),
                previous_manifest_digest: "digest-11".to_string(),
                manifest_generation: ManifestGeneration::new(10),
                manifest_digest: "digest-10".to_string(),
            }),
        };
        let response_value = serde_json::to_value(&response).expect("encode rollback response");
        assert_eq!(response_value["payload"]["kind"], json!("RollbackAck"));
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
}
