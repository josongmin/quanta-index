use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{PublishedGenerationSet, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchGenerationActivateRequest {
    pub generation: PublishedGenerationSet,
    pub lexical_ready: bool,
    pub semantic_ready: bool,
    pub active_at_ms: u64,
}

const PUBLISHED_SEARCH_GENERATION_ACTIVATE_REQUEST_FIELDS: &[&str] = &[
    "generation",
    "lexical_ready",
    "semantic_ready",
    "active_at_ms",
];

impl Serialize for PublishedSearchGenerationActivateRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state =
            serializer.serialize_struct("PublishedSearchGenerationActivateRequest", 4)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("lexical_ready", &self.lexical_ready)?;
        state.serialize_field("semantic_ready", &self.semantic_ready)?;
        state.serialize_field("active_at_ms", &self.active_at_ms)?;
        state.end()
    }
}

struct PublishedSearchGenerationActivateRequestVisitor;

impl<'de> Visitor<'de> for PublishedSearchGenerationActivateRequestVisitor {
    type Value = PublishedSearchGenerationActivateRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchGenerationActivateRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<PublishedGenerationSet> = None;
        let mut lexical_ready: Option<bool> = None;
        let mut semantic_ready: Option<bool> = None;
        let mut active_at_ms: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "lexical_ready" => {
                    if lexical_ready.is_some() {
                        return Err(de::Error::duplicate_field("lexical_ready"));
                    }
                    lexical_ready = Some(map.next_value()?);
                }
                "semantic_ready" => {
                    if semantic_ready.is_some() {
                        return Err(de::Error::duplicate_field("semantic_ready"));
                    }
                    semantic_ready = Some(map.next_value()?);
                }
                "active_at_ms" => {
                    if active_at_ms.is_some() {
                        return Err(de::Error::duplicate_field("active_at_ms"));
                    }
                    active_at_ms = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_SEARCH_GENERATION_ACTIVATE_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        let lexical_ready =
            lexical_ready.ok_or_else(|| de::Error::missing_field("lexical_ready"))?;
        let semantic_ready =
            semantic_ready.ok_or_else(|| de::Error::missing_field("semantic_ready"))?;
        let active_at_ms = active_at_ms.ok_or_else(|| de::Error::missing_field("active_at_ms"))?;
        Ok(PublishedSearchGenerationActivateRequest {
            generation,
            lexical_ready,
            semantic_ready,
            active_at_ms,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchGenerationActivateRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchGenerationActivateRequest",
            PUBLISHED_SEARCH_GENERATION_ACTIVATE_REQUEST_FIELDS,
            PublishedSearchGenerationActivateRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchGenerationActivateResponse {
    pub activated: bool,
    pub active_generation: Option<PublishedGenerationSet>,
    pub reason: Option<String>,
}

const PUBLISHED_SEARCH_GENERATION_ACTIVATE_RESPONSE_FIELDS: &[&str] =
    &["activated", "active_generation", "reason"];

impl Serialize for PublishedSearchGenerationActivateResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 1;
        if self.active_generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.reason.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer
            .serialize_struct("PublishedSearchGenerationActivateResponse", field_count)?;
        state.serialize_field("activated", &self.activated)?;
        if let Some(active_generation) = &self.active_generation {
            state.serialize_field("active_generation", active_generation)?;
        }
        if let Some(reason) = &self.reason {
            state.serialize_field("reason", reason)?;
        }
        state.end()
    }
}

struct PublishedSearchGenerationActivateResponseVisitor;

impl<'de> Visitor<'de> for PublishedSearchGenerationActivateResponseVisitor {
    type Value = PublishedSearchGenerationActivateResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchGenerationActivateResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut activated: Option<bool> = None;
        let mut active_generation: Option<PublishedGenerationSet> = None;
        let mut active_generation_seen = false;
        let mut reason: Option<String> = None;
        let mut reason_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "activated" => {
                    if activated.is_some() {
                        return Err(de::Error::duplicate_field("activated"));
                    }
                    activated = Some(map.next_value()?);
                }
                "active_generation" => {
                    if active_generation_seen {
                        return Err(de::Error::duplicate_field("active_generation"));
                    }
                    active_generation_seen = true;
                    active_generation = Some(map.next_value()?);
                }
                "reason" => {
                    if reason_seen {
                        return Err(de::Error::duplicate_field("reason"));
                    }
                    reason_seen = true;
                    reason = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_SEARCH_GENERATION_ACTIVATE_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let activated = activated.ok_or_else(|| de::Error::missing_field("activated"))?;
        Ok(PublishedSearchGenerationActivateResponse {
            activated,
            active_generation,
            reason,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchGenerationActivateResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchGenerationActivateResponse",
            PUBLISHED_SEARCH_GENERATION_ACTIVATE_RESPONSE_FIELDS,
            PublishedSearchGenerationActivateResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchGenerationReadinessResponse {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub prepared_bundle_count: u64,
    pub active_generation: Option<PublishedGenerationSet>,
    pub lexical_ready: bool,
    pub semantic_ready: bool,
    pub mode: String,
    pub reason: Option<String>,
}

const PUBLISHED_SEARCH_GENERATION_READINESS_RESPONSE_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "prepared_bundle_count",
    "active_generation",
    "lexical_ready",
    "semantic_ready",
    "mode",
    "reason",
];

impl Serialize for PublishedSearchGenerationReadinessResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 6;
        if self.active_generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.reason.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer
            .serialize_struct("PublishedSearchGenerationReadinessResponse", field_count)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("prepared_bundle_count", &self.prepared_bundle_count)?;
        if let Some(active_generation) = &self.active_generation {
            state.serialize_field("active_generation", active_generation)?;
        }
        state.serialize_field("lexical_ready", &self.lexical_ready)?;
        state.serialize_field("semantic_ready", &self.semantic_ready)?;
        state.serialize_field("mode", &self.mode)?;
        if let Some(reason) = &self.reason {
            state.serialize_field("reason", reason)?;
        }
        state.end()
    }
}

struct PublishedSearchGenerationReadinessResponseVisitor;

impl<'de> Visitor<'de> for PublishedSearchGenerationReadinessResponseVisitor {
    type Value = PublishedSearchGenerationReadinessResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchGenerationReadinessResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut prepared_bundle_count: Option<u64> = None;
        let mut active_generation: Option<PublishedGenerationSet> = None;
        let mut active_generation_seen = false;
        let mut lexical_ready: Option<bool> = None;
        let mut semantic_ready: Option<bool> = None;
        let mut mode: Option<String> = None;
        let mut reason: Option<String> = None;
        let mut reason_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "prepared_bundle_count" => {
                    if prepared_bundle_count.is_some() {
                        return Err(de::Error::duplicate_field("prepared_bundle_count"));
                    }
                    prepared_bundle_count = Some(map.next_value()?);
                }
                "active_generation" => {
                    if active_generation_seen {
                        return Err(de::Error::duplicate_field("active_generation"));
                    }
                    active_generation_seen = true;
                    active_generation = Some(map.next_value()?);
                }
                "lexical_ready" => {
                    if lexical_ready.is_some() {
                        return Err(de::Error::duplicate_field("lexical_ready"));
                    }
                    lexical_ready = Some(map.next_value()?);
                }
                "semantic_ready" => {
                    if semantic_ready.is_some() {
                        return Err(de::Error::duplicate_field("semantic_ready"));
                    }
                    semantic_ready = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "reason" => {
                    if reason_seen {
                        return Err(de::Error::duplicate_field("reason"));
                    }
                    reason_seen = true;
                    reason = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_SEARCH_GENERATION_READINESS_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let prepared_bundle_count = prepared_bundle_count
            .ok_or_else(|| de::Error::missing_field("prepared_bundle_count"))?;
        let lexical_ready =
            lexical_ready.ok_or_else(|| de::Error::missing_field("lexical_ready"))?;
        let semantic_ready =
            semantic_ready.ok_or_else(|| de::Error::missing_field("semantic_ready"))?;
        let mode = mode.ok_or_else(|| de::Error::missing_field("mode"))?;
        Ok(PublishedSearchGenerationReadinessResponse {
            repo_id,
            revision_id,
            prepared_bundle_count,
            active_generation,
            lexical_ready,
            semantic_ready,
            mode,
            reason,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchGenerationReadinessResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchGenerationReadinessResponse",
            PUBLISHED_SEARCH_GENERATION_READINESS_RESPONSE_FIELDS,
            PublishedSearchGenerationReadinessResponseVisitor,
        )
    }
}
