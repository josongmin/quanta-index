use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    PreparedBundleOutbox, PublishedGenerationSet, RepoId, RevisionId, SearchBundleMutationDelta,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundlePrepareRequest {
    pub outbox: PreparedBundleOutbox,
}

const PUBLISHED_SEARCH_BUNDLE_PREPARE_REQUEST_FIELDS: &[&str] = &["outbox"];

impl Serialize for PublishedSearchBundlePrepareRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PublishedSearchBundlePrepareRequest", 1)?;
        state.serialize_field("outbox", &self.outbox)?;
        state.end()
    }
}

struct PublishedSearchBundlePrepareRequestVisitor;

impl<'de> Visitor<'de> for PublishedSearchBundlePrepareRequestVisitor {
    type Value = PublishedSearchBundlePrepareRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchBundlePrepareRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut outbox: Option<PreparedBundleOutbox> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "outbox" => {
                    if outbox.is_some() {
                        return Err(de::Error::duplicate_field("outbox"));
                    }
                    outbox = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_SEARCH_BUNDLE_PREPARE_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let outbox = outbox.ok_or_else(|| de::Error::missing_field("outbox"))?;
        Ok(PublishedSearchBundlePrepareRequest { outbox })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchBundlePrepareRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchBundlePrepareRequest",
            PUBLISHED_SEARCH_BUNDLE_PREPARE_REQUEST_FIELDS,
            PublishedSearchBundlePrepareRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundlePrepareResponse {
    pub accepted: bool,
    pub external_bundle_id: String,
    pub state: String,
    pub reason: Option<String>,
}

const PUBLISHED_SEARCH_BUNDLE_PREPARE_RESPONSE_FIELDS: &[&str] =
    &["accepted", "external_bundle_id", "state", "reason"];

impl Serialize for PublishedSearchBundlePrepareResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 3;
        if self.reason.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state =
            serializer.serialize_struct("PublishedSearchBundlePrepareResponse", field_count)?;
        state.serialize_field("accepted", &self.accepted)?;
        state.serialize_field("external_bundle_id", &self.external_bundle_id)?;
        state.serialize_field("state", &self.state)?;
        if let Some(reason) = &self.reason {
            state.serialize_field("reason", reason)?;
        }
        state.end()
    }
}

struct PublishedSearchBundlePrepareResponseVisitor;

impl<'de> Visitor<'de> for PublishedSearchBundlePrepareResponseVisitor {
    type Value = PublishedSearchBundlePrepareResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchBundlePrepareResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut accepted: Option<bool> = None;
        let mut external_bundle_id: Option<String> = None;
        let mut state_field: Option<String> = None;
        let mut reason: Option<String> = None;
        let mut reason_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "accepted" => {
                    if accepted.is_some() {
                        return Err(de::Error::duplicate_field("accepted"));
                    }
                    accepted = Some(map.next_value()?);
                }
                "external_bundle_id" => {
                    if external_bundle_id.is_some() {
                        return Err(de::Error::duplicate_field("external_bundle_id"));
                    }
                    external_bundle_id = Some(map.next_value()?);
                }
                "state" => {
                    if state_field.is_some() {
                        return Err(de::Error::duplicate_field("state"));
                    }
                    state_field = Some(map.next_value()?);
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
                        PUBLISHED_SEARCH_BUNDLE_PREPARE_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let accepted = accepted.ok_or_else(|| de::Error::missing_field("accepted"))?;
        let external_bundle_id =
            external_bundle_id.ok_or_else(|| de::Error::missing_field("external_bundle_id"))?;
        let state_field = state_field.ok_or_else(|| de::Error::missing_field("state"))?;
        Ok(PublishedSearchBundlePrepareResponse {
            accepted,
            external_bundle_id,
            state: state_field,
            reason,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchBundlePrepareResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchBundlePrepareResponse",
            PUBLISHED_SEARCH_BUNDLE_PREPARE_RESPONSE_FIELDS,
            PublishedSearchBundlePrepareResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleDeltaApplyRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: PublishedGenerationSet,
    pub delta: SearchBundleMutationDelta,
}

const PUBLISHED_SEARCH_BUNDLE_DELTA_APPLY_REQUEST_FIELDS: &[&str] =
    &["repo_id", "revision_id", "generation", "delta"];

impl Serialize for PublishedSearchBundleDeltaApplyRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PublishedSearchBundleDeltaApplyRequest", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("delta", &self.delta)?;
        state.end()
    }
}

struct PublishedSearchBundleDeltaApplyRequestVisitor;

impl<'de> Visitor<'de> for PublishedSearchBundleDeltaApplyRequestVisitor {
    type Value = PublishedSearchBundleDeltaApplyRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchBundleDeltaApplyRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<PublishedGenerationSet> = None;
        let mut delta: Option<SearchBundleMutationDelta> = None;
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
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "delta" => {
                    if delta.is_some() {
                        return Err(de::Error::duplicate_field("delta"));
                    }
                    delta = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_SEARCH_BUNDLE_DELTA_APPLY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        let delta = delta.ok_or_else(|| de::Error::missing_field("delta"))?;
        Ok(PublishedSearchBundleDeltaApplyRequest {
            repo_id,
            revision_id,
            generation,
            delta,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchBundleDeltaApplyRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchBundleDeltaApplyRequest",
            PUBLISHED_SEARCH_BUNDLE_DELTA_APPLY_REQUEST_FIELDS,
            PublishedSearchBundleDeltaApplyRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleDeltaApplyResponse {
    pub applied: bool,
    pub indexed_generation: PublishedGenerationSet,
    pub reason: Option<String>,
}

const PUBLISHED_SEARCH_BUNDLE_DELTA_APPLY_RESPONSE_FIELDS: &[&str] =
    &["applied", "indexed_generation", "reason"];

impl Serialize for PublishedSearchBundleDeltaApplyResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 2;
        if self.reason.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state =
            serializer.serialize_struct("PublishedSearchBundleDeltaApplyResponse", field_count)?;
        state.serialize_field("applied", &self.applied)?;
        state.serialize_field("indexed_generation", &self.indexed_generation)?;
        if let Some(reason) = &self.reason {
            state.serialize_field("reason", reason)?;
        }
        state.end()
    }
}

struct PublishedSearchBundleDeltaApplyResponseVisitor;

impl<'de> Visitor<'de> for PublishedSearchBundleDeltaApplyResponseVisitor {
    type Value = PublishedSearchBundleDeltaApplyResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchBundleDeltaApplyResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut applied: Option<bool> = None;
        let mut indexed_generation: Option<PublishedGenerationSet> = None;
        let mut reason: Option<String> = None;
        let mut reason_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "applied" => {
                    if applied.is_some() {
                        return Err(de::Error::duplicate_field("applied"));
                    }
                    applied = Some(map.next_value()?);
                }
                "indexed_generation" => {
                    if indexed_generation.is_some() {
                        return Err(de::Error::duplicate_field("indexed_generation"));
                    }
                    indexed_generation = Some(map.next_value()?);
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
                        PUBLISHED_SEARCH_BUNDLE_DELTA_APPLY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let applied = applied.ok_or_else(|| de::Error::missing_field("applied"))?;
        let indexed_generation =
            indexed_generation.ok_or_else(|| de::Error::missing_field("indexed_generation"))?;
        Ok(PublishedSearchBundleDeltaApplyResponse {
            applied,
            indexed_generation,
            reason,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchBundleDeltaApplyResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchBundleDeltaApplyResponse",
            PUBLISHED_SEARCH_BUNDLE_DELTA_APPLY_RESPONSE_FIELDS,
            PublishedSearchBundleDeltaApplyResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleNotify {
    pub outbox_id: String,
}

const BUNDLE_NOTIFY_FIELDS: &[&str] = &["outbox_id"];

impl Serialize for BundleNotify {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BundleNotify", 1)?;
        state.serialize_field("outbox_id", &self.outbox_id)?;
        state.end()
    }
}

struct BundleNotifyVisitor;

impl<'de> Visitor<'de> for BundleNotifyVisitor {
    type Value = BundleNotify;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BundleNotify map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut outbox_id: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "outbox_id" => {
                    if outbox_id.is_some() {
                        return Err(de::Error::duplicate_field("outbox_id"));
                    }
                    outbox_id = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, BUNDLE_NOTIFY_FIELDS)),
            }
        }
        let outbox_id = outbox_id.ok_or_else(|| de::Error::missing_field("outbox_id"))?;
        Ok(BundleNotify { outbox_id })
    }
}

impl<'de> Deserialize<'de> for BundleNotify {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("BundleNotify", BUNDLE_NOTIFY_FIELDS, BundleNotifyVisitor)
    }
}
