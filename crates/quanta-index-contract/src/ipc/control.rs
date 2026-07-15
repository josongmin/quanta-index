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

/// Validation failure for a search-corpus generation identity.
///
/// A corpus generation is one logical reader-visible generation across the
/// lexical and semantic planes.  Keeping the malformed states explicit lets
/// the dispatcher reject them before any durable activation mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchCorpusGenerationIdentityValidationErrorV1 {
    LexicalTrackRequired,
    SemanticTrackRequired,
    RepoMismatch,
    RevisionMismatch,
    GenerationMismatch,
    DigestMismatch,
    EmptyDigest,
}

impl SearchCorpusGenerationIdentityValidationErrorV1 {
    #[must_use]
    pub const fn code_v1(self) -> &'static str {
        match self {
            Self::LexicalTrackRequired => "LEXICAL_TRACK_REQUIRED",
            Self::SemanticTrackRequired => "SEMANTIC_TRACK_REQUIRED",
            Self::RepoMismatch => "REPO_MISMATCH",
            Self::RevisionMismatch => "REVISION_MISMATCH",
            Self::GenerationMismatch => "GENERATION_MISMATCH",
            Self::DigestMismatch => "DIGEST_MISMATCH",
            Self::EmptyDigest => "EMPTY_DIGEST",
        }
    }
}

impl fmt::Display for SearchCorpusGenerationIdentityValidationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code_v1())
    }
}

/// Complete reader-visible generation identity for a search corpus.
///
/// The two records deliberately stay separate so each plane keeps its typed
/// track identity, while this composite makes lexical-only activation
/// unrepresentable on the production activation path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusGenerationIdentityV1 {
    pub lexical: GenerationSnapshot,
    pub semantic: GenerationSnapshot,
}

impl SearchCorpusGenerationIdentityV1 {
    pub fn validate_v1(&self) -> Result<(), SearchCorpusGenerationIdentityValidationErrorV1> {
        if self.lexical.track != SearchPlaneTrackKind::Lexical {
            return Err(SearchCorpusGenerationIdentityValidationErrorV1::LexicalTrackRequired);
        }
        if self.semantic.track != SearchPlaneTrackKind::Semantic {
            return Err(SearchCorpusGenerationIdentityValidationErrorV1::SemanticTrackRequired);
        }
        if self.lexical.repo_id != self.semantic.repo_id {
            return Err(SearchCorpusGenerationIdentityValidationErrorV1::RepoMismatch);
        }
        if self.lexical.revision_id != self.semantic.revision_id {
            return Err(SearchCorpusGenerationIdentityValidationErrorV1::RevisionMismatch);
        }
        if self.lexical.manifest_generation != self.semantic.manifest_generation {
            return Err(SearchCorpusGenerationIdentityValidationErrorV1::GenerationMismatch);
        }
        if self.lexical.manifest_digest != self.semantic.manifest_digest {
            return Err(SearchCorpusGenerationIdentityValidationErrorV1::DigestMismatch);
        }
        if self.lexical.manifest_digest.trim().is_empty() {
            return Err(SearchCorpusGenerationIdentityValidationErrorV1::EmptyDigest);
        }
        Ok(())
    }
}

/// Atomic activation request for one complete search-corpus generation.
///
/// `expected_active` is required on the wire. `null` explicitly denotes a
/// first activation; omission is rejected to prevent accidental blind writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneActivateSearchCorpusGenerationCasRequest {
    pub candidate: SearchCorpusGenerationIdentityV1,
    pub expected_active: Option<SearchCorpusGenerationIdentityV1>,
}

/// Receipt of one successful composite search-corpus activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneSearchCorpusActivationCasAck {
    pub active: SearchCorpusGenerationIdentityV1,
    pub previous_sealed_active: Option<SearchCorpusGenerationIdentityV1>,
}

const SEARCH_CORPUS_GENERATION_IDENTITY_V1_FIELDS: &[&str] = &["lexical", "semantic"];
const SEARCH_PLANE_ACTIVATE_SEARCH_CORPUS_GENERATION_CAS_REQUEST_FIELDS: &[&str] =
    &["candidate", "expected_active"];
const SEARCH_PLANE_SEARCH_CORPUS_ACTIVATION_CAS_ACK_FIELDS: &[&str] =
    &["active", "previous_sealed_active"];

impl Serialize for SearchCorpusGenerationIdentityV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchCorpusGenerationIdentityV1", 2)?;
        state.serialize_field("lexical", &self.lexical)?;
        state.serialize_field("semantic", &self.semantic)?;
        state.end()
    }
}

struct SearchCorpusGenerationIdentityV1Visitor;

impl<'de> Visitor<'de> for SearchCorpusGenerationIdentityV1Visitor {
    type Value = SearchCorpusGenerationIdentityV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchCorpusGenerationIdentityV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut lexical: Option<GenerationSnapshot> = None;
        let mut semantic: Option<GenerationSnapshot> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "lexical" => {
                    if lexical.is_some() {
                        return Err(de::Error::duplicate_field("lexical"));
                    }
                    lexical = Some(map.next_value()?);
                }
                "semantic" => {
                    if semantic.is_some() {
                        return Err(de::Error::duplicate_field("semantic"));
                    }
                    semantic = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_CORPUS_GENERATION_IDENTITY_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(Self::Value {
            lexical: lexical.ok_or_else(|| de::Error::missing_field("lexical"))?,
            semantic: semantic.ok_or_else(|| de::Error::missing_field("semantic"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchCorpusGenerationIdentityV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchCorpusGenerationIdentityV1",
            SEARCH_CORPUS_GENERATION_IDENTITY_V1_FIELDS,
            SearchCorpusGenerationIdentityV1Visitor,
        )
    }
}

impl Serialize for SearchPlaneActivateSearchCorpusGenerationCasRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer
            .serialize_struct("SearchPlaneActivateSearchCorpusGenerationCasRequest", 2)?;
        state.serialize_field("candidate", &self.candidate)?;
        state.serialize_field("expected_active", &self.expected_active)?;
        state.end()
    }
}

struct SearchPlaneActivateSearchCorpusGenerationCasRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneActivateSearchCorpusGenerationCasRequestVisitor {
    type Value = SearchPlaneActivateSearchCorpusGenerationCasRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneActivateSearchCorpusGenerationCasRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut candidate: Option<SearchCorpusGenerationIdentityV1> = None;
        let mut expected_active: Option<Option<SearchCorpusGenerationIdentityV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "candidate" => {
                    if candidate.is_some() {
                        return Err(de::Error::duplicate_field("candidate"));
                    }
                    candidate = Some(map.next_value()?);
                }
                "expected_active" => {
                    if expected_active.is_some() {
                        return Err(de::Error::duplicate_field("expected_active"));
                    }
                    expected_active = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_ACTIVATE_SEARCH_CORPUS_GENERATION_CAS_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(Self::Value {
            candidate: candidate.ok_or_else(|| de::Error::missing_field("candidate"))?,
            expected_active: expected_active
                .ok_or_else(|| de::Error::missing_field("expected_active"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneActivateSearchCorpusGenerationCasRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneActivateSearchCorpusGenerationCasRequest",
            SEARCH_PLANE_ACTIVATE_SEARCH_CORPUS_GENERATION_CAS_REQUEST_FIELDS,
            SearchPlaneActivateSearchCorpusGenerationCasRequestVisitor,
        )
    }
}

impl Serialize for SearchPlaneSearchCorpusActivationCasAck {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state =
            serializer.serialize_struct("SearchPlaneSearchCorpusActivationCasAck", 2)?;
        state.serialize_field("active", &self.active)?;
        state.serialize_field("previous_sealed_active", &self.previous_sealed_active)?;
        state.end()
    }
}

struct SearchPlaneSearchCorpusActivationCasAckVisitor;

impl<'de> Visitor<'de> for SearchPlaneSearchCorpusActivationCasAckVisitor {
    type Value = SearchPlaneSearchCorpusActivationCasAck;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneSearchCorpusActivationCasAck map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut active: Option<SearchCorpusGenerationIdentityV1> = None;
        let mut previous_sealed_active: Option<Option<SearchCorpusGenerationIdentityV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "active" => {
                    if active.is_some() {
                        return Err(de::Error::duplicate_field("active"));
                    }
                    active = Some(map.next_value()?);
                }
                "previous_sealed_active" => {
                    if previous_sealed_active.is_some() {
                        return Err(de::Error::duplicate_field("previous_sealed_active"));
                    }
                    previous_sealed_active = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_SEARCH_CORPUS_ACTIVATION_CAS_ACK_FIELDS,
                    ));
                }
            }
        }
        Ok(Self::Value {
            active: active.ok_or_else(|| de::Error::missing_field("active"))?,
            previous_sealed_active: previous_sealed_active
                .ok_or_else(|| de::Error::missing_field("previous_sealed_active"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneSearchCorpusActivationCasAck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneSearchCorpusActivationCasAck",
            SEARCH_PLANE_SEARCH_CORPUS_ACTIVATION_CAS_ACK_FIELDS,
            SearchPlaneSearchCorpusActivationCasAckVisitor,
        )
    }
}

/// Explicit compare-and-swap rollback request for one composite search corpus.
///
/// `track` is retained as the semantic rollback-capability selector and MUST
/// be `Semantic`; it does not select a single-track state. The control plane
/// restores lexical and semantic identities together from the exact target
/// tuple, so a stale operator cannot overwrite a concurrent composite
/// activation or create a semantic-only active head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneRollbackGenerationRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub track: SearchPlaneTrackKind,
    pub expected_active_generation: ManifestGeneration,
    pub expected_active_manifest_digest: String,
    pub target_generation: ManifestGeneration,
    pub target_manifest_digest: String,
}

/// Result of a successful explicit composite rollback CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneRollbackGenerationAck {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub track: SearchPlaneTrackKind,
    pub previous_generation: ManifestGeneration,
    pub previous_manifest_digest: String,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
}

const SEARCH_PLANE_ROLLBACK_GENERATION_REQUEST_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "track",
    "expected_active_generation",
    "expected_active_manifest_digest",
    "target_generation",
    "target_manifest_digest",
];

const SEARCH_PLANE_ROLLBACK_GENERATION_ACK_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "track",
    "previous_generation",
    "previous_manifest_digest",
    "manifest_generation",
    "manifest_digest",
];

fn serialize_rollback_request<S>(
    request: &SearchPlaneRollbackGenerationRequest,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut state = serializer.serialize_struct("SearchPlaneRollbackGenerationRequest", 7)?;
    state.serialize_field("repo_id", &request.repo_id)?;
    state.serialize_field("revision_id", &request.revision_id)?;
    state.serialize_field("track", &request.track)?;
    state.serialize_field(
        "expected_active_generation",
        &request.expected_active_generation,
    )?;
    state.serialize_field(
        "expected_active_manifest_digest",
        &request.expected_active_manifest_digest,
    )?;
    state.serialize_field("target_generation", &request.target_generation)?;
    state.serialize_field("target_manifest_digest", &request.target_manifest_digest)?;
    state.end()
}

impl Serialize for SearchPlaneRollbackGenerationRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_rollback_request(self, serializer)
    }
}

struct SearchPlaneRollbackGenerationRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneRollbackGenerationRequestVisitor {
    type Value = SearchPlaneRollbackGenerationRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneRollbackGenerationRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id = None;
        let mut revision_id = None;
        let mut track = None;
        let mut expected_active_generation = None;
        let mut expected_active_manifest_digest = None;
        let mut target_generation = None;
        let mut target_manifest_digest = None;
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
                "expected_active_generation" => {
                    if expected_active_generation.is_some() {
                        return Err(de::Error::duplicate_field("expected_active_generation"));
                    }
                    expected_active_generation = Some(map.next_value()?);
                }
                "expected_active_manifest_digest" => {
                    if expected_active_manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field(
                            "expected_active_manifest_digest",
                        ));
                    }
                    expected_active_manifest_digest = Some(map.next_value()?);
                }
                "target_generation" => {
                    if target_generation.is_some() {
                        return Err(de::Error::duplicate_field("target_generation"));
                    }
                    target_generation = Some(map.next_value()?);
                }
                "target_manifest_digest" => {
                    if target_manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("target_manifest_digest"));
                    }
                    target_manifest_digest = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_ROLLBACK_GENERATION_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneRollbackGenerationRequest {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            track: track.ok_or_else(|| de::Error::missing_field("track"))?,
            expected_active_generation: expected_active_generation
                .ok_or_else(|| de::Error::missing_field("expected_active_generation"))?,
            expected_active_manifest_digest: expected_active_manifest_digest
                .ok_or_else(|| de::Error::missing_field("expected_active_manifest_digest"))?,
            target_generation: target_generation
                .ok_or_else(|| de::Error::missing_field("target_generation"))?,
            target_manifest_digest: target_manifest_digest
                .ok_or_else(|| de::Error::missing_field("target_manifest_digest"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneRollbackGenerationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneRollbackGenerationRequest",
            SEARCH_PLANE_ROLLBACK_GENERATION_REQUEST_FIELDS,
            SearchPlaneRollbackGenerationRequestVisitor,
        )
    }
}

impl Serialize for SearchPlaneRollbackGenerationAck {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneRollbackGenerationAck", 7)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("track", &self.track)?;
        state.serialize_field("previous_generation", &self.previous_generation)?;
        state.serialize_field("previous_manifest_digest", &self.previous_manifest_digest)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.end()
    }
}

struct SearchPlaneRollbackGenerationAckVisitor;

impl<'de> Visitor<'de> for SearchPlaneRollbackGenerationAckVisitor {
    type Value = SearchPlaneRollbackGenerationAck;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneRollbackGenerationAck map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id = None;
        let mut revision_id = None;
        let mut track = None;
        let mut previous_generation = None;
        let mut previous_manifest_digest = None;
        let mut manifest_generation = None;
        let mut manifest_digest = None;
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
                "previous_generation" => {
                    if previous_generation.is_some() {
                        return Err(de::Error::duplicate_field("previous_generation"));
                    }
                    previous_generation = Some(map.next_value()?);
                }
                "previous_manifest_digest" => {
                    if previous_manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("previous_manifest_digest"));
                    }
                    previous_manifest_digest = Some(map.next_value()?);
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
                        SEARCH_PLANE_ROLLBACK_GENERATION_ACK_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneRollbackGenerationAck {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            track: track.ok_or_else(|| de::Error::missing_field("track"))?,
            previous_generation: previous_generation
                .ok_or_else(|| de::Error::missing_field("previous_generation"))?,
            previous_manifest_digest: previous_manifest_digest
                .ok_or_else(|| de::Error::missing_field("previous_manifest_digest"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneRollbackGenerationAck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneRollbackGenerationAck",
            SEARCH_PLANE_ROLLBACK_GENERATION_ACK_FIELDS,
            SearchPlaneRollbackGenerationAckVisitor,
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

    fn corpus_identity(generation: u64, digest: &str) -> SearchCorpusGenerationIdentityV1 {
        SearchCorpusGenerationIdentityV1 {
            lexical: GenerationSnapshot {
                repo_id: fixture_repo(),
                revision_id: fixture_rev(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: digest.to_string(),
            },
            semantic: GenerationSnapshot {
                repo_id: fixture_repo(),
                revision_id: fixture_rev(),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: digest.to_string(),
            },
        }
    }

    #[test]
    fn search_corpus_activation_round_trips_only_composite_identity_v1() {
        let previous = corpus_identity(10, "digest-10");
        let candidate = corpus_identity(11, "digest-11");
        assert_eq!(candidate.validate_v1(), Ok(()));

        let request = SearchPlaneActivateSearchCorpusGenerationCasRequest {
            candidate: candidate.clone(),
            expected_active: Some(previous.clone()),
        };
        let bytes = encode(&request).expect("encode composite activation request");
        assert_eq!(
            decode::<SearchPlaneActivateSearchCorpusGenerationCasRequest>(&bytes)
                .expect("decode composite activation request"),
            request
        );

        let ack = SearchPlaneSearchCorpusActivationCasAck {
            active: candidate,
            previous_sealed_active: Some(previous),
        };
        let bytes = encode(&ack).expect("encode composite activation ack");
        assert_eq!(
            decode::<SearchPlaneSearchCorpusActivationCasAck>(&bytes)
                .expect("decode composite activation ack"),
            ack
        );
    }

    #[test]
    fn search_corpus_activation_rejects_lexical_only_identity_v1() {
        let mut identity = corpus_identity(11, "digest-11");
        identity.semantic.track = SearchPlaneTrackKind::Lexical;
        assert_eq!(
            identity.validate_v1(),
            Err(SearchCorpusGenerationIdentityValidationErrorV1::SemanticTrackRequired)
        );

        let missing_semantic = serde_json::json!({
            "lexical": {
                "repo_id": "repo",
                "revision_id": "rev",
                "track": "Lexical",
                "manifest_generation": 11,
                "manifest_digest": "digest-11"
            }
        });
        let error = SearchCorpusGenerationIdentityV1::deserialize(missing_semantic)
            .expect_err("lexical-only identity must not deserialize");
        assert!(error.to_string().contains("missing field `semantic`"));
    }

    #[test]
    fn rollback_generation_request_and_ack_round_trip() {
        let request = SearchPlaneRollbackGenerationRequest {
            repo_id: fixture_repo(),
            revision_id: fixture_rev(),
            track: SearchPlaneTrackKind::Semantic,
            expected_active_generation: ManifestGeneration::new(11),
            expected_active_manifest_digest: "digest-11".to_string(),
            target_generation: ManifestGeneration::new(10),
            target_manifest_digest: "digest-10".to_string(),
        };
        let Ok(bytes) = encode(&request) else {
            assert!(
                false,
                "failed to encode SearchPlaneRollbackGenerationRequest"
            );
            return;
        };
        let Ok(decoded) = decode::<SearchPlaneRollbackGenerationRequest>(&bytes) else {
            assert!(
                false,
                "failed to decode SearchPlaneRollbackGenerationRequest"
            );
            return;
        };
        assert_eq!(decoded, request);

        let ack = SearchPlaneRollbackGenerationAck {
            repo_id: fixture_repo(),
            revision_id: fixture_rev(),
            track: SearchPlaneTrackKind::Semantic,
            previous_generation: ManifestGeneration::new(11),
            previous_manifest_digest: "digest-11".to_string(),
            manifest_generation: ManifestGeneration::new(10),
            manifest_digest: "digest-10".to_string(),
        };
        let Ok(bytes) = encode(&ack) else {
            assert!(false, "failed to encode SearchPlaneRollbackGenerationAck");
            return;
        };
        let Ok(decoded) = decode::<SearchPlaneRollbackGenerationAck>(&bytes) else {
            assert!(false, "failed to decode SearchPlaneRollbackGenerationAck");
            return;
        };
        assert_eq!(decoded, ack);
    }

    #[test]
    fn rollback_generation_request_rejects_unknown_and_missing_fields() {
        let unknown = serde_json::json!({
            "repo_id": "repo",
            "revision_id": "rev",
            "track": "Semantic",
            "expected_active_generation": 11,
            "expected_active_manifest_digest": "digest-11",
            "target_generation": 10,
            "target_manifest_digest": "digest-10",
            "unexpected": true,
        });
        assert!(SearchPlaneRollbackGenerationRequest::deserialize(unknown).is_err());

        let missing = serde_json::json!({
            "repo_id": "repo",
            "revision_id": "rev",
            "track": "Semantic",
            "expected_active_generation": 11,
            "expected_active_manifest_digest": "digest-11",
            "target_generation": 10,
        });
        let Err(err) = SearchPlaneRollbackGenerationRequest::deserialize(missing) else {
            assert!(false, "missing target digest unexpectedly deserialized");
            return;
        };
        assert!(
            err.to_string()
                .contains("missing field `target_manifest_digest`")
        );
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
