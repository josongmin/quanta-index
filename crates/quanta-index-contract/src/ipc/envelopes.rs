use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    RepoMapQueryRequestV1, RepoMapQueryResponseV1, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest, SearchPlaneHybridQueryResponse,
    SearchPlaneLexicalQueryRequest, SearchPlaneLexicalQueryResponse,
    SearchPlaneSemanticQueryRequest, SearchPlaneSemanticQueryResponse,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneIpcRequest,
}

const SEARCH_PLANE_IPC_REQUEST_ENVELOPE_FIELDS: &[&str] = &["request_id", "payload"];

impl Serialize for SearchPlaneIpcRequestEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIpcRequestEnvelope", 2)?;
        state.serialize_field("request_id", &self.request_id)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

struct SearchPlaneIpcRequestEnvelopeVisitor;

impl<'de> Visitor<'de> for SearchPlaneIpcRequestEnvelopeVisitor {
    type Value = SearchPlaneIpcRequestEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIpcRequestEnvelope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut request_id: Option<u64> = None;
        let mut payload: Option<SearchPlaneIpcRequest> = None;
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
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_IPC_REQUEST_ENVELOPE_FIELDS,
                    ));
                }
            }
        }
        let request_id = request_id.ok_or_else(|| de::Error::missing_field("request_id"))?;
        let payload = payload.ok_or_else(|| de::Error::missing_field("payload"))?;
        Ok(SearchPlaneIpcRequestEnvelope {
            request_id,
            payload,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIpcRequestEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIpcRequestEnvelope",
            SEARCH_PLANE_IPC_REQUEST_ENVELOPE_FIELDS,
            SearchPlaneIpcRequestEnvelopeVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneIpcRequest {
    Lexical(SearchPlaneLexicalQueryRequest),
    Semantic(SearchPlaneSemanticQueryRequest),
    Hybrid(SearchPlaneHybridQueryRequest),
    RepoMapQuery(RepoMapQueryRequestV1),
    Explain(SearchPlaneExplainQueryRequest),
}

impl SearchPlaneIpcRequest {
    const VARIANTS: &'static [&'static str] =
        &["Lexical", "Semantic", "Hybrid", "RepoMapQuery", "Explain"];

    const fn kind(&self) -> &'static str {
        match self {
            Self::Lexical(_) => "Lexical",
            Self::Semantic(_) => "Semantic",
            Self::Hybrid(_) => "Hybrid",
            Self::RepoMapQuery(_) => "RepoMapQuery",
            Self::Explain(_) => "Explain",
        }
    }
}

const SEARCH_PLANE_IPC_REQUEST_FIELDS: &[&str] = &["kind", "payload"];

impl Serialize for SearchPlaneIpcRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIpcRequest", 2)?;
        state.serialize_field("kind", self.kind())?;
        match self {
            Self::Lexical(inner) => state.serialize_field("payload", inner)?,
            Self::Semantic(inner) => state.serialize_field("payload", inner)?,
            Self::Hybrid(inner) => state.serialize_field("payload", inner)?,
            Self::RepoMapQuery(inner) => state.serialize_field("payload", inner)?,
            Self::Explain(inner) => state.serialize_field("payload", inner)?,
        }
        state.end()
    }
}

struct SearchPlaneIpcRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneIpcRequestVisitor {
    type Value = SearchPlaneIpcRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIpcRequest map with kind and payload fields")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut value: Option<SearchPlaneIpcRequest> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if value.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let Some(current_kind) = kind.as_deref() else {
                        return Err(de::Error::custom(
                            "`kind` must appear before `payload` in SearchPlaneIpcRequest",
                        ));
                    };
                    let parsed = match current_kind {
                        "Lexical" => {
                            let inner: SearchPlaneLexicalQueryRequest = map.next_value()?;
                            SearchPlaneIpcRequest::Lexical(inner)
                        }
                        "Semantic" => {
                            let inner: SearchPlaneSemanticQueryRequest = map.next_value()?;
                            SearchPlaneIpcRequest::Semantic(inner)
                        }
                        "Hybrid" => {
                            let inner: SearchPlaneHybridQueryRequest = map.next_value()?;
                            SearchPlaneIpcRequest::Hybrid(inner)
                        }
                        "RepoMapQuery" => {
                            let inner: RepoMapQueryRequestV1 = map.next_value()?;
                            SearchPlaneIpcRequest::RepoMapQuery(inner)
                        }
                        "Explain" => {
                            let inner: SearchPlaneExplainQueryRequest = map.next_value()?;
                            SearchPlaneIpcRequest::Explain(inner)
                        }
                        other => {
                            return Err(de::Error::unknown_variant(
                                other,
                                SearchPlaneIpcRequest::VARIANTS,
                            ));
                        }
                    };
                    value = Some(parsed);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_IPC_REQUEST_FIELDS,
                    ));
                }
            }
        }
        value.ok_or_else(|| de::Error::missing_field("payload"))
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIpcRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIpcRequest",
            SEARCH_PLANE_IPC_REQUEST_FIELDS,
            SearchPlaneIpcRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneIpcResponse,
}

const SEARCH_PLANE_IPC_RESPONSE_ENVELOPE_FIELDS: &[&str] = &["request_id", "payload"];

impl Serialize for SearchPlaneIpcResponseEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIpcResponseEnvelope", 2)?;
        state.serialize_field("request_id", &self.request_id)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

struct SearchPlaneIpcResponseEnvelopeVisitor;

impl<'de> Visitor<'de> for SearchPlaneIpcResponseEnvelopeVisitor {
    type Value = SearchPlaneIpcResponseEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIpcResponseEnvelope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut request_id: Option<u64> = None;
        let mut payload: Option<SearchPlaneIpcResponse> = None;
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
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_IPC_RESPONSE_ENVELOPE_FIELDS,
                    ));
                }
            }
        }
        let request_id = request_id.ok_or_else(|| de::Error::missing_field("request_id"))?;
        let payload = payload.ok_or_else(|| de::Error::missing_field("payload"))?;
        Ok(SearchPlaneIpcResponseEnvelope {
            request_id,
            payload,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIpcResponseEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIpcResponseEnvelope",
            SEARCH_PLANE_IPC_RESPONSE_ENVELOPE_FIELDS,
            SearchPlaneIpcResponseEnvelopeVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneIpcResponse {
    Lexical(SearchPlaneLexicalQueryResponse),
    Semantic(SearchPlaneSemanticQueryResponse),
    Hybrid(SearchPlaneHybridQueryResponse),
    RepoMapQuery(RepoMapQueryResponseV1),
    Explain(SearchPlaneExplainQueryResponse),
    Error(SearchPlaneIpcError),
}

impl SearchPlaneIpcResponse {
    const VARIANTS: &'static [&'static str] = &[
        "Lexical",
        "Semantic",
        "Hybrid",
        "RepoMapQuery",
        "Explain",
        "Error",
    ];

    const fn kind(&self) -> &'static str {
        match self {
            Self::Lexical(_) => "Lexical",
            Self::Semantic(_) => "Semantic",
            Self::Hybrid(_) => "Hybrid",
            Self::RepoMapQuery(_) => "RepoMapQuery",
            Self::Explain(_) => "Explain",
            Self::Error(_) => "Error",
        }
    }
}

const SEARCH_PLANE_IPC_RESPONSE_FIELDS: &[&str] = &["kind", "payload"];

impl Serialize for SearchPlaneIpcResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIpcResponse", 2)?;
        state.serialize_field("kind", self.kind())?;
        match self {
            Self::Lexical(inner) => state.serialize_field("payload", inner)?,
            Self::Semantic(inner) => state.serialize_field("payload", inner)?,
            Self::Hybrid(inner) => state.serialize_field("payload", inner)?,
            Self::RepoMapQuery(inner) => state.serialize_field("payload", inner)?,
            Self::Explain(inner) => state.serialize_field("payload", inner)?,
            Self::Error(inner) => state.serialize_field("payload", inner)?,
        }
        state.end()
    }
}

struct SearchPlaneIpcResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneIpcResponseVisitor {
    type Value = SearchPlaneIpcResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIpcResponse map with kind and payload fields")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut value: Option<SearchPlaneIpcResponse> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if value.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let Some(current_kind) = kind.as_deref() else {
                        return Err(de::Error::custom(
                            "`kind` must appear before `payload` in SearchPlaneIpcResponse",
                        ));
                    };
                    let parsed = match current_kind {
                        "Lexical" => {
                            let inner: SearchPlaneLexicalQueryResponse = map.next_value()?;
                            SearchPlaneIpcResponse::Lexical(inner)
                        }
                        "Semantic" => {
                            let inner: SearchPlaneSemanticQueryResponse = map.next_value()?;
                            SearchPlaneIpcResponse::Semantic(inner)
                        }
                        "Hybrid" => {
                            let inner: SearchPlaneHybridQueryResponse = map.next_value()?;
                            SearchPlaneIpcResponse::Hybrid(inner)
                        }
                        "RepoMapQuery" => {
                            let inner: RepoMapQueryResponseV1 = map.next_value()?;
                            SearchPlaneIpcResponse::RepoMapQuery(inner)
                        }
                        "Explain" => {
                            let inner: SearchPlaneExplainQueryResponse = map.next_value()?;
                            SearchPlaneIpcResponse::Explain(inner)
                        }
                        "Error" => {
                            let inner: SearchPlaneIpcError = map.next_value()?;
                            SearchPlaneIpcResponse::Error(inner)
                        }
                        other => {
                            return Err(de::Error::unknown_variant(
                                other,
                                SearchPlaneIpcResponse::VARIANTS,
                            ));
                        }
                    };
                    value = Some(parsed);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_IPC_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        value.ok_or_else(|| de::Error::missing_field("payload"))
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIpcResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIpcResponse",
            SEARCH_PLANE_IPC_RESPONSE_FIELDS,
            SearchPlaneIpcResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneIpcError {
    pub code: String,
    pub message: String,
}

const SEARCH_PLANE_IPC_ERROR_FIELDS: &[&str] = &["code", "message"];

impl Serialize for SearchPlaneIpcError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIpcError", 2)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.message)?;
        state.end()
    }
}

struct SearchPlaneIpcErrorVisitor;

impl<'de> Visitor<'de> for SearchPlaneIpcErrorVisitor {
    type Value = SearchPlaneIpcError;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIpcError map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut code: Option<String> = None;
        let mut message: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "code" => {
                    if code.is_some() {
                        return Err(de::Error::duplicate_field("code"));
                    }
                    code = Some(map.next_value()?);
                }
                "message" => {
                    if message.is_some() {
                        return Err(de::Error::duplicate_field("message"));
                    }
                    message = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_IPC_ERROR_FIELDS,
                    ));
                }
            }
        }
        let code = code.ok_or_else(|| de::Error::missing_field("code"))?;
        let message = message.ok_or_else(|| de::Error::missing_field("message"))?;
        Ok(SearchPlaneIpcError { code, message })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIpcError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIpcError",
            SEARCH_PLANE_IPC_ERROR_FIELDS,
            SearchPlaneIpcErrorVisitor,
        )
    }
}
