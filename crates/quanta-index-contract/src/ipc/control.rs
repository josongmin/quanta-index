use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use quanta_index_contract_base::ids::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SearchPlaneTrackKind {
    Lexical,
    Semantic,
    Structural,
}

impl SearchPlaneTrackKind {
    const VARIANTS: &'static [&'static str] = &["Lexical", "Semantic", "Structural"];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Lexical => "Lexical",
            Self::Semantic => "Semantic",
            Self::Structural => "Structural",
        }
    }
}

impl Serialize for SearchPlaneTrackKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct SearchPlaneTrackKindVisitor;

impl Visitor<'_> for SearchPlaneTrackKindVisitor {
    type Value = SearchPlaneTrackKind;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneTrackKind string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "Lexical" => Ok(SearchPlaneTrackKind::Lexical),
            "Semantic" => Ok(SearchPlaneTrackKind::Semantic),
            "Structural" => Ok(SearchPlaneTrackKind::Structural),
            other => Err(de::Error::unknown_variant(
                other,
                SearchPlaneTrackKind::VARIANTS,
            )),
        }
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for SearchPlaneTrackKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SearchPlaneTrackKindVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneActivateGenerationRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
    pub tracks: Vec<SearchPlaneTrackKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneActivationAck {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
    pub tracks: Vec<SearchPlaneTrackKind>,
}

const SEARCH_PLANE_ACTIVATE_GENERATION_REQUEST_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "manifest_digest",
    "tracks",
];

fn serialize_activation_like<S>(
    name: &'static str,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    manifest_generation: &ManifestGeneration,
    manifest_digest: &str,
    tracks: &[SearchPlaneTrackKind],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut state = serializer.serialize_struct(name, 5)?;
    state.serialize_field("repo_id", repo_id)?;
    state.serialize_field("revision_id", revision_id)?;
    state.serialize_field("manifest_generation", manifest_generation)?;
    state.serialize_field("manifest_digest", manifest_digest)?;
    state.serialize_field("tracks", tracks)?;
    state.end()
}

impl Serialize for SearchPlaneActivateGenerationRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_activation_like(
            "SearchPlaneActivateGenerationRequest",
            &self.repo_id,
            &self.revision_id,
            &self.manifest_generation,
            &self.manifest_digest,
            &self.tracks,
            serializer,
        )
    }
}

impl Serialize for SearchPlaneActivationAck {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_activation_like(
            "SearchPlaneActivationAck",
            &self.repo_id,
            &self.revision_id,
            &self.manifest_generation,
            &self.manifest_digest,
            &self.tracks,
            serializer,
        )
    }
}

struct SearchPlaneActivateGenerationRequestVisitor;
struct SearchPlaneActivationAckVisitor;

type ActivationLikeTuple = (
    RepoId,
    RevisionId,
    ManifestGeneration,
    String,
    Vec<SearchPlaneTrackKind>,
);

fn deserialize_activation_like<'de, A>(mut map: A) -> Result<ActivationLikeTuple, A::Error>
where
    A: MapAccess<'de>,
{
    let mut repo_id: Option<RepoId> = None;
    let mut revision_id: Option<RevisionId> = None;
    let mut manifest_generation: Option<ManifestGeneration> = None;
    let mut manifest_digest: Option<String> = None;
    let mut tracks: Option<Vec<SearchPlaneTrackKind>> = None;
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
            "manifest_generation" => {
                if manifest_generation.is_some() {
                    return Err(de::Error::duplicate_field("manifest_generation"));
                }
                manifest_generation = Some(map.next_value()?);
            }
            "manifest_digest" => {
                if manifest_digest.is_some() {
                    return Err(de::Error::duplicate_field("manifest_digest"));
                }
                manifest_digest = Some(map.next_value()?);
            }
            "tracks" => {
                if tracks.is_some() {
                    return Err(de::Error::duplicate_field("tracks"));
                }
                tracks = Some(map.next_value()?);
            }
            other => {
                return Err(de::Error::unknown_field(
                    other,
                    SEARCH_PLANE_ACTIVATE_GENERATION_REQUEST_FIELDS,
                ));
            }
        }
    }
    Ok((
        repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
        revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
        manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
        manifest_digest.ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
        tracks.ok_or_else(|| de::Error::missing_field("tracks"))?,
    ))
}

impl<'de> Visitor<'de> for SearchPlaneActivateGenerationRequestVisitor {
    type Value = SearchPlaneActivateGenerationRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneActivateGenerationRequest map")
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let (repo_id, revision_id, manifest_generation, manifest_digest, tracks) =
            deserialize_activation_like(map)?;
        Ok(SearchPlaneActivateGenerationRequest {
            repo_id,
            revision_id,
            manifest_generation,
            manifest_digest,
            tracks,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneActivateGenerationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneActivateGenerationRequest",
            SEARCH_PLANE_ACTIVATE_GENERATION_REQUEST_FIELDS,
            SearchPlaneActivateGenerationRequestVisitor,
        )
    }
}

impl<'de> Visitor<'de> for SearchPlaneActivationAckVisitor {
    type Value = SearchPlaneActivationAck;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneActivationAck map")
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let (repo_id, revision_id, manifest_generation, manifest_digest, tracks) =
            deserialize_activation_like(map)?;
        Ok(SearchPlaneActivationAck {
            repo_id,
            revision_id,
            manifest_generation,
            manifest_digest,
            tracks,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneActivationAck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneActivationAck",
            SEARCH_PLANE_ACTIVATE_GENERATION_REQUEST_FIELDS,
            SearchPlaneActivationAckVisitor,
        )
    }
}

// =============================================================================
// QI-ACT-01 — Generation Admin Surface
// =============================================================================
//
// Read-only queries against the `ActivationCatalog`. `current()` returns the
// active generation for one (repo, revision, track) triple; `status()`
// returns all active tracks for one (repo, revision) pair.
//
// Both share the same `SearchPlaneIpcError { code, message }` failure shape
// as the rest of the control IPC — there is no typed `NoActiveGeneration`
// response variant. Missing entries surface as `Error { code: "NOT_READY",
// message: ... }`, propagating `CoreError::NotReady` from
// `ActivationCatalog::resolve()` (fail-closed per CLAUDE.md safety rule
// "no silent fallback").

/// Look up the active generation for one `(repo, revision, track)` triple.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentGenerationRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub track: SearchPlaneTrackKind,
}

const CURRENT_GENERATION_REQUEST_FIELDS: &[&str] = &["repo_id", "revision_id", "track"];

impl Serialize for CurrentGenerationRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CurrentGenerationRequest", 3)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("track", &self.track)?;
        state.end()
    }
}

struct CurrentGenerationRequestVisitor;

impl<'de> Visitor<'de> for CurrentGenerationRequestVisitor {
    type Value = CurrentGenerationRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CurrentGenerationRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut track: Option<SearchPlaneTrackKind> = None;
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
                "track" => {
                    if track.is_some() {
                        return Err(de::Error::duplicate_field("track"));
                    }
                    track = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        CURRENT_GENERATION_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(CurrentGenerationRequest {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            track: track.ok_or_else(|| de::Error::missing_field("track"))?,
        })
    }
}

impl<'de> Deserialize<'de> for CurrentGenerationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "CurrentGenerationRequest",
            CURRENT_GENERATION_REQUEST_FIELDS,
            CurrentGenerationRequestVisitor,
        )
    }
}

/// Snapshot of one active generation entry (mirrors
/// `ActiveGenerationRecord` on the search-plane side).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationSnapshot {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub track: SearchPlaneTrackKind,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
}

const GENERATION_SNAPSHOT_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "track",
    "manifest_generation",
    "manifest_digest",
];

impl Serialize for GenerationSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("GenerationSnapshot", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("track", &self.track)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.end()
    }
}

struct GenerationSnapshotVisitor;

impl<'de> Visitor<'de> for GenerationSnapshotVisitor {
    type Value = GenerationSnapshot;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a GenerationSnapshot map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut track: Option<SearchPlaneTrackKind> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<String> = None;
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
                "track" => {
                    if track.is_some() {
                        return Err(de::Error::duplicate_field("track"));
                    }
                    track = Some(map.next_value()?);
                }
                "manifest_generation" => {
                    if manifest_generation.is_some() {
                        return Err(de::Error::duplicate_field("manifest_generation"));
                    }
                    manifest_generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, GENERATION_SNAPSHOT_FIELDS));
                }
            }
        }
        Ok(GenerationSnapshot {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            track: track.ok_or_else(|| de::Error::missing_field("track"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
        })
    }
}

impl<'de> Deserialize<'de> for GenerationSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "GenerationSnapshot",
            GENERATION_SNAPSHOT_FIELDS,
            GenerationSnapshotVisitor,
        )
    }
}

/// List all active tracks for one `(repo, revision)` pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationStatusRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
}

const GENERATION_STATUS_REQUEST_FIELDS: &[&str] = &["repo_id", "revision_id"];

impl Serialize for GenerationStatusRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("GenerationStatusRequest", 2)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.end()
    }
}

struct GenerationStatusRequestVisitor;

impl<'de> Visitor<'de> for GenerationStatusRequestVisitor {
    type Value = GenerationStatusRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a GenerationStatusRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
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
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        GENERATION_STATUS_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(GenerationStatusRequest {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for GenerationStatusRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "GenerationStatusRequest",
            GENERATION_STATUS_REQUEST_FIELDS,
            GenerationStatusRequestVisitor,
        )
    }
}

/// Per-track readiness record in a [`GenerationStatusReport`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackReadinessRecord {
    pub track: SearchPlaneTrackKind,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
}

const TRACK_READINESS_RECORD_FIELDS: &[&str] = &["track", "manifest_generation", "manifest_digest"];

impl Serialize for TrackReadinessRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("TrackReadinessRecord", 3)?;
        state.serialize_field("track", &self.track)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.end()
    }
}

struct TrackReadinessRecordVisitor;

impl<'de> Visitor<'de> for TrackReadinessRecordVisitor {
    type Value = TrackReadinessRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a TrackReadinessRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut track: Option<SearchPlaneTrackKind> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "track" => {
                    if track.is_some() {
                        return Err(de::Error::duplicate_field("track"));
                    }
                    track = Some(map.next_value()?);
                }
                "manifest_generation" => {
                    if manifest_generation.is_some() {
                        return Err(de::Error::duplicate_field("manifest_generation"));
                    }
                    manifest_generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        TRACK_READINESS_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(TrackReadinessRecord {
            track: track.ok_or_else(|| de::Error::missing_field("track"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
        })
    }
}

impl<'de> Deserialize<'de> for TrackReadinessRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "TrackReadinessRecord",
            TRACK_READINESS_RECORD_FIELDS,
            TrackReadinessRecordVisitor,
        )
    }
}

/// Aggregate readiness report across all activated tracks for one
/// `(repo, revision)` pair. `tracks` is order-stable (lexical before
/// semantic per `SearchPlaneTrackKind` declaration order).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationStatusReport {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub tracks: Vec<TrackReadinessRecord>,
}

const GENERATION_STATUS_REPORT_FIELDS: &[&str] = &["repo_id", "revision_id", "tracks"];

impl Serialize for GenerationStatusReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("GenerationStatusReport", 3)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("tracks", &self.tracks)?;
        state.end()
    }
}

struct GenerationStatusReportVisitor;

impl<'de> Visitor<'de> for GenerationStatusReportVisitor {
    type Value = GenerationStatusReport;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a GenerationStatusReport map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut tracks: Option<Vec<TrackReadinessRecord>> = None;
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
                "tracks" => {
                    if tracks.is_some() {
                        return Err(de::Error::duplicate_field("tracks"));
                    }
                    tracks = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        GENERATION_STATUS_REPORT_FIELDS,
                    ));
                }
            }
        }
        Ok(GenerationStatusReport {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            tracks: tracks.ok_or_else(|| de::Error::missing_field("tracks"))?,
        })
    }
}

impl<'de> Deserialize<'de> for GenerationStatusReport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "GenerationStatusReport",
            GENERATION_STATUS_REPORT_FIELDS,
            GenerationStatusReportVisitor,
        )
    }
}

// =============================================================================
// QI-ACT-01 round-trip tests
// =============================================================================

#[cfg(test)]
mod qi_act_01_tests {
    use super::*;

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

    fn fixture_rev() -> RevisionId {
        RevisionId::new("rev")
    }

    #[test]
    fn current_generation_request_round_trip() {
        for track in [
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Semantic,
            SearchPlaneTrackKind::Structural,
        ] {
            let request = CurrentGenerationRequest {
                repo_id: fixture_repo(),
                revision_id: fixture_rev(),
                track,
            };
            let Ok(bytes) = encode(&request) else {
                assert!(false, "failed to encode CurrentGenerationRequest");
                return;
            };
            let Ok(decoded) = decode::<CurrentGenerationRequest>(&bytes) else {
                assert!(false, "failed to decode CurrentGenerationRequest");
                return;
            };
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn generation_snapshot_round_trip() {
        let snapshot = GenerationSnapshot {
            repo_id: fixture_repo(),
            revision_id: fixture_rev(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(11),
            manifest_digest: "digest-11".to_string(),
        };
        let Ok(bytes) = encode(&snapshot) else {
            assert!(false, "failed to encode GenerationSnapshot");
            return;
        };
        let Ok(decoded) = decode::<GenerationSnapshot>(&bytes) else {
            assert!(false, "failed to decode GenerationSnapshot");
            return;
        };
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn generation_status_request_round_trip() {
        let request = GenerationStatusRequest {
            repo_id: fixture_repo(),
            revision_id: fixture_rev(),
        };
        let Ok(bytes) = encode(&request) else {
            assert!(false, "failed to encode GenerationStatusRequest");
            return;
        };
        let Ok(decoded) = decode::<GenerationStatusRequest>(&bytes) else {
            assert!(false, "failed to decode GenerationStatusRequest");
            return;
        };
        assert_eq!(decoded, request);
    }

    #[test]
    fn track_readiness_record_round_trip() {
        let record = TrackReadinessRecord {
            track: SearchPlaneTrackKind::Structural,
            manifest_generation: ManifestGeneration::new(7),
            manifest_digest: "digest-str".to_string(),
        };
        let Ok(bytes) = encode(&record) else {
            assert!(false, "failed to encode TrackReadinessRecord");
            return;
        };
        let Ok(decoded) = decode::<TrackReadinessRecord>(&bytes) else {
            assert!(false, "failed to decode TrackReadinessRecord");
            return;
        };
        assert_eq!(decoded, record);
    }

    #[test]
    fn generation_status_report_round_trip_with_all_tracks() {
        let report = GenerationStatusReport {
            repo_id: fixture_repo(),
            revision_id: fixture_rev(),
            tracks: vec![
                TrackReadinessRecord {
                    track: SearchPlaneTrackKind::Lexical,
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: "lex".to_string(),
                },
                TrackReadinessRecord {
                    track: SearchPlaneTrackKind::Semantic,
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: "sem".to_string(),
                },
                TrackReadinessRecord {
                    track: SearchPlaneTrackKind::Structural,
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: "str".to_string(),
                },
            ],
        };
        let Ok(bytes) = encode(&report) else {
            assert!(false, "failed to encode GenerationStatusReport");
            return;
        };
        let Ok(decoded) = decode::<GenerationStatusReport>(&bytes) else {
            assert!(false, "failed to decode GenerationStatusReport");
            return;
        };
        assert_eq!(decoded, report);
        assert_eq!(decoded.tracks.len(), 3);
    }

    #[test]
    fn generation_status_report_round_trip_empty() {
        let report = GenerationStatusReport {
            repo_id: fixture_repo(),
            revision_id: fixture_rev(),
            tracks: vec![],
        };
        let Ok(bytes) = encode(&report) else {
            assert!(false, "failed to encode empty GenerationStatusReport");
            return;
        };
        let Ok(decoded) = decode::<GenerationStatusReport>(&bytes) else {
            assert!(false, "failed to decode empty GenerationStatusReport");
            return;
        };
        assert!(decoded.tracks.is_empty());
    }

    #[test]
    fn current_generation_request_missing_track_field_fails_closed() {
        // QI-ACT-01: missing required field rejected at deserialize.
        let bad = serde_json::json!({"repo_id": "repo", "revision_id": "rev"});
        let Err(err) = CurrentGenerationRequest::deserialize(bad) else {
            assert!(false, "missing track unexpectedly deserialized");
            return;
        };
        assert!(err.to_string().contains("missing field `track`"));
    }
}
